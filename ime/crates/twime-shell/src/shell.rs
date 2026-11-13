//! Contexts and sessions (the contract of LINUX-plan 4.2). One `Shell` per front-end process holds the one
//! connection; one `Context` per input context of the framework maps to one engine session.
//!
//! A session belongs to the connection it was made on. When that connection is gone (engine ended, timeout, broken)
//! the session is void: the context forgets what it showed and the next key makes a new connection and a new session,
//! which first hears `Focus(true)` and, in a sensitive field, `Password(true)`.
//!
//! Two things depend on the client behind the framework (LINUX-plan 7.7), which the front end reports with
//! `Input::Client`:
//! - Holding keys while the engine starts. The engine is started as soon as a context gets the focus; a key that
//!   arrives before it answers is held (the front end keeps the framework's event) and later released, in order: to
//!   the engine once it answers, or back to the application as it was after `HOLD_LIMIT`. Only for clients that accept
//!   keys sent on later (`defer`).
//! - Where a composition goes when the focus leaves. Some clients commit it, but into whatever field has the focus by
//!   then (IBus through gnome-shell on Wayland: `focus_commit` false). For those a composition is finished before any
//!   key the input method passes on (such a key may move the focus: Tab, F6, Alt+Tab), and dropped on a focus loss the
//!   input method only hears of afterwards (a mouse click, a program moving the focus).

use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    time::{Duration, Instant},
};

use twime_proto::{Applied, Cands, CommitResult, Comp, Event, Key, KeyEvent, Mode, Mods, PeekView, Reply as Wire, Request, ToggleKey, Update, peek};

use crate::{link::{Config, Link}, shift::ShiftTap};

/// Longest time keys are held while the engine starts; after it they go on to the application as they were.
pub const HOLD_LIMIT: Duration = Duration::from_secs(2);

/// A key as the front end names it: a protocol key, one of the Shift keys (which the protocol has no code for), or a
/// modifier key itself (Ctrl, Alt, Super, Caps Lock, ...: never sent to the engine, and never a reason to finish a
/// composition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    Key(Key),
    ShiftLeft,
    ShiftRight,
    Modifier,
}

/// One event of the framework. The modifier bits are those of `twime_proto::Mods`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    KeyDown(KeyCode, u8),
    KeyUp(KeyCode, u8),
    FocusIn,
    FocusOut,
    /// The framework asks for a reset: a composition is discarded, and what was typed before is no longer the context.
    Reset,
    /// The context is (not) a password or otherwise sensitive field now.
    Sensitive(bool),
    /// The candidate with this index in the page shown was clicked.
    Pick(u16),
    /// The Chinese / English indicator was clicked.
    ToggleMode,
    /// What the client behind the context can do (see the module documentation). `defer`: keys may be held and sent
    /// to it later. `focus_commit`: a composition committed when the focus leaves arrives in the field it was typed
    /// in. A new context has `defer` false and `focus_commit` true.
    Client { defer: bool, focus_commit: bool },
    /// Release held keys (the front end asks again and again while `Reply::holding`).
    Release,
    /// A key the panel sends for a click (page or cursor buttons). Not a key of the keyboard: the front end keeps no
    /// event for it, so it is never held; held keys go on first, as for any other event.
    PanelKey(KeyCode, u8),
}

/// What the front end shows for a context.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ui {
    /// Empty segments: no composition.
    pub comp: Comp,
    pub candidates_open: bool,
    /// The page shown; meaningful while `candidates_open`.
    pub candidates: Cands,
    /// There is a composition and the engine would commit it, as shown, when the focus leaves (otherwise it drops it).
    pub committable: bool,
}

/// Answer to one event: what to do now, and the complete state to show afterwards.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    /// The key is ours; otherwise it goes on to the application.
    pub handled: bool,
    /// `ui` differs from the one of the previous reply of this context: show it again.
    pub ui_changed: bool,
    pub mode_changed: bool,
    /// This key needed the engine and it was not there (not running yet, gone, too slow, wrong version).
    pub no_engine: bool,
    /// Text to commit before anything else; empty: none.
    pub commit: String,
    pub ui: Ui,
    pub mode: Mode,
    /// Before anything else of this reply: the front end sends this many of the oldest held keys on to the application
    /// unchanged (the engine did not come in time, or another event needs them out first).
    pub release_before: u32,
    /// This key is held: it is not the application's now (the front end keeps the event and reports it handled).
    pub held: bool,
    /// (`Release`) The oldest held key went through the engine; this reply is its reply. When not `handled`, the front
    /// end sends that key on to the application after applying the rest of this reply.
    pub released: bool,
    /// Keys are still held: the front end asks `Release` again soon.
    pub holding: bool,
}

struct Session {
    id: u32,
    /// The connection it lives on.
    generation: u32,
    seq: u32,
    /// Confirmation of the previous Update, sent with the next request.
    ack: Option<CommitResult>,
    /// What the engine was last told with `Password`.
    told_password: bool,
}

struct Inner {
    link: Link,
    next_session: u32,
    /// A hold ended without an engine: no key is held again before an engine answers (a slow start must not delay
    /// every key by the limit).
    hold_off: bool,
}

#[derive(Clone)]
pub struct Shell(Rc<RefCell<Inner>>);

impl Shell {
    pub fn new(cfg: Config) -> Shell {
        let next_session = cfg.first_session;
        Shell(Rc::new(RefCell::new(Inner { link: Link::new(cfg), next_session, hold_off: false })))
    }

    pub fn context(&self) -> Context {
        Context {
            shell: self.0.clone(),
            session: None,
            focused: false,
            sensitive: false,
            ui: Ui::default(),
            view: PeekView { ux: true, ..PeekView::default() },
            mode: Mode::default(),
            toggle: ToggleKey::default(),
            shift: ShiftTap::new(),
            reply: Reply::default(),
            reset_done: false,
            caps: false,
            held: VecDeque::new(),
            hold_since: None,
            defer: false,
            focus_commit: true,
            engine_committable: false,
        }
    }

    /// Process id of the engine at the other end of the connection (tests).
    pub fn engine_pid(&self) -> Option<u32> {
        self.0.borrow().link.server_pid()
    }

    /// How many times this shell has tried to start the engine (tests).
    pub fn spawn_attempts(&self) -> u32 {
        self.0.borrow().link.spawn_attempts()
    }

    /// The last start attempt is not over: the engine it started is running, or the start itself is still under way (tests).
    pub fn start_pending(&self) -> bool {
        self.0.borrow().link.start_pending()
    }
}

pub struct Context {
    shell: Rc<RefCell<Inner>>,
    session: Option<Session>,
    focused: bool,
    sensitive: bool,
    ui: Ui,
    /// What `peek` needs to tell whether the engine may want a key, from the last Update.
    view: PeekView,
    mode: Mode,
    toggle: ToggleKey,
    shift: ShiftTap,
    reply: Reply,
    /// The engine was told a reset and nothing was sent to it since: another reset would change nothing.
    reset_done: bool,
    /// Caps Lock as the last key event reported it.
    caps: bool,
    /// Keys waiting for the engine, with the time they came (a Shift tap is judged by it), oldest first.
    held: VecDeque<(Input, Instant)>,
    /// When the oldest key of the current hold came.
    hold_since: Option<Instant>,
    defer: bool,
    focus_commit: bool,
    /// What the engine last said about committing the composition on a focus loss.
    engine_committable: bool,
}

impl Context {
    /// Handles one event. The reply stays valid until the next event of this context.
    pub fn event(&mut self, input: Input) -> &Reply {
        let shell = self.shell.clone();
        let inner = &mut *shell.borrow_mut();
        let (ui_before, mode_before) = (self.ui.clone(), self.mode);
        let deadline = inner.link.deadline();
        let mut out = Reply::default();
        // Whatever the event is: a session whose connection is gone is void, and what it showed with it.
        if self.session.is_some() && !self.live(inner) {
            self.lost();
        }
        match input {
            // Behind held keys every key waits too: the order is kept.
            Input::KeyDown(..) | Input::KeyUp(..) if !self.held.is_empty() => self.hold(input, Instant::now(), &mut out),
            Input::Release => self.release(inner, deadline, &mut out),
            _ => {
                // Anything else that changes the context comes after the keys typed before it: those that still wait go
                // on to the application first, unchanged (the engine has not seen them).
                let changes = match input {
                    Input::Sensitive(now) => now != self.sensitive,
                    Input::Client { defer, focus_commit } => (defer, focus_commit) != (self.defer, self.focus_commit),
                    _ => true,
                };
                if changes && !self.held.is_empty() {
                    self.send_held_on(&mut out);
                }
                match input {
                    Input::PanelKey(code, mods) => self.handle(inner, Input::KeyDown(code, mods), Instant::now(), deadline, false, &mut out),
                    _ => self.handle(inner, input, Instant::now(), deadline, true, &mut out),
                }
            }
        }
        self.ui.committable = self.engine_committable && self.focus_commit && !self.ui.comp.segments.is_empty();
        // A Shift press or release always goes on to the application, whatever the engine said to `ToggleMode`
        // (unless it is held: then the front end sends it on later).
        if matches!(input, Input::KeyDown(KeyCode::ShiftLeft | KeyCode::ShiftRight, _) | Input::KeyUp(..)) && !out.held {
            out.handled = false;
        }
        out.holding = !self.held.is_empty();
        out.ui_changed = self.ui != ui_before;
        out.mode_changed = self.mode != mode_before;
        out.ui = self.ui.clone();
        out.mode = self.mode;
        self.reply = out;
        &self.reply
    }

    /// Keeps a key for later: the front end holds the framework's event.
    fn hold(&mut self, input: Input, at: Instant, out: &mut Reply) {
        if self.held.is_empty() {
            self.hold_since = Some(at);
        }
        self.held.push_back((input, at));
        out.held = true;
        out.handled = true;
    }

    /// `Input::Release`: the oldest held key goes to the engine once it answers; all of them go back to the application
    /// when it is not coming (any more) or the limit is over.
    fn release(&mut self, inner: &mut Inner, deadline: Instant, out: &mut Reply) {
        let Some(&(input, at)) = self.held.front() else { return };
        let over = self.hold_since.is_some_and(|t| t.elapsed() >= HOLD_LIMIT);
        // The limit is for waiting: once the engine answers, every held key goes to it, however long that takes.
        if inner.link.up() || (!over && inner.link.ensure(deadline)) {
            self.held.pop_front();
            if self.held.is_empty() {
                self.hold_since = None;
            }
            out.released = true;
            self.handle(inner, input, at, deadline, false, out);
        } else if over || !inner.link.start_pending() {
            // No engine in time, or none coming: until one answers, keys are not held again.
            inner.hold_off = true;
            inner.link.note("no engine for the held keys: they go on to the application");
            self.send_held_on(out);
        }
    }

    /// All held keys go on to the application unchanged (`release_before`). The engine never sees them, so what this
    /// context derives from key events is brought up to date here: a Shift press or release among them ends any tap
    /// (a press seen before the hold would otherwise wait for its release forever), and Caps Lock is as the last of
    /// them reported it.
    fn send_held_on(&mut self, out: &mut Reply) {
        if let Some(&(Input::KeyDown(_, mods) | Input::KeyUp(_, mods), _)) = self.held.back() {
            self.caps = Mods(mods).caps();
        }
        self.shift.reset();
        out.release_before += self.held.len() as u32;
        self.held.clear();
        self.hold_since = None;
    }

    /// One event of the framework, `at` the time it happened. `may_hold`: a key that finds no engine may be held.
    fn handle(&mut self, inner: &mut Inner, input: Input, at: Instant, deadline: Instant, may_hold: bool, out: &mut Reply) {
        if let Input::KeyDown(_, mods) | Input::KeyUp(_, mods) = input {
            self.caps = Mods(mods).caps();
        }
        match input {
            Input::KeyDown(code @ (KeyCode::Key(_) | KeyCode::Modifier), mods) => {
                let key = if let KeyCode::Key(k) = code { k } else { Key::Other };
                self.shift.other_key();
                self.focused = true;
                // Keys the engine never takes (English mode, shortcuts, ...) are not sent at all.
                let ev = KeyEvent { key, mods: Mods(mods & Mods::ALL), down: true };
                if !self.establish(inner, deadline, out) {
                    if may_hold && self.defer && !self.sensitive && !inner.hold_off && inner.link.start_pending() && peek(&self.view, ev) {
                        self.hold(input, at, out);
                    } else {
                        out.no_engine = true;
                    }
                } else if !self.sensitive {
                    if peek(&self.view, ev) {
                        match self.exchange(inner, Event::Key(ev), deadline, out) {
                            Some(handled) => out.handled = handled,
                            None => out.no_engine = true,
                        }
                    }
                    // A key that goes on to the application may move the focus, and where the client would put the
                    // composition then is not to be trusted: it is finished here, before the key.
                    if !out.handled && !out.no_engine && code != KeyCode::Modifier && !self.focus_commit && !self.ui.comp.segments.is_empty() {
                        self.finish(inner, deadline, out);
                    }
                }
            }
            Input::KeyDown(shift, mods) => {
                self.focused = true;
                let ready = self.establish(inner, deadline, out) && !self.sensitive;
                self.shift.down(at, shift == KeyCode::ShiftRight, Mods(mods).command(), ready);
            }
            Input::KeyUp(KeyCode::Key(_) | KeyCode::Modifier, _) => {}
            Input::KeyUp(shift, mods) => {
                let ready = self.session.is_some() && !self.sensitive;
                let m = Mods(mods);
                if self.shift.up(at, shift == KeyCode::ShiftRight, m.command(), m.caps(), ready, self.toggle) {
                    self.exchange(inner, Event::ToggleMode, deadline, out);
                }
            }
            Input::FocusIn => {
                self.focused = true;
                self.shift.reset();
                if self.session.is_some() {
                    if self.exchange(inner, Event::Focus(true), deadline, out).is_some() {
                        self.sync_password(inner, deadline, out);
                    }
                } else {
                    // The engine is wanted soon: connect now, or start it, instead of at the first key.
                    inner.link.ensure(deadline);
                }
            }
            Input::FocusOut => {
                self.focused = false;
                self.shift.reset();
                if self.session.is_some() {
                    // A client that cannot put it into the right field gets nothing: the engine drops the composition
                    // (and does not take it for typed text) before it hears of the focus loss.
                    if !self.focus_commit && !self.ui.comp.segments.is_empty() && self.exchange(inner, Event::CompositionAborted, deadline, out).is_none() {
                        return;
                    }
                    self.exchange(inner, Event::Focus(false), deadline, out);
                    if !self.focus_commit {
                        out.commit.clear();
                    }
                }
            }
            Input::Reset => {
                // Also without a composition: the engine forgets the text typed before (its recent-output context),
                // which no longer describes what is in front of the caret.
                if self.session.is_some() && !self.reset_done {
                    self.reset_done = self.exchange(inner, Event::CompositionAborted, deadline, out).is_some();
                }
            }
            Input::Sensitive(now) => {
                self.sensitive = now;
                if self.session.is_some() {
                    self.sync_password(inner, deadline, out);
                }
            }
            Input::Pick(index) => {
                if self.session.is_some() && !self.sensitive && self.ui.candidates_open {
                    if let Some(handled) = self.exchange(inner, Event::CandidatePick(index), deadline, out) {
                        out.handled = handled;
                    }
                }
            }
            Input::ToggleMode => {
                // As a Shift tap: only a session that takes keys, and never with Caps Lock on.
                if self.session.is_some() && !self.sensitive && !self.caps {
                    self.exchange(inner, Event::ToggleMode, deadline, out);
                }
            }
            Input::Client { defer, focus_commit } => {
                self.defer = defer;
                self.focus_commit = focus_commit;
            }
            Input::Release | Input::PanelKey(..) => {}
        }
    }

    /// Ends the composition as a focus loss would (the engine commits what can be committed and drops the rest) and
    /// goes on with the focus: before a key leaves the input method for a client that is not trusted with it.
    fn finish(&mut self, inner: &mut Inner, deadline: Instant, out: &mut Reply) {
        if self.exchange(inner, Event::Focus(false), deadline, out).is_some() && self.exchange(inner, Event::Focus(true), deadline, out).is_some() {
            self.sync_password(inner, deadline, out);
        }
    }

    /// The reply of the last event.
    pub fn reply(&self) -> &Reply {
        &self.reply
    }

    /// The session is on the current connection and that connection is up.
    fn live(&self, inner: &mut Inner) -> bool {
        inner.link.up() && self.session.as_ref().is_some_and(|s| s.generation == inner.link.generation())
    }

    /// The session is void: nothing of it is shown any more. The mode stays as last known until the engine says more.
    fn lost(&mut self) {
        self.session = None;
        self.ui = Ui::default();
        self.view = PeekView { ux: true, mode: self.mode, ..PeekView::default() };
    }

    /// For a key: a connection and a session that knows the focus and the sensitive state. False: no engine now.
    fn establish(&mut self, inner: &mut Inner, deadline: Instant, out: &mut Reply) -> bool {
        if !inner.link.ensure(deadline) {
            self.lost();
            return false;
        }
        inner.hold_off = false;
        if self.session.as_ref().is_none_or(|s| s.generation != inner.link.generation()) {
            self.lost();
            let id = inner.next_session;
            // Never 0 (control requests) and never the engine's test range at the top.
            inner.next_session = if id >= 0xFFFC_FFFF { 1 } else { id + 1 };
            self.session = Some(Session { id, generation: inner.link.generation(), seq: 0, ack: None, told_password: false });
            // A key means the context has the focus, whether the framework said so or not.
            if self.exchange(inner, Event::Focus(true), deadline, out).is_none() {
                return false;
            }
        }
        self.sync_password(inner, deadline, out)
    }

    /// Tells the engine the sensitive state if it differs from what it was last told. False: the engine was lost.
    fn sync_password(&mut self, inner: &mut Inner, deadline: Instant, out: &mut Reply) -> bool {
        let now = self.sensitive;
        match &self.session {
            None => false,
            Some(s) if s.told_password == now => true,
            Some(_) => {
                if self.exchange(inner, Event::Password(now), deadline, out).is_none() {
                    return false;
                }
                if let Some(s) = &mut self.session {
                    s.told_password = now;
                }
                true
            }
        }
    }

    /// One request of the session and its Update, applied. `Some(handled)`; `None`: the engine did not answer in time
    /// or answered something else, the connection is dropped and the session is void.
    fn exchange(&mut self, inner: &mut Inner, event: Event, deadline: Instant, out: &mut Reply) -> Option<bool> {
        let s = self.session.as_mut()?;
        self.reset_done = false;
        s.seq = s.seq.wrapping_add(1);
        let (session, seq) = (s.id, s.seq);
        let req = Request { session, seq, ack: s.ack.take(), event };
        match inner.link.call(&req, deadline) {
            Some(Wire::Update { session: rs, seq: rq, update }) if rs == session && rq == seq => {
                // The frameworks' commit and preedit calls report no failure: whatever the Update carries counts as applied.
                let part = |present: bool| if present { Applied::Applied } else { Applied::NotApplicable };
                s.ack = Some(CommitResult { seq, commit: part(update.commit.is_some()), comp: part(update.composition.is_some()) });
                Some(self.apply(update, out))
            }
            _ => {
                inner.link.drop_connection();
                self.lost();
                None
            }
        }
    }

    fn apply(&mut self, u: Update, out: &mut Reply) -> bool {
        self.view = u.peek_view(true);
        self.mode = u.mode;
        self.toggle = u.toggle_key;
        if let Some(text) = &u.commit {
            out.commit.push_str(text);
        }
        // An Update without a part leaves that part as it is, unless the state says there is none any more.
        match u.composition {
            Some(c) => self.ui.comp = c,
            None if !u.composing => self.ui.comp = Comp::default(),
            None => {}
        }
        self.ui.candidates_open = u.candidates_open;
        match u.candidates {
            Some(c) if u.candidates_open => self.ui.candidates = c,
            _ if !u.candidates_open => self.ui.candidates = Cands::default(),
            _ => {}
        }
        self.engine_committable = u.committable;
        u.handled
    }
}

impl Drop for Context {
    /// The framework's object is gone: the session is closed (when its connection is still there).
    fn drop(&mut self) {
        let Ok(mut inner) = self.shell.try_borrow_mut() else { return };
        let inner = &mut *inner;
        if !self.live(inner) {
            return;
        }
        if let Some(s) = &mut self.session {
            let req = Request { session: s.id, seq: s.seq.wrapping_add(1), ack: s.ack.take(), event: Event::Close };
            let deadline = inner.link.deadline();
            if inner.link.call(&req, deadline).is_none() {
                inner.link.drop_connection();
            }
        }
    }
}

#[cfg(test)]
mod tests;
