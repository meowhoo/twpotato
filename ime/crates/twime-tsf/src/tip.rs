//! The text input processor object and the per-context shell state machine.
//!
//! Rules kept throughout: no RefCell borrow is held across a call into TSF or into the pipe (callbacks can re-enter);
//! every COM entry is wrapped in `guard`; `OnTestKeyDown` never touches the pipe (only `peek` on the last confirmed
//! view); `OnKeyDown` sends exactly one request per key (plus rare queued events after a failure or an external edit).

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};

use twime_proto::{
    Applied, Cands, CommitResult, Event, LexError, Mode, PeekView, Reply, Request, ToggleKey,
    Update, UserReply, peek,
};
use windows::{
    Win32::{
        Foundation::{E_FAIL, HWND, LPARAM, WPARAM},
        UI::{
            TextServices::{
                GUID_COMPARTMENT_KEYBOARD_DISABLED, ITfCompartmentMgr, ITfComposition,
                ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfDocumentMgr,
                ITfEditRecord, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfSource,
                ITfTextEditSink, ITfTextEditSink_Impl, ITfTextInputProcessor_Impl,
                ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfThreadMgr,
                ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl, TF_GTP_INCL_TEXT,
            },
            WindowsAndMessaging::{KillTimer, SetTimer},
        },
    },
    core::{BOOL, GUID, IUnknownImpl, Interface, Ref, Result, implement},
};

use crate::{
    LIVE, cand, edit, guard,
    keys::{caps_lock_on, command_held, is_shift_vk, key_event, other_shift_held},
    link::Link,
    shift::{Release, ShiftTap},
    trace,
};

const POP_DEBOUNCE_MS: u32 = 250;

/// State kept for one `ITfContext` (one engine session).
struct Ctx {
    ctx: ITfContext,
    raw: usize,
    session: u32,
    trace_id: u32,
    cookie: Option<u32>,
    layout_cookie: Option<u32>,
    seq: u32,
    pending_ack: Option<CommitResult>,
    view: PeekView,
    last: Option<Update>,
    composition: Option<ITfComposition>,
    /// Events the engine must see before the next key (after a failure or an external edit).
    queue: Vec<Event>,
    /// The host state is unknown; keys pass through until the old composition is confirmed gone.
    unsynced: bool,
    /// An external edit happened while composing: the composition must be cancelled, never committed.
    cancel_reason: bool,
    cleanup_pending: bool,
    /// Range of a convertible composition the host ended on its own, kept until we learn why (focus change or edit).
    term_loc: Option<(Vec<u16>, Instant)>,
    /// Bumped whenever the composition object changes; stale async cleanups compare it.
    epoch: u32,
    disabled: bool,
    /// Generation of the engine connection this context was last synchronised with (`Focus(true)` round trip); 0 = never.
    /// The context is "ready" only while this equals the link's generation.
    sync_gen: u32,
    /// The mode the host document is known to agree with: updated when an `Update` was not refused by the host.
    confirmed_mode: Mode,
    toggle_key: ToggleKey,
}

impl Ctx {
    /// The text part of the peek view goes back to empty; the connection and user state (`ux`, `mode`) stays.
    fn reset_text_view(&mut self) {
        self.view = PeekView { mode: self.view.mode, ux: self.view.ux, ..PeekView::default() };
    }
}

struct Core {
    tm: Option<ITfThreadMgr>,
    tid: u32,
    tm_cookie: Option<u32>,
    profile_cookie: Option<u32>,
    key_sink: Option<ITfKeyEventSink>,
    tm_sink: Option<ITfThreadMgrEventSink>,
    comp_sink: Option<ITfCompositionSink>,
    edit_sink: Option<ITfTextEditSink>,
    layout_sink: Option<windows::Win32::UI::TextServices::ITfTextLayoutSink>,
    next_session: u32,
    langbar: Option<(windows::Win32::UI::TextServices::ITfLangBarItemMgr, windows::Win32::UI::TextServices::ITfLangBarItem)>,
}

struct Shared {
    pop_preferences: RefCell<Option<twime_proto::UserSettings>>,
    pop_timer: Cell<usize>,
    pop_pending: Cell<Option<(usize, u32, u32, u32)>>,
    completion: RefCell<Option<(usize, u32, u32, twime_proto::CompletionHint)>>,
    pop_down: Cell<Option<(usize, u32, u32, u32)>>,
    request_busy: Cell<bool>,
    hint: RefCell<Option<crate::hint::Hint>>,
    hint_epoch: Cell<u64>,
    hint_atom: Cell<u32>,
    langbar: RefCell<Option<Rc<crate::langbar::State>>>,
    core: RefCell<Core>,
    ctxs: RefCell<Vec<Ctx>>,
    link: RefCell<Link>,
    /// >0 while this shell is writing to the host (its own edits are not "external").
    own_edit: Cell<u32>,
    /// The engine answered `Disabled`: the next `fail_over` also closes the session.
    disabled_seen: Cell<bool>,
    /// Edit notifications still expected for our own writes (hosts may deliver them after the session returned).
    own_pending: Cell<(u32, Option<Instant>)>,
    /// The context the expectations in `own_pending` are for.
    own_ctx: Cell<usize>,
    /// Time of the first callback of the current key (OnTestKeyDown), for the path latency.
    first_cb: Cell<Option<Instant>>,
    /// Shift single tap bookkeeping (shell-local, never sent as such).
    shift: RefCell<ShiftTap>,
    /// The context that was seen last by a key test (a change resets the tap).
    last_ctx: Cell<usize>,
    /// The context of the document that has the keyboard focus (0 = none).
    focus: Cell<usize>,
    keyboard_focus: Cell<bool>,
    /// Timer id of the pending synchronisation attempt (0 = none) and the number of failed attempts since the last success.
    sync_timer: Cell<usize>,
    sync_attempts: Cell<u32>,
    /// Independent settings refresh timer; never owns synchronisation retries.
    settings_timer: Cell<usize>,
    last_sync_try: Cell<Option<Instant>>,
    /// The mode label waits for a timer so that its window (created and measured with GDI) is not on the key path.
    label_timer: Cell<usize>,
    label_pending: Cell<Option<(usize, Mode)>>,
    /// Test hook (`TWIME_FAULT=async-late`, dev mode): asynchronous composition writes that wait for a timer.
    late_jobs: RefCell<Vec<Box<dyn FnOnce()>>>,
    /// Test hook (`TWIME_FAULT=commit-fail`): the first commit fails after the composition was ended.
    commit_fault_done: Cell<bool>,
    hint_fault_done: Cell<bool>,
}

#[implement(
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfCompositionSink,
    ITfThreadMgrEventSink,
    windows::Win32::UI::TextServices::ITfInputProcessorProfileActivationSink,
    ITfTextEditSink,
    windows::Win32::UI::TextServices::ITfDisplayAttributeProvider,
    windows::Win32::UI::TextServices::ITfTextLayoutSink
)]
pub struct Tip {
    sh: Rc<Shared>,
}

impl Tip {
    pub fn new() -> Tip {
        LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Tip {
            sh: Rc::new(Shared {
                pop_preferences: RefCell::new(None),
                pop_timer: Cell::new(0),
                pop_pending: Cell::new(None),
                completion: RefCell::new(None),
                pop_down: Cell::new(None),
                request_busy: Cell::new(false),
                hint: RefCell::new(Some(Default::default())),
                hint_epoch: Cell::new(0),
                hint_atom: Cell::new(0),
                langbar: RefCell::new(None),
                core: RefCell::new(Core {
                    tm: None,
                    tid: 0,
                    tm_cookie: None,
                    profile_cookie: None,
                    key_sink: None,
                    tm_sink: None,
                    comp_sink: None,
                    edit_sink: None,
                    layout_sink: None,
                    next_session: 1,
                    langbar: None,
                }),
                ctxs: RefCell::new(Vec::new()),
                link: RefCell::new(Link::new()),
                own_edit: Cell::new(0),
                disabled_seen: Cell::new(false),
                own_pending: Cell::new((0, None)),
                own_ctx: Cell::new(0),
                first_cb: Cell::new(None),
                shift: RefCell::new(ShiftTap::new()),
                last_ctx: Cell::new(0),
                focus: Cell::new(0),
                keyboard_focus: Cell::new(false),
                sync_timer: Cell::new(0),
                sync_attempts: Cell::new(0),
                settings_timer: Cell::new(0),
                last_sync_try: Cell::new(None),
                label_timer: Cell::new(0),
                label_pending: Cell::new(None),
                late_jobs: RefCell::new(Vec::new()),
                commit_fault_done: Cell::new(false),
                hint_fault_done: Cell::new(false),
            }),
        }
    }
}

impl Drop for Tip {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn ok_or_fail<T>(r: Option<Result<T>>) -> Result<T> {
    r.unwrap_or(Err(E_FAIL.into()))
}

fn utf16_caret(text: &str, cursor_chars: usize) -> usize {
    text.chars().take(cursor_chars).map(char::len_utf16).sum()
}

#[derive(Clone, Copy)]
enum SyncSource {
    Key,
    Push(bool),
    Pop(usize),
}

/// Decide both the cached focus and whether a handshake is needed. Readiness
/// includes the current connection generation; an unready key is authoritative.
fn sync_target(focus: usize, context: usize, ready: bool, source: SyncSource) -> (usize, bool) {
    // A pop callback may precede the stack update. Never reattach the removed top;
    // a zero target asks the timer to look up the focused document again later.
    if let SyncSource::Pop(popped) = source {
        return if context == 0 || context == popped {
            (0, false)
        } else {
            (context, !ready)
        };
    }
    if context == 0 {
        return (focus, false);
    }
    match source {
        SyncSource::Key if !ready => (context, true),
        SyncSource::Push(true) => (context, !ready),
        _ => (focus, false),
    }
}

impl Shared {
    fn context_id(&self, key: usize) -> u32 {
        self.with(key, |c| c.trace_id).unwrap_or(0)
    }

    fn set_focus(&self, key: usize, reason: &str) {
        let old = self.focus.replace(key);
        if old != key {
            self.hide_hint();
            self.refresh_langbar();
            trace::log(&format!(
                "focus old={} new={} reason={reason}",
                self.context_id(old),
                self.context_id(key)
            ));
        }
    }

    fn refresh_langbar(&self) {
        let key = self.focus.get();
        let mode = if self.is_ready(key) {
            self.with(key, |c| c.view.mode)
        } else {
            None
        };
        let state = self.langbar.borrow().clone();
        if let Some(state) = state {
            state.update(key, mode);
        }
    }

    fn new_langbar_state(&self) -> Rc<crate::langbar::State> {
        let state = crate::langbar::State::new();
        let previous = self.langbar.replace(Some(state.clone()));
        if let Some(previous) = previous {
            previous.deactivate();
        }
        state
    }

    fn focused_doc(&self) -> Option<ITfDocumentMgr> {
        let tm = self.core.borrow().tm.clone()?;
        match unsafe { tm.GetFocus() } {
            Ok(doc) => Some(doc),
            Err(e) => {
                trace::log(&format!("focus.lookup reason=get_focus hr={:?}", e.code()));
                None
            }
        }
    }

    fn recover_focus(&self) {
        if let Some(doc) = self.focused_doc() {
            self.attach_doc(&doc);
        }
    }

    /// Only selects and schedules; never performs a Focus handshake on the key path.
    fn key_sync(&self, pic: Option<&ITfContext>) {
        let Some(pic) = pic else { return };
        let key = self.ctx_key(pic);
        let (target, sync) =
            sync_target(self.focus.get(), key, self.is_ready(key), SyncSource::Key);
        if sync {
            self.set_focus(target, "unready_key");
            self.keyboard_focus.set(true);
            if self.with(key, |c| !c.disabled && !c.unsynced && !c.cleanup_pending) == Some(true) {
                if self.link.borrow().is_up() {
                    self.nudge_sync();
                } else {
                    self.schedule_sync(self.retry_delay());
                }
            }
        }
    }

    fn trace_key(
        &self,
        pic: Option<&ITfContext>,
        vk: usize,
        test: bool,
        down: bool,
        eaten: bool,
        reason: &str,
    ) {
        if trace::enabled() {
            let key = pic.map(|p| p.as_raw() as usize).unwrap_or(0);
            // Exit diagnostics also run after guard caught a reentrant failure.
            let ready = self.link.try_borrow().ok().is_some_and(|link| {
                link.is_up() && link.generation() != 0
                    && self.with(key, |c| c.sync_gen == link.generation()).unwrap_or(false)
            });
            trace::log(&format!(
                "key.exit ctx={} vk={vk:#x} test={test} down={down} ready={} eaten={eaten} reason={reason}",
                self.context_id(key),
                ready
            ));
        }
    }
    /// Runs a write to the host. `f` returns its result and whether it changed the document. A changed document is expected to be
    /// reported to us once afterwards (the notification of a write that arrives while it runs is ignored by `end_edit`; some hosts,
    /// Rich Edit among them, report such a write a second time a little later, through their own message queue: evidence from the
    /// UX1 host scenarios, where counting the first as the echo made the second look like an external edit and lost the next
    /// composition write). A write that failed or changed nothing leaves no expectation.
    fn own_write<R>(&self, key: usize, f: impl FnOnce() -> (R, bool)) -> R {
        self.own_edit.set(self.own_edit.get() + 1);
        let (r, changed) = f();
        self.own_edit.set(self.own_edit.get() - 1);
        if changed {
            self.note_own(key);
        }
        r
    }

    fn note_own(&self, key: usize) {
        let (n, t) = self.own_pending.get();
        let fresh = n > 0 && t.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(300)) && self.own_ctx.get() == key;
        self.own_ctx.set(key);
        self.own_pending.set((if fresh { n + 1 } else { 1 }, Some(Instant::now())));
    }

    /// True if this notification, for context `key`, is the echo of one of our own writes. A host may report all the writes of one
    /// key as one notification (Rich Edit does): the first echo settles every write we were still waiting for in that context,
    /// so that a real edit right after cannot be mistaken for an echo. Expectations expire after 300 ms. There is no further
    /// time-based guessing: what is not an expected echo is an external edit.
    fn take_own(&self, key: usize) -> bool {
        let (n, t) = self.own_pending.get();
        let fresh = n > 0 && t.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(300));
        if fresh && self.own_ctx.get() == key {
            self.own_pending.set((0, None));
            return true;
        }
        if !fresh {
            self.own_pending.set((0, None));
        }
        false
    }

    fn tid(&self) -> u32 {
        self.core.borrow().tid
    }

    // ---- context table ----------------------------------------------------------------------------------------

    /// Finds or creates the entry of a context; returns its key.
    fn ctx_key(&self, pic: &ITfContext) -> usize {
        let raw = pic.as_raw() as usize;
        if self.ctxs.borrow().iter().any(|c| c.raw == raw) {
            return raw;
        }
        let (session, edit_sink) = {
            let mut c = self.core.borrow_mut();
            let s = c.next_session;
            c.next_session += 1;
            (s, c.edit_sink.clone())
        };
        let layout_sink = self.core.borrow().layout_sink.clone();
        let layout_cookie = layout_sink.and_then(|sink| unsafe {
            pic.cast::<ITfSource>().ok()?.AdviseSink(
                &windows::Win32::UI::TextServices::ITfTextLayoutSink::IID, &sink).ok()
        });
        let cookie = edit_sink.and_then(|sink| unsafe {
            pic.cast::<ITfSource>()
                .ok()?
                .AdviseSink(&ITfTextEditSink::IID, &sink)
                .ok()
        });
        static NEXT_CONTEXT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let trace_id = NEXT_CONTEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        trace::context_status(pic, trace_id, "new");
        let disabled = keyboard_disabled(pic, trace_id);
        trace::log(&format!(
            "ctx new session={session} ctx={trace_id} disabled={disabled}"
        ));
        self.ctxs.borrow_mut().push(Ctx {
            ctx: pic.clone(),
            raw,
            session,
            trace_id,
            cookie,
            layout_cookie,
            seq: 0,
            pending_ack: None,
            view: PeekView::default(),
            last: None,
            composition: None,
            queue: if disabled {
                vec![Event::Password(true)]
            } else {
                Vec::new()
            },
            unsynced: false,
            cancel_reason: false,
            cleanup_pending: false,
            term_loc: None,
            epoch: 0,
            disabled,
            sync_gen: 0,
            confirmed_mode: Mode::Chinese,
            toggle_key: ToggleKey::Shift,
        });
        raw
    }

    // ---- readiness and synchronisation -------------------------------------------------

    /// The context may take keys: the connection is up and this context has done its `Focus(true)` round trip on it.
    fn is_ready(&self, key: usize) -> bool {
        let (up, generation) = {
            let l = self.link.borrow();
            (l.is_up(), l.generation())
        };
        up && generation != 0 && self.with(key, |c| c.sync_gen == generation).unwrap_or(false)
    }

    /// Connects (if needed), negotiates and synchronises the context: one `Focus(true)` round trip that brings the
    /// engine's mode into the peek view. True when the context is ready afterwards. Not for the key-test callback.
    fn sync_ctx(&self, key: usize) -> bool {
        trace::log(&format!("sync target={}", self.context_id(key)));
        if trace::enabled() && let Some((ctx, id)) = self.with(key, |c| (c.ctx.clone(), c.trace_id)) {
            trace::context_status(&ctx, id, "before_sync");
        }
        if self.is_ready(key) {
            trace::log("sync.skip reason=ready");
            return true;
        }
        self.last_sync_try.set(Some(Instant::now()));
        if self.link.borrow().marker_present() {
            trace::log("sync.skip reason=marker");
            return false;
        }
        if !self.link.borrow_mut().ensure() {
            trace::log("sync: not connected");
            trace::note_unready("sync_not_connected");
            trace::log("sync.skip reason=not_connected");
            return false;
        }
        let blocked = self
            .with(key, |c| c.unsynced || c.cleanup_pending || c.disabled)
            .unwrap_or(true);
        if blocked {
            trace::log("sync.skip reason=disabled_unsynced_cleanup_or_missing");
            return false;
        }
        // A host composition left over from an earlier connection belongs to the engine that is gone: end it as then (committed
        // when fully convertible, else cancelled) before the new engine starts afresh.
        if self.with(key, |c| c.composition.is_some()).unwrap_or(false) {
            self.fail_over(key);
            if self
                .with(key, |c| c.composition.is_some() || c.unsynced)
                .unwrap_or(true)
            {
                trace::log("sync.skip reason=composition_cleanup");
                return false;
            }
        }
        if !self.flush_queue(key) {
            self.link.borrow_mut().drop_connection();
            trace::log("sync.skip reason=queue_failed");
            return false;
        }
        let Some(u) = self.exchange(key, Event::Focus(true)) else {
            trace::log("sync failed");
            trace::log("sync.skip reason=focus_failed");
            return false;
        };
        let (generation, version) = {
            let l = self.link.borrow();
            (l.generation(), l.version())
        };
        self.finish_update(key, u);
        self.with(key, |c| {
            c.sync_gen = generation;
            c.confirmed_mode = c.view.mode;
        });
        trace::note_sync_ok();
        self.refresh_langbar();
        trace::log(&format!(
            "sync ok gen={generation} version={version} ctx={}",
            self.context_id(key)
        ));
        true
    }

    /// A key arrived while the context is not ready: try soon, but not more than once a second.
    fn nudge_sync(&self) {
        if let Some(last) = self.last_sync_try.get() {
            let elapsed = last.elapsed().as_millis();
            if elapsed < 1000 {
                // A different context may have synced recently and left no timer.
                // Keep the throttle, but guarantee an attempt for the new target.
                self.schedule_sync((1000 - elapsed) as u32);
                return;
            }
        }
        self.stop_sync_timer();
        self.schedule_sync(1);
    }

    /// Starts a one-shot attempt after `delay_ms` unless one is already pending.
    fn schedule_sync(&self, delay_ms: u32) {
        if self.sync_timer.get() != 0 {
            return;
        }
        let id = unsafe { SetTimer(None, 0, delay_ms, Some(sync_timer_proc)) };
        self.sync_timer.set(id);
        trace::log(&format!(
            "timer.create kind=sync id={id} delay={delay_ms} failed={}",
            id == 0
        ));
    }

    fn stop_sync_timer(&self) {
        let id = self.sync_timer.replace(0);
        if id != 0 {
            unsafe {
                let _ = KillTimer(None, id);
            }
        }
    }

    /// Timer: retry synchronisation every second five times, then every five seconds until ready.
    fn on_sync_timer(&self) {
        trace::log("timer.fire kind=sync");
        self.stop_sync_timer();
        if self.focus.get() == 0 {
            self.recover_focus();
        }
        let key = self.focus.get();
        if key == 0 {
            trace::log("sync.skip reason=no_focus");
            self.schedule_sync(self.retry_delay());
            return;
        }
        if self.link.borrow().marker_present() {
            trace::log("sync.skip reason=marker");
            self.sync_attempts.set(0);
            return;
        }
        if self.is_ready(key) || self.sync_ctx(key) {
            self.sync_attempts.set(0);
            // A key can recover focus without attach_context ever running.
            // Even an English/None session needs polling to learn a new toggle.
            if self.focus.get() == key && self.keyboard_focus.get() {
                self.schedule_settings(1);
            }
            return;
        }
        self.sync_attempts.set(self.sync_attempts.get() + 1);
        self.schedule_sync(self.retry_delay());
    }

    fn schedule_settings(&self, delay_ms: u32) {
        if self.settings_timer.get() == 0 {
            let id = unsafe { SetTimer(None, 0, delay_ms, Some(settings_timer_proc)) };
            self.settings_timer.set(id);
            trace::log(&format!(
                "timer.create kind=settings id={id} delay={delay_ms} failed={}",
                id == 0
            ));
        }
    }

    fn stop_settings_timer(&self) {
        let id = self.settings_timer.replace(0);
        if id != 0 {
            unsafe {
                let _ = KillTimer(None, id);
            }
        }
    }

    fn on_settings_timer(&self) {
        trace::log("timer.fire kind=settings");
        self.stop_settings_timer();
        let key = self.focus.get();
        if key == 0 || !self.keyboard_focus.get() || self.link.borrow().marker_present() {
            return;
        }
        // Readiness is only inspected: settings must never ensure/negotiate the input connection.
        if self.is_ready(key) {
            self.refresh_settings(key);
        }
        self.schedule_settings(5000);
    }

    /// Timer-only control request. Never changes session sequence numbers or consumes their pending ack.
    fn refresh_settings(&self, key: usize) {
        let pop_active = self.completion.borrow().is_some() || self.pop_pending.get().is_some();
        if self.link.borrow().version() < 5
            || self.request_busy.get()
            || (!pop_active && self
                .with(key, |c| {
                    c.composition.is_some()
                        || c.view.composing
                        || c.view.run
                        || c.view.symbols
                        || c.unsynced
                        || c.cleanup_pending
                        || c.disabled
                })
                .unwrap_or(true))
        {
            return;
        }
        self.request_busy.set(true);
        let reply = self.link.borrow().user_settings();
        self.request_busy.set(false);
        let toggle = match reply {
            Ok(UserReply::Settings { settings, .. }) => {
                let toggle = settings.toggle_key;
                self.observe_pop_preferences(settings);
                toggle
            },
            Ok(UserReply::Error(LexError::Disabled)) => {
                if self.link.borrow().settings().shift_tap {
                    ToggleKey::Shift
                } else {
                    ToggleKey::None
                }
            }
            _ => {
                trace::log("settings refresh failed");
                return;
            }
        };
        self.with(key, |c| c.toggle_key = toggle);
        trace::log(&format!("settings refresh toggle={toggle:?}"));
    }

    fn observe_pop_preferences(&self, settings: twime_proto::UserSettings) {
        let changed = self.pop_preferences.borrow().as_ref().is_some_and(|previous| previous != &settings);
        *self.pop_preferences.borrow_mut() = Some(settings);
        let pop_active = self.completion.borrow().is_some() || self.pop_pending.get().is_some();
        if changed && pop_active { self.hide_hint(); }
    }

    /// About a second for the first five failed attempts, then about five seconds.
    fn retry_delay(&self) -> u32 {
        if self.sync_attempts.get() < 5 { 1000 } else { 5000 }
    }

    fn with<R>(&self, key: usize, f: impl FnOnce(&mut Ctx) -> R) -> Option<R> {
        let mut v = self.ctxs.try_borrow_mut().ok()?;
        v.iter_mut().find(|c| c.raw == key).map(f)
    }

    fn drop_ctx(&self, raw: usize) {
        if self.focus.get() == raw {
            self.set_focus(0, "drop_context");
            self.stop_sync_timer();
            self.stop_settings_timer();
        }
        if self.last_ctx.get() == raw {
            self.last_ctx.set(0);
        }
        let removed = {
            let mut v = self.ctxs.borrow_mut();
            v.iter().position(|c| c.raw == raw).map(|i| v.remove(i))
        };
        if let Some(c) = removed {
            if let Ok(src) = c.ctx.cast::<ITfSource>() {
                for ck in [c.cookie, c.layout_cookie].into_iter().flatten() {
                    let _ = unsafe { src.UnadviseSink(ck) };
                }
            }
        }
    }

    // ---- engine exchange --------------------------------------------------------------------------------------

    /// One request for the context; None if the engine did not answer in time or answered garbage.
    fn exchange(&self, key: usize, event: Event) -> Option<Update> {
        if !matches!(event, Event::Idle(_)) { self.cancel_pop(); }
        if self.request_busy.replace(true) { return None; }
        let r = self.exchange_inner(key, event);
        self.request_busy.set(false);
        if r.is_none() && self.focus.get() != 0 && !self.link.borrow().marker_present() {
            // The engine did not answer: the connection is gone or about to be; keep trying in the background while the
            // focus stays.
            self.schedule_sync(self.retry_delay());
        }
        r
    }

    fn exchange_inner(&self, key: usize, event: Event) -> Option<Update> {
        let (session, seq, ack) = self.with(key, |c| {
            c.seq = c.seq.wrapping_add(1);
            (c.session, c.seq, c.pending_ack.take())
        })?;
        let req = Request {
            session,
            seq,
            ack,
            event,
        };
        trace::log("exchange");
        if !self.link.borrow_mut().negotiate() {
            return None;
        }
        let reply = self.link.borrow_mut().call(&req).ok()?;
        match reply {
            Reply::Update {
                session: s,
                seq: q,
                update,
            } if s == session && q == seq => {
                if self.link.borrow().version() >= 5 && !matches!(req.event, Event::Idle(_)) {
                    self.with(key, |c| c.toggle_key = update.toggle_key);
                }
                Some(update)
            }
            Reply::Disabled { .. } => {
                // The engine refuses input (uninstall in progress): the composition is ended by the caller, which then
                // closes the session so the engine can see a clean end.
                self.disabled_seen.set(true);
                None
            }
            _ => {
                // Wrong seq / session or an unexpected message: do not trust this connection any more.
                self.link.borrow_mut().drop_connection();
                None
            }
        }
    }

    /// Sends the queued events (rare). False if the engine failed.
    fn flush_queue(&self, key: usize) -> bool {
        loop {
            let ev = self
                .with(key, |c| {
                    if c.queue.is_empty() {
                        None
                    } else {
                        Some(c.queue.remove(0))
                    }
                })
                .flatten();
            let Some(ev) = ev else { return true };
            if self.exchange(key, ev).is_none() {
                return false;
            }
        }
    }

    // ---- applying updates to the host ----------------------------------------------------------------------------

    /// Applies the Update's commit / composition. Returns (commit, comp) results as defined in plans 4.1.
    fn apply_update(&self, key: usize, u: &Update) -> (Applied, Applied) {
        if !self.hint_fault_done.get() && self.link.borrow().is_dev()
            && std::env::var("TWIME_FAULT").as_deref() == Ok("hint-attribute-fail")
            && u.composition.as_ref().is_some_and(|c| c.segments.iter().any(|s| s.hint != 0)) {
            self.hint_fault_done.set(true); edit::inject_fail_hint_attributes();
        }
        let hints = (self.link.borrow().version() >= 6).then(|| (
            self.hint_atom.get(), u.composition.as_ref().map_or_else(Vec::new, |c| crate::attribute::ranges(&c.segments))));
        let (ctx, composition) = match self.with(key, |c| (c.ctx.clone(), c.composition.clone())) {
            Some(x) => x,
            None => return (Applied::NotApplicable, Applied::NotApplicable),
        };
        let new_text = u.composition.as_ref().map(|c| {
            let t = c.text();
            let caret = utf16_caret(&t, c.cursor as usize);
            (t, caret)
        });
        if u.commit.is_none() && new_text.is_none() && composition.is_none() {
            return (Applied::NotApplicable, Applied::NotApplicable);
        }
        let Some(sink) = self.core.borrow().comp_sink.clone() else {
            return (Applied::Failed, Applied::NotApplicable);
        };
        if u.commit.is_some()
            && !self.commit_fault_done.get()
            && self.link.borrow().is_dev()
            && std::env::var("TWIME_FAULT").as_deref() == Ok("commit-fail")
        {
            self.commit_fault_done.set(true);
            edit::inject_fail_after_end();
        }
        let tid = self.tid();
        // A write that leaves the document as it was produces no host notification: do not expect an echo for it.
        let changes_doc = u.commit.is_some()
            || self
                .with(key, |c| {
                    c.last
                        .as_ref()
                        .and_then(|l| l.composition.as_ref().map(|k| (k.text(), k.cursor)))
                })
                .flatten()
                != u.composition.as_ref().map(|k| (k.text(), k.cursor));
        let run = |input: edit::ApplyInput| {
            let out = self.own_write(key, || {
                let out = edit::apply(tid, input);
                // A refused commit may still have changed the document (the interim text is taken away again): expect its echo too.
                let changed = changes_doc
                    && (out.commit == Some(Applied::Applied)
                        || out.commit == Some(Applied::Failed)
                        || out.comp == Some(Applied::Applied));
                (out, changed)
            });
            self.with(key, |c| {
                c.composition = out.composition.clone();
                c.epoch = c.epoch.wrapping_add(1);
            });
            out
        };
        // A commit with no composition open (a mark or a number pad key in an empty field) is written the way every commit
        // is here, through a composition: first a composition that holds the text, then, in the next edit session, its end.
        // (Text written to a Win32 edit control through the bare range, or with `InsertTextAtSelection`, stays in the IMM
        // bridge's own copy and never reaches the control: found by the UX1 host scenarios.)
        // A commit together with a new composition, when a composition is open (the automatic commit of a long
        // composition, a Chinese word followed by an English run), is applied in ONE edit session (`apply_body`: commit,
        // then start the new composition). It used to be split into a synchronous commit and an asynchronous start; on the
        // IMM bridge of Win32 edit controls (classic Edit, WinForms TextBox) the committed text of the split form was
        // dropped (STABILITY T3, T3-SPLIT-plan). Only the shaped no-composition commit below still splits.
        let had_composition = composition.is_some();
        let composition = if u.commit.is_some()
            && composition.is_none()
            && std::env::var_os("TWIME_NOSPLIT").is_none()
        {
            let t = u.commit.clone().unwrap_or_default();
            let n = t.encode_utf16().count();
            let pre = run(edit::ApplyInput {
                ctx: ctx.clone(),
                composition: None,
                sink: sink.clone(),
                commit: None,
                hints: hints.as_ref().map(|h| (h.0, vec![])),
                new_text: Some((t, n)),
            });
            if pre.comp != Some(Applied::Applied) {
                return (Applied::Failed, Applied::NotApplicable);
            }
            self.with(key, |c| c.composition.clone()).flatten()
        } else {
            composition
        };
        if u.commit.is_some()
            && new_text.is_some()
            && !had_composition
            && std::env::var_os("TWIME_NOSPLIT").is_none()
        {
            let composition = self.with(key, |c| c.composition.clone()).flatten();
            let first = run(edit::ApplyInput {
                ctx: ctx.clone(),
                composition,
                sink: sink.clone(),
                commit: u.commit.clone(),
                hints: hints.as_ref().map(|h| (h.0, vec![])),
                new_text: None,
            });
            if first.commit != Some(Applied::Applied) {
                return (
                    first.commit.unwrap_or(Applied::Failed),
                    Applied::NotApplicable,
                );
            }
            // The new composition starts in its own (asynchronous) session, after the host has finished the commit.
            // A newer Update may overtake it (its synchronous write creates the composition itself): the job checks, before
            // it writes anything, that nothing happened since it was scheduled.
            let weak = SharedRef::get();
            let weak_check = weak.clone();
            let (epoch_now, seq_now) = self.with(key, |c| (c.epoch, c.seq)).unwrap_or((0, 0));
            // Dev-mode fault injection (tests of the failure contract).
            let fault = if self.link.borrow().is_dev() {
                std::env::var("TWIME_FAULT").ok()
            } else {
                None
            };
            if fault.as_deref() == Some("async-fail") {
                edit::inject_fail_after_start();
            }
            let refuse = fault.as_deref() == Some("async-refuse");
            let late = fault.as_deref() == Some("async-late");
            self.hide_hint();
            let hint_update = u.clone();
            let job = move || {
                edit::apply_async(
                    tid,
                    edit::ApplyInput {
                        ctx,
                        composition: None,
                        sink,
                        commit: None,
                        hints,
                        new_text,
                    },
                    move || {
                        let current = weak_check.upgrade().and_then(|sh| {
                            sh.with(key, |c| c.epoch == epoch_now && c.seq == seq_now)
                        }) == Some(true);
                        if !current {
                            trace::log("async composition write superseded: nothing written");
                        }
                        current
                    },
                    move |out| {
                        if let Some(sh) = weak.upgrade() {
                            if out.commit == Some(Applied::Applied)
                                || out.comp == Some(Applied::Applied)
                            {
                                sh.note_own(key);
                            }
                            sh.with(key, |c| {
                                c.composition = out.composition.clone();
                                c.epoch = c.epoch.wrapping_add(1);
                            });
                            sh.show_hint(key, &hint_update, out.comp == Some(Applied::Applied));
                            if out.comp == Some(Applied::Failed) {
                                // The composition may exist half-written: whatever the ack says, go through the unsynced cleanup.
                                sh.with(key, |c| {
                                    if let Some(a) = c.pending_ack.as_mut()
                                        && a.seq == seq_now
                                    {
                                        a.comp = Applied::Failed;
                                    }
                                });
                                sh.enter_unsynced(key);
                            }
                        }
                    },
                )
            };
            // `async-late` emulates a host that grants the asynchronous session late: the job is requested 150 ms from now.
            let scheduled = if refuse {
                false
            } else if late {
                self.late_jobs.borrow_mut().push(Box::new(move || {
                    job();
                }));
                unsafe {
                    let id = SetTimer(None, 0, 150, Some(late_timer_proc));
                    trace::log(&format!(
                        "timer.create kind=late id={id} failed={}",
                        id == 0
                    ));
                }
                true
            } else {
                job()
            };
            // The request itself was refused: report the composition write as failed right away (still the latest Update).
            return (
                Applied::Applied,
                if scheduled {
                    Applied::NotApplicable
                } else {
                    Applied::Failed
                },
            );
        }
        if u.commit.is_some()
            && new_text.is_some()
            && self.link.borrow().is_dev()
            && std::env::var("TWIME_FAULT").as_deref() == Ok("async-fail")
        {
            // Dev-mode fault injection: the new composition's write fails right after it started (after the commit stood).
            edit::inject_fail_after_start();
        }
        let out = run(edit::ApplyInput {
            ctx,
            composition,
            sink,
            commit: u.commit.clone(),
            hints,
            new_text,
        });
        (
            out.commit.unwrap_or(Applied::NotApplicable),
            out.comp.unwrap_or(Applied::NotApplicable),
        )
    }

    /// Handles the Update of a key request: apply, confirm to the engine (piggy-backed), keep the peek view.
    fn finish_update(&self, key: usize, u: Update) -> bool {
        let pop_gen = u.pop_gen;
        let notice = u.notice.clone();
        let symbols = u.symbols;
        // End the tooltip before opening either popup, including during host edits.
        if u.candidates.is_some() || symbols { self.hide_hint(); }
        let v5 = self.link.borrow().version() >= 5;
        if trace::enabled() {
            trace::log(&format!(
                "update pop_gen={pop_gen} version={} commit_chars={:?} comp_chars={:?}",
                self.link.borrow().version(),
                u.commit.as_ref().map(|c| c.chars().count()),
                u.composition.as_ref().map(|c| c.text().chars().count())
            ));
        }
        let (commit, comp) = self.apply_update(key, &u);
        self.show_candidates(key, u.candidates.as_ref(), v5 && !symbols);
        let handled = u.handled;
        let seq = self.with(key, |c| c.seq).unwrap_or(0);
        let failed = commit == Applied::Failed || comp == Applied::Failed;
        self.show_hint(key, &u, !failed && comp == Applied::Applied);
        let ux = self.link.borrow().version() >= 3;
        self.with(key, |c| {
            c.pending_ack = Some(CommitResult { seq, commit, comp });
            if failed {
                // Only the text part is forgotten; a refused commit also takes the mode back to the one the host agrees with.
                c.reset_text_view();
                c.view.ux = ux;
                c.view.mode = if commit == Applied::Failed {
                    c.confirmed_mode
                } else {
                    u.mode
                };
            } else {
                c.view = u.peek_view(ux);
            }
            if commit != Applied::Failed {
                c.confirmed_mode = u.mode;
            }
            c.last = Some(u);
        });
        if failed {
            trace::log("host write failed -> unsynced");
            self.enter_unsynced(key);
        }
        self.refresh_langbar();
        if let Some(notice) = notice {
            if let Some(ctx) = self.with(key, |c| c.ctx.clone()) {
                self.own_edit.set(self.own_edit.get() + 1);
                let r = edit::caret_rect(self.tid(), &ctx);
                self.own_edit.set(self.own_edit.get() - 1);
                if let Some((rect, owner)) = r {
                    cand::flash(owner, rect, &notice, 1500);
                    trace::log("notice shown");
                } else {
                    trace::log("notice: no caret rectangle");
                }
            }
        }
        if !failed { self.arm_pop(key, pop_gen); }
        else { trace::log("pop.arm skip=host_write_failed"); }
        handled
    }

    fn stop_pop_timer(&self) {
        let id = self.pop_timer.replace(0);
        if id != 0 {
            trace::log(&format!("pop.stop timer={id} pending={:?}", self.pop_pending.get()));
            let _ = unsafe { KillTimer(None, id) };
        }
        self.pop_pending.set(None);
    }
    fn cancel_pop(&self) {
        self.stop_pop_timer();
        self.clear_completion();
    }
    fn clear_completion(&self) {
        let had_completion = self.completion.borrow_mut().take().is_some();
        self.pop_down.set(None);
        if had_completion {
            self.hint_epoch.set(self.hint_epoch.get().wrapping_add(1));
            let hint = self.hint.borrow_mut().take();
            if let Some(mut hint) = hint { hint.hide(); *self.hint.borrow_mut() = Some(hint); }
        }
    }
    fn pop_key(&self, vk: usize) {
        if matches!(vk, 0x10..=0x12 | 0x5b | 0x5c | 0xa0..=0xa5) { return; }
        let mut down = None;
        if vk == 0x28 {
            if self.hint.borrow().as_ref().is_some_and(|h| h.visible()) {
                down = self.completion.borrow().as_ref().map(|(k,g,session,h)| (*k,*g,*session,h.id));
            }
            if down.is_none() { down = self.pop_down.get(); }
        }
        self.hide_hint();
        self.pop_down.set(down);
    }
    fn arm_pop(&self, key: usize, generation: u32) {
        self.stop_pop_timer();
        if generation == 0 || self.link.borrow().version() < 7 || !self.is_ready(key) || self.focus.get() != key {
            if trace::enabled() { trace::log(&format!("pop.arm skip=gate ctx={} pop_gen={generation} version={} ready={} focus={} keyboard_focus={}", self.context_id(key), self.link.borrow().version(), self.is_ready(key), self.context_id(self.focus.get()), self.keyboard_focus.get())); }
            return;
        }
        let Some(session) = self.with(key, |c| c.session) else { trace::log("pop.arm skip=no_session"); return; };
        let id = unsafe { SetTimer(None, 0, POP_DEBOUNCE_MS, Some(pop_timer_proc)) };
        trace::log(&format!("pop.arm ctx={} session={session} connection={} pop_gen={generation} timer={id} failed={}", self.context_id(key), self.link.borrow().generation(), id == 0));
        if id != 0 {
            self.pop_pending.set(Some((key, self.link.borrow().generation(), session, generation)));
            self.pop_timer.set(id);
        }
    }
    fn on_pop_timer(&self) {
        let pending = self.pop_pending.get();
        self.stop_pop_timer();
        let Some((key, connection, session, generation)) = pending else { trace::log("pop.timer skip=no_pending"); return; };
        if trace::enabled() {
            let state = self.with(key, |c| (c.session, c.sync_gen, c.unsynced, c.cleanup_pending, c.disabled, c.queue.len(), c.composition.is_some(), c.seq, c.pending_ack));
            trace::log(&format!("pop.timer ctx={} connection={connection} session={session} pop_gen={generation} state(session,sync_gen,unsynced,cleanup,disabled,queue,composition,seq,ack)={state:?} keyboard_focus={} focus={} busy={} own_edit={} current_connection={}", self.context_id(key), self.keyboard_focus.get(), self.context_id(self.focus.get()), self.request_busy.get(), self.own_edit.get(), self.link.borrow().generation()));
        }
        let ready = self.with(key, |c| c.session == session && c.sync_gen == connection && !c.unsynced && !c.cleanup_pending && !c.disabled
            && c.queue.is_empty() && c.composition.is_some()
            && c.pending_ack.as_ref().is_some_and(|a| a.seq == c.seq && a.comp == Applied::Applied));
        if ready != Some(true) || !self.keyboard_focus.get() || self.focus.get() != key || self.request_busy.get()
            || self.own_edit.get() != 0 || self.link.borrow().generation() != connection { trace::log(&format!("pop.timer skip=gate ready={ready:?}")); return; }
        let revision = self.hint_epoch.get();
        trace::log(&format!("pop.idle send pop_gen={generation}"));
        let Some(u) = self.exchange(key, Event::Idle(generation)) else {
            trace::log("pop.timer skip=exchange_failed"); self.hide_hint(); self.fail_over(key); return;
        };
        trace::log(&format!("pop.idle reply pop_gen={} completion={:?}", u.pop_gen, u.completion.as_ref().map(|h| (h.id, h.options.len(), h.valid()))));
        if self.link.borrow().generation() != connection || self.with(key, |c| c.session) != Some(session) { trace::log("pop.timer skip=connection_or_session_changed"); return; }
        let seq = self.with(key, |c| c.seq).unwrap_or(0);
        self.with(key, |c| c.pending_ack = Some(CommitResult { seq, commit: Applied::NotApplicable, comp: Applied::NotApplicable }));
        if self.hint_epoch.get() != revision || self.link.borrow().generation() != connection || self.focus.get() != key { trace::log("pop.timer skip=revision_connection_or_focus_changed"); return; }
        if let Some(h) = u.completion.filter(|h| h.valid() && h.id == generation) {
            *self.completion.borrow_mut() = Some((key, connection, session, h));
            self.redraw_completion(key);
        } else { trace::log("pop.timer skip=no_valid_matching_completion"); }
    }
    fn redraw_completion(&self, key: usize) -> bool {
        let current = self.completion.borrow().clone();
        let Some((k, connection, session, h)) = current else { trace::log("pop.redraw skip=no_completion"); return false; };
        if k != key || !self.completion_valid(key, connection, session, h.id) {
            trace::log("pop.redraw skip=invalid_completion"); self.clear_completion(); self.hide_hint_window(); return false;
        }
        let revision = self.hint_epoch.get();
        let Some((ctx, Some(comp), epoch, true)) = self.with(key, |c| (c.ctx.clone(), c.composition.clone(), c.epoch,
            !c.unsynced && !c.cleanup_pending && !c.disabled && !c.view.candidates_open && !c.view.symbols)) else { trace::log("pop.redraw skip=context_gate"); self.clear_completion(); self.hide_hint_window(); return false; };
        let tm = self.core.borrow().tm.clone();
        let geometry = edit::composition_rect(self.tid(), &ctx, &comp);
        if self.hint_epoch.get() != revision || !self.completion_valid(key, connection, session, h.id) || self.with(key, |c| c.epoch == epoch) != Some(true) { trace::log("pop.redraw skip=stale_geometry"); return true; }
        if let (Some(tm), Some((rect, owner))) = (tm, geometry) {
            let hint = self.hint.borrow_mut().take();
            if let Some(mut hint) = hint {
                hint.update_completion(&tm, owner, rect, &h);
                trace::log(&format!("pop.redraw updated visible={}", hint.visible()));
                if self.hint_epoch.get() != revision || !self.completion_valid(key, connection, session, h.id) { hint.hide(); }
                else if hint.visible() { trace::log(&format!("completion.visible ctx={} connection={connection} pop_gen={}", self.context_id(key), h.id)); }
                *self.hint.borrow_mut() = Some(hint);
            } else { trace::log("pop.redraw skip=no_hint"); }
        } else {
            trace::log("pop.redraw skip=no_geometry_or_tm");
            let hint = self.hint.borrow_mut().take();
            if let Some(mut hint) = hint { hint.hide(); *self.hint.borrow_mut() = Some(hint); }
        }
        true
    }
    fn redraw_hints(&self, key: usize) {
        if self.redraw_completion(key) { return; }
        let update = self.with(key, |c| {
            (!c.unsynced && !c.cleanup_pending && c.composition.is_some()
                && c.pending_ack.as_ref().is_some_and(|a| a.seq == c.seq && a.comp != Applied::Failed && a.commit != Applied::Failed))
                .then(|| c.last.clone()).flatten()
        }).flatten();
        if let Some(update) = update { self.show_hint(key, &update, true); }
        else { self.hide_hint_window(); }
    }

    fn completion_valid(&self, key: usize, connection: u32, session: u32, id: u32) -> bool {
        self.focus.get() == key && self.keyboard_focus.get() && self.is_ready(key)
            && self.link.borrow().generation() == connection
            && self.completion.borrow().as_ref().is_some_and(|(k,g,s,h)| *k == key && *g == connection && *s == session && h.id == id && h.valid())
            && self.with(key, |c| c.session == session && !c.unsynced && !c.cleanup_pending && !c.disabled && c.composition.is_some() && !c.view.candidates_open && !c.view.symbols) == Some(true)
    }

    fn lexux_attributes(&self) -> bool { self.link.borrow().version() >= 6 }

    fn hide_hint(&self) {
        self.cancel_pop();
        self.hide_hint_window();
    }

    // Presentation changes (including LEXUX redraws) are not POP cancellation events.
    fn hide_hint_window(&self) {
        self.hint_epoch.set(self.hint_epoch.get().wrapping_add(1));
        let current = self.hint.borrow_mut().take();
        if let Some(mut hint) = current {
            hint.hide();
            *self.hint.borrow_mut() = Some(hint);
        } else { cand::hint_hide(); }
    }

    fn show_hint(&self, key: usize, u: &Update, applied: bool) {
        let revision = self.hint_epoch.get().wrapping_add(1);
        self.hint_epoch.set(revision);
        let next = u.hint.as_ref().filter(|h| h.valid());
        let visible = applied && self.link.borrow().version() >= 6
            && self.keyboard_focus.get() && self.focus.get() == key
            && crate::hint::available(next, u.candidates.is_some(), u.symbols);
        if !visible { self.hide_hint_window(); return; }
        let Some((ctx, Some(comp), seq, epoch)) = self.with(key, |c| (c.ctx.clone(), c.composition.clone(), c.seq, c.epoch)) else {
            self.hide_hint_window(); return;
        };
        let tm = self.core.borrow().tm.clone();
        let geometry = edit::composition_rect(self.tid(), &ctx, &comp);
        if self.hint_epoch.get() != revision || self.with(key, |c| c.seq == seq && c.epoch == epoch) != Some(true) { return; }
        if let (Some(tm), Some((rect, owner))) = (tm, geometry) {
            let current = self.hint.borrow_mut().take();
            if let Some(mut hint) = current {
                hint.update(&tm, owner, rect, next.unwrap());
                if self.hint_epoch.get() != revision || !self.keyboard_focus.get() || self.focus.get() != key {
                    hint.hide();
                }
                *self.hint.borrow_mut() = Some(hint);
            } else { self.hide_hint_window(); }
        } else { self.hide_hint_window(); }
    }

    /// Shows the candidate window under the composition, or hides it.
    fn show_candidates(&self, key: usize, cands: Option<&Cands>, add: bool) {
        let Some(c) = cands else {
            cand::hide();
            return;
        };
        let Some((ctx, comp)) = self.with(key, |c| (c.ctx.clone(), c.composition.clone())) else {
            return;
        };
        let Some(comp) = comp else {
            cand::hide();
            return;
        };
        self.own_edit.set(self.own_edit.get() + 1);
        let r = edit::composition_rect(self.tid(), &ctx, &comp);
        self.own_edit.set(self.own_edit.get() - 1);
        let Some((rect, owner)) = r else {
            cand::hide();
            return;
        };
        let weak = SharedRef::get();
        let weak_add = weak.clone();
        // Page indicator only from an engine that reported the totals (protocol version 2).
        let page =
            (self.link.borrow().version() >= 2 && c.total > 0 && c.per_page > 0).then(|| {
                (
                    (c.page as usize + 1),
                    (c.total as usize).div_ceil(c.per_page as usize).max(1),
                )
            });
        trace::log(&format!("cands page={page:?}"));
        cand::show(
            owner,
            rect,
            &c.items,
            c.selected as usize,
            page,
            Box::new(move |i| {
                if let Some(sh) = weak.upgrade() {
                    sh.pick(key, i);
                }
            }),
            add.then(|| {
                Box::new(move || {
                    if let Some(sh) = weak_add.upgrade() {
                        let supported = sh.link.borrow().version() >= 5;
                        if sh.is_ready(key)
                            && supported
                            && sh.flush_queue(key)
                            && let Some(u) = sh.exchange(key, Event::AddToLexicon)
                        {
                            sh.finish_update(key, u);
                        } else {
                            sh.fail_over(key);
                        }
                    }
                }) as Box<dyn Fn()>
            }),
        );
    }

    /// A click on a candidate.
    fn pick(&self, key: usize, i: usize) {
        if self.flush_queue(key)
            && let Some(u) = self.exchange(key, Event::CandidatePick(i as u16))
        {
            self.finish_update(key, u);
        } else {
            self.fail_over(key);
        }
    }

    fn enter_unsynced(&self, key: usize) {
        self.with(key, |c| {
            c.unsynced = true;
            c.queue.push(Event::Unsynced);
        });
        self.try_cleanup(key);
    }

    /// Ends the host composition without committing it. True when the context is clean afterwards.
    fn try_cleanup(&self, key: usize) -> bool {
        let Some((ctx, comp)) = self.with(key, |c| (c.ctx.clone(), c.composition.clone())) else {
            return true;
        };
        let ok = match comp {
            None => true,
            Some(comp) => {
                let r = self.own_write(key, || {
                    let r = edit::cancel_composition(self.tid(), &ctx, &comp, true, self.lexux_attributes());
                    let ok = r.is_ok();
                    (r, ok)
                });
                r.is_ok()
            }
        };
        if ok {
            self.with(key, |c| {
                c.composition = None;
                c.unsynced = false;
                c.cleanup_pending = false;
                c.cancel_reason = false;
                c.reset_text_view();
                c.epoch = c.epoch.wrapping_add(1);
            });
        }
        trace::log(&format!("cleanup ok={ok}"));
        ok
    }

    /// Engine lost (plans 4.1 d): this key is not swallowed; the composition is committed if it was fully
    /// convertible and nothing says otherwise, else cancelled; the session then passes keys through.
    /// Tells the engine this session is over (clean end). Best effort, one request.
    fn close_session(&self, key: usize) {
        let owns_pop = self.pop_pending.get().is_some_and(|(k, _, _, _)| k == key)
            || self.completion.borrow().as_ref().is_some_and(|(k, _, _, _)| *k == key);
        if owns_pop { self.hide_hint(); }
        let Some((session, seq)) = self.with(key, |c| {
            c.seq = c.seq.wrapping_add(1);
            (c.session, c.seq)
        }) else {
            return;
        };
        let _ = self.link.borrow_mut().call(&Request { session, seq, ack: None, event: Event::Close });
    }

    fn fail_over(&self, key: usize) {
        self.hide_hint();
        cand::hide();
        let Some((ctx, comp, text, cancel)) = self.with(key, |c| {
            let text = c.last.as_ref().filter(|u| u.committable).and_then(|u| u.composition.as_ref().map(|k| k.text()));
            (c.ctx.clone(), c.composition.clone(), text, c.cancel_reason || c.unsynced)
        }) else {
            return;
        };
        if let Some(comp) = comp {
            let r = self.own_write(key, || {
                let r = match (&text, cancel) {
                    (Some(t), false) => edit::commit_composition(self.tid(), &ctx, &comp, t, self.lexux_attributes()),
                    _ => edit::cancel_composition(self.tid(), &ctx, &comp, true, self.lexux_attributes()),
                };
                let ok = r.is_ok();
                (r, ok)
            });
            trace::log(&format!("fail_over text_some={} cancel={} commit={} ok={}", text.is_some(), cancel, text.is_some() && !cancel, r.is_ok()));
            self.with(key, |c| {
                if r.is_ok() {
                    c.composition = None;
                } else {
                    c.unsynced = true;
                }
                c.epoch = c.epoch.wrapping_add(1);
            });
        }
        self.with(key, |c| {
            c.reset_text_view();
            c.queue.clear();
            c.pending_ack = None;
            c.last = None;
        });
        if self.disabled_seen.replace(false) {
            self.close_session(key);
        }
    }

    // ---- key events -------------------------------------------------------------------------------------------

    /// Re-reads the "keyboard disabled" compartment (password and similar fields): when it flips the engine is told,
    /// and the composition is dropped on the way in.
    fn refresh_disabled(&self, key: usize, pic: &ITfContext) {
        let now = keyboard_disabled(pic, self.context_id(key));
        let changed = self.with(key, |c| {
            let ch = c.disabled != now;
            c.disabled = now;
            if ch {
                c.queue.push(Event::Password(now));
                c.reset_text_view();
            }
            ch
        });
        if changed == Some(true) {
            trace::log(&format!("keyboard disabled now {now}"));
            if now {
                self.try_cleanup(key);
            }
        }
    }

    fn test_key(&self, pic: Option<&ITfContext>, wparam: usize, lparam: isize) -> bool {
        self.pop_key(wparam);
        let trace_pic = pic;
        self.key_sync(pic);
        self.first_cb.set(Some(Instant::now()));
        self.link.borrow_mut().ensure();
        if self.link.borrow().marker_present() {
            // Uninstall in progress: never swallow a key. Whatever composition exists is ended once (plans 4.1) and the
            // session closed so the engine sees a clean end; this is the only engine traffic from a test call.
            if self.link.borrow().is_up() {
                let _ = pic;
                let keys: Vec<usize> = self.ctxs.borrow().iter().map(|c| c.raw).collect();
                for k in keys {
                    self.fail_over(k);
                    self.close_session(k);
                }
                self.link.borrow_mut().drop_connection();
            }
            self.trace_key(trace_pic, wparam, true, true, false, "marker");
            return false;
        }
        let Some(pic) = pic else {
            self.trace_key(trace_pic, wparam, true, true, false, "no_context");
            return false;
        };
        let key = self.ctx_key(pic);
        self.refresh_disabled(key, pic);
        self.track_shift(key, wparam, lparam);
        // Unknown host state: try to end the old composition now (a TSF edit session, no engine traffic) so keys do
        // not stay blocked forever once the host accepts writes again.
        if self.with(key, |c| (c.unsynced || c.cleanup_pending) && !c.disabled) == Some(true) {
            self.try_cleanup(key);
        }
        // The engine is gone while a composition is open (this key test found the connection dead): end the composition now
        // (committed when it is fully convertible, else cancelled; a synchronous edit session, no engine traffic) and let the
        // key pass, as `key_down` does when it finds the engine gone.
        // "Gone" includes a connection that was dead and has already been replaced by a new engine (another generation): the
        // composition belongs to the old session either way.
        if !self.is_ready(key)
            && self.with(key, |c| {
                c.composition.is_some() && !c.unsynced && !c.cleanup_pending && !c.disabled
            }) == Some(true)
        {
            trace::log("engine gone (or replaced) at key test with a composition open");
            self.fail_over(key);
        }
        // Not synchronised with the engine yet (new or restarted connection): every key passes through, Shift taps
        // included; a synchronisation attempt is asked for right away (it cannot run in this callback).
        if !self.is_ready(key) {
            if self.with(key, |c| !c.disabled && !c.unsynced && !c.cleanup_pending) == Some(true) {
                if self.link.borrow().is_up() {
                    self.nudge_sync();
                } else {
                    self.schedule_sync(self.retry_delay());
                }
            }
            trace::note_unready("key_test_not_ready");
            self.trace_key(trace_pic, wparam, true, true, false, "not_ready");
            return false;
        }
        if self
            .with(key, |c| c.unsynced || c.cleanup_pending || c.disabled)
            .unwrap_or(true)
        {
            self.trace_key(trace_pic, wparam, true, true, false, "blocked");
            return false;
        }
        let r = self
            .with(key, |c| peek(&c.view, key_event(wparam, true, c.view.ux)))
            .unwrap_or(false);
        r && self.link.borrow().is_up()
    }

    /// Shift single tap bookkeeping (no engine traffic): another context starts a new round, Shift arms, any other key
    /// spoils. Runs from `OnTestKeyDown` and from `OnKeyDown`: a real host (Notepad on Windows 11, measured) calls
    /// `OnKeyDown` without a preceding `OnTestKeyDown` for keys the TIP does not claim, and then neither test call nor
    /// `OnTestKeyUp` happens. Calling it twice for the same key is harmless.
    fn track_shift(&self, key: usize, wparam: usize, lparam: isize) {
        if self.last_ctx.replace(key) != key {
            self.shift.borrow_mut().reset_reason("context reset");
        }
        // A press that starts while the context is not ready can never become a tap, even if it is ready by the release.
        let ready_now = self.is_ready(key)
            && self
                .with(key, |c| {
                    c.view.ux && !c.unsynced && !c.cleanup_pending && !c.disabled
                })
                .unwrap_or(false);
        let mut t = self.shift.borrow_mut();
        if is_shift_vk(wparam) {
            t.shift_down(
                Instant::now(),
                lparam & (1 << 30) == 0,
                other_shift_held(wparam, lparam),
                command_held(),
                ready_now,
            );
        } else {
            if matches!(wparam, 0x11 | 0x12 | 0x5b | 0x5c | 0xa2..=0xa5) { t.other_key_down_reason("modifier"); } else { t.other_key_down(); }
        }
    }

    fn key_down(&self, pic: Option<&ITfContext>, wparam: usize, lparam: isize) -> bool {
        self.pop_key(wparam);
        let completion_down = self.pop_down.take();
        let trace_pic = pic;
        self.key_sync(pic);
        let t0 = self.first_cb.take().unwrap_or_else(Instant::now);
        self.link.borrow_mut().ensure();
        let Some(pic) = pic else {
            self.trace_key(trace_pic, wparam, false, true, false, "no_context");
            return false;
        };
        let key = self.ctx_key(pic);
        if self.link.borrow().marker_present() {
            self.fail_over(key);
            self.close_session(key);
            self.link.borrow_mut().drop_connection();
            self.trace_key(trace_pic, wparam, false, true, false, "marker");
            return false;
        }
        self.refresh_disabled(key, pic);
        self.track_shift(key, wparam, lparam);
        let state = self.with(key, |c| {
            (
                c.unsynced || c.cleanup_pending,
                c.disabled,
                c.view,
                c.composition.is_some(),
            )
        });
        let Some((bad, disabled, view, has_comp)) = state else {
            self.trace_key(trace_pic, wparam, false, true, false, "missing_state");
            return false;
        };
        if disabled {
            self.trace_key(trace_pic, wparam, false, true, false, "disabled");
            return false;
        }
        if bad {
            // Unknown host state: never swallow; keep trying to end the old composition.
            self.try_cleanup(key);
            self.trace_key(trace_pic, wparam, false, true, false, "unsynced_or_cleanup");
            return false;
        }
        if !self.link.borrow().is_up() {
            if has_comp {
                self.fail_over(key);
            }
            trace::note_unready("key_link_down");
            self.trace_key(trace_pic, wparam, false, true, false, "link_down");
            return false;
        }
        if !self.is_ready(key) {
            if has_comp {
                self.fail_over(key);
            }
            trace::note_unready("key_not_ready");
            self.trace_key(trace_pic, wparam, false, true, false, "not_ready");
            return false;
        }
        let ev = key_event(wparam, true, view.ux);
        if !peek(&view, ev) {
            self.trace_key(trace_pic, wparam, false, true, false, "peek_passthrough");
            return false;
        }
        if !self.flush_queue(key) {
            self.link.borrow_mut().drop_connection();
            self.fail_over(key);
            self.trace_key(trace_pic, wparam, false, true, false, "queue_failed");
            return false;
        }
        let (mut doc_units, mut doc_us, mut doc_ok) = (None::<usize>, 0u128, false);
        // CACHE plan 3.2: a bare bopomofo key that starts a new composition first sends the document text before the caret
        // (only when negotiated capabilities allow it; empty when it cannot be read). Only its length is traced.
        if self.link.borrow().doc_text_allowed()
            && !has_comp
            && view.ux
            && !view.composing
            && !view.run
            && !view.symbols
            && view.mode == Mode::Chinese
            && !ev.mods.shift()
            && !ev.mods.caps()
            && twime_proto::is_zhuyin_key(ev.key)
        {
            let td = Instant::now();
            let read = edit::text_before(self.tid(), pic, twime_proto::DOC_TEXT_UNITS);
            doc_ok = read.is_some();
            let text = read.unwrap_or_default();
            doc_units = Some(text.encode_utf16().count());
            let Some(u) = self.exchange(key, Event::DocText(text)) else {
                trace::log("engine lost on doctext");
                self.fail_over(key);
                self.trace_key(trace_pic, wparam, false, true, false, "doctext_failed");
                return false;
            };
            self.finish_update(key, u);
            doc_us = td.elapsed().as_micros();
        }
        let event = completion_down.filter(|(k, g, session, _)| *k == key && *g == self.link.borrow().generation() && self.with(key, |c| c.session) == Some(*session) && ev.key == twime_proto::Key::Down && !ev.mods.command())
            .map_or(Event::Key(ev), |(_, _, _, id)| Event::DownCompletion(id));
        let Some(update) = self.exchange(key, event) else {
            trace::log("engine lost on key");
            self.fail_over(key);
            self.trace_key(trace_pic, wparam, false, true, false, "key_failed");
            return false;
        };
        let t_engine = t0.elapsed();
        let finished = |u: &Update| {
            u.composition
                .as_ref()
                .map_or(0, |c| c.segments.iter().filter(|s| !s.pending).count())
        };
        let trig = self
            .with(key, |c| c.last.as_ref().map_or(0, finished))
            .unwrap_or(0)
            != finished(&update)
            && update.engine_us > 300;
        let engine_us = update.engine_us;
        let eaten = self.finish_update(key, update);
        if trace::enabled() {
            let doc = doc_units.map_or(String::new(), |n| {
                format!(
                    " doc=1 doc_ok={} doc_units={n} doc_us={doc_us}",
                    u8::from(doc_ok)
                )
            });
            trace::log(&format!(
                "KEYDOWN vk={:#x} eaten={eaten} total_us={} until_reply_us={} engine_us={engine_us} trig={}{doc}",
                wparam,
                t0.elapsed().as_micros(),
                t_engine.as_micros(),
                u8::from(trig)
            ));
        }
        eaten
    }

    // ---- Shift single tap -----------------------------------------------------------

    /// What is known at the moment of a Shift release.
    fn shift_release(
        &self,
        pic: Option<&ITfContext>,
        wparam: usize,
        lparam: isize,
    ) -> (Release, Option<usize>) {
        let key = pic.map(|p| p.as_raw() as usize).filter(|k| {
            self.ctxs
                .try_borrow()
                .map(|v| v.iter().any(|c| c.raw == *k))
                .unwrap_or(false)
        });
        let ready = key.is_some_and(|k| {
            self.is_ready(k)
                && self
                    .with(k, |c| {
                        c.view.ux && !c.unsynced && !c.cleanup_pending && !c.disabled
                    })
                    .unwrap_or(false)
        });
        let rel = Release {
            other_shift_held: other_shift_held(wparam, lparam),
            command_held: command_held(),
            enabled: if self.link.borrow().version() >= 5 {
                key.and_then(|k| {
                    self.with(k, |c| crate::shift::toggle_enabled(c.toggle_key, lparam))
                })
                .unwrap_or(false)
            } else {
                self.link.borrow().settings().shift_tap
            },
            ready,
            caps: caps_lock_on(),
        };
        (rel, key)
    }

    /// OnTestKeyUp exceptionally performs IPC for a Shift tap because CUAS hosts (mintty) do not call OnKeyUp.
    fn test_key_up(&self, pic: Option<&ITfContext>, wparam: usize, lparam: isize) -> bool {
        let trace_pic = pic;
        self.key_sync(pic);
        if !is_shift_vk(wparam) {
            self.trace_key(trace_pic, wparam, true, false, false, "non_shift");
            return false;
        }
        let (rel, key) = self.shift_release(pic, wparam, lparam);
        let t0 = Instant::now();
        let yes = self.shift.borrow_mut().test_up(t0, &rel);
        if yes && let Some(key) = key {
            // The Armed state and its RefCell borrow are consumed before IPC or TSF can re-enter.
            trace::log("shift.toggle via=testkeyup");
            self.toggle_mode(key);
            if trace::enabled() {
                trace::log(&format!("TOGGLE total_us={}", t0.elapsed().as_micros()));
            }
        }
        trace::log(&format!("TestKeyUp shift eligible={yes}"));
        yes
    }

    /// `OnKeyUp`: a clean Shift tap sends exactly one `ToggleMode`. Always lets the key-up through.
    fn key_up(&self, pic: Option<&ITfContext>, wparam: usize, lparam: isize) {
        let trace_pic = pic;
        self.key_sync(pic);
        if !is_shift_vk(wparam) {
            self.trace_key(trace_pic, wparam, false, false, false, "non_shift");
            return;
        }
        let t0 = Instant::now();
        let (rel, key) = self.shift_release(pic, wparam, lparam);
        let tap = self.shift.borrow_mut().key_up(Instant::now(), &rel);
        trace::log(&format!("KeyUp shift tap={tap}"));
        if tap && let Some(key) = key {
            trace::log("shift.toggle via=keyup");
            self.toggle_mode(key);
            if trace::enabled() {
                trace::log(&format!("TOGGLE total_us={}", t0.elapsed().as_micros()));
            }
        }
    }

    /// Sends `ToggleMode`, applies the commit it may carry and shows the mode label.
    fn toggle_mode(&self, key: usize) {
        let before = self.with(key, |c| c.view.mode);
        if !self.flush_queue(key) {
            self.link.borrow_mut().drop_connection();
            self.fail_over(key);
            return;
        }
        let Some(update) = self.exchange(key, Event::ToggleMode) else {
            trace::log("engine lost on ToggleMode");
            self.fail_over(key);
            return;
        };
        let mode = update.mode;
        self.finish_update(key, update);
        trace::log(&format!("ToggleMode sent, mode now {mode:?}"));
        let ok = self.with(key, |c| !c.unsynced).unwrap_or(false);
        if ok && before != Some(mode) {
            self.flash_mode(key, mode);
        }
    }

    /// Asks for the small label at the insertion point (about 600 ms): shown from a timer a moment later, off the key path.
    fn flash_mode(&self, key: usize, mode: Mode) {
        self.label_pending.set(Some((key, mode)));
        if self.label_timer.get() == 0 {
            let id = unsafe { SetTimer(None, 0, 1, Some(label_timer_proc)) };
            self.label_timer.set(id);
            trace::log(&format!(
                "timer.create kind=label id={id} failed={}",
                id == 0
            ));
        }
    }

    fn show_label(&self) {
        let id = self.label_timer.replace(0);
        if id != 0 {
            unsafe {
                let _ = KillTimer(None, id);
            }
        }
        let Some((key, mode)) = self.label_pending.take() else {
            return;
        };
        let Some(ctx) = self.with(key, |c| c.ctx.clone()) else {
            return;
        };
        self.own_edit.set(self.own_edit.get() + 1);
        let r = edit::caret_rect(self.tid(), &ctx);
        self.own_edit.set(self.own_edit.get() - 1);
        let Some((rect, owner)) = r else {
            trace::log("mode label: no caret rectangle");
            return;
        };
        cand::flash(
            owner,
            rect,
            if mode == Mode::English { "英" } else { "中" },
            600,
        );
        trace::log(&format!("mode label {mode:?}"));
    }

    // ---- focus / document / edit notifications --------------------------------------------------------------------

    fn attach_doc(&self, dim: &ITfDocumentMgr) {
        match unsafe { dim.GetTop() } {
            Ok(top) => self.attach_context(&top, "attach_doc"),
            Err(e) => {
                trace::log(&format!("focus.lookup reason=get_top hr={:?}", e.code()));
                self.set_focus(0, "empty_document");
                self.schedule_sync(self.retry_delay());
            }
        }
    }

    fn attach_context(&self, context: &ITfContext, reason: &str) {
        let key = self.ctx_key(context);
        let (_, sync) = sync_target(self.focus.get(), key, self.is_ready(key), SyncSource::Push(true));
        self.set_focus(key, reason);
        self.keyboard_focus.set(true);
        self.shift.borrow_mut().reset();
        if sync && !self.sync_ctx(key) && self.with(key, |c| !c.disabled) == Some(true) {
            self.schedule_sync(1000);
        }
        self.stop_settings_timer();
        self.schedule_settings(1);
    }

    /// Focus left the context: commit a fully convertible composition (else cancel), clear the engine's context.
    fn focus_leave(&self, key: usize) {
        self.hide_hint();
        cand::hide();
        self.shift.borrow_mut().reset();
        if self.focus.get() == key {
            self.set_focus(0, "focus_leave");
            self.sync_attempts.set(0);
            self.stop_sync_timer();
            self.stop_settings_timer();
        }
        self.with(key, |c| c.epoch = c.epoch.wrapping_add(1)); // pending asynchronous composition writes are void
        let has_comp = self.with(key, |c| c.composition.is_some()).unwrap_or(false);
        let (unsynced, cancel) = self
            .with(key, |c| (c.unsynced || c.cleanup_pending, c.cancel_reason))
            .unwrap_or((false, false));
        if unsynced {
            self.try_cleanup(key);
            return;
        }
        if cancel && has_comp {
            self.try_cleanup(key);
            return;
        }
        if self.link.borrow().is_up() && self.flush_queue(key) {
            match self.exchange(key, Event::Focus(false)) {
                Some(u) => {
                    self.finish_update(key, u);
                }
                None => self.fail_over(key),
            }
        } else if has_comp {
            self.fail_over(key);
        }
        self.with(key, |c| c.reset_text_view());
    }

    fn end_edit(&self, pic: &ITfContext, rec: &ITfEditRecord) {
        if self.own_edit.get() > 0 {
            return;
        }
        let key = pic.as_raw() as usize;
        let echo = self.take_own(key);
        trace::log(&format!("OnEndEdit echo={echo}"));
        if echo {
            return;
        }
        if self.ctxs.try_borrow().map(|v| v.iter().all(|c| c.raw != key)).unwrap_or(true) {
            return;
        }
        let changed = unsafe {
            let sel = rec.GetSelectionStatus().map(|b| b.as_bool()).unwrap_or(false);
            let text = rec
                .GetTextAndPropertyUpdates(TF_GTP_INCL_TEXT, &[])
                .ok()
                .and_then(|e| {
                    let mut r = [None];
                    let mut n = 0u32;
                    e.Next(&mut r, &mut n).ok()?;
                    Some(n > 0)
                })
                .unwrap_or(false);
            sel || text
        };
        trace::log(&format!("OnEndEdit changed={changed}"));
        if !changed {
            return;
        }
        trace::log("external edit");
        if self.focus.get() == key { self.hide_hint(); }
        let term = self.with(key, |c| c.term_loc.take()).flatten();
        if let Some((text, t)) = term
            && t.elapsed() < std::time::Duration::from_millis(400)
        {
            // The host ended a convertible composition because of this edit: cancel it (plan 4.1).
            let weak = SharedRef::get();
            let ctx = pic.clone();
            let ctx2 = ctx.clone();
            let r = edit::run_session(&ctx, self.tid(), false, move |ec| {
                let res = edit::delete_unique(ec, &ctx2, &text);
                trace::log(&format!("cancel ended composition: deleted={res:?}"));
                if matches!(&res, Ok(true))
                    && let Some(sh) = weak.upgrade()
                {
                    sh.note_own(key);
                }
                res.map(|_| ())
            });
            trace::log(&format!("cancel ended composition after external edit: requested ok={}", r.is_ok()));
        }
        let composing = self.with(key, |c| {
            c.queue.retain(|e| *e != Event::ExternalEdit);
            c.queue.push(Event::ExternalEdit);
            c.last = None;
            c.reset_text_view();
            // A composition write that is still waiting for its edit session belongs to the text as it was: drop it.
            c.epoch = c.epoch.wrapping_add(1);
            c.composition.is_some()
        });
        if composing == Some(true) {
            self.with(key, |c| {
                c.cancel_reason = true;
                c.cleanup_pending = true;
            });
            self.schedule_cleanup(key);
        }
    }

    /// Asynchronous cancel of the composition after an external edit (a write needs its own edit session).
    fn schedule_cleanup(&self, key: usize) {
        let Some((ctx, comp, epoch)) =
            self.with(key, |c| (c.ctx.clone(), c.composition.clone(), c.epoch))
        else {
            return;
        };
        let Some(comp) = comp else { return };
        let tid = self.tid();
        // The job runs later inside the host's edit session; it only touches the table through a fresh borrow.
        let me = SharedRef::get();
        let comp2 = comp.clone();
        let r = edit::run_session(&ctx, tid, false, move |ec| unsafe {
            let Some(sh) = me.upgrade() else {
                return Ok(());
            };
            let current = sh.with(key, |c| {
                (c.epoch, c.composition.as_ref().map(|x| x.as_raw() as usize))
            });
            if current != Some((epoch, Some(comp2.as_raw() as usize))) {
                return Ok(()); // a newer composition or a state change superseded this cleanup
            }
            sh.own_edit.set(sh.own_edit.get() + 1);
            let res = (|| -> Result<()> {
                let range = comp2.GetRange()?;
                range.SetText(ec, 0, &[])?;
                comp2.EndComposition(ec)
            })();
            sh.own_edit.set(sh.own_edit.get() - 1);
            if res.is_ok() {
                sh.note_own(key);
                sh.with(key, |c| {
                    c.composition = None;
                    c.cleanup_pending = false;
                    c.cancel_reason = false;
                    c.epoch = c.epoch.wrapping_add(1);
                });
            }
            res
        });
        trace::log(&format!("schedule_cleanup requested ok={}", r.is_ok()));
    }

    fn composition_terminated(&self, ec: u32, comp: &ITfComposition) {
        // EndComposition from our own write is not a host cancellation.
        if self.own_edit.get() > 0 { return; }
        self.hide_hint();
        let raw = comp.as_raw() as usize;
        let key = self.ctxs.try_borrow().ok().and_then(|v| {
            v.iter()
                .find(|c| c.composition.as_ref().map(|x| x.as_raw() as usize) == Some(raw))
                .map(|c| c.raw)
        });
        let Some(key) = key else { return };
        if self.link.borrow().version() >= 6 {
            if let Some(ctx) = self.with(key, |c| c.ctx.clone()) {
                if let Ok(range) = unsafe { comp.GetRange() } {
                    let _ = edit::clear_attributes(&ctx, ec, &range);
                }
            }
        }
        // Plan 4.1: a composition the host ends on its own (external edit, selection move, ...) is cancelled, never committed.
        // (Focus changes are handled earlier, in `focus_leave`, which commits a convertible composition first.)
        let convertible = self
            .with(key, |c| {
                !(c.cancel_reason || c.unsynced) && c.last.as_ref().is_some_and(|u| u.committable)
            })
            .unwrap_or(false);
        // A convertible composition may have been ended by a focus change (TSF ends it before `OnSetFocus`; then the text stays
        // as committed, plan 4.1 c) or by an external edit / selection move (then it is cancelled, 4.1). The host does not say
        // which: keep the range and decide when the next notification arrives.
        // Told apart by the caret: after a selection move / click it is no longer at the end of the ended composition.
        // Told apart by the window: when the keyboard focus is no longer in the window this context belongs to, the
        // composition ended because of a focus change (the text then stays as committed, plan 4.1 c); otherwise the host
        // ended it because of an edit or a selection move, and it is cancelled (plan 4.1).
        let focus_moved = self.with(key, |c| c.ctx.clone()).is_some_and(|ctx| unsafe {
            let view_wnd = ctx
                .GetActiveView()
                .and_then(|v| v.GetWnd())
                .unwrap_or_default();
            let focus = windows::Win32::UI::Input::KeyboardAndMouse::GetFocus();
            // A null focus means this thread lost the keyboard focus altogether (another window or process took it): moved.
            // Measured in Notepad and Edge (Windows 11): when another process takes the foreground, TSF ends the composition while
            // this thread's GetFocus() still answers the old window, so the foreground window is asked too: a foreground that is
            // not the (owner-)root of the context's window means the user left the window; a selection move or click keeps it.
            let root = windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                view_wnd,
                windows::Win32::UI::WindowsAndMessaging::GA_ROOTOWNER,
            );
            let fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
            let left_foreground = !fg.is_invalid() && !root.is_invalid() && fg != root;
            trace::log(&format!(
                "terminated: focus_null={} same_focus={} left_foreground={left_foreground}",
                focus.is_invalid(),
                view_wnd == focus
            ));
            !view_wnd.is_invalid() && (focus.is_invalid() || view_wnd != focus || left_foreground)
        });
        trace::log(&format!(
            "terminated: convertible={convertible} focus_moved={focus_moved}"
        ));
        let must_cancel = !convertible || !focus_moved;
        trace::log(&format!(
            "composition terminated by host, cancel_text={must_cancel}"
        ));
        if must_cancel {
            // The callback has write access: remove the text that still belongs to this composition.
            unsafe {
                match comp.GetRange() {
                    Ok(range) => {
                        let mut buf = [0u16; 512];
                        let mut n = 0u32;
                        let before = range.GetText(ec, 0, &mut buf, &mut n).is_ok().then_some(n);
                        let r = range.SetText(ec, 0, &[]);
                        let mut m = 0u32;
                        let _ = range.GetText(ec, 0, &mut buf, &mut m);
                        trace::log(&format!(
                            "terminated: delete range len {before:?} -> {m}, SetText ok={}",
                            r.is_ok()
                        ));
                    }
                    Err(e) => trace::log(&format!("terminated: GetRange failed {e:?}")),
                }
            }
        }
        self.with(key, |c| {
            c.composition = None;
            c.cleanup_pending = false;
            c.cancel_reason = false;
            c.reset_text_view();
            c.last = None;
            c.epoch = c.epoch.wrapping_add(1);
            c.queue.push(Event::CompositionAborted);
        });
    }
}

/// Weak handle to the shared state for deferred edit-session callbacks.
struct SharedRef;
impl SharedRef {
    fn get() -> std::rc::Weak<Shared> {
        CURRENT.with(|c| c.borrow().clone().unwrap_or_default())
    }
}

/// Timer callback of the synchronisation retry (runs on the thread that set it, inside the host's message loop).
/// Timer callback of the `async-late` test hook.
unsafe extern "system" fn pop_timer_proc(_hwnd: HWND, _msg: u32, id: usize, _time: u32) {
    guard(|| CURRENT.with(|slot| {
        if let Some(sh) = slot.borrow().as_ref().and_then(|w| w.upgrade()) {
            if sh.pop_timer.get() == id { sh.on_pop_timer(); }
            else { trace::log(&format!("pop.callback skip=timer_mismatch fired={id} current={}", sh.pop_timer.get())); }
        }
    }));
}

unsafe extern "system" fn late_timer_proc(_hwnd: HWND, _msg: u32, id: usize, _time: u32) {
    unsafe {
        let _ = KillTimer(None, id);
    }
    guard(|| {
        trace::log("timer.fire kind=late");
        if let Some(sh) = SharedRef::get().upgrade() {
            let jobs: Vec<Box<dyn FnOnce()>> = sh.late_jobs.borrow_mut().drain(..).collect();
            for j in jobs {
                j();
            }
        }
    });
}

unsafe extern "system" fn label_timer_proc(_hwnd: HWND, _msg: u32, _id: usize, _time: u32) {
    guard(|| {
        if let Some(sh) = SharedRef::get().upgrade() {
            trace::log("timer.fire kind=label");
            sh.show_label();
        }
    });
}

unsafe extern "system" fn sync_timer_proc(_hwnd: HWND, _msg: u32, id: usize, _time: u32) {
    guard(|| {
        if let Some(sh) = SharedRef::get().upgrade() {
            if sh.sync_timer.get() == id {
                sh.on_sync_timer();
            } else {
                trace::log("timer.skip kind=sync reason=stale_id");
            }
        } else {
            trace::log("timer.skip kind=sync reason=no_current");
        }
    });
}

unsafe extern "system" fn settings_timer_proc(_hwnd: HWND, _msg: u32, id: usize, _time: u32) {
    guard(|| {
        if let Some(sh) = SharedRef::get().upgrade()
            && sh.settings_timer.get() == id
        {
            sh.on_settings_timer();
        }
    });
}

thread_local! {
    static CURRENT: RefCell<Option<std::rc::Weak<Shared>>> = const { RefCell::new(None) };
}

fn keyboard_disabled(ctx: &ITfContext, trace_id: u32) -> bool {
    unsafe {
        // Edit / RichEdit password controls: the window style is visible through the context view.
        if let Ok(view) = ctx.GetActiveView()
            && let Ok(hwnd) = view.GetWnd()
            && !hwnd.is_invalid()
        {
            use windows::Win32::UI::WindowsAndMessaging::{
                GWL_STYLE, GetClassNameW, GetWindowLongW,
            };
            let mut cls = [0u16; 32];
            let n = GetClassNameW(hwnd, &mut cls) as usize;
            let name = String::from_utf16_lossy(&cls[..n]).to_ascii_lowercase();
            if (name == "edit" || name.starts_with("richedit"))
                && GetWindowLongW(hwnd, GWL_STYLE) as u32 & 0x20 != 0
            {
                trace::log(&format!(
                    "compartment.keyboard_disabled ctx={trace_id} reason=password_style"
                ));
                return true;
            }
        }
        let Ok(mgr) = ctx.cast::<ITfCompartmentMgr>() else {
            trace::log(&format!(
                "compartment.keyboard_disabled ctx={trace_id} reason=no_manager"
            ));
            return false;
        };
        let Ok(c) = mgr.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED) else {
            trace::log(&format!(
                "compartment.keyboard_disabled ctx={trace_id} reason=no_compartment"
            ));
            return false;
        };
        let Ok(v) = c.GetValue() else {
            trace::log(&format!(
                "compartment.keyboard_disabled ctx={trace_id} reason=no_value"
            ));
            return false;
        };
        let value = i32::try_from(&v).ok();
        trace::log(&format!(
            "compartment.keyboard_disabled ctx={trace_id} value={value:?}"
        ));
        value.is_some_and(|n| n != 0)
    }
}

/// CARET-MINTTY caret2: report the input method as open at activation through the thread manager's
/// `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE`, as other IMEs do. IMM32 hosts behind CUAS read it as `ImmGetOpenStatus`;
/// mintty only places its composition window at the cursor while that is TRUE. Write only, once per activation: the
/// shell never reads it (a host closing the input method does not stop composing -- unchanged behaviour, accepted
/// limitation, full open/close semantics are r9 work), deactivation and Chinese/English mode leave it alone.
fn report_open(tm: &ITfThreadMgr, tid: u32) {
    let r = tm.cast::<ITfCompartmentMgr>().and_then(|mgr| unsafe {
        mgr.GetCompartment(&windows::Win32::UI::TextServices::GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)?
            .SetValue(tid, &windows::Win32::System::Variant::VARIANT::from(1i32))
    });
    trace::log(&format!("compartment.openclose set=1 ok={}", r.is_ok()));
}

// =====================================================================================================================
// COM interface implementations

impl ITfTextInputProcessor_Impl for Tip_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> Result<()> {
        ITfTextInputProcessorEx_Impl::ActivateEx(self, ptim, tid, 0)
    }

    fn Deactivate(&self) -> Result<()> {
        ok_or_fail(guard(|| {
            let sh = &self.sh;
            trace::log("Deactivate");
            let state = sh.langbar.take();
            if let Some(state) = state {
                state.deactivate();
            }
            let langbar = sh.core.borrow_mut().langbar.take();
            if let Some((mgr, item)) = langbar {
                let result = unsafe { mgr.RemoveItem(&item) };
                trace::log(&format!("langbar.remove result={result:?}"));
                // Even on failure, release our references. A manager retaining the
                // item can only reach the inactive state with no subscribed sinks.
                drop(item);
                drop(mgr);
            }
            sh.hide_hint();
            sh.hint_atom.set(0);
            cand::hide();
            let keys: Vec<usize> = sh.ctxs.borrow().iter().map(|c| c.raw).collect();
            for k in keys {
                sh.focus_leave(k);
                sh.try_cleanup(k);
                sh.close_session(k);
                sh.drop_ctx(k);
            }
            let (tm, tid, cookie, sinks) = {
                let mut c = sh.core.borrow_mut();
                (
                    c.tm.take(), c.tid, (c.tm_cookie.take(), c.profile_cookie.take()),
                    (c.key_sink.take(), c.tm_sink.take(), c.comp_sink.take(), c.edit_sink.take(), c.layout_sink.take()),
                )
            };
            if let Some(tm) = tm {
                unsafe {
                    if let Ok(km) = tm.cast::<ITfKeystrokeMgr>() {
                        let _ = km.UnadviseKeyEventSink(tid);
                    }
                    if let Ok(src) = tm.cast::<ITfSource>() {
                        for ck in [cookie.0, cookie.1].into_iter().flatten() {
                            let _ = src.UnadviseSink(ck);
                        }
                    }
                }
            }
            drop(sinks);
            sh.stop_sync_timer();
            sh.stop_settings_timer();
            sh.label_pending.set(None);
            sh.show_label();
            sh.set_focus(0, "deactivate");
            sh.shift.borrow_mut().reset_reason("deactivate");
            sh.link.borrow_mut().drop_connection();
            CURRENT.with(|c| *c.borrow_mut() = None);
            Ok(())
        }))
    }
}

impl ITfTextInputProcessorEx_Impl for Tip_Impl {
    fn ActivateEx(&self, ptim: Ref<ITfThreadMgr>, tid: u32, flags: u32) -> Result<()> {
        ok_or_fail(guard(|| {
            trace::init();
            let sh = &self.sh;
            let tm: ITfThreadMgr = ptim.ok()?.clone();
            let atom = unsafe {
                use windows::Win32::{System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER}, UI::TextServices::{ITfCategoryMgr, CLSID_TF_CategoryMgr}};
                CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)
                    .and_then(|m| m.RegisterGUID(&crate::attribute::GUID_HINT)).unwrap_or(0)
            };
            sh.hint_atom.set(atom);
            let langbar_state = sh.new_langbar_state();
            CURRENT.with(|c| *c.borrow_mut() = Some(Rc::downgrade(sh)));
            {
                let mut c = sh.core.borrow_mut();
                c.tm = Some(tm.clone());
                c.tid = tid;
                c.key_sink = Some(self.to_interface::<ITfKeyEventSink>());
                c.tm_sink = Some(self.to_interface::<ITfThreadMgrEventSink>());
                c.comp_sink = Some(self.to_interface::<ITfCompositionSink>());
                c.edit_sink = Some(self.to_interface::<ITfTextEditSink>());
                c.layout_sink = Some(self.to_interface::<windows::Win32::UI::TextServices::ITfTextLayoutSink>());
            }
            trace::activation(&tm, flags);
            let (ks, ts) = {
                let c = sh.core.borrow();
                (c.key_sink.clone(), c.tm_sink.clone())
            };
            unsafe {
                if let Some(ks) = ks {
                    let r = tm.cast::<ITfKeystrokeMgr>()?.AdviseKeyEventSink(tid, &ks, true);
                    trace::log(&format!("AdviseKeyEventSink: {r:?}"));
                    if r.is_err() && std::env::var_os("TWIME_ALLOW_NO_KEYSINK").is_none() {
                        r?;
                    }
                }
                if let Some(ts) = ts {
                    let ck = tm.cast::<ITfSource>()?.AdviseSink(&ITfThreadMgrEventSink::IID, &ts)?;
                    sh.core.borrow_mut().tm_cookie = Some(ck);
                }
                // Record-only sink (trace): never registered unless the trace is on, never acts on what it sees.
                if trace::enabled() {
                    let sink = self.to_interface::<windows::Win32::UI::TextServices::ITfInputProcessorProfileActivationSink>();
                    let r = tm.cast::<ITfSource>().and_then(|s| s.AdviseSink(&windows::Win32::UI::TextServices::ITfInputProcessorProfileActivationSink::IID, &sink));
                    trace::log(&format!("AdviseSink profile activation: ok={}", r.is_ok()));
                    sh.core.borrow_mut().profile_cookie = r.ok();
                }
            }
            report_open(&tm, tid);
            let add = (|| -> Result<()> {
                let mgr = tm.cast::<windows::Win32::UI::TextServices::ITfLangBarItemMgr>()?;
                let weak = Rc::downgrade(sh);
                let item = crate::langbar::Button::create(langbar_state, move || {
                    if let Some(sh) = weak.upgrade() {
                        let key = sh.focus.get();
                        if sh.is_ready(key) {
                            sh.toggle_mode(key);
                        }
                    }
                });
                unsafe { mgr.AddItem(&item)?; }
                sh.core.borrow_mut().langbar = Some((mgr, item));
                Ok(())
            })();
            trace::log(&format!("langbar.add result={add:?}"));
            sh.link.borrow_mut().ensure();
            if let Ok(dim) = unsafe { tm.GetFocus() } {
                sh.attach_doc(&dim);
            }
            Ok(())
        }))
    }
}

impl ITfKeyEventSink_Impl for Tip_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        guard(|| {
            self.sh.shift.borrow_mut().reset();
            if !foreground.as_bool() { self.sh.hide_hint(); }
            self.sh.keyboard_focus.set(foreground.as_bool());
            self.sh.stop_settings_timer();
            // The keyboard focus came back to this TIP: bring the focused context up to date if it is not yet.
            let old = self.sh.focus.get();
            if foreground.as_bool() {
                if let Some(doc) = self.sh.focused_doc() {
                    match unsafe { doc.GetTop() } {
                        Ok(top) => {
                            let key = self.sh.ctx_key(&top);
                            if old != key {
                                self.sh.attach_context(&top, "key_focus");
                            }
                        }
                        Err(e) => {
                            trace::log(&format!("focus.lookup reason=get_top hr={:?}", e.code()));
                            self.sh.set_focus(0, "key_focus_empty");
                            self.sh.schedule_sync(self.sh.retry_delay());
                        }
                    }
                }
            }
            let key = self.sh.focus.get();
            trace::log(&format!(
                "focus.key foreground={} old={} new={}",
                foreground.as_bool(),
                self.sh.context_id(old),
                self.sh.context_id(key)
            ));
            if foreground.as_bool() && key != 0 && !self.sh.is_ready(key) && !self.sh.sync_ctx(key)
            {
                self.sh.schedule_sync(self.sh.retry_delay());
            }
            if foreground.as_bool() && key != 0 {
                self.sh.schedule_settings(1);
            }
        });
        Ok(())
    }

    fn OnTestKeyDown(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        trace::log("TestKeyDown-begin");
        let r = guard(|| self.sh.test_key(pic.as_ref(), wparam.0, lparam.0)).unwrap_or(false);
        trace::log(&format!("TestKeyDown vk={:#x} eaten={r}", wparam.0));
        self.sh
            .trace_key(pic.as_ref(), wparam.0, true, true, r, "callback_result");
        Ok(BOOL::from(r))
    }

    fn OnTestKeyUp(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        let r = guard(|| self.sh.test_key_up(pic.as_ref(), wparam.0, lparam.0)).unwrap_or(false);
        self.sh
            .trace_key(pic.as_ref(), wparam.0, true, false, r, "callback_result");
        Ok(BOOL::from(r))
    }

    fn OnKeyDown(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        trace::log(&format!("KeyDown vk={:#x} begin", wparam.0));
        let r = guard(|| self.sh.key_down(pic.as_ref(), wparam.0, lparam.0)).unwrap_or(false);
        self.sh
            .trace_key(pic.as_ref(), wparam.0, false, true, r, "callback_result");
        Ok(BOOL::from(r))
    }

    fn OnKeyUp(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(|| self.sh.key_up(pic.as_ref(), wparam.0, lparam.0));
        self.sh.trace_key(
            pic.as_ref(),
            wparam.0,
            false,
            false,
            false,
            "key_up_passthrough",
        );
        Ok(BOOL::from(false))
    }

    fn OnPreservedKey(&self, _pic: Ref<ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }
}

impl ITfCompositionSink_Impl for Tip_Impl {
    fn OnCompositionTerminated(&self, ecwrite: u32, pcomposition: Ref<ITfComposition>) -> Result<()> {
        guard(|| {
            if let Some(c) = pcomposition.as_ref() {
                self.sh.composition_terminated(ecwrite, c);
            }
        });
        Ok(())
    }
}

impl windows::Win32::UI::TextServices::ITfInputProcessorProfileActivationSink_Impl for Tip_Impl {
    fn OnActivated(&self, dwprofiletype: u32, langid: u16, clsid: *const GUID, _catid: *const GUID, guidprofile: *const GUID, hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL, dwflags: u32) -> Result<()> {
        // Record only: nothing here changes the state of the shell, the profile or the system.
        guard(|| unsafe {
            if let (Some(c), Some(p)) = (clsid.as_ref(), guidprofile.as_ref()) {
                trace::profile_activated(dwprofiletype, langid, c, p, hkl.0 as isize, dwflags);
            }
        });
        Ok(())
    }
}

impl ITfThreadMgrEventSink_Impl for Tip_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Ref<ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, pdim: Ref<ITfDocumentMgr>) -> Result<()> {
        guard(|| {
            if let Some(dim) = pdim.as_ref() {
                let top = unsafe { dim.GetTop().ok() };
                if let Some(top) = top {
                    let key = top.as_raw() as usize;
                    self.sh.try_cleanup(key);
                    self.sh.drop_ctx(key);
                }
            }
        });
        Ok(())
    }

    fn OnSetFocus(
        &self,
        pdimfocus: Ref<ITfDocumentMgr>,
        pdimprevfocus: Ref<ITfDocumentMgr>,
    ) -> Result<()> {
        guard(|| {
            self.sh.hide_hint();
            let old = self.sh.focus.get();
            let incoming = pdimfocus
                .as_ref()
                .and_then(|d| unsafe { d.GetTop().ok() })
                .map(|c| self.sh.ctx_key(&c))
                .unwrap_or(0);
            trace::log(&format!(
                "doc focus changed foreground={} old={} new={}",
                self.sh.keyboard_focus.get(),
                self.sh.context_id(old),
                self.sh.context_id(incoming)
            ));
            if let Ok(mut v) = self.sh.ctxs.try_borrow_mut() {
                for c in v.iter_mut() {
                    c.term_loc = None; // focus change: the text stays as committed (plan 4.1 c)
                }
            }
            if let Some(prev) = pdimprevfocus.as_ref()
                && let Ok(top) = unsafe { prev.GetTop() }
            {
                let key = top.as_raw() as usize;
                self.sh.focus_leave(key);
            }
            if let Some(f) = pdimfocus.as_ref() {
                self.sh.attach_doc(f);
            } else {
                self.sh.set_focus(0, "no_document");
            }
        });
        Ok(())
    }

    fn OnPushContext(&self, pic: Ref<ITfContext>) -> Result<()> {
        guard(|| {
            if let Some(pic) = pic.as_ref() {
                let key = self.sh.ctx_key(pic);
                let belongs = unsafe { pic.GetDocumentMgr() }
                    .ok()
                    .zip(self.sh.focused_doc())
                    .is_some_and(|(a, b)| {
                        a.cast::<windows::core::IUnknown>().ok()
                            .zip(b.cast::<windows::core::IUnknown>().ok())
                            .is_some_and(|(a, b)| a == b)
                    });
                trace::log(&format!(
                    "context.push ctx={} focused_document={belongs}",
                    self.sh.context_id(key)
                ));
                let (target, sync) = sync_target(
                    self.sh.focus.get(),
                    key,
                    self.sh.is_ready(key),
                    SyncSource::Push(belongs),
                );
                if belongs && (sync || self.sh.focus.get() != target) {
                    self.sh.attach_context(pic, "push_context");
                }
            }
        });
        Ok(())
    }

    fn OnPopContext(&self, pic: Ref<ITfContext>) -> Result<()> {
        guard(|| {
            if let Some(c) = pic.as_ref() {
                let key = c.as_raw() as usize;
                trace::log(&format!("context.pop ctx={}", self.sh.context_id(key)));
                self.sh.try_cleanup(key);
                self.sh.drop_ctx(key);
                // Query the focused document, not the popped context's document:
                // a background document may also pop contexts.
                if self.sh.focus.get() != 0 {
                    return;
                }
                let top = self.sh.focused_doc().and_then(|doc| unsafe { doc.GetTop().ok() });
                let raw = top.as_ref().map_or(0, |top| top.as_raw() as usize);
                let (target, _) = sync_target(
                    self.sh.focus.get(), raw, self.sh.is_ready(raw), SyncSource::Pop(key),
                );
                if target != 0 {
                    if let Some(top) = top {
                        self.sh.attach_context(&top, "pop_context");
                    }
                } else {
                    self.sh.set_focus(0, "pop_pending");
                }
                // Also handles hosts that notify before GetTop changes. A ready
                // target only restores tracking/timers; no Focus(true) is sent.
                self.sh.schedule_sync(1);
            }
        });
        Ok(())
    }
}

impl ITfTextEditSink_Impl for Tip_Impl {
    fn OnEndEdit(&self, pic: Ref<ITfContext>, _ecreadonly: u32, peditrecord: Ref<ITfEditRecord>) -> Result<()> {
        guard(|| {
            if let (Some(c), Some(r)) = (pic.as_ref(), peditrecord.as_ref()) {
                self.sh.end_edit(c, r);
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod timer_tests {
    use super::*;

    #[test]
    fn deactivate_and_reactivate_create_distinct_langbar_states() {
        use windows::Win32::UI::TextServices::ITfLangBarItemButton;
        let tip = Tip::new();
        let sh = tip.sh.clone();
        let processor: ITfTextInputProcessorEx = tip.into();
        let old = sh.new_langbar_state();
        let old_item = crate::langbar::Button::create(old.clone(), || {});
        let old_button: ITfLangBarItemButton = old_item.cast().unwrap();
        old.update(1, Some(Mode::English));
        unsafe { processor.Deactivate().unwrap(); }
        assert!(sh.langbar.borrow().is_none());
        let fresh = sh.new_langbar_state();
        assert!(!Rc::ptr_eq(&old, &fresh));
        let item = crate::langbar::Button::create(fresh.clone(), || {});
        let button: ITfLangBarItemButton = item.cast().unwrap();
        unsafe { assert_eq!(button.GetText().unwrap().to_string(), "中"); }
        fresh.update(2, Some(Mode::English));
        old.update(3, Some(Mode::English));
        unsafe {
            assert_eq!(button.GetText().unwrap().to_string(), "英");
            assert_eq!(old_button.GetText().unwrap().to_string(), "中");
            processor.Deactivate().unwrap();
        }
    }

    #[test]
    fn key_only_focus_recovery_resumes_settings_after_sync_and_when_already_ready() {
        use twime_proto::{UserReq, UserSettings};
        use twime_winpipe::PipeServer;
        use windows::Win32::{System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED}, UI::TextServices::CLSID_TF_ThreadMgr};

        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap(); }
        {
            let tm: ITfThreadMgr = unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }.unwrap();
            let tid = unsafe { tm.Activate() }.unwrap();
            let doc = unsafe { tm.CreateDocumentMgr() }.unwrap();
            let mut context = None;
            let mut cookie = 0;
            unsafe { doc.CreateContext(tid, 0, None::<&windows::core::IUnknown>, &mut context, &mut cookie) }.unwrap();
            let context = context.unwrap();
            let name = format!("twime-key-settings-{}", std::process::id());
            let server = PipeServer::with_name(Some(name.clone())).unwrap();
            let worker = std::thread::spawn(move || {
                let input = server.accept().unwrap();
                let hello = Request::decode(&input.read().unwrap().unwrap()).unwrap();
                assert!(matches!(hello.event, Event::Hello(_)));
                input.write(&Reply::HelloCaps { version: 5, caps: 0 }.encode()).unwrap();
                let focus = Request::decode(&input.read().unwrap().unwrap()).unwrap();
                assert_eq!(focus.event, Event::Focus(true));
                input.write(&Reply::Update { session: focus.session, seq: focus.seq,
                    update: Update { mode: Mode::English, toggle_key: ToggleKey::None, ..Default::default() }
                }.encode_for(5)).unwrap();
                for _ in 0..2 {
                    let control = server.accept().unwrap();
                    let hello = Request::decode(&control.read().unwrap().unwrap()).unwrap();
                    assert!(matches!(hello.event, Event::Hello(_)));
                    control.write(&Reply::HelloCaps { version: 5, caps: 0 }.encode()).unwrap();
                    let settings = Request::decode(&control.read().unwrap().unwrap()).unwrap();
                    assert_eq!(settings.event, Event::User(UserReq::GetSettings));
                    control.write(&Reply::User(UserReply::Settings {
                        settings: UserSettings { toggle_key: ToggleKey::Shift, ..Default::default() },
                        file_error: false, last_error: None,
                    }).encode_for(5)).unwrap();
                }
                // Readiness must not cause a second Focus request.
                assert!(input.read().unwrap().is_none());
            });
            let tip = Tip::new();
            let sh = &tip.sh;
            *sh.link.borrow_mut() = crate::link::tests::link_to(&name);
            assert_eq!(sh.focus.get(), 0);
            assert_eq!(sh.settings_timer.get(), 0);
            sh.key_sync(Some(&context));
            let key = sh.focus.get();
            assert_ne!(key, 0);
            assert_ne!(sh.sync_timer.get(), 0);
            sh.on_sync_timer();
            assert!(sh.is_ready(key));
            assert_eq!(sh.with(key, |c| (c.view.mode, c.toggle_key)), Some((Mode::English, ToggleKey::None)));
            assert_ne!(sh.settings_timer.get(), 0);
            sh.on_settings_timer();
            assert_eq!(sh.with(key, |c| c.toggle_key), Some(ToggleKey::Shift));
            sh.stop_settings_timer();
            sh.schedule_sync(1);
            sh.on_sync_timer();
            assert_ne!(sh.settings_timer.get(), 0);
            sh.on_settings_timer();
            sh.stop_settings_timer();
            sh.stop_sync_timer();
            sh.link.borrow_mut().drop_connection();
            worker.join().unwrap();
            unsafe { tm.Deactivate() }.unwrap();
        }
        unsafe { CoUninitialize(); }
    }

    #[test]
    fn recent_sync_does_not_leave_a_new_key_target_without_a_timer() {
        let tip = Tip::new();
        let sh = &tip.sh;
        sh.last_sync_try.set(Some(Instant::now()));
        sh.nudge_sync();
        assert_ne!(sh.sync_timer.get(), 0);
        let pending = sh.sync_timer.get();
        sh.nudge_sync();
        assert_eq!(sh.sync_timer.get(), pending);
        sh.stop_sync_timer();
    }

    #[test]
    fn settings_timer_does_not_block_retries_or_key_nudges() {
        let tip = Tip::new();
        let sh = &tip.sh;
        sh.focus.set(1);
        sh.keyboard_focus.set(true);
        sh.sync_attempts.set(2);
        sh.schedule_settings(5000);
        let settings_id = sh.settings_timer.get();
        assert_ne!(settings_id, 0);
        sh.schedule_sync(sh.retry_delay());
        let sync_id = sh.sync_timer.get();
        assert_ne!(sync_id, 0);
        assert_ne!(sync_id, settings_id);

        // An unready context skips the control request without replacing its pending retry.
        sh.on_settings_timer();
        assert_eq!(sh.sync_timer.get(), sync_id);
        assert_eq!(sh.sync_attempts.get(), 2);
        assert_eq!(sh.last_sync_try.get(), None);
        let settings_id = sh.settings_timer.get();
        assert_ne!(settings_id, 0);

        sh.nudge_sync();
        assert_ne!(sh.sync_timer.get(), 0);
        assert_eq!(sh.settings_timer.get(), settings_id);
        assert_eq!(sh.sync_attempts.get(), 2);
        sh.stop_settings_timer();
        assert_ne!(sh.sync_timer.get(), 0);
        sh.stop_sync_timer();
    }
}

#[cfg(test)]
mod pop_tests {
    use super::*;
    fn arm_pending(sh: &Shared) -> usize {
        let timer = unsafe { SetTimer(None, 0, POP_DEBOUNCE_MS, Some(pop_timer_proc)) };
        assert_ne!(timer, 0);
        // The state installed by arm_pop after its connection/context gates pass.
        sh.pop_pending.set(Some((1, 2, 4, 3)));
        sh.pop_timer.set(timer);
        timer
    }
    #[test]
    fn layout_redraw_without_completion_preserves_armed_timer() {
        let tip = Tip::new(); let sh = &tip.sh;
        let timer = arm_pending(sh);
        let pending = sh.pop_pending.get();
        sh.redraw_hints(1);
        // Reproduce c.last with no LEXUX hint, the Sandbox cancellation path.
        sh.show_hint(1, &Update::default(), true);
        assert_eq!(sh.pop_timer.get(), timer);
        assert_eq!(sh.pop_pending.get(), pending);
        assert!(sh.completion.borrow().is_none());
        sh.stop_pop_timer();
    }
    #[test]
    fn key_after_arm_still_stops_timer() {
        let tip = Tip::new(); let sh = &tip.sh;
        arm_pending(sh);
        sh.pop_key(0x41);
        assert_eq!(sh.pop_timer.get(), 0);
        assert!(sh.pop_pending.get().is_none());
    }
    #[test]
    fn stale_completion_redraw_does_not_cancel_new_pending_timer() {
        let tip = Tip::new(); let sh = &tip.sh;
        let timer = arm_pending(sh);
        set_pending(sh);
        let pending = sh.pop_pending.get();
        sh.redraw_hints(1);
        assert!(sh.completion.borrow().is_none());
        assert_eq!(sh.pop_timer.get(), timer);
        assert_eq!(sh.pop_pending.get(), pending);
        sh.stop_pop_timer();
    }
    #[test]
    fn own_edit_echo_preserves_armed_timer() {
        let tip = Tip::new(); let sh = &tip.sh;
        let timer = arm_pending(sh);
        sh.note_own(1);
        assert!(sh.take_own(1));
        assert_eq!(sh.pop_timer.get(), timer);
        assert!(sh.pop_pending.get().is_some());
        // The next real edit must not be swallowed as another echo.
        assert!(!sh.take_own(1));
        sh.stop_pop_timer();
    }
    fn set_pending(sh: &Shared) {
        sh.pop_pending.set(Some((1, 2, 4, 3)));
        *sh.completion.borrow_mut() = Some((1,2,4,twime_proto::CompletionHint { id:3,options:vec!["隊".into()] }));
    }
    #[test]
    fn standalone_modifiers_preserve_pending_completion_other_keys_cancel_before_passthrough() {
        let tip=Tip::new(); let sh=&tip.sh;
        for vk in [0x10,0x11,0x12,0x5b,0x5c,0xa0,0xa1,0xa2,0xa3,0xa4,0xa5] {
            set_pending(sh); sh.pop_key(vk);
            assert!(sh.pop_pending.get().is_some() && sh.completion.borrow().is_some());
        }
        for vk in [0x1b,0x25,0x41,0x70,0x09,0x0d,0x20,0x31] {
            set_pending(sh); sh.pop_key(vk);
            assert!(sh.pop_pending.get().is_none() && sh.completion.borrow().is_none());
            assert!(sh.pop_down.get().is_none());
            assert!(!sh.redraw_completion(1));
        }
    }
    #[test]
    fn invisible_completion_down_falls_back_and_cancellation_is_final() {
        let tip=Tip::new();let sh=&tip.sh;
        set_pending(sh); sh.pop_key(0x28);
        assert!(sh.pop_down.get().is_none());
        assert!(sh.completion.borrow().is_none());
        assert!(!sh.redraw_completion(1));
        set_pending(sh); sh.hide_hint();
        assert!(sh.pop_pending.get().is_none());
        assert!(!sh.redraw_completion(1));
    }
    #[test]
    fn timer_is_single_shot_stale_context_cannot_send_and_hide_cancels_timer() {
        let tip=Tip::new();let sh=&tip.sh;
        sh.pop_timer.set(unsafe { SetTimer(None,0,POP_DEBOUNCE_MS,Some(pop_timer_proc)) });
        set_pending(sh);
        sh.on_pop_timer();
        assert_eq!(sh.pop_timer.get(),0);
        assert!(sh.pop_pending.get().is_none());
        sh.pop_timer.set(unsafe { SetTimer(None,0,POP_DEBOUNCE_MS,Some(pop_timer_proc)) });
        set_pending(sh);sh.hide_hint();
        assert_eq!(sh.pop_timer.get(),0);
        assert!(sh.pop_pending.get().is_none() && sh.completion.borrow().is_none());
    }
    #[test]
    fn settings_changes_cancel_but_an_unchanged_poll_keeps_completion() {
        use twime_proto::UserSettings;
        let tip=Tip::new(); let sh=&tip.sh;
        sh.observe_pop_preferences(UserSettings::default());
        set_pending(sh); sh.observe_pop_preferences(UserSettings::default());
        assert!(sh.pop_pending.get().is_some() && sh.completion.borrow().is_some());
        for settings in [
            UserSettings { toggle_key:ToggleKey::None,..Default::default() },
            UserSettings { default_mode:Mode::English,..Default::default() },
            UserSettings { punct_half:true,..Default::default() },
            UserSettings { symbol_table:false,..Default::default() },
            UserSettings { cache:true,..Default::default() },
        ] {
            *sh.pop_preferences.borrow_mut()=Some(UserSettings::default());
            set_pending(sh); sh.observe_pop_preferences(settings);
            assert!(sh.pop_pending.get().is_none() && sh.completion.borrow().is_none());
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    #[test]
    fn pop_restores_ready_underlying_context_without_handshake() {
        for old in [0, 1, 2] {
            assert_eq!(sync_target(old, 1, true, SyncSource::Pop(2)), (1, false));
            assert_eq!(sync_target(old, 1, false, SyncSource::Pop(2)), (1, true));
        }
    }
    #[test]
    fn pop_defers_when_top_is_missing_or_still_being_removed() {
        for ready in [false, true] {
            assert_eq!(sync_target(2, 0, ready, SyncSource::Pop(2)), (0, false));
            assert_eq!(sync_target(2, 2, ready, SyncSource::Pop(2)), (0, false));
        }
    }
    #[test]
    fn unready_key_replaces_missing_or_stale_focus() {
        for old in [0, 1, 2] {
            assert_eq!(sync_target(old, 2, false, SyncSource::Key), (2, true));
        }
    }
    #[test]
    fn ready_context_never_requests_another_focus_handshake() {
        assert_eq!(sync_target(2, 2, true, SyncSource::Key), (2, false));
        assert_eq!(sync_target(1, 2, true, SyncSource::Key), (1, false));
        assert_eq!(sync_target(1, 2, true, SyncSource::Push(true)), (2, false));
    }
    #[test]
    fn push_only_adopts_context_in_focused_document() {
        for ready in [false, true] {
            assert_eq!(
                sync_target(1, 2, ready, SyncSource::Push(false)),
                (1, false)
            );
            assert_eq!(
                sync_target(0, 2, ready, SyncSource::Push(true)),
                (2, !ready)
            );
        }
        assert_eq!(sync_target(1, 0, false, SyncSource::Key), (1, false));
    }
}

impl windows::Win32::UI::TextServices::ITfDisplayAttributeProvider_Impl for Tip_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<windows::Win32::UI::TextServices::IEnumTfDisplayAttributeInfo> {
        guard(crate::attribute::enumeration).ok_or_else(|| E_FAIL.into())
    }
    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<windows::Win32::UI::TextServices::ITfDisplayAttributeInfo> {
        guard(|| crate::attribute::get(guid)).unwrap_or_else(|| Err(E_FAIL.into()))
    }
}

impl windows::Win32::UI::TextServices::ITfTextLayoutSink_Impl for Tip_Impl {
    fn OnLayoutChange(&self, context: Ref<ITfContext>, code: windows::Win32::UI::TextServices::TfLayoutCode,
        _view: Ref<windows::Win32::UI::TextServices::ITfContextView>) -> Result<()> {
        guard(|| {
            use windows::Win32::UI::TextServices::TF_LC_DESTROY;
            if self.sh.own_edit.get() > 0 { return; }
            let Some(ctx) = context.as_ref() else { return; };
            let key = ctx.as_raw() as usize;
            if self.sh.focus.get() != key { return; }
            trace::log(&format!("pop.layout ctx={} code={} timer={}", self.sh.context_id(key), code.0, self.sh.pop_timer.get()));
            if code == TF_LC_DESTROY { self.sh.clear_completion(); self.sh.hide_hint_window(); return; }
            self.sh.redraw_hints(key);
        }); Ok(())
    }
}

/// EMOJI (EMOJI-plan 3.4): the caret of a composition is counted in UTF-16 units of the characters before it.
#[cfg(test)]
mod emoji_caret_tests {
    use super::utf16_caret;

    #[test]
    fn caret_counts_utf16_units_of_the_characters_before_it() {
        let t = "a\u{1F600}b\u{2764}\u{FE0F}c";
        assert_eq!(utf16_caret(t, 0), 0);
        assert_eq!(utf16_caret(t, 2), 3); // a + a surrogate pair
        assert_eq!(utf16_caret(t, 5), 6); // + b + U+2764 (one unit) + U+FE0F (one unit) = 3 + 1 + 1 + 1
        assert_eq!(utf16_caret(t, 6), 7);
        assert_eq!(utf16_caret(t, 99), 7);
    }
}

/// ZHUYIN-OUT (ZHUYIN-OUT-plan 5): literal bopomofo (symbols and the four tone marks) is one UTF-16 unit per
/// character; mixed with a supplementary-plane character the caret still counts units, not characters.
#[cfg(test)]
mod zyout_caret_tests {
    use super::utf16_caret;

    #[test]
    fn bopomofo_and_tone_marks_are_one_unit_each() {
        for c in ('\u{3105}'..='\u{3129}').chain(['\u{02C7}', '\u{02CA}', '\u{02CB}', '\u{02D9}']) {
            assert_eq!(c.len_utf16(), 1, "{c:?}");
        }
        let t = "ㄅㄚˇ\u{20000}ㄉㄜ˙天";
        assert_eq!(t.encode_utf16().count(), 3 + 2 + 3 + 1);
        assert_eq!(utf16_caret(t, 3), 3);
        assert_eq!(utf16_caret(t, 4), 5); // + the surrogate pair
        assert_eq!(utf16_caret(t, 7), 8);
        assert_eq!(utf16_caret(t, 8), 9);
    }
}
