//! The C interface (`include/twime_shell.h`). Every function returns a status code; a panic never crosses it.

use std::{ffi::{c_char, c_int}, panic::{AssertUnwindSafe, catch_unwind}, ptr};

use twime_proto::{Key, Mode};

use crate::{link::Config, shell::{Context, Input, KeyCode, Reply, Shell}};

const OK: c_int = 0;
const ERR_ARGUMENT: c_int = 1;
const ERR_INTERNAL: c_int = 2;

const EV_KEY_DOWN: u32 = 1;
const EV_KEY_UP: u32 = 2;
const EV_FOCUS_IN: u32 = 3;
const EV_FOCUS_OUT: u32 = 4;
const EV_RESET: u32 = 5;
const EV_SENSITIVE: u32 = 6;
const EV_PICK: u32 = 7;
const EV_TOGGLE_MODE: u32 = 8;
const EV_CLIENT: u32 = 9;
const EV_RELEASE: u32 = 10;
const EV_PANEL_KEY: u32 = 11;

const CLIENT_DEFER: u32 = 1;
const CLIENT_NO_FOCUS_COMMIT: u32 = 2;

const KEY_SHIFT_LEFT: u32 = 200;
const KEY_SHIFT_RIGHT: u32 = 201;
const KEY_MODIFIER: u32 = 202;

const R_HANDLED: u32 = 1;
const R_UI_CHANGED: u32 = 2;
const R_CANDIDATES_OPEN: u32 = 4;
const R_MODE_CHANGED: u32 = 8;
const R_NO_ENGINE: u32 = 16;
const R_COMMITTABLE: u32 = 32;
const R_HELD: u32 = 64;
const R_RELEASED: u32 = 128;
const R_HOLDING: u32 = 256;

const SEG_LOCKED: u32 = 1;
const SEG_PENDING: u32 = 2;
const SEG_UNCONVERTED: u32 = 4;
const SEG_RUN: u32 = 8;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TwimeStr {
    ptr: *const c_char,
    len: usize,
}

#[repr(C)]
pub struct TwimeSegment {
    bytes: u32,
    chars: u32,
    flags: u32,
}

#[repr(C)]
pub struct TwimeReply {
    flags: u32,
    mode: u32,
    commit: TwimeStr,
    preedit: TwimeStr,
    cursor: u32,
    n_segments: u32,
    segments: *const TwimeSegment,
    candidates: *const TwimeStr,
    n_candidates: u32,
    selected: u32,
    page: u32,
    total: u32,
    per_page: u32,
    release_before: u32,
}

#[allow(non_camel_case_types)]
pub struct twime_shell(Shell);

#[allow(non_camel_case_types)]
pub struct twime_ctx {
    ctx: Context,
    // What `reply` points into, besides the strings of the context's own reply.
    preedit: String,
    segments: Vec<TwimeSegment>,
    candidates: Vec<TwimeStr>,
    reply: TwimeReply,
}

fn text(s: &str) -> TwimeStr {
    TwimeStr { ptr: s.as_ptr().cast(), len: s.len() }
}

fn empty_reply() -> TwimeReply {
    TwimeReply {
        flags: 0, mode: 0, commit: text(""), preedit: text(""), cursor: 0, n_segments: 0, segments: ptr::null(),
        candidates: ptr::null(), n_candidates: 0, selected: 0, page: 0, total: 0, per_page: 0, release_before: 0,
    }
}

impl twime_ctx {
    /// Lays the context's last reply out for C. The pointers stay good until the next event of this context.
    fn fill(&mut self) {
        let r: &Reply = self.ctx.reply();
        self.preedit = r.ui.comp.text();
        self.segments = r.ui.comp.segments.iter().map(|s| TwimeSegment {
            bytes: s.text.len() as u32,
            chars: s.text.chars().count() as u32,
            flags: if s.locked { SEG_LOCKED } else { 0 } | if s.pending { SEG_PENDING } else { 0 }
                | if s.unconverted { SEG_UNCONVERTED } else { 0 } | if s.run { SEG_RUN } else { 0 },
        }).collect();
        self.candidates = r.ui.candidates.items.iter().map(|s| text(s)).collect();
        let c = &r.ui.candidates;
        self.reply = TwimeReply {
            flags: if r.handled { R_HANDLED } else { 0 } | if r.ui_changed { R_UI_CHANGED } else { 0 }
                | if r.ui.candidates_open { R_CANDIDATES_OPEN } else { 0 } | if r.mode_changed { R_MODE_CHANGED } else { 0 }
                | if r.no_engine { R_NO_ENGINE } else { 0 } | if r.ui.committable { R_COMMITTABLE } else { 0 }
                | if r.held { R_HELD } else { 0 } | if r.released { R_RELEASED } else { 0 } | if r.holding { R_HOLDING } else { 0 },
            mode: u32::from(r.mode == Mode::English),
            commit: text(&r.commit),
            preedit: text(&self.preedit),
            cursor: u32::from(r.ui.comp.cursor),
            n_segments: self.segments.len() as u32,
            segments: self.segments.as_ptr(),
            candidates: self.candidates.as_ptr(),
            n_candidates: self.candidates.len() as u32,
            selected: u32::from(c.selected),
            page: u32::from(c.page),
            total: u32::from(c.total),
            per_page: u32::from(c.per_page),
            release_before: r.release_before,
        };
    }
}

fn guard(f: impl FnOnce() -> c_int) -> c_int {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(ERR_INTERNAL)
}

fn key_code(code: u32) -> KeyCode {
    match code {
        KEY_SHIFT_LEFT => KeyCode::ShiftLeft,
        KEY_SHIFT_RIGHT => KeyCode::ShiftRight,
        KEY_MODIFIER => KeyCode::Modifier,
        _ => KeyCode::Key(u8::try_from(code).ok().and_then(Key::from_code).unwrap_or(Key::Other)),
    }
}

/// # Safety
/// `out` is null or points to writable storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn twime_shell_open(out: *mut *mut twime_shell) -> c_int {
    guard(|| {
        if out.is_null() {
            return ERR_ARGUMENT;
        }
        let shell = Box::into_raw(Box::new(twime_shell(Shell::new(Config::default()))));
        unsafe { *out = shell };
        OK
    })
}

/// # Safety
/// `shell` is null or came from `twime_shell_open` and was not closed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn twime_shell_close(shell: *mut twime_shell) -> c_int {
    guard(|| {
        if !shell.is_null() {
            drop(unsafe { Box::from_raw(shell) });
        }
        OK
    })
}

/// # Safety
/// `shell` is null or a live shell; `out` is null or points to writable storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn twime_ctx_new(shell: *mut twime_shell, out: *mut *mut twime_ctx) -> c_int {
    guard(|| {
        if shell.is_null() || out.is_null() {
            return ERR_ARGUMENT;
        }
        let ctx = unsafe { &*shell }.0.context();
        let boxed = Box::new(twime_ctx { ctx, preedit: String::new(), segments: vec![], candidates: vec![], reply: empty_reply() });
        unsafe { *out = Box::into_raw(boxed) };
        OK
    })
}

/// # Safety
/// `ctx` is null or came from `twime_ctx_new` and was not freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn twime_ctx_free(ctx: *mut twime_ctx) -> c_int {
    guard(|| {
        if !ctx.is_null() {
            drop(unsafe { Box::from_raw(ctx) });
        }
        OK
    })
}

/// # Safety
/// `ctx` is null or a live context; `out` is null or points to writable storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn twime_ctx_event(ctx: *mut twime_ctx, kind: u32, a: u32, b: u32, out: *mut *const TwimeReply) -> c_int {
    guard(|| {
        if ctx.is_null() {
            return ERR_ARGUMENT;
        }
        let input = match kind {
            EV_KEY_DOWN => Input::KeyDown(key_code(a), b as u8),
            EV_KEY_UP => Input::KeyUp(key_code(a), b as u8),
            EV_FOCUS_IN => Input::FocusIn,
            EV_FOCUS_OUT => Input::FocusOut,
            EV_RESET => Input::Reset,
            EV_SENSITIVE => Input::Sensitive(a != 0),
            EV_PICK => match u16::try_from(a) {
                Ok(i) => Input::Pick(i),
                Err(_) => return ERR_ARGUMENT,
            },
            EV_TOGGLE_MODE => Input::ToggleMode,
            EV_CLIENT if a & !(CLIENT_DEFER | CLIENT_NO_FOCUS_COMMIT) == 0 => {
                Input::Client { defer: a & CLIENT_DEFER != 0, focus_commit: a & CLIENT_NO_FOCUS_COMMIT == 0 }
            }
            EV_RELEASE => Input::Release,
            EV_PANEL_KEY => Input::PanelKey(key_code(a), b as u8),
            _ => return ERR_ARGUMENT,
        };
        let c = unsafe { &mut *ctx };
        c.ctx.event(input);
        c.fill();
        if !out.is_null() {
            unsafe { *out = &c.reply };
        }
        OK
    })
}
