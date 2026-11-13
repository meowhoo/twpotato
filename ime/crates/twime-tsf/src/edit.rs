//! Edit sessions: apply an engine `Update` to the host document and read back what happened.
//!
//! Key-path sessions are synchronous read-write sessions (`TF_ES_SYNC | TF_ES_READWRITE`); the HRESULT of the request,
//! the session result and the actual outcome are all checked (a `StartComposition` may succeed and still leave nothing
//! usable). `TF_S_ASYNC` is never counted as done.

use std::{cell::RefCell, mem::ManuallyDrop, rc::Rc};

use twime_proto::Applied;
use windows::{
    Win32::UI::TextServices::{
        ITfComposition, ITfCompositionSink, ITfContext, ITfContextComposition, ITfEditSession,
        ITfEditSession_Impl, ITfRange, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START,
        TF_DEFAULT_SELECTION, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
        TF_SELECTIONSTYLE, TfAnchor,
    },
    core::{Interface, Result, implement},
};

use crate::guard;

/// Preserve the raw success HRESULT too (the generated Result wrappers erase S_FALSE).
fn step_hr(step: &'static str, fatal: bool, hr: windows::core::HRESULT) -> windows::core::HRESULT {
    if crate::trace::enabled() {
        crate::trace::log(&format!(
            "edit.step name={step} hr={:#010x} fatal={} fatal_on_failure={fatal}",
            hr.0,
            fatal && hr.is_err()
        ));
    }
    hr
}

fn set_text(range: &ITfRange, ec: u32, text: &[u16], fatal: bool) -> Result<()> {
    unsafe {
        step_hr(
            "SetText",
            fatal,
            (range.vtable().SetText)(
                range.as_raw(),
                ec,
                0,
                windows::core::PCWSTR(text.as_ptr()),
                text.len().try_into().unwrap(),
            ),
        )
        .ok()
    }
}

fn end_composition(comp: &ITfComposition, ec: u32) -> Result<()> {
    unsafe {
        step_hr(
            "EndComposition",
            true,
            (comp.vtable().EndComposition)(comp.as_raw(), ec),
        )
        .ok()
    }
}

fn start_composition(
    cc: &ITfContextComposition,
    ec: u32,
    range: &ITfRange,
    sink: &ITfCompositionSink,
) -> Result<ITfComposition> {
    unsafe {
        let mut out = std::ptr::null_mut();
        step_hr(
            "StartComposition",
            true,
            (cc.vtable().StartComposition)(
                cc.as_raw(),
                ec,
                range.as_raw(),
                sink.as_raw(),
                &mut out,
            ),
        )
        .and_then(|| windows::core::Type::from_abi(out))
    }
}

fn composition_range(comp: &ITfComposition, fatal: bool) -> Result<ITfRange> {
    unsafe {
        let mut out = std::ptr::null_mut();
        step_hr(
            "GetRange",
            fatal,
            (comp.vtable().GetRange)(comp.as_raw(), &mut out),
        )
        .and_then(|| windows::core::Type::from_abi(out))
    }
}

fn clone_range(range: &ITfRange, fatal: bool) -> Result<ITfRange> {
    unsafe {
        let mut out = std::ptr::null_mut();
        step_hr(
            "Clone",
            fatal,
            (range.vtable().Clone)(range.as_raw(), &mut out),
        )
        .and_then(|| windows::core::Type::from_abi(out))
    }
}

fn collapse(range: &ITfRange, ec: u32, anchor: TfAnchor, fatal: bool) -> Result<()> {
    unsafe {
        step_hr(
            "Collapse",
            fatal,
            (range.vtable().Collapse)(range.as_raw(), ec, anchor),
        )
        .ok()
    }
}

fn shift_end(range: &ITfRange, ec: u32, count: i32, shifted: &mut i32, fatal: bool) -> Result<()> {
    unsafe {
        step_hr(
            "ShiftEnd",
            fatal,
            (range.vtable().ShiftEnd)(range.as_raw(), ec, count, shifted, std::ptr::null()),
        )
        .ok()
    }
}

fn shift_start(
    range: &ITfRange,
    ec: u32,
    count: i32,
    shifted: &mut i32,
    fatal: bool,
) -> Result<()> {
    unsafe {
        step_hr(
            "ShiftStart",
            fatal,
            (range.vtable().ShiftStart)(range.as_raw(), ec, count, shifted, std::ptr::null()),
        )
        .ok()
    }
}

fn session_kind(
    flags: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS,
) -> (&'static str, &'static str) {
    (
        if flags.0 & TF_ES_SYNC.0 != 0 {
            "sync"
        } else {
            "async"
        },
        if flags.0 & TF_ES_READWRITE.0 == TF_ES_READWRITE.0 {
            "readwrite"
        } else {
            "read"
        },
    )
}

#[cfg(test)]
mod trace_tests {
    use super::*;
    use windows::Win32::UI::TextServices::TF_ES_ASYNC;

    #[test]
    fn session_access_does_not_confuse_the_shared_read_bit_with_readwrite() {
        assert_eq!(session_kind(TF_ES_SYNC | TF_ES_READ), ("sync", "read"));
        assert_eq!(
            session_kind(TF_ES_SYNC | TF_ES_READWRITE),
            ("sync", "readwrite")
        );
        assert_eq!(session_kind(TF_ES_ASYNC | TF_ES_READ), ("async", "read"));
        assert_eq!(
            session_kind(TF_ES_ASYNC | TF_ES_READWRITE),
            ("async", "readwrite")
        );
    }
}

fn request_session(
    ctx: &ITfContext,
    tid: u32,
    es: &ITfEditSession,
    flags: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS,
) -> Result<windows::core::HRESULT> {
    // Same zero initialisation and HRESULT conversion as the generated binding.
    let mut session = windows::core::HRESULT(0);
    let outer = unsafe {
        (ctx.vtable().RequestEditSession)(ctx.as_raw(), tid, es.as_raw(), flags, &mut session)
    };
    if crate::trace::enabled() {
        let (kind, access) = session_kind(flags);
        crate::trace::log(&format!(
            "RequestEditSession type={} access={} outer_hr={:#010x} phrSession={:#010x} phrSession_valid={}",
            kind,
            access,
            outer.0,
            session.0,
            outer.is_ok()
        ));
    }
    outer.map(|| session)
}

/// Test hook (dev-mode only, set through `TWIME_FAULT=async-fail`): the next composition that is started fails right after it
/// exists, as a host can when it accepts the start but not the text or the caret.
static FAIL_AFTER_START: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn inject_fail_after_start() {
    FAIL_AFTER_START.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Test hook (dev-mode only, `TWIME_FAULT=commit-fail`): the next commit fails right after the composition was ended, as if the
/// host refused the final text.
static FAIL_AFTER_END: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn inject_fail_after_end() {
    FAIL_AFTER_END.store(true, std::sync::atomic::Ordering::SeqCst);
}

type Job = Box<dyn FnOnce(u32) -> Result<()>>;

#[implement(ITfEditSession)]
struct EditSessionObj {
    job: RefCell<Option<Job>>,
}

impl ITfEditSession_Impl for EditSessionObj_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let job = self.job.borrow_mut().take();
        match job {
            Some(j) => guard(|| j(ec)).unwrap_or(Err(windows::Win32::Foundation::E_FAIL.into())),
            None => Ok(()),
        }
    }
}

/// Requests an edit session on `ctx` and runs `job` inside it. Ok only if the request, the session and the job all
/// succeeded; `sync` selects a synchronous session.
pub fn run_session(
    ctx: &ITfContext,
    tid: u32,
    sync: bool,
    job: impl FnOnce(u32) -> Result<()> + 'static,
) -> Result<()> {
    let es: ITfEditSession = EditSessionObj {
        job: RefCell::new(Some(Box::new(job))),
    }
    .into();
    let flags = if sync {
        TF_ES_SYNC | TF_ES_READWRITE
    } else {
        windows::Win32::UI::TextServices::TF_ES_ASYNC | TF_ES_READWRITE
    };
    let hr = request_session(ctx, tid, &es, flags)?;
    hr.ok()
}

/// Geometry only needs read access, including during OnLayoutChange. Keep this
/// synchronous like the candidate window so existing lifetime checks still apply.
fn geometry_session(
    request: impl FnOnce(&ITfEditSession, windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS) -> Result<HRESULT>,
    job: impl FnOnce(u32) -> Result<()> + 'static,
) -> Result<()> {
    let es: ITfEditSession = EditSessionObj {
        job: RefCell::new(Some(Box::new(job))),
    }.into();
    request(&es, TF_ES_SYNC | TF_ES_READ)?.ok()
}

#[derive(Default)]
pub struct Outcome {
    pub commit: Option<Applied>,
    pub comp: Option<Applied>,
    /// The composition object that exists after the job (even if a later step failed), None if none.
    pub composition: Option<ITfComposition>,
}

pub struct ApplyInput {
    pub ctx: ITfContext,
    pub composition: Option<ITfComposition>,
    pub sink: ITfCompositionSink,
    /// Text to insert as final text (replaces the composition text if there is a composition).
    pub commit: Option<String>,
    /// Composition text and caret (in UTF-16 units from the start); None cancels / keeps no composition.
    pub new_text: Option<(String, usize)>,
    /// None preserves the legacy path; Some clears all old hints before setting these UTF-16 ranges.
    pub hints: Option<(u32, Vec<(i32, i32)>)>,
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn set_caret(
    ec: u32,
    ctx: &ITfContext,
    range: &ITfRange,
    offset: Option<usize>,
    fatal: bool,
) -> Result<()> {
    unsafe {
        let r = clone_range(range, fatal)?;
        match offset {
            None => collapse(&r, ec, TF_ANCHOR_END, fatal)?,
            Some(n) => {
                collapse(&r, ec, TF_ANCHOR_START, fatal)?;
                let mut shifted = 0i32;
                shift_end(&r, ec, n as i32, &mut shifted, fatal)?;
                collapse(&r, ec, TF_ANCHOR_END, fatal)?;
            }
        }
        let sel = TF_SELECTION {
            range: ManuallyDrop::new(Some(r)),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_NONE,
                fInterimChar: false.into(),
            },
        };
        let res = step_hr(
            "SetSelection",
            fatal,
            (ctx.vtable().SetSelection)(ctx.as_raw(), ec, 1, &sel),
        )
        .ok();
        res
    }
}

/// The insertion point: the first selection range, or the end of the document.
fn insertion_range(ec: u32, ctx: &ITfContext) -> Result<ITfRange> {
    unsafe {
        let mut sel = [TF_SELECTION::default()];
        let mut fetched = 0u32;
        if ctx
            .GetSelection(ec, TF_DEFAULT_SELECTION, &mut sel, &mut fetched)
            .is_ok()
            && fetched == 1
            && let Some(r) = ManuallyDrop::into_inner(std::mem::take(&mut sel[0].range))
        {
            return Ok(r);
        }
        ctx.GetEnd(ec)
    }
}

pub(crate) fn clear_attributes(ctx: &ITfContext, ec: u32, range: &ITfRange) -> Result<()> {
    unsafe { ctx.GetProperty(&windows::Win32::UI::TextServices::GUID_PROP_ATTRIBUTE)?.Clear(ec, range) }
}
static FAIL_HINT_ATTRIBUTES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn inject_fail_hint_attributes() { FAIL_HINT_ATTRIBUTES.store(true, std::sync::atomic::Ordering::SeqCst); }
fn apply_attributes(ctx: &ITfContext, ec: u32, range: &ITfRange, hints: &(u32, Vec<(i32, i32)>)) -> Result<()> {
    use windows::Win32::{System::Variant::VARIANT, UI::TextServices::GUID_PROP_ATTRIBUTE};
    clear_attributes(ctx, ec, range)?;
    if !hints.1.is_empty() && FAIL_HINT_ATTRIBUTES.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Err(windows::Win32::Foundation::E_FAIL.into());
    }
    unsafe {
        let prop = ctx.GetProperty(&GUID_PROP_ATTRIBUTE)?;
        for &(start, len) in &hints.1 {
            if hints.0 == 0 { return Err(windows::Win32::Foundation::E_FAIL.into()); }
            let r = range.Clone()?;
            r.Collapse(ec, TF_ANCHOR_START)?;
            let mut moved = 0;
            r.ShiftEnd(ec, start + len, &mut moved, std::ptr::null())?;
            if moved != start + len { return Err(windows::Win32::Foundation::E_FAIL.into()); }
            r.ShiftStart(ec, start, &mut moved, std::ptr::null())?;
            if moved != start { return Err(windows::Win32::Foundation::E_FAIL.into()); }
            prop.SetValue(ec, &r, &VARIANT::from(hints.0 as i32))?;
        }
    } Ok(())
}

/// Applies `commit` and then `new_text` in one synchronous session. The outcome records what was actually done.
fn apply_body(ec: u32, input: &ApplyInput, o: &mut Outcome) -> Result<()> {
    let ctx = &input.ctx;
    let mut comp = input.composition.clone();
    if let Some(text) = &input.commit {
        // The text first: once `SetText` has succeeded the commit stands, whatever happens to the caret afterwards.
        let mut ended = false;
        let wrote = (|| -> Result<ITfRange> {
            {
                match &comp {
                    Some(c) => {
                        let range = composition_range(&c, true)?;
                        if input.hints.is_some() { clear_attributes(ctx, ec, &range)?; }
                        end_composition(&c, ec)?;
                        ended = true;
                        let written =
                            if FAIL_AFTER_END.swap(false, std::sync::atomic::Ordering::SeqCst) {
                                Err(windows::Win32::Foundation::E_FAIL.into())
                            } else {
                                set_text(&range, ec, &utf16(text), true)
                            };
                        if let Err(e) = written {
                            // The composition is over but the final text was refused: take the interim text away again, so that
                            // nothing half-committed stays in the document; the state is then "nothing committed".
                            let _ = set_text(&range, ec, &[], false);
                            return Err(e);
                        }
                        dbg_len(ec, &range, "commit after SetText");
                        Ok(range)
                    }
                    None => {
                        // No composition: the text goes in as a composition that is ended at once, the way every commit is made
                        // here. (A bare `SetText` on the selection range, or `InsertTextAtSelection`, was not flushed by Rich Edit
                        // controls until the next composition ended: found by the UX1 host scenarios.)
                        let at = insertion_range(ec, ctx)?;
                        let cc: ITfContextComposition = ctx.cast()?;
                        let c = start_composition(&cc, ec, &at, &input.sink)?;
                        let range = composition_range(&c, true)?;
                        if input.hints.is_some() { clear_attributes(ctx, ec, &range)?; }
                        end_composition(&c, ec)?;
                        set_text(&range, ec, &utf16(text), true)?;
                        dbg_len(ec, &range, "insert after SetText");
                        Ok(range)
                    }
                }
            }
        })();
        match wrote {
            Ok(range) => {
                o.commit = Some(Applied::Applied);
                comp = None;
                o.composition = None;
                // Best effort: a host that refuses to move the insertion point still has the text.
                if let Err(e) = set_caret(ec, ctx, &range, None, false) {
                    crate::trace::log("commit: the caret could not be placed after the text");
                    // A new composition would start at whatever the selection is now (it could overwrite the committed text):
                    // the commit stands, the new composition is reported as failed.
                    if input.new_text.is_some() {
                        o.comp = Some(Applied::Failed);
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                o.commit = Some(Applied::Failed);
                if ended {
                    // The composition is over (its interim text was taken away again): there is nothing left to cancel, and
                    // the shell must not try to end the dead object again.
                    o.composition = None;
                }
                return Err(e);
            }
        }
    }

    let step = (|| -> Result<()> {
        {
            match &input.new_text {
                Some((text, caret)) => {
                    if comp.is_none() {
                        let at = insertion_range(ec, ctx)?;
                        let cc: ITfContextComposition = ctx.cast()?;
                        let c = start_composition(&cc, ec, &at, &input.sink)?;
                        o.composition = Some(c.clone());
                        comp = Some(c);
                        if FAIL_AFTER_START.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            return Err(windows::Win32::Foundation::E_FAIL.into());
                        }
                    }
                    let c = comp.as_ref().ok_or_else(|| {
                        windows::core::Error::from(windows::Win32::Foundation::E_FAIL)
                    })?;
                    let range = composition_range(&c, true)?;
                    if input.hints.is_some() { clear_attributes(ctx, ec, &range)?; }
                    set_text(&range, ec, &utf16(text), true)?;
                    if let Some(hints) = &input.hints { apply_attributes(ctx, ec, &range, hints)?; }
                    dbg_len(ec, &range, "comp after SetText");
                    set_caret(ec, ctx, &range, Some(*caret), true)?;
                }
                None => {
                    if let Some(c) = &comp {
                        let range = composition_range(&c, true)?;
                        if input.hints.is_some() { clear_attributes(ctx, ec, &range)?; }
                        set_text(&range, ec, &[], true)?;
                        end_composition(&c, ec)?;
                        comp = None;
                        o.composition = None;
                    }
                }
            }
        }
        Ok(())
    })();
    match step {
        Ok(()) => {
            if input.new_text.is_some() || input.composition.is_some() {
                o.comp = Some(Applied::Applied);
            }
            Ok(())
        }
        Err(e) => {
            o.comp = Some(Applied::Failed);
            Err(e)
        }
    }
}

/// Asynchronous variant: runs when the host grants the session and then calls `done` with what was done.
///
/// `current` is asked first, inside the session and before anything is written: when it says no, a newer `Update` has
/// superseded this one (its synchronous write already created or changed the composition), so the job writes nothing,
/// reports nothing and leaves every state alone. Returns false when the
/// session could not even be requested (the caller then reports a failed composition write).
pub fn apply_async(
    tid: u32,
    input: ApplyInput,
    current: impl FnOnce() -> bool + 'static,
    done: impl FnOnce(Outcome) + 'static,
) -> bool {
    let ctx = input.ctx.clone();
    let (want_commit, want_comp) = (input.commit_requested(), input.comp_requested());
    let mut done = Some(done);
    let mut current = Some(current);
    let job = move |ec: u32| -> Result<()> {
        if !current.take().is_some_and(|f| f()) {
            return Ok(());
        }
        let mut o = Outcome {
            composition: input.composition.clone(),
            ..Outcome::default()
        };
        let r = apply_body(ec, &input, &mut o);
        if r.is_err() {
            if o.commit.is_none() && want_commit {
                o.commit = Some(Applied::Failed);
            } else if o.comp.is_none() && want_comp {
                o.comp = Some(Applied::Failed);
            }
        }
        if let Some(d) = done.take() {
            d(o);
        }
        r
    };
    run_session(&ctx, tid, false, job).is_ok()
}

/// Applies `commit` and then `new_text` in one synchronous session. The outcome records what was actually done.
pub fn apply(tid: u32, input: ApplyInput) -> Outcome {
    let out = Rc::new(RefCell::new(Outcome {
        composition: input.composition.clone(),
        ..Outcome::default()
    }));
    let o2 = out.clone();
    let ctx = input.ctx.clone();
    let (want_commit, want_comp) = (input.commit_requested(), input.comp_requested());
    let job = move |ec: u32| -> Result<()> { apply_body(ec, &input, &mut o2.borrow_mut()) };
    let res = run_session(&ctx, tid, true, job);
    let mut o = Rc::try_unwrap(out)
        .map(RefCell::into_inner)
        .unwrap_or_else(|rc| {
            let mut b = rc.borrow_mut();
            Outcome {
                commit: b.commit,
                comp: b.comp,
                composition: b.composition.take(),
            }
        });
    if res.is_err() {
        // The session did not run (lock denied, read-only host, ...) or failed midway.
        if o.commit.is_none() && want_commit {
            o.commit = Some(Applied::Failed);
        } else if o.commit == Some(Applied::Applied) && o.comp.is_none() && want_comp {
            o.comp = Some(Applied::Failed);
        } else if o.comp.is_none() && want_comp {
            o.comp = Some(Applied::Failed);
        }
    }
    o
}

impl ApplyInput {
    fn commit_requested(&self) -> bool {
        self.commit.is_some()
    }
    fn comp_requested(&self) -> bool {
        self.new_text.is_some() || self.composition.is_some()
    }
}

/// Removes the composition text and ends the composition (cancel). Ok means the composition is gone.
pub fn cancel_composition(
    tid: u32,
    ctx: &ITfContext,
    comp: &ITfComposition,
    sync: bool,
    attributes: bool,
) -> Result<()> {
    let comp = comp.clone();
    let context = ctx.clone();
    run_session(ctx, tid, sync, move |ec| {
        let range = composition_range(&comp, true)?;
        if attributes { clear_attributes(&context, ec, &range)?; }
        set_text(&range, ec, &[], true)?;
        end_composition(&comp, ec)
    })
}

/// Final-commits the composition text `text` and ends the composition.
pub fn commit_composition(
    tid: u32,
    ctx: &ITfContext,
    comp: &ITfComposition,
    text: &str,
    attributes: bool,
) -> Result<()> {
    let (comp, ctx2, text) = (comp.clone(), ctx.clone(), text.to_string());
    run_session(ctx, tid, true, move |ec| {
        let range = composition_range(&comp, true)?;
        if attributes { clear_attributes(&ctx2, ec, &range)?; }
        set_text(&range, ec, &utf16(&text), true)?;
        set_caret(ec, &ctx2, &range, None, true)?;
        end_composition(&comp, ec)
    })
}

/// Reads the whole composition text (diagnostics / tests only; never logged by the shell).
pub fn composition_text(tid: u32, ctx: &ITfContext, comp: &ITfComposition) -> Option<String> {
    let out = Rc::new(RefCell::new(None));
    let o2 = out.clone();
    let comp = comp.clone();
    run_session(ctx, tid, true, move |ec| unsafe {
        let range = composition_range(&comp, true)?;
        let mut buf = [0u16; 512];
        let mut n = 0u32;
        range.GetText(ec, 0, &mut buf, &mut n)?;
        *o2.borrow_mut() = Some(String::from_utf16_lossy(&buf[..n as usize]));
        Ok(())
    })
    .ok()?;
    let r = out.borrow().clone();
    r
}

/// The document text before the insertion point (start of the selection), at most `max_units` UTF-16 units, read in a
/// synchronous read-only session. None if the host refused or the read failed. Never logged.
pub fn text_before(tid: u32, ctx: &ITfContext, max_units: usize) -> Option<String> {
    let out = Rc::new(RefCell::new(None));
    let o2 = out.clone();
    let ctx2 = ctx.clone();
    let es: ITfEditSession = EditSessionObj {
        job: RefCell::new(Some(Box::new(move |ec| unsafe {
            // Only the default selection: no fallback to the document end (text after the caret is never read).
            let mut sel = [TF_SELECTION::default()];
            let mut fetched = 0u32;
            ctx2.GetSelection(ec, TF_DEFAULT_SELECTION, &mut sel, &mut fetched)?;
            let Some(r) = (fetched == 1)
                .then(|| ManuallyDrop::into_inner(std::mem::take(&mut sel[0].range)))
                .flatten()
            else {
                return Err(windows::Win32::Foundation::E_FAIL.into());
            };
            collapse(&r, ec, TF_ANCHOR_START, true)?;
            let mut moved = 0i32;
            shift_start(&r, ec, -(max_units as i32), &mut moved, true)?;
            let want = moved.unsigned_abs() as usize;
            if moved > 0 || want > max_units {
                return Err(windows::Win32::Foundation::E_FAIL.into());
            }
            let mut buf = vec![0u16; max_units + 8];
            let mut n = 0u32;
            r.GetText(ec, 0, &mut buf, &mut n)?;
            // Complete or nothing: the text must reach the fixed end (the caret); a short read is not used.
            if n as usize != want {
                return Err(windows::Win32::Foundation::E_FAIL.into());
            }
            *o2.borrow_mut() = Some(String::from_utf16_lossy(&buf[..want]));
            Ok(())
        }))),
    }
    .into();
    let hr = request_session(ctx, tid, &es, TF_ES_SYNC | TF_ES_READ).ok()?;
    hr.ok().ok()?;
    let r = out.borrow_mut().take();
    r
}

/// Trace helper: logs the length of the text a range covers (numbers only).
fn dbg_len(ec: u32, range: &ITfRange, tag: &str) {
    if crate::trace::enabled() {
        let mut buf = vec![0u16; 4096];
        let mut n = 0u32;
        let ok = unsafe { range.GetText(ec, 0, &mut buf, &mut n) }.is_ok();
        crate::trace::log(&format!("range {tag}: ok={ok} len={n}"));
    }
}

/// Screen rectangle of the caret (the insertion point) and the host window, for the mode label.
pub fn caret_rect(
    tid: u32,
    ctx: &ITfContext,
) -> Option<(
    windows::Win32::Foundation::RECT,
    windows::Win32::Foundation::HWND,
)> {
    use windows::Win32::Foundation::{HWND, RECT};
    let out = Rc::new(RefCell::new(None));
    let o2 = out.clone();
    let ctx2 = ctx.clone();
    let result = run_session(ctx, tid, true, move |ec| unsafe {
        let view = ctx2.GetActiveView()?;
        let range = insertion_range(ec, &ctx2)?;
        let mut r = RECT::default();
        let mut clipped = false.into();
        step_hr(
            "caret.GetTextExt",
            false,
            (view.vtable().GetTextExt)(view.as_raw(), ec, range.as_raw(), &mut r, &mut clipped),
        )
        .ok()?;
        let hwnd = view.GetWnd().unwrap_or(HWND::default());
        *o2.borrow_mut() = Some((r, hwnd));
        Ok(())
    })
    .map_err(|e| e.code());
    resolve_position(ctx, result, *out.borrow())
}

/// Screen rectangle of the composition and the host window it belongs to (for placing the candidate window).
pub fn composition_rect(
    tid: u32,
    ctx: &ITfContext,
    comp: &ITfComposition,
) -> Option<(
    windows::Win32::Foundation::RECT,
    windows::Win32::Foundation::HWND,
)> {
    use windows::Win32::Foundation::{HWND, RECT};
    let out = Rc::new(RefCell::new(None));
    let o2 = out.clone();
    let (comp, ctx2) = (comp.clone(), ctx.clone());
    let result = geometry_session(|es, flags| request_session(ctx, tid, es, flags), move |ec| unsafe {
        let view = ctx2.GetActiveView()?;
        let range = composition_range(&comp, true)?;
        let mut r = RECT::default();
        let mut clipped = false.into();
        step_hr(
            "composition.GetTextExt",
            false,
            (view.vtable().GetTextExt)(view.as_raw(), ec, range.as_raw(), &mut r, &mut clipped),
        )
        .ok()?;
        let hwnd = view.GetWnd().unwrap_or(HWND::default());
        *o2.borrow_mut() = Some((r, hwnd));
        Ok(())
    })
    .map_err(|e| e.code());
    resolve_position(ctx, result, *out.borrow())
}

use windows::{
    Win32::{
        Foundation::{E_FAIL, HWND, POINT, RECT},
        Graphics::Gdi::{
            ClientToScreen, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONULL,
            MONITORINFO, MonitorFromPoint,
        },
        System::Threading::GetCurrentThreadId,
        UI::{
            Input::{
                Ime::{
                    CFS_FORCE_POSITION, CFS_POINT, CFS_RECT, COMPOSITIONFORM,
                    ImmGetCompositionWindow, ImmGetContext, ImmReleaseContext,
                },
                KeyboardAndMouse::GetFocus,
            },
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{
                GA_ROOT, GUITHREADINFO, GetAncestor, GetClientRect, GetGUIThreadInfo, GetWindowRect, IsIconic,
                IsWindowVisible, GWL_STYLE, GetWindowLongW, WS_CAPTION,
            },
        },
    },
    core::HRESULT,
};

type Position = (RECT, HWND);
type PositionResult = std::result::Result<Position, HRESULT>;

/// Normalize collapsed carets and check only the placement anchor, allowing rectangles
/// to cross monitor edges. The injected predicate also keeps monitor tests deterministic.
fn valid_position(hr: HRESULT, mut r: RECT, on_screen: &impl Fn(POINT) -> bool) -> Option<RECT> {
    if hr.is_err() || r.right < r.left || r.bottom <= r.top {
        return None;
    }
    if r.right == r.left {
        r.right = r.left.checked_add(1)?;
    }
    // The lower-left pixel inside the rectangle (bottom is exclusive), so a rectangle touching the lower screen edge stays
    // valid; the original coordinates go to the candidate window, which moves above the composition near an edge.
    on_screen(POINT {
        x: r.left,
        y: r.bottom - 1,
    })
    .then_some(r)
}

/// Pure and lazy: stop querying providers as soon as one supplies a valid rectangle.
fn select_position(
    attempts: impl IntoIterator<Item = PositionResult>,
    on_screen: &impl Fn(POINT) -> bool,
) -> Option<(usize, Position)> {
    attempts.into_iter().enumerate().find_map(|(layer, a)| {
        let (r, hwnd) = a.ok()?;
        valid_position(HRESULT(0), r, on_screen).map(|r| (layer, (r, hwnd)))
    })
}

fn anchor_on_screen(p: POINT) -> bool {
    !unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONULL) }.is_invalid()
}

fn imm_anchor(form: &COMPOSITIONFORM) -> Option<POINT> {
    match form.dwStyle {
        // An empty or inverted area carries no position.
        CFS_RECT if form.rcArea.right > form.rcArea.left && form.rcArea.bottom > form.rcArea.top => Some(POINT {
            x: form.rcArea.left,
            y: form.rcArea.bottom - 1,
        }),
        CFS_POINT | CFS_FORCE_POSITION => Some(form.ptCurrentPos),
        _ => None,
    }
}

fn system_position() -> Result<Position> {
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(GetCurrentThreadId(), &mut info)?;
        if info.hwndCaret.is_invalid() {
            return Err(E_FAIL.into());
        }
        let mut tl = POINT {
            x: info.rcCaret.left,
            y: info.rcCaret.top,
        };
        let mut br = POINT {
            x: info.rcCaret.right,
            y: info.rcCaret.bottom,
        };
        ClientToScreen(info.hwndCaret, &mut tl).ok()?;
        ClientToScreen(info.hwndCaret, &mut br).ok()?;
        Ok((
            RECT {
                left: tl.x,
                top: tl.y,
                right: br.x,
                bottom: br.y,
            },
            info.hwndCaret,
        ))
    }
}

fn imm_position(hwnd: HWND) -> Result<Position> {
    unsafe {
        if hwnd.is_invalid() {
            return Err(E_FAIL.into());
        }
        let imc = ImmGetContext(hwnd);
        if imc.is_invalid() {
            return Err(E_FAIL.into());
        }
        let mut form = COMPOSITIONFORM::default();
        // Capture the query error before releasing; every acquired context is released.
        let query = ImmGetCompositionWindow(imc, &mut form).ok();
        let released = ImmReleaseContext(hwnd, imc);
        if crate::trace::enabled() {
            crate::trace::log(&format!(
                "caret.imm release_ok={} style={:#x}",
                released.as_bool(),
                form.dwStyle
            ));
        }
        query?;
        let mut p = imm_anchor(&form).ok_or(E_FAIL)?;
        ClientToScreen(hwnd, &mut p).ok()?;
        // IMM supplies a point, so give that anchor a nonempty one-pixel extent.
        Ok((
            RECT {
                left: p.x,
                top: p.y,
                right: p.x.saturating_add(1),
                bottom: p.y.checked_add(1).ok_or(E_FAIL)?,
            },
            hwnd,
        ))
    }
}

thread_local! {
    /// Last text_ext that passed the plausibility checks and was used (stale-translation test only).
    static LAST_TEXT_EXT: std::cell::Cell<Option<crate::caret_trust::Memo>> = const { std::cell::Cell::new(None) };
}

fn monitors() -> Vec<crate::caret_trust::Mon> {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, data: windows::Win32::Foundation::LPARAM) -> windows::core::BOOL {
        let out = unsafe { &mut *(data.0 as *mut Vec<crate::caret_trust::Mon>) };
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if unsafe { GetMonitorInfoW(m, &mut info) }.as_bool() {
            out.push(crate::caret_trust::Mon { monitor: info.rcMonitor, work: info.rcWork });
        }
        true.into()
    }
    let mut v = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), windows::Win32::Foundation::LPARAM(&mut v as *mut _ as isize));
    }
    v
}

/// P0: is the layer-0 rectangle plausible? On success returns the memo to store if this layer ends up selected.
fn trust_text_ext(r: RECT, view_wnd: HWND) -> std::result::Result<Option<crate::caret_trust::Memo>, crate::caret_trust::Reject> {
    use crate::caret_trust::{Host, Inputs, Memo, check_text_ext};
    let (host, dpi, host_client) = unsafe {
        let wnd = if view_wnd.is_invalid() { GetFocus() } else { view_wnd };
        let root = if wnd.is_invalid() { HWND::default() } else { GetAncestor(wnd, GA_ROOT) };
        let mut wr = RECT::default();
        if root.is_invalid()
            || !IsWindowVisible(root).as_bool()
            || IsIconic(root).as_bool()
            || GetWindowRect(root, &mut wr).is_err()
            || wr.right <= wr.left
            || wr.bottom <= wr.top
        {
            (None, 96, None)
        } else {
            let style = GetWindowLongW(root, GWL_STYLE) as u32;
            let mut client = POINT::default();
            let client_ok = ClientToScreen(root, &mut client).as_bool();
            let dpi = GetDpiForWindow(root);
            let captioned = client_ok
                && crate::caret_trust::has_title_bar(style & WS_CAPTION.0 == WS_CAPTION.0, client.y - wr.top, dpi);
            // CARET-MINTTY: the host's client area in screen coordinates, for the client-area check.
            let mut cr = RECT::default();
            let area = if GetClientRect(root, &mut cr).is_ok() {
                let mut tl = POINT { x: cr.left, y: cr.top };
                let mut br = POINT { x: cr.right, y: cr.bottom };
                (ClientToScreen(root, &mut tl).as_bool() && ClientToScreen(root, &mut br).as_bool())
                    .then_some(RECT { left: tl.x, top: tl.y, right: br.x, bottom: br.y })
            } else {
                None
            };
            (Some(Host { root: root.0 as isize, rect: wr, captioned }), dpi, area)
        }
    };
    let sys_caret = system_position()
        .ok()
        .map(|(c, h)| (unsafe { GetAncestor(h, GA_ROOT) }.0 as isize, c));
    let prev = LAST_TEXT_EXT.with(|m| m.get());
    let mons = monitors();
    let input = Inputs { text: r, host, sys_caret, monitors: &mons, dpi, prev, host_client };
    if crate::trace::enabled() {
        crate::trace::log(&crate::caret_trust::describe(&input));
    }
    check_text_ext(&input)?;
    Ok(host.map(|h| Memo { root: h.root, text: r, host: h.rect }))
}

fn window_position(hwnd: HWND) -> Result<Position> {
    unsafe {
        if hwnd.is_invalid() {
            return Err(E_FAIL.into());
        }
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r)?;
        if r.right <= r.left || r.bottom <= r.top {
            return Err(E_FAIL.into());
        }
        let x = r.left.saturating_add(8).min(r.right - 1);
        let y = r.bottom.saturating_sub(8).max(r.top);
        Ok((
            RECT {
                left: x,
                top: y,
                right: x + 1,
                bottom: y + 1,
            },
            hwnd,
        ))
    }
}

fn resolve_position(
    ctx: &ITfContext,
    session: std::result::Result<(), HRESULT>,
    text: Option<Position>,
) -> Option<Position> {
    let mut focus = HWND::default();
    let pending = std::cell::Cell::new(None);
    let names = ["text_ext", "system_caret", "imm", "focus_window"];
    let attempts = (0..4).map(|layer| {
        let result = if layer == 0 {
            session.and_then(|()| text.ok_or(E_FAIL)).and_then(|(r, h)| match trust_text_ext(r, h) {
                Ok(memo) => {
                    pending.set(memo);
                    Ok((r, h))
                }
                Err(why) => {
                    if crate::trace::enabled() {
                        crate::trace::log(&format!(
                            "caret.trust text_ext rejected={} rect={},{},{},{}",
                            why.name(),
                            r.left,
                            r.top,
                            r.right,
                            r.bottom
                        ));
                    }
                    Err(E_FAIL)
                }
            })
        } else {
            if layer == 1 {
                focus = unsafe { GetFocus() };
                if focus.is_invalid() {
                    focus = unsafe {
                        ctx.GetActiveView()
                            .and_then(|v| v.GetWnd())
                            .unwrap_or_default()
                    };
                }
            }
            match layer {
                1 => system_position(),
                2 => imm_position(focus),
                _ => window_position(focus),
            }
            .map_err(|e| e.code())
        };
        if crate::trace::enabled() {
            let (hr, rect) = match result {
                Ok((r, _)) => (HRESULT(0), r),
                Err(hr) => (hr, RECT::default()),
            };
            crate::trace::log(&format!(
                "caret.layer name={} hr={:#010x} valid={} rect={},{},{},{}",
                names[layer],
                hr.0,
                valid_position(hr, rect, &anchor_on_screen).is_some(),
                rect.left,
                rect.top,
                rect.right,
                rect.bottom
            ));
        }
        result
    });
    let selected = select_position(attempts, &anchor_on_screen);
    // Only a text_ext that passed every check and was actually used becomes the stale-test reference.
    if let Some((0, _)) = selected {
        LAST_TEXT_EXT.with(|m| m.set(pending.get()));
    }
    if crate::trace::enabled() {
        match selected {
            Some((layer, (r, _))) => crate::trace::log(&format!(
                "caret.selected layer={} rect={},{},{},{} skipped={}",
                names[layer],
                r.left,
                r.top,
                r.right,
                r.bottom,
                names[layer + 1..].join(",")
            )),
            None => crate::trace::log("caret.selected layer=none rect=none"),
        }
    }
    selected.map(|(_, p)| p)
}

#[cfg(test)]
mod position_tests {
    use super::*;

    #[test]
    fn layout_change_keeps_updated_composition_position_without_fallback() {
        // Simulate a layout notification host: read/write access is forbidden,
        // and no system caret or IMM position is available.
        for top in [120, 360] {
            let rect = RECT { left: 240, top, right: 242, bottom: top + 24 };
            let out = Rc::new(RefCell::new(None));
            let output = out.clone();
            let session = geometry_session(
                |es, flags| {
                    assert_eq!(flags, TF_ES_SYNC | TF_ES_READ);
                    if flags.0 & TF_ES_READWRITE.0 == TF_ES_READWRITE.0 {
                        return Err(E_FAIL.into());
                    }
                    unsafe { es.DoEditSession(7)?; }
                    Ok(HRESULT(0))
                },
                move |ec| {
                    assert_eq!(ec, 7);
                    *output.borrow_mut() = Some((rect, HWND::default()));
                    Ok(())
                },
            ).map_err(|e| e.code());
            let attempts = [session.and_then(|()| out.borrow().ok_or(E_FAIL)), Err(E_FAIL), Err(E_FAIL), Err(E_FAIL)];
            let (layer, (actual, _)) = select_position(attempts, &on_screens(&[SCREEN])).unwrap();
            assert_eq!(layer, 0);
            assert_eq!((actual.left, actual.top, actual.bottom), (240, top, top + 24));
        }
    }

    fn on_screens(screens: &[RECT]) -> impl Fn(POINT) -> bool + '_ {
        |p| {
            screens
                .iter()
                .any(|s| p.x >= s.left && p.x < s.right && p.y >= s.top && p.y < s.bottom)
        }
    }

    fn valid_position(hr: HRESULT, r: RECT, screens: &[RECT]) -> bool {
        super::valid_position(hr, r, &on_screens(screens)).is_some()
    }
    const SCREEN: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    const GOOD: RECT = RECT {
        left: 120,
        top: 240,
        right: 122,
        bottom: 260,
    };

    #[test]
    fn rejects_empty_zero_failed_and_offscreen_rectangles() {
        for r in [
            RECT::default(),
            RECT { right: 0, ..GOOD },
            RECT {
                bottom: GOOD.top,
                ..GOOD
            },
            RECT {
                left: 2000,
                right: 2002,
                ..GOOD
            },
            RECT { left: -1, ..GOOD },
        ] {
            assert!(!valid_position(HRESULT(0), r, &[SCREEN]));
        }
        assert!(!valid_position(E_FAIL, GOOD, &[SCREEN]));
        assert!(!valid_position(HRESULT(0), GOOD, &[]));
        assert!(valid_position(HRESULT(0), GOOD, &[SCREEN]));
        assert!(valid_position(
            HRESULT(0),
            RECT {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1
            },
            &[SCREEN]
        ));
    }

    #[test]
    fn zero_width_caret_is_normalized_and_keeps_the_first_layer() {
        for r in [
            RECT {
                right: GOOD.left,
                ..GOOD
            },
            RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 20,
            },
        ] {
            let attempts =
                std::iter::once(Ok((r, HWND::default()))).chain(std::iter::once_with(|| {
                    panic!("fallback must not be queried")
                }));
            let (layer, (normalized, _)) =
                select_position(attempts, &on_screens(&[SCREEN])).unwrap();
            assert_eq!(layer, 0);
            assert_eq!(normalized.left, r.left);
            assert_eq!(normalized.right, r.left + 1);
            assert_eq!(normalized.top, r.top);
            assert_eq!(normalized.bottom, r.bottom);
        }
    }

    #[test]
    fn accepts_rectangles_crossing_monitors_using_only_the_lower_left_anchor() {
        let right = RECT {
            left: 1920,
            right: 3840,
            ..SCREEN
        };
        let crossing = RECT {
            left: 1910,
            top: 100,
            right: 1930,
            bottom: 120,
        };
        assert!(valid_position(HRESULT(0), crossing, &[SCREEN, right]));
        let selected = select_position(
            [Ok((crossing, HWND::default()))],
            &on_screens(&[SCREEN, right]),
        )
        .unwrap();
        assert_eq!(selected.0, 0);
        assert_eq!(selected.1.0, crossing);
        // The exact seam belongs to the monitor on the right; its far edge is excluded.
        assert!(valid_position(
            HRESULT(0),
            RECT {
                left: 1920,
                ..crossing
            },
            &[SCREEN, right]
        ));
        assert!(!valid_position(
            HRESULT(0),
            RECT {
                left: 3840,
                right: 3850,
                ..crossing
            },
            &[SCREEN, right]
        ));
        // A rectangle whose lower edge touches the screen bottom is still on screen (bottom is exclusive).
        assert!(valid_position(
            HRESULT(0),
            RECT {
                top: 1060,
                bottom: 1080,
                ..GOOD
            },
            &[SCREEN]
        ));
        // A rectangle entirely below the screen is not.
        assert!(!valid_position(
            HRESULT(0),
            RECT {
                top: 1080,
                bottom: 1100,
                ..GOOD
            },
            &[SCREEN]
        ));
        assert!(valid_position(
            HRESULT(0),
            RECT { top: -20, ..GOOD },
            &[SCREEN]
        ));
    }

    #[test]
    fn imm_styles_select_only_their_defined_coordinates() {
        use windows::Win32::UI::Input::Ime::CFS_DEFAULT;
        let mut form = COMPOSITIONFORM {
            dwStyle: CFS_RECT,
            ptCurrentPos: POINT { x: 0, y: 0 },
            rcArea: GOOD,
        };
        assert_eq!(
            imm_anchor(&form),
            Some(POINT {
                x: GOOD.left,
                y: GOOD.bottom - 1
            })
        );
        form.ptCurrentPos = POINT { x: 700, y: 800 };
        assert_eq!(
            imm_anchor(&form),
            Some(POINT {
                x: GOOD.left,
                y: GOOD.bottom - 1
            })
        );
        for style in [CFS_POINT, CFS_FORCE_POSITION] {
            form.dwStyle = style;
            assert_eq!(imm_anchor(&form), Some(form.ptCurrentPos));
        }
        for style in [CFS_DEFAULT, 0x8000_0000, CFS_RECT | CFS_POINT] {
            form.dwStyle = style;
            assert_eq!(imm_anchor(&form), None);
        }
        // CFS_RECT with an empty or inverted area has no position.
        form.dwStyle = CFS_RECT;
        for area in [RECT { right: GOOD.left, ..GOOD }, RECT { bottom: GOOD.top, ..GOOD }, RECT { left: 200, right: 100, ..GOOD }, RECT::default()] {
            form.rcArea = area;
            assert_eq!(imm_anchor(&form), None);
        }
    }

    #[test]
    fn accepts_negative_monitor_but_rejects_desktop_gap() {
        let left = RECT {
            left: -1920,
            top: 0,
            right: -100,
            bottom: 1080,
        };
        assert!(valid_position(
            HRESULT(0),
            RECT {
                left: -300,
                right: -299,
                ..GOOD
            },
            &[left, SCREEN]
        ));
        assert!(!valid_position(
            HRESULT(0),
            RECT {
                left: -50,
                right: -49,
                ..GOOD
            },
            &[left, SCREEN]
        ));
    }

    #[test]
    fn selects_first_valid_layer_for_every_failure_combination() {
        for mask in 0..16 {
            let attempts = (0..4).map(|layer| {
                if mask & (1 << layer) != 0 {
                    Ok((GOOD, HWND::default()))
                } else {
                    Err(E_FAIL)
                }
            });
            assert_eq!(
                select_position(attempts, &on_screens(&[SCREEN])).map(|(i, _)| i),
                (0..4).find(|i| mask & (1 << i) != 0)
            );
        }
    }

    #[test]
    fn skips_invalid_geometry_and_stops_at_first_valid() {
        let rects = [
            RECT::default(),
            RECT {
                left: 2000,
                right: 2001,
                ..GOOD
            },
            GOOD,
        ];
        let attempts =
            rects
                .into_iter()
                .map(|r| Ok((r, HWND::default())))
                .chain(std::iter::once_with(|| {
                    panic!("later layer must not be queried")
                }));
        assert_eq!(
            select_position(attempts, &on_screens(&[SCREEN])).map(|(i, _)| i),
            Some(2)
        );
    }
}

/// Where a (just ended) composition's text sits in the document: (UTF-16 offset from the document start, its text).
/// Documents longer than the buffer are not located.
pub fn locate(ec: u32, ctx: &ITfContext, range: &ITfRange) -> Option<(usize, Vec<u16>)> {
    unsafe {
        let mut buf = vec![0u16; 4096];
        let start = ctx.GetStart(ec).ok()?;
        let pre = clone_range(&start, false).ok()?;
        pre.ShiftEndToRange(ec, range, TfAnchor(0)).ok()?; // TF_ANCHOR_START
        let mut n = 0u32;
        pre.GetText(ec, 0, &mut buf, &mut n).ok()?;
        let offset = n as usize;
        let mut m = 0u32;
        range.GetText(ec, 0, &mut buf, &mut m).ok()?;
        if offset + m as usize >= 4000 {
            return None;
        }
        Some((offset, buf[..m as usize].to_vec()))
    }
}

/// Deletes the text at `offset` if it still equals `text`. Returns whether it was deleted.
pub fn delete_located(ec: u32, ctx: &ITfContext, offset: usize, text: &[u16]) -> Result<bool> {
    unsafe {
        let start = ctx.GetStart(ec)?;
        let r = clone_range(&start, true)?;
        let mut shifted = 0i32;
        shift_end(&r, ec, (offset + text.len()) as i32, &mut shifted, true)?;
        shift_start(&r, ec, offset as i32, &mut shifted, true)?;
        let mut buf = vec![0u16; text.len() + 8];
        let mut n = 0u32;
        r.GetText(ec, 0, &mut buf, &mut n)?;
        if buf[..n as usize] != *text {
            crate::trace::log(&format!(
                "delete_located: range len {n} differs (offset {offset})"
            ));
            return Ok(false);
        }
        set_text(&r, ec, &[], true)?;
        Ok(true)
    }
}

/// Deletes `text` from the document if it occurs exactly once (documents over ~4000 UTF-16 units are not searched).
pub fn delete_unique(ec: u32, ctx: &ITfContext, text: &[u16]) -> Result<bool> {
    unsafe {
        let start = ctx.GetStart(ec)?;
        let all = clone_range(&start, true)?;
        let mut moved = 0i32;
        shift_end(&all, ec, 4000, &mut moved, true)?;
        let mut buf = vec![0u16; 4096];
        let mut n = 0u32;
        all.GetText(ec, 0, &mut buf, &mut n)?;
        if n as usize >= 4000 {
            return Ok(false);
        }
        let hay = &buf[..n as usize];
        crate::trace::log(&format!("delete_unique: doc len {}", hay.len()));
        if text.is_empty() {
            return Ok(false);
        }
        let hits: Vec<usize> = (0..=hay.len().saturating_sub(text.len()))
            .filter(|&i| hay[i..].starts_with(text))
            .collect();
        crate::trace::log(&format!(
            "delete_unique: doc len {} text len {} hits {}",
            hay.len(),
            text.len(),
            hits.len()
        ));
        if hits.len() != 1 {
            return Ok(false);
        }
        delete_located(ec, ctx, hits[0], text)
    }
}

/// EMOJI (EMOJI-plan 3.4): a commit is written with its UTF-16 length (`SetText` takes UTF-16 units), so text outside
/// the BMP and variation selectors keep their exact unit counts.
#[cfg(test)]
mod emoji_utf16_tests {
    use super::utf16;

    #[test]
    fn emoji_commit_lengths_are_utf16_units() {
        // (text, UTF-16 units): a surrogate pair; a BMP symbol plus U+FE0F; a surrogate pair plus U+FE0F
        for (t, units) in [("\u{1F600}", 2), ("\u{2764}\u{FE0F}", 2), ("\u{1F441}\u{FE0F}", 3)] {
            assert_eq!(utf16(t).len(), units, "{t}");
            assert_eq!(String::from_utf16(&utf16(t)).unwrap(), t);
        }
        // emoji followed by a punctuation mark (the combined commit of a highlighted emoji and a key)
        assert_eq!(utf16("\u{2764}\u{FE0F}，").len(), 3);
    }
}
