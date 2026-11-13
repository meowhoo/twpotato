//! TSF test host: a Win32 window with edit controls in which the TwIme profile is activated
//! for this process only (`TF_IPPMF_FORPROCESS`); keys are sent with `SendInput` and the results are read back from
//! the edit controls. Nothing outside this process is switched, and the foreground window is checked before keys are sent.
//!
//!   testhost [scenario ...]      scenarios: basic edit forty focus kill stall readonly external password peek word
//!                                UX1: ux-punct ux-run ux-mode ux-sym ux-caps ux-restart (named only: ux-restart-slow ux-default-english)
//!                                (UX1 fault scenarios, named only, dev-mode stage with TWIME_FAULT: ux-fault-commit, ux-fault-commit-run (commit-fail), ux-fault-fail (async-fail))
//!                                SETUI (named only): setui-shift setui-english-refresh setui-add focus-cycle late-push
//!                                EMOJI (named only, engine with emoji=on and TH_EMOJI_TSV set): emoji, emoji-fault (+ TWIME_FAULT=commit-fail)
//!                                Sandbox P6 (named only): pop-all pop-idle pop-fast pop-digit pop-select pop-layout pop-dismiss
//!                                ZHUYIN-OUT (named only, Sandbox runner): zy-all, zy-<name> of the TH_ZY_SCEN file (testhost/zyout.rs)
//!
//! The expected texts are fixed phrases written for this test.

#![cfg(windows)]

use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use twime_core::keymap::key_of;
use twime_proto::{Key, ids};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx},
            LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
            Threading::{OpenProcess, PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE, TerminateProcess},
        },
        UI::{
            Input::KeyboardAndMouse::{
                HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, SetFocus, VIRTUAL_KEY, VK_CONTROL,
            },
            TextServices::{CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr, TF_IPPMF_FORPROCESS, TF_PROFILETYPE_INPUTPROCESSOR, TF_PROFILETYPE_KEYBOARDLAYOUT},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowTextW, MSG, PostMessageW,
                PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, ShowWindow, TranslateMessage, WNDCLASSW,
                WM_APP, WM_CLOSE, WM_DESTROY, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE, ES_AUTOHSCROLL, ES_PASSWORD,
                SW_SHOW, WM_SETTEXT,
            },
        },
    },
    core::{PCWSTR, w},
};

#[path = "testhost/lexux.rs"]
mod lexux;
#[path = "testhost/pop.rs"]
mod pop;
#[path = "testhost/zyout.rs"]
mod zyout;

const EM_SETSEL: u32 = 0x00B1;
const EM_SETREADONLY: u32 = 0x00CF;
const WM_FOCUS_EDIT: u32 = WM_APP + 1;
const WM_ACTIVATE_TIP: u32 = WM_APP + 2;
const WM_QUERY_FOCUS: u32 = WM_APP + 6;
const WM_LATE_PUSH: u32 = WM_APP + 7;
const WM_KEY_FOCUS: u32 = WM_APP + 8;
const WM_RECOVER_KEY_FOCUS: u32 = WM_APP + 9;

#[derive(Clone, Copy)]
struct Ui {
    main: isize,
    a: isize,
    b: isize,
    pw: isize,
    other: isize,
    other_edit: isize,
}
impl Ui {
    fn h(v: isize) -> HWND {
        HWND(v as _)
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ---- input --------------------------------------------------------------------------------------------------------

fn vk_of(k: Key) -> u16 {
    match k {
        Key::Letter(c) => u16::from(c) - 32,
        Key::Digit(d) => 0x30 + u16::from(d),
        Key::Minus => 0xBD,
        Key::Comma => 0xBC,
        Key::Period => 0xBE,
        Key::Slash => 0xBF,
        Key::Semicolon => 0xBA,
        Key::Space => 0x20,
        Key::Enter => 0x0D,
        Key::Esc => 0x1B,
        Key::Backspace => 0x08,
        Key::Delete => 0x2E,
        Key::Tab => 0x09,
        Key::Left => 0x25,
        Key::Right => 0x27,
        Key::Up => 0x26,
        Key::Down => 0x28,
        Key::Home => 0x24,
        Key::End => 0x23,
        Key::PageUp => 0x21,
        Key::PageDown => 0x22,
        Key::Other => 0,
        Key::Backquote => 0xC0,
        Key::LBracket => 0xDB,
        Key::RBracket => 0xDD,
        Key::Quote => 0xDE,
        Key::Backslash => 0xDC,
        Key::Equals => 0xBB,
        Key::NumpadDecimal => 0x6E,
        Key::NumpadDigit(d) => 0x60 + u16::from(d),
        Key::NumpadOp(o) => [0x6B, 0x6D, 0x6A, 0x6F][o as usize & 3],
    }
}

static HOST_WS_BEFORE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MAIN_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static ABORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn aborted() -> bool {
    ABORTED.load(std::sync::atomic::Ordering::SeqCst)
}

fn send_vk(vk: u16) {
    send_raw(&[(vk, false), (vk, true)]);
}

/// Injects key events `(virtual key, is key-up)` in one batch, only while the test window is in the foreground.
fn send_raw(events: &[(u16, bool)]) {
    // Never inject a key unless the test window is in the foreground (the user may have switched windows).
    let fg = unsafe { GetForegroundWindow() };
    if aborted() || fg.0 as isize != MAIN_HWND.load(std::sync::atomic::Ordering::SeqCst) {
        if !aborted() {
            let mut cls = [0u16; 64];
            let n = unsafe { windows::Win32::UI::WindowsAndMessaging::GetClassNameW(fg, &mut cls) } as usize;
            let mut pid = 0u32;
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(fg, Some(&mut pid)) };
            println!("INFO	foreground changed to class {:?} pid {pid}", String::from_utf16_lossy(&cls[..n]));
        }
        ABORTED.store(true, std::sync::atomic::Ordering::SeqCst);
        return;
    }
    let inputs: Vec<INPUT> = events
        .iter()
        .map(|&(vk, up)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, time: 0, dwExtraInfo: 0 } },
        })
        .collect();
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

fn press(k: Key) {
    send_vk(vk_of(k));
    std::thread::sleep(Duration::from_millis(25));
}

/// Types bopomofo text: ' ' is Space; tone marks and symbols go through the standard layout.
fn typ(s: &str) {
    for c in s.chars() {
        if c == ' ' {
            press(Key::Space);
        } else if let Some(k) = key_of(c) {
            press(k);
        } else {
            panic!("no key for {c}");
        }
    }
    std::thread::sleep(Duration::from_millis(60));
}

fn text_of(h: isize) -> String {
    let mut buf = vec![0u16; 4096];
    let n = unsafe { GetWindowTextW(Ui::h(h), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn set_text(h: isize, s: &str) {
    let w = wide(s);
    unsafe {
        SendMessageW(Ui::h(h), WM_SETTEXT, Some(WPARAM(0)), Some(LPARAM(w.as_ptr() as isize)));
    }
}

fn wait_for(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < limit {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

// ---- engine control (the process the TIP started) --------------------------------------------------------------------

fn engine_pid() -> Option<u32> {
    let id = twime_winpipe::current_identity().ok()?;
    // A private stage (dev-mode, N3b) names its pipe with TWIME_PIPE; otherwise the per-user name.
    let pipe = std::env::var("TWIME_PIPE").map(|p| twime_winpipe::qualify(&p)).unwrap_or_else(|_| twime_winpipe::pipe_name(&id));
    twime_winpipe::Client::connect(&pipe, None, 0).ok().map(|c| c.server_pid)
}

fn ntdll_call(name: &'static [u8], pid: u32) -> bool {
    unsafe {
        let lib = match LoadLibraryW(w!("ntdll.dll")) {
            Ok(l) => l,
            Err(_) => return false,
        };
        let Some(f) = GetProcAddress(lib, windows::core::PCSTR(name.as_ptr())) else { return false };
        let f: extern "system" fn(HANDLE) -> i32 = std::mem::transmute(f);
        let Ok(h) = OpenProcess(PROCESS_SUSPEND_RESUME, false, pid) else { return false };
        let r = f(h);
        let _ = CloseHandle(h);
        r >= 0
    }
}

fn kill_engine() -> bool {
    let Some(pid) = engine_pid() else { return false };
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) else { return false };
        let r = TerminateProcess(h, 9).is_ok();
        let _ = CloseHandle(h);
        r
    }
}

// ---- checks ---------------------------------------------------------------------------------------------------------

struct Report {
    pass: usize,
    fail: usize,
}
impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl FnOnce() -> String) {
        if ok {
            self.pass += 1;
            println!("PASS\t{name}");
        } else {
            self.fail += 1;
            println!("FAIL\t{name}\t{}", detail());
        }
    }
}

fn clear(ui: Ui) {
    set_text(ui.a, "");
    std::thread::sleep(Duration::from_millis(80));
}

/// Waits until the user has not touched keyboard or mouse for `secs` seconds (the test window takes the foreground and
/// receives keys, so it must not appear while someone is typing).
fn wait_user_idle(secs: u32, max_wait: Duration) -> bool {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let t = Instant::now();
    loop {
        let mut li = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        let idle_ms = unsafe { if GetLastInputInfo(&mut li).as_bool() { GetTickCount().wrapping_sub(li.dwTime) } else { 0 } };
        if idle_ms >= secs * 1000 {
            return true;
        }
        if t.elapsed() > max_wait {
            return false;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn focus_a(ui: Ui) -> bool {
    unsafe {
        SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.a as usize)), None);
        let fg = wait_for(Duration::from_secs(3), || GetForegroundWindow() == Ui::h(ui.main));
        fg
    }
}

/// Keys drive the shell (it starts the engine and reconnects only when a key arrives): Esc is harmless without a composition.
fn wake_engine() -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(25) {
        if engine_pid().is_some() {
            std::thread::sleep(Duration::from_millis(1200));
            press(Key::Esc);
            std::thread::sleep(Duration::from_millis(700));
            press(Key::Esc);
            std::thread::sleep(Duration::from_millis(300));
            return true;
        }
        press(Key::Esc);
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

fn ready_engine() -> bool {
    // Keys typed before the engine is up pass through; wait until the pipe answers.
    wait_for(Duration::from_secs(20), || engine_pid().is_some())
}

const SENTENCE: &str = "ㄐㄧㄣ ㄊㄧㄢ ㄊㄧㄢ ㄑㄧˋㄏㄣˇㄏㄠˇ"; // 今天天氣很好

fn scenarios(ui: Ui, names: &[String], r: &mut Report) {
    let want = |n: &str| names.is_empty() || names.iter().any(|x| x == n);
    if !wait_user_idle(20, Duration::from_secs(1800)) {
        r.check("user idle", false, || "the user kept typing for 30 minutes; not starting".into());
        return;
    }
    unsafe {
        SendMessageW(Ui::h(ui.main), WM_SHOWME, None, None);
    }
    if !focus_a(ui) {
        r.check("foreground", false, || "test window is not in the foreground; aborting before sending any key".into());
        return;
    }
    r.check("engine up", ready_engine(), || "engine did not start".into());
    if pop::run(ui, names, r) { return; }
    if zyout::run(ui, names, r) { return; }
    if lexux::run(ui, names, r) { return; }
    // The first key after activation may race the connect; a warm-up key sequence settles it.
    typ("ㄅ");
    press(Key::Esc);
    clear(ui);

    if want("basic") {
        typ(SENTENCE);
        let shown = comp_text(ui).unwrap_or_default();
        r.check("basic: composition shows the converted sentence", shown == "今天天氣很好", || format!("shown {shown:?}"));
        press(Key::Enter);
        std::thread::sleep(Duration::from_millis(100));
        let t = text_of(ui.a);
        r.check("basic: Enter commits once", t == "今天天氣很好", || format!("text {t:?}"));
        // The next sentence is converted with the committed text as context and appended, not replaced.
        typ(SENTENCE);
        press(Key::Enter);
        std::thread::sleep(Duration::from_millis(100));
        let t = text_of(ui.a);
        r.check("basic: second commit appends", t.chars().count() == 12, || format!("len {}", t.chars().count()));
        clear(ui);
    }
    if want("edit") {
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        press(Key::Backspace);
        r.check("edit: Backspace removes the last syllable", matches!(comp_text(ui).as_deref(), Some("今") | Some("金")), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        std::thread::sleep(Duration::from_millis(80));
        r.check("edit: Esc cancels without committing", text_of(ui.a).is_empty() && comp_text(ui).is_none(), || format!("{:?}", text_of(ui.a)));
        // A syllable that does not exist stays as typed and blocks Enter.
        typ("ㄅㄅ ");
        press(Key::Enter);
        std::thread::sleep(Duration::from_millis(80));
        r.check("edit: unconvertible syllable is kept, nothing committed", comp_text(ui).as_deref() == Some("ㄅ"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        std::thread::sleep(Duration::from_millis(80));
        clear(ui);
    }
    if want("len") {
        for n in [10usize, 20, 30, 39] {
            for _ in 0..n {
                typ("ㄊㄧㄢ ");
            }
            press(Key::Enter);
            std::thread::sleep(Duration::from_millis(100));
            let t = text_of(ui.a);
            r.check(&format!("len: {n} syllables commit once"), t.chars().count() == n, || format!("len {}", t.chars().count()));
            clear(ui);
        }
    }
    if want("forty") {
        for i in 0..40 {
            typ("ㄊㄧㄢ ");
            if i >= 36 {
                println!("INFO	forty i={i}: text {} comp {:?}", text_of(ui.a).chars().count(), comp_text(ui).map(|c| c.chars().count()));
            }
        }
        println!("INFO	forty: text {} chars, comp {:?} chars", text_of(ui.a).chars().count(), comp_text(ui).map(|c| c.chars().count()));
        let t = text_of(ui.a); // a TSF-native control holds the composition text in its buffer
        r.check("forty: 40 syllables show 40 characters (39 committed + 1 composing)", t.chars().count() == 40, || format!("len {}", t.chars().count()));
        r.check("forty: the composition is the last syllable only", comp_text(ui).is_none_or(|c| c.chars().count() == 1), || format!("{:?}", comp_text(ui)));
        r.check("forty: every character is the typed word", t.chars().all(|c| c == '天'), || format!("{t:?}"));
        press(Key::Enter);
        std::thread::sleep(Duration::from_millis(100));
        let t = text_of(ui.a);
        println!("INFO	forty after Enter: {} chars, comp {:?}, tian={}", t.chars().count(), comp_text(ui), t.chars().filter(|c| *c == '天').count());
        r.check("forty: nothing lost or duplicated after Enter", t.chars().count() == 40, || format!("len {}", t.chars().count()));
        clear(ui);
    }
    if want("focus") {
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        unsafe {
            SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.b as usize)), None);
        }
        std::thread::sleep(Duration::from_millis(200));
        let t = text_of(ui.a);
        r.check("focus: leaving with a convertible composition commits it once", t == "今天", || format!("{t:?}"));
        focus_a(ui);
        typ("ㄅㄅ ");
        unsafe {
            SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.b as usize)), None);
        }
        std::thread::sleep(Duration::from_millis(200));
        let t = text_of(ui.a);
        r.check("focus: leaving with an unconvertible composition discards it", t == "今天", || format!("{t:?}"));
        focus_a(ui);
        clear(ui);
    }
    if want("kill") {
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        r.check("kill: engine process found", engine_pid().is_some(), || "no engine".into());
        let t0 = Instant::now();
        kill_engine();
        press(Key::Letter(b'a')); // would be bopomofo; with the engine gone it must pass to the host
        std::thread::sleep(Duration::from_millis(150));
        let t = text_of(ui.a);
        r.check("kill: convertible composition committed, current key not lost, host not frozen", t.starts_with("今天") && t.chars().count() == 3, || format!("{t:?} after {} ms", t0.elapsed().as_millis()));
        press(Key::Letter(b'b'));
        std::thread::sleep(Duration::from_millis(80));
        r.check("kill: later keys are not lost (raw while the engine is down, bopomofo after it is back)", (3..=4).contains(&text_of(ui.a).chars().count()), || format!("{:?}", text_of(ui.a)));
        clear(ui);
        r.check("kill: engine restarts by itself", ready_engine(), || "engine did not come back".into());
        std::thread::sleep(Duration::from_millis(700));
        typ(SENTENCE);
        r.check("kill: conversion works again", comp_text(ui).as_deref() == Some("今天天氣很好"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
        // Unconvertible composition + engine gone: cancelled, not committed.
        typ("ㄐㄧㄣ ㄅㄅ ");
        kill_engine();
        press(Key::Letter(b'a'));
        std::thread::sleep(Duration::from_millis(150));
        let t = text_of(ui.a);
        r.check("kill: unconvertible composition cancelled, key not lost", t == "a", || format!("{t:?}"));
        clear(ui);
        ready_engine();
        std::thread::sleep(Duration::from_millis(700));
    }
    if want("stall") {
        wake_engine();
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        let Some(pid) = engine_pid() else {
            r.check("stall: engine found", false, || "none".into());
            return;
        };
        ntdll_call(b"NtSuspendProcess\0", pid);
        let t0 = Instant::now();
        press(Key::Letter(b'a'));
        let ok = wait_for(Duration::from_secs(2), || { let t = text_of(ui.a); t.starts_with("今天") && t.chars().count() == 3 });
        let dt = t0.elapsed().as_millis();
        r.check("stall: engine not answering -> composition committed, key delivered within the time limit", ok && dt < 400, || format!("{:?} after {dt} ms", text_of(ui.a)));
        ntdll_call(b"NtResumeProcess\0", pid);
        std::thread::sleep(Duration::from_millis(300));
        r.check("stall: the late reply changed nothing", text_of(ui.a).starts_with("今天") && text_of(ui.a).chars().count() == 3, || format!("{:?}", text_of(ui.a)));
        clear(ui);
        std::thread::sleep(Duration::from_millis(800));
        typ(SENTENCE);
        r.check("stall: works again after the engine resumed", comp_text(ui).as_deref() == Some("今天天氣很好"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
    }
    if want("readonly") {
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        unsafe {
            SendMessageW(Ui::h(ui.a), EM_SETREADONLY, Some(WPARAM(1)), None);
        }
        press(Key::Letter(b'j')); // ㄨ: the host now refuses writes
        std::thread::sleep(Duration::from_millis(150));
        unsafe {
            SendMessageW(Ui::h(ui.a), EM_SETREADONLY, Some(WPARAM(0)), None);
        }
        let t1 = text_of(ui.a);
        press(Key::Letter(b'a'));
        press(Key::Letter(b'b'));
        std::thread::sleep(Duration::from_millis(200));
        let t2 = text_of(ui.a);
        println!("INFO\treadonly: after refused write {t1:?}; after two more keys {t2:?}");
        r.check("readonly: no duplicated text after a refused write", t2.chars().filter(|c| *c == '今').count() <= 1, || format!("{t2:?}"));
        clear(ui);
        std::thread::sleep(Duration::from_millis(100));
        typ(SENTENCE);
        r.check("readonly: normal again after recovery", comp_text(ui).as_deref() == Some("今天天氣很好"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
    }
    if want("external") {
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        set_text(ui.a, "xyz");
        std::thread::sleep(Duration::from_millis(200));
        press(Key::Letter(b'a'));
        std::thread::sleep(Duration::from_millis(100));
        let t = text_of(ui.a);
        r.check("external: composition dropped after the host text changed, typed key passes", !t.contains('今') && !t.contains('天'), || format!("{t:?}"));
        clear(ui);
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        unsafe {
            SendMessageW(Ui::h(ui.a), EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        std::thread::sleep(Duration::from_millis(200));
        press(Key::Letter(b'a'));
        std::thread::sleep(Duration::from_millis(100));
        let t = text_of(ui.a);
        println!("INFO\texternal: after selection move {t:?}");
        r.check("external: selection move cancels the composition (text removed, not committed)", !t.contains('今') && !t.contains('金'), || format!("{t:?}"));
        clear(ui);
        std::thread::sleep(Duration::from_millis(100));
        typ(SENTENCE);
        r.check("external: typing works again", comp_text(ui).as_deref() == Some("今天天氣很好"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
    }
    if want("password") {
        unsafe {
            SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.pw as usize)), None);
        }
        std::thread::sleep(Duration::from_millis(200));
        typ("ㄐㄧㄣ ");
        let t = text_of(ui.pw);
        println!("INFO\tpassword: field text after typing bopomofo keys: {t:?}");
        r.check("password: no composition / conversion in a password field", !t.contains('今'), || format!("{t:?}"));
        set_text(ui.pw, "");
        focus_a(ui);
    }
    if want("cand") {
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect, IsWindowVisible};
        let find = || unsafe { FindWindowW(w!("TwImeCandidate"), PCWSTR::null()).ok() };
        let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(Ui::h(ui.main)) } as i32;
        println!("INFO\tcand: host DPI {dpi}");
        typ("ㄐㄧㄣ ");
        let first = comp_text(ui).unwrap_or_default();
        press(Key::Down);
        std::thread::sleep(Duration::from_millis(200));
        let w = find();
        r.check("cand: Down opens the candidate window", w.is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() }), || "no window".into());
        if let Some(h) = w {
            let (mut cr, mut ar) = (windows::Win32::Foundation::RECT::default(), windows::Win32::Foundation::RECT::default());
            unsafe {
                let _ = GetWindowRect(h, &mut cr);
                let _ = GetWindowRect(Ui::h(ui.a), &mut ar);
            }
            println!("INFO\tcand: window {:?} editor {:?}", (cr.left, cr.top, cr.right, cr.bottom), (ar.left, ar.top, ar.right, ar.bottom));
            let slack = (80 * dpi) / 96;
            r.check("cand: placed under the composition line", cr.top >= ar.top && cr.top <= ar.bottom + slack && cr.left >= ar.left - 5 && cr.left < ar.right, || format!("{:?} vs {:?}", (cr.left, cr.top), (ar.left, ar.bottom)));
            let hgt = cr.bottom - cr.top;
            r.check("cand: height follows DPI", hgt >= (16 * dpi) / 96 && hgt <= (80 * dpi) / 96, || format!("{hgt}"));
        }
        r.check("cand: the host keeps the foreground", unsafe { GetForegroundWindow() } == Ui::h(ui.main), || "foreground moved".into());
        press(Key::Digit(2));
        std::thread::sleep(Duration::from_millis(150));
        let second = comp_text(ui).unwrap_or_default();
        r.check("cand: digit picks the second candidate and closes the list", second != first && find().is_none(), || format!("{first:?} -> {second:?}, window {}", find().is_some()));
        typ("ㄊㄧㄢ ");
        press(Key::Down);
        std::thread::sleep(Duration::from_millis(200));
        let before = comp_text(ui).unwrap_or_default();
        if let Some(h) = find() {
            let mut cr = windows::Win32::Foundation::RECT::default();
            unsafe {
                let _ = GetWindowRect(h, &mut cr);
                let x = ((cr.right - cr.left) * 6) / 10;
                PostMessageW(Some(h), 0x0201, WPARAM(1), LPARAM((10 << 16) | x as isize)).ok();
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let after = comp_text(ui).unwrap_or_default();
        r.check("cand: a click picks a candidate and closes the list", find().is_none() && after != before, || format!("{before:?} -> {after:?}, window {}", find().is_some()));
        press(Key::Down);
        std::thread::sleep(Duration::from_millis(100));
        press(Key::Esc);
        r.check("cand: Esc closes the list but keeps the composition", find().is_none() && comp_text(ui).is_some(), || "unexpected".into());
        press(Key::Esc);
        clear(ui);
    }
    if want("word") {
        use twime_core::{Config, Engine};
        use twime_model::{Assets, ModelPaths};
        use twime_proto::{KeyEvent, Update};
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, IsWindowVisible};
        let find = || unsafe { FindWindowW(w!("TwImeCandidate"), PCWSTR::null()).ok() };
        let visible = || find().is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() });
        // The same assets and defaults as the engine process (TRAIN T-model weights, prior, lambda 0.5; G1K-s07 before 2026-10-22): an in-process session
        // that gets the same keys is the reference for the candidate list and the composition.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let assets = Assets::load(&ModelPaths {
            data_txt: root.join("third_party/mcbopomofo/data.txt"),
            aliases: root.join("engine/data/common-readings.txt"),
            model: root.join("data/train/T-model.twg1"),
            prior: Some(root.join("data/vd/prior/freq.pack")),
        })
        .expect("assets");
        let engine = Engine::new(Arc::new(assets), Config::default());
        let mut refs = engine.new_session();
        let mut feed = |k: Key| -> Update { refs.on_key(KeyEvent::press(k)) };
        let comp_of = |u: &Update| u.composition.as_ref().map(twime_proto::Comp::text).unwrap_or_default();
        let trace_pages = || -> Option<String> {
            let p = std::env::var("TWIME_TRACE").ok()?;
            std::fs::read_to_string(p).ok()?.lines().rev().find(|l| l.contains("cands page=")).map(str::to_string)
        };
        // Type 權利 / 權力 / 全力: ㄑㄩㄢˊ ㄌㄧˋ, on the host and on the reference.
        let keys: Vec<Key> = "ㄑㄩㄢˊㄌㄧˋ".chars().map(|c| key_of(c).unwrap()).collect();
        for &k in &keys {
            press(k);
            feed(k);
        }
        let shown = comp_text(ui).unwrap_or_default();
        r.check("word: the converted two-syllable text is shown", shown.chars().count() == 2, || format!("{} chars", shown.chars().count()));
        press(Key::Down);
        let u = feed(Key::Down);
        std::thread::sleep(Duration::from_millis(250));
        let c = u.candidates.clone().expect("reference opens the list");
        r.check("word: Down opens the candidate window", visible(), || "no window".into());
        r.check("word: the engine reports totals for the page indicator", c.total as usize > c.items.len() && c.per_page == 9, || format!("total {} per page {}", c.total, c.per_page));
        // Walk the reference through every page to find the first multi-character item that is not the current one.
        let mut all: Vec<String> = c.items.clone();
        for _ in 1..(c.total as usize).div_ceil(9) {
            let n = feed(Key::PageDown).candidates.unwrap();
            all.extend(n.items);
        }
        feed(Key::PageDown); // wraps to the first page
        let target = (1..all.len()).find(|&i| all[i].chars().count() >= 2).or_else(|| (all[0].chars().count() >= 2).then_some(0));
        r.check("word: the list contains a multi-character word to choose", target.is_some(), || format!("{} items, none with two characters", all.len()));
        if let Some(t) = target {
        let (page, idx) = (t / 9, t % 9);
        println!("INFO\tword: {} candidates, {} pages; choosing item {} (a {}-character word) on page {}", all.len(), all.len().div_ceil(9), t + 1, all[t].chars().count(), page + 1);
        // Paging: Tab forward and Shift-less PageUp back leave the window open and the composition unchanged.
        press(Key::Tab);
        feed(Key::Tab);
        std::thread::sleep(Duration::from_millis(150));
        r.check("word: Tab pages forward and the window stays open", visible() && comp_text(ui).as_deref() == Some(shown.as_str()), || "window or composition changed".into());
        if std::env::var("TWIME_TRACE").is_ok() {
            let l = trace_pages();
            r.check("word: the shell shows the page indicator (page 2 of the reference page count)", l.as_deref().is_some_and(|l| l.contains(&format!("page=Some((2, {}))", all.len().div_ceil(9)))), || format!("{l:?}"));
        } else {
            println!("INFO	word: TWIME_TRACE not set; the page indicator check is skipped");
        }
        press(Key::PageUp);
        feed(Key::PageUp);
        std::thread::sleep(Duration::from_millis(150));
        r.check("word: PageUp pages back", visible(), || "window closed".into());
        for _ in 0..page {
            press(Key::PageDown);
            feed(Key::PageDown);
        }
        press(Key::Digit(idx as u8 + 1));
        let u = feed(Key::Digit(idx as u8 + 1));
        std::thread::sleep(Duration::from_millis(200));
        let want = comp_of(&u);
        let got = comp_text(ui).unwrap_or_default();
        r.check("word: the digit chooses the word; the composition equals the reference", got == want && got == all[t] && !visible(), || format!("window {}", visible()));
        // A later syllable: the chosen word stays, the new character is converted by the model.
        let third: Vec<Key> = "ㄧˇ".chars().map(|c| key_of(c).unwrap()).collect();
        for &k in &third {
            press(k);
            feed(k);
        }
        let want3 = comp_of(&feed(Key::Other));
        let got3 = comp_text(ui).unwrap_or_default();
        r.check("word: after another syllable the chosen word is kept and the rest equals the reference", got3 == want3 && got3.starts_with(&all[t]), || format!("{} chars", got3.chars().count()));
        press(Key::Down);
        feed(Key::Down);
        std::thread::sleep(Duration::from_millis(200));
        r.check("word: Down opens the list for the third syllable", visible(), || "no window".into());
        press(Key::Esc);
        feed(Key::Esc);
        std::thread::sleep(Duration::from_millis(150));
        r.check("word: Esc closes the list and keeps the composition", !visible() && comp_text(ui).is_some(), || "unexpected".into());
        press(Key::Esc);
        clear(ui);
        } else {
            press(Key::Esc);
            press(Key::Esc);
            clear(ui);
        }
    }
    if want("m4") {
        use twime_model::{Assets, ModelPaths};
        let h0 = twime_winpipe::memory_of_pid(std::process::id()).unwrap_or((0, 0));
        println!("INFO	m4 host increment from the TIP (before the harness loads its own copy of the model): working set {:.1} MB -> {:.1} MB, private {:.1} MB", HOST_WS_BEFORE.load(std::sync::atomic::Ordering::SeqCst) as f64 / 1048576.0, h0.0 as f64 / 1048576.0, h0.1 as f64 / 1048576.0);
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let n_units: usize = std::env::var("TH_M4_UNITS").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
        let assets = Assets::load(&ModelPaths {
            data_txt: root.join("third_party/mcbopomofo/data.txt"),
            aliases: root.join("engine/data/common-readings.txt"),
            model: root.join("data/train/T-model.twg1"),
            prior: Some(root.join("data/vd/prior/freq.pack")),
        })
        .expect("assets");
        let cases = twime_tools::cases::load_cases(root.join("data/units-dev-r2.jsonl").to_str().unwrap(), root.join("data/docs.jsonl").to_str().unwrap()).expect("cases");
        let usable: Vec<&twime_tools::cases::Case> = cases
            .iter()
            .filter(|c| (4..=16).contains(&c.symbols.len()) && c.symbols.iter().all(|s| s.chars().all(|ch| key_of(ch).is_some())) && assets.convert("", &c.symbols, &[], 0.5).text().is_some())
            .collect();
        let step = (usable.len() / n_units).max(1);
        let chosen: Vec<&&twime_tools::cases::Case> = usable.iter().step_by(step).take(n_units).collect();
        let type_unit = |c: &twime_tools::cases::Case| {
            for s in &c.symbols {
                let mut keys: Vec<Key> = s.chars().map(|ch| key_of(ch).unwrap()).collect();
                if !s.ends_with(['ˊ', 'ˇ', 'ˋ', '˙']) {
                    keys.push(Key::Space);
                }
                for k in keys {
                    press(k);
                }
            }
            press(Key::Enter);
            std::thread::sleep(Duration::from_millis(120));
        };
        let do_unit = |c: &twime_tools::cases::Case| -> String {
            loop {
                if aborted() {
                    resume(ui);
                }
                type_unit(c);
                if aborted() {
                    resume(ui);
                    press(Key::Esc);
                    set_text(ui.a, "");
                    std::thread::sleep(Duration::from_millis(150));
                    continue;
                }
                return text_of(ui.a);
            }
        };
        let busy0 = cpu_times();
        let mut outputs: Vec<String> = vec![];
        let (mut ok, mut bad) = (0usize, 0usize);
        for c in &chosen {
            let got = do_unit(c);
            let want_text = assets.convert("", &c.symbols, &[], 0.5).text().unwrap_or_default();
            if got == want_text {
                ok += 1
            } else {
                bad += 1;
                let prev = outputs.last().cloned().unwrap_or_default();
                let with_ctx = assets.convert(&prev, &c.symbols, &[], 0.5).text().unwrap_or_default();
                println!("INFO	m4 mismatch unit {}: got {} chars, direct {} chars, equals conversion with previous unit as context: {}", ok + bad, got.chars().count(), want_text.chars().count(), got == with_ctx);
            }
            outputs.push(got);
            set_text(ui.a, "");
            // App-driven WM_SETTEXT is not reported to the TIP; moving the focus away and back is (it clears the context).
            unsafe {
                SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.b as usize)), None);
                std::thread::sleep(Duration::from_millis(60));
                SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.a as usize)), None);
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        r.check(&format!("m4: {} dev units typed key by key, committed text equals the direct conversion", chosen.len()), bad == 0 && ok > 0, || format!("{bad} differ, {ok} equal"));
        // Replay the first 20 units ten times: outputs must be identical every time.
        let mut differ = 0usize;
        for _ in 0..10 {
            for (i, c) in chosen.iter().take(20).enumerate() {
                if do_unit(c) != outputs[i] {
                    differ += 1;
                }
                set_text(ui.a, "");
                unsafe {
                    SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.b as usize)), None);
                    std::thread::sleep(Duration::from_millis(60));
                    SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.a as usize)), None);
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        r.check("m4: replaying 20 units 10 times gives identical output (no loss, no duplication)", differ == 0, || format!("{differ} differ"));
        let busy1 = cpu_times();
        println!("INFO\tm4: machine CPU busy during the run: {:.0}%", cpu_busy(busy0, busy1));
        report_latency();
        let host = twime_winpipe::memory_of_pid(std::process::id()).unwrap_or((0, 0));
        let eng = engine_pid().and_then(|p| twime_winpipe::memory_of_pid(p).ok()).unwrap_or((0, 0));
        println!("INFO	m4 memory: host working set before TIP {:.1} MB -> after run {:.1} MB (host private {:.1} MB); engine working set {:.1} MB, private {:.1} MB", HOST_WS_BEFORE.load(std::sync::atomic::Ordering::SeqCst) as f64 / 1048576.0, host.0 as f64 / 1048576.0, host.1 as f64 / 1048576.0, eng.0 as f64 / 1048576.0, eng.1 as f64 / 1048576.0);
    }
    if want("marker") {
        let stage = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/stage");
        let marker = stage.join(ids::UNINSTALL_MARKER);
        let setup = std::env::current_exe().unwrap().with_file_name("twime-setup.exe");
        let run = |arg: &str| {
            std::process::Command::new(&setup)
                .arg(arg)
                .env("TWIME_ENGINE_IMAGE", stage.join("twime-engined.exe").canonicalize().unwrap().display().to_string().trim_start_matches("\\\\?\\"))
                .output()
                .map(|o| (o.status.success(), String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr)))
                .unwrap_or((false, "cannot run twime-setup".into()))
        };
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        let (ok, msg) = run("confirm");
        r.check("marker: with an open composition the uninstall cannot be confirmed", !ok && msg.contains("cannot confirm"), || msg.clone());
        std::fs::write(&marker, "1\n").ok();
        std::thread::sleep(Duration::from_millis(250));
        press(Key::Letter(b'a'));
        std::thread::sleep(Duration::from_millis(250));
        let t = text_of(ui.a);
        r.check("marker: the shell ends the composition (committed once) and passes the key", t.chars().count() == 3 && t.ends_with('a') && comp_text(ui).is_none(), || format!("{t:?}"));
        press(Key::Letter(b'b'));
        std::thread::sleep(Duration::from_millis(150));
        r.check("marker: later keys pass through untouched", text_of(ui.a).ends_with("ab"), || format!("{:?}", text_of(ui.a)));
        let (ok, msg) = run("confirm");
        r.check("marker: after the sessions are closed the uninstall is confirmed", ok && msg.contains("confirmed"), || msg.clone());
        let (ok, msg) = run("shutdown");
        r.check("marker: the engine ends on request", ok && msg.contains("engine ended"), || msg.clone());
        let lock_gone = !twime_winpipe::state::data_dir().join("running.lock").exists();
        r.check("marker: running.lock is removed after the clean end", lock_gone, || "lock still there".into());
        std::fs::remove_file(&marker).ok();
        clear(ui);
    }
    if want("aborted") {
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        unsafe {
            SendMessageW(Ui::h(ui.main), WM_TERMINATE, None, None);
        }
        std::thread::sleep(Duration::from_millis(150));
        let t = text_of(ui.a);
        press(Key::Letter(b'r'));
        press(Key::Letter(b'u'));
        press(Key::Letter(b'p'));
        press(Key::Space);
        std::thread::sleep(Duration::from_millis(100));
        r.check("aborted: after the host ended the composition the next keys start from an empty state", matches!(comp_text(ui).as_deref(), Some("今") | Some("金")), || format!("text {t:?} comp {:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
    }
    if want("rapid") {
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        set_text(ui.a, "xyz");
        for _ in 0..3 {
            typ("ㄐㄧㄣ ");
        }
        std::thread::sleep(Duration::from_millis(300));
        let t = text_of(ui.a);
        println!("INFO	rapid: text after external edit and immediate typing: {t:?}");
        r.check("rapid: external edit then immediate typing never duplicates or loses the external text", t.contains("xyz") && t.matches('今').count() <= 3, || format!("{t:?}"));
        press(Key::Esc);
        clear(ui);
    }
    if want("deadro") {
        let t0 = Instant::now();
        let mark = |s: &str| println!("INFO	deadro +{} ms: {s}", t0.elapsed().as_millis());
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        unsafe {
            SendMessageW(Ui::h(ui.a), EM_SETREADONLY, Some(WPARAM(1)), None);
        }
        mark("before kill");
        kill_engine();
        press(Key::Letter(b'a'));
        std::thread::sleep(Duration::from_millis(200));
        unsafe {
            SendMessageW(Ui::h(ui.a), EM_SETREADONLY, Some(WPARAM(0)), None);
        mark("after readonly off");
        }
        let t = text_of(ui.a);
        r.check("deadro: engine gone and host read-only: nothing duplicated", t.matches('今').count() <= 1, || format!("{t:?}"));
        clear(ui);
        std::thread::sleep(Duration::from_millis(800));
        mark("before wake");
        wake_engine();
        mark("after wake");
        typ(SENTENCE);
        mark("typed");
        r.check("deadro: normal again afterwards", comp_text(ui).as_deref() == Some("今天天氣很好"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
    }
    if names.iter().any(|n| n == "focus-cycle") { focus_cycle(ui, r); }
    if names.iter().any(|n| n == "late-push") { late_push(ui, r); }
    ux_scenarios(ui, &want, r);
    if names.iter().any(|n| n == "setui-shift") {
        setui_shift(ui, r);
    }
    if names.iter().any(|n| n == "setui-add") {
        setui_add(ui, r);
    }
    if names.iter().any(|n| n == "setui-english-refresh") {
        setui_english_refresh(ui, r);
    }
    if names.iter().any(|x| x == "ux-lat") {
        ux_lat(ui);
    }
    let only = |n: &str| names.iter().any(|x| x == n);
    if want("ux-restart") {
        ux_restart(ui, r, false);
    }
    if only("ux-restart-slow") {
        ux_restart(ui, r, true);
    }
    if only("ux-default-english") {
        ux_default_english(ui, r);
    }
    if only("ux-fault-commit") {
        ux_fault_commit(ui, r);
    }
    if only("ux-fault-commit-run") {
        ux_fault_commit_run(ui, r);
    }
    if only("emoji") {
        emoji_commit(ui, r);
    }
    if only("emoji-fault") {
        emoji_fault(ui, r);
    }
    if only("ux-fault-fail") {
        ux_fault(ui, r, "ux-fault-fail (commit stands, the new composition's write fails)");
    }
    if want("peek") {
        typ(SENTENCE);
        press(Key::Letter(b'z'));
        press(Key::Esc);
        press(Key::Enter);
        press(Key::Esc);
        clear(ui);
        check_peek_log(r);
    }
}

// ---- window -----------------------------------------------------------------------------------------------------------

static UI: Mutex<Option<Ui>> = Mutex::new(None);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            lexux::WM_CATEGORY => LRESULT(isize::from(lexux::category_matches(wp.0 != 0))),
            lexux::WM_SNAPSHOT => { lexux::snapshot_on_ui(); LRESULT(0) },
            WM_KEY_FOCUS => {
                let sink = HOST_SINK.with(|h| h.borrow().clone());
                LRESULT(isize::from(sink.is_some_and(|sink| sink.OnSetFocus((wp.0 != 0).into()).is_ok())))
            }
            WM_LATE_PUSH => LRESULT(isize::from(late_push_on_ui(wp.0).is_ok())),
            WM_RECOVER_KEY_FOCUS => LRESULT(isize::from(recover_key_focus_on_ui(wp.0 != 0).is_ok())),
            WM_QUERY_FOCUS => LRESULT(windows::Win32::UI::Input::KeyboardAndMouse::GetFocus().0 as isize),
            WM_FOCUS_EDIT => {
                grab_foreground(hwnd);
                let _ = SetFocus(Some(HWND(wp.0 as _)));
                LRESULT(0)
            }
            WM_TERMINATE => {
                HOST_TM.with(|h| {
                    if let Some((tm, _)) = h.borrow().as_ref()
                        && let Ok(ctx) = tm.GetFocus().and_then(|d| d.GetTop())
                        && let Ok(svc) = windows::core::Interface::cast::<windows::Win32::UI::TextServices::ITfContextOwnerCompositionServices>(&ctx)
                    {
                        let r = svc.TerminateComposition(None);
                        println!("INFO	TerminateComposition: {r:?}");
                    }
                });
                LRESULT(0)
            }
            WM_SHOWME => {
                let _ = ShowWindow(hwnd, SW_SHOW);
                grab_foreground(hwnd);
                // Activation applies the user's global input method again (a real TwIme install would then also handle
                // the keys): select the plain layout once more after taking the foreground.
                if std::env::var_os("TWIME_TIP_DLL").is_some() && switch_to_us_keyboard().is_err() {
                    println!("FAIL\tplain keyboard layout after activation");
                }
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATE if std::env::var_os("TWIME_TIP_DLL").is_some() && (wp.0 & 0xffff) != 0 => {
                let r = DefWindowProcW(hwnd, msg, wp, lp);
                let _ = switch_to_us_keyboard();
                r
            }
            WM_READ_COMP => {
                *COMP.lock().unwrap() = read_comp_on_ui_thread();
                LRESULT(0)
            }
            WM_ACTIVATE_TIP => {
                activate_tip();
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn activate_tip() {
    if std::env::var_os("TWIME_TIP_DLL").is_some() {
        // Disable the installed TIP in this process before direct-driving the private DLL.
        if switch_to_us_keyboard().is_err() {
            std::process::exit(1);
        }
        println!("INFO\tTWIME_TIP_DLL: US keyboard active for this process; no registration");
        return;
    }
    unsafe {
        let mgr: ITfInputProcessorProfileMgr = match CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
            Ok(m) => m,
            Err(e) => {
                println!("FAIL\tactivate: profile manager: {e}");
                return;
            }
        };
        let desc: Vec<u16> = ids::DESCRIPTION.encode_utf16().collect();
        let reg = mgr.RegisterProfile(
            &windows::core::GUID::from_u128(ids::CLSID_TIP),
            ids::LANGID_ZH_TW,
            &windows::core::GUID::from_u128(ids::PROFILE),
            &desc,
            &[],
            0,
            HKL::default(),
            0,
            true,
            4, // TF_RP_LOCALPROCESS: this process only, nothing persisted
        );
        println!("INFO\tRegisterProfile(TF_RP_LOCALPROCESS): {reg:?}");
        let r = mgr.ActivateProfile(
            TF_PROFILETYPE_INPUTPROCESSOR,
            ids::LANGID_ZH_TW,
            &windows::core::GUID::from_u128(ids::CLSID_TIP),
            &windows::core::GUID::from_u128(ids::PROFILE),
            HKL::default(),
            TF_IPPMF_FORPROCESS,
        );
        println!("INFO\tActivateProfile(for this process only): {r:?}");
    }
}

/// Select a plain keyboard layout (no text-service TIP) for this process only.
fn switch_to_us_keyboard() -> windows::core::Result<()> {
    use windows::{core::GUID, Win32::{Foundation::E_FAIL, UI::{
        Input::KeyboardAndMouse::{ActivateKeyboardLayout, GetKeyboardLayout, GetKeyboardLayoutList, KLF_SETFORPROCESS},
        TextServices::{TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE, GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE},
    }}};
    unsafe {
        let mut attempts = Vec::new();
        // SendInput carries virtual-key codes only (wScan 0), so a plain layout with other key positions (en-GB) is fine too.
        for wanted in [0x04090409usize, 0x04040404, 0x08090809] {
            let result = (|| -> windows::core::Result<()> {
                let count = GetKeyboardLayoutList(None);
                if count <= 0 { return Err(E_FAIL.into()); }
                let mut layouts = vec![HKL::default(); count as usize];
                let count = GetKeyboardLayoutList(Some(&mut layouts));
                if count <= 0 { return Err(E_FAIL.into()); }
                let selected = layouts[..count as usize].iter().copied()
                    .find(|hkl| hkl.0 as usize & 0xffffffff == wanted)
                    .ok_or(windows::core::Error::from(E_FAIL))?;
                let mgr: ITfInputProcessorProfileMgr = CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
                let zero = GUID::zeroed();
                let _ = mgr.ActivateProfile(TF_PROFILETYPE_KEYBOARDLAYOUT, (selected.0 as usize & 0xffff) as u16,
                    &zero, &zero, selected, TF_IPPMF_FORPROCESS | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE);
                ActivateKeyboardLayout(selected, KLF_SETFORPROCESS)?;
                let mut active = TF_INPUTPROCESSORPROFILE::default();
                mgr.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut active)?;
                if GetKeyboardLayout(0) != selected || active.dwProfileType != TF_PROFILETYPE_KEYBOARDLAYOUT
                    || active.hkl != selected || active.clsid != zero || active.guidProfile != zero
                {
                    return Err(E_FAIL.into());
                }
                println!("INFO\tUI thread plain keyboard layout: {wanted:08X}");
                Ok(())
            })();
            match result {
                Ok(()) => return Ok(()),
                Err(e) => attempts.push((wanted, e.code())),
            }
        }
        for (hkl, hr) in attempts {
            println!("FAIL\tHKL={hkl:08X}\tHRESULT={:08X}", hr.0 as u32);
        }
        Err(E_FAIL.into())
    }
}

/// Loads the private TIP without COM registration. Intentionally never FreeLibrary:
/// the TIP and its COM callbacks must remain backed by this DLL until process exit.
fn create_tip() -> windows::Win32::UI::TextServices::ITfTextInputProcessorEx {
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::{GUID, HRESULT, IUnknown, Interface, s}, Win32::System::Com::IClassFactory};
    unsafe {
        let clsid = GUID::from_u128(ids::CLSID_TIP);
        let Some(path) = std::env::var_os("TWIME_TIP_DLL") else {
            return CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER).expect("create TIP (is the COM class registered?)");
        };
        assert!(std::path::Path::new(&path).is_absolute(), "TWIME_TIP_DLL must be an absolute DLL path");
        let mut path_w: Vec<u16> = path.encode_wide().collect();
        assert!(!path_w.contains(&0), "TWIME_TIP_DLL contains a NUL");
        path_w.push(0);
        let module = LoadLibraryW(PCWSTR(path_w.as_ptr())).expect("LoadLibraryW(TWIME_TIP_DLL)");
        let entry = GetProcAddress(module, s!("DllGetClassObject")).expect("DllGetClassObject export");
        let get_class_object: unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut std::ffi::c_void) -> HRESULT = std::mem::transmute(entry);
        let mut raw = std::ptr::null_mut();
        get_class_object(&clsid, &IClassFactory::IID, &mut raw).ok().expect("DllGetClassObject(IID_IClassFactory)");
        let factory = IClassFactory::from_raw(raw);
        let tip = factory.CreateInstance(None::<&IUnknown>).expect("IClassFactory::CreateInstance(TIP)");
        println!("INFO\tTWIME_TIP_DLL: created TIP directly from {path:?}");
        tip
    }
}

fn main() {
    let names: Vec<String> = std::env::args().skip(1).collect();
    // POP tests are explicit-only and require the Sandbox runner's output directory.
    if names.iter().any(|n| n.starts_with("pop-")) {
        if let Err(e) = pop::validate(&names) {
            eprintln!("FAIL\tpop-preflight\t{e}");
            std::process::exit(2);
        }
    }
    // ZHUYIN-OUT scenarios are explicit-only and Sandbox-only.
    if names.iter().any(|n| n.starts_with("zy-")) {
        if let Err(e) = zyout::validate(&names) {
            eprintln!("FAIL	zy-preflight	{e}");
            std::process::exit(2);
        }
    }
    if names.first().is_some_and(|n| n=="lexux-lat-compare") {
        let ok=names.get(1).is_some_and(|p|lexux::compare_file(p));
        std::process::exit(i32::from(!ok));
    }
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if std::env::var_os("TWIME_TIP_DLL").is_some() { activate_tip(); }
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let _ = LoadLibraryW(w!("Msftedit.dll"));
        let cls = w!("TwImeTestHost");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: cls, ..Default::default() };
        RegisterClassW(&wc);
        let main = CreateWindowExW(Default::default(), cls, w!("TwIme test host"), WS_OVERLAPPEDWINDOW, 80, 80, 640, 260, None, None, Some(hinst), None).unwrap();
        let edit = |y: i32, extra: u32| {
            CreateWindowExW(Default::default(), w!("RICHEDIT50W"), PCWSTR::null(), WS_CHILD | WS_VISIBLE | WS_BORDER | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(ES_AUTOHSCROLL as u32 | extra), 10, y, 600, 28, Some(main), None, Some(hinst), None).unwrap()
        };
        let a = edit(10, 0);
        let b = edit(50, 0);
        let pw = edit(90, ES_PASSWORD as u32);
        let other = CreateWindowExW(Default::default(), cls, w!("TwIme focus target"), WS_OVERLAPPEDWINDOW, 760, 80, 400, 180, None, None, Some(hinst), None).unwrap();
        let other_edit = CreateWindowExW(Default::default(), w!("RICHEDIT50W"), PCWSTR::null(), WS_CHILD | WS_VISIBLE | WS_BORDER, 10, 10, 350, 28, Some(other), None, Some(hinst), None).unwrap();
        MAIN_HWND.store(main.0 as isize, std::sync::atomic::Ordering::SeqCst);
        let ui = Ui { main: main.0 as isize, a: a.0 as isize, b: b.0 as isize, pw: pw.0 as isize, other: other.0 as isize, other_edit: other_edit.0 as isize };
        *UI.lock().unwrap() = Some(ui);

        let (tx, rx) = mpsc::channel::<(usize, usize)>();
        let lexux_selected = names.iter().any(|n| n.starts_with("lexux-"));
        let pop_selected = names.iter().any(|n| n.starts_with("pop-"));
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            let mut r = Report { pass: 0, fail: 0 };
            scenarios(ui, &names, &mut r);
            if aborted() {
                println!("ABORTED	the test window lost the foreground; no further keys were sent");
                r.fail += 1;
            }
            println!("SUMMARY\tpass={}\tfail={}", r.pass, r.fail);
            let _ = tx.send((r.pass, r.fail));
            let _ = PostMessageW(Some(Ui::h(ui.main)), WM_CLOSE, WPARAM(0), LPARAM(0));
        });
        // Non-POP direct drive: the process-local profile registration does not make the system route keys to the TIP
        // (the keyboard category cannot be registered without HKLM write access), so the host plays the part of the
        // keystroke manager: it activates the TIP object itself and offers it every key before dispatching it.
        use windows::Win32::UI::TextServices::{CLSID_TF_ThreadMgr, ITfKeyEventSink, ITfThreadMgr};
        if !pop_selected { std::env::set_var("TWIME_ALLOW_NO_KEYSINK", "1"); }
        HOST_WS_BEFORE.store(twime_winpipe::memory_of_pid(std::process::id()).map(|m| m.0).unwrap_or(0), std::sync::atomic::Ordering::SeqCst);
        let tm: ITfThreadMgr = CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).expect("thread manager");
        let tid = tm.Activate().expect("Activate");
        HOST_TM.with(|h| *h.borrow_mut() = Some((tm.clone(), tid)));
        if lexux_selected {
            assert!(lexux::attach(&tm).is_ok(), "count=1");
        }
        // Installed POP runs already have a system-activated TIP. A second manual instance
        // fails AdviseKeyEventSink and replaces the DLL's thread-local timer target.
        let tip = (!pop_selected).then(|| {
            let tip = create_tip();
            let ar = tip.ActivateEx(&tm, tid, 0);
            println!("INFO\tActivateEx: {ar:?}");
            tip
        });
        let sink: Option<ITfKeyEventSink> = tip.as_ref().map(|t| windows::core::Interface::cast(t).expect("key sink"));
        HOST_SINK.with(|h| *h.borrow_mut() = sink.clone());
        let system_keys = pop_selected.then(|| {
            let profiles: ITfInputProcessorProfileMgr = CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER).expect("profile manager");
            profiles.ActivateProfile(TF_PROFILETYPE_INPUTPROCESSOR, ids::LANGID_ZH_TW,
                &windows::core::GUID::from_u128(ids::CLSID_TIP), &windows::core::GUID::from_u128(ids::PROFILE),
                HKL::default(), TF_IPPMF_FORPROCESS).expect("activate installed POP profile");
            println!("INFO\tPOP uses the registered TIP through ITfKeystrokeMgr (no manual ActivateEx)");
            windows::core::Interface::cast::<windows::Win32::UI::TextServices::ITfKeystrokeMgr>(&tm).expect("keystroke manager")
        });
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            if let Some(keys) = &system_keys {
                let eaten = match m.message {
                    0x0100 | 0x0104 => keys.TestKeyDown(m.wParam, m.lParam)
                        .and_then(|test| if test.as_bool() { keys.KeyDown(m.wParam, m.lParam) } else { Ok(false.into()) }),
                    0x0101 | 0x0105 => keys.TestKeyUp(m.wParam, m.lParam)
                        .and_then(|test| if test.as_bool() { keys.KeyUp(m.wParam, m.lParam) } else { Ok(false.into()) }),
                    _ => Ok(false.into()),
                }.expect("TSF key routing");
                if eaten.as_bool() { continue; }
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
                continue;
            }
            let sink = sink.as_ref().expect("manual key sink");
            if m.message == 0x0101 || m.message == 0x0105 {
                // Key up: the keystroke manager asks the TIP first and delivers only when it wants the key-up.
                if let Ok(ctx) = tm.GetFocus().and_then(|d| d.GetTop())
                    && sink.OnTestKeyUp(&ctx, m.wParam, m.lParam).map(|b| b.as_bool()).unwrap_or(false)
                    && sink.OnKeyUp(&ctx, m.wParam, m.lParam).map(|b| b.as_bool()).unwrap_or(false)
                {
                    continue;
                }
            }
            if m.message == 0x0100 || m.message == 0x0104 {
                let ctx = tm.GetFocus().and_then(|d| d.GetTop());
                if let Ok(ctx) = ctx
                    && sink.OnTestKeyDown(&ctx, m.wParam, m.lParam).map(|b| b.as_bool()).unwrap_or(false)
                    && sink.OnKeyDown(&ctx, m.wParam, m.lParam).map(|b| b.as_bool()).unwrap_or(false)
                {
                    if lexux_selected { lexux::key_done(); }
                    continue;
                }
            }
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
            if lexux_selected && (m.message == 0x0100 || m.message == 0x0104) { lexux::key_done(); }
        }
        if let Some(tip) = tip { let _ = tip.Deactivate(); }
        lexux::detach();
        let _ = tm.Deactivate();
        let _ = h.join();
        let (_, fail) = rx.try_recv().unwrap_or((0, 1));
        let _ = Arc::new(0);
        let _ = VK_CONTROL;
        std::process::exit(if aborted() { 3 } else { i32::from(fail != 0) });
    }
}

/// Brings the test window to the foreground without sending any key: attaches to the foreground thread's input queue.
fn grab_foreground(hwnd: HWND) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    unsafe {
        let fg = GetForegroundWindow();
        let fg_tid = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        if fg_tid != 0 && fg_tid != me {
            let _ = AttachThreadInput(me, fg_tid, true);
            let _ = SetForegroundWindow(hwnd);
            let _ = AttachThreadInput(me, fg_tid, false);
        } else {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

// ---- reading the composition string through TSF (a CUAS edit control keeps it out of its own buffer) ----------------------

const WM_READ_COMP: u32 = WM_APP + 3;
const WM_SHOWME: u32 = WM_APP + 4;
const WM_TERMINATE: u32 = WM_APP + 5;
static COMP: Mutex<Option<String>> = Mutex::new(None);

thread_local! {
    static HOST_TM: std::cell::RefCell<Option<(windows::Win32::UI::TextServices::ITfThreadMgr, u32)>> = const { std::cell::RefCell::new(None) };
}

#[windows::core::implement(windows::Win32::UI::TextServices::ITfEditSession)]
struct HostEs {
    f: std::cell::RefCell<Option<Box<dyn FnOnce(u32)>>>,
}

impl windows::Win32::UI::TextServices::ITfEditSession_Impl for HostEs_Impl {
    fn DoEditSession(&self, ec: u32) -> windows::core::Result<()> {
        if let Some(f) = self.f.borrow_mut().take() {
            f(ec);
        }
        Ok(())
    }
}

/// Runs on the UI thread: the text of the composition in the focused context, if any.
fn read_comp_on_ui_thread() -> Option<String> {
    use windows::Win32::UI::TextServices::{ITfContextComposition, ITfEditSession, TF_ES_READ, TF_ES_SYNC};
    HOST_TM.with(|h| {
        let b = h.borrow();
        let (tm, tid) = b.as_ref()?;
        unsafe {
            let ctx = tm.GetFocus().ok()?.GetTop().ok()?;
            let cc: ITfContextComposition = windows::core::Interface::cast(&ctx).ok()?;
            let out = std::rc::Rc::new(std::cell::RefCell::new(None));
            let o2 = out.clone();
            let es: ITfEditSession = HostEs {
                f: std::cell::RefCell::new(Some(Box::new(move |ec| {
                    let Ok(e) = cc.EnumCompositions() else { return };
                    let mut c = [None];
                    let mut n = 0u32;
                    if e.Next(&mut c, &mut n).is_ok()
                        && n == 1
                        && let Some(comp) = c[0].take()
                        && let Ok(range) = comp.GetRange()
                    {
                        let mut buf = [0u16; 512];
                        let mut got = 0u32;
                        if range.GetText(ec, 0, &mut buf, &mut got).is_ok() {
                            *o2.borrow_mut() = Some(String::from_utf16_lossy(&buf[..got as usize]));
                        }
                    }
                }))),
            }
            .into();
            ctx.RequestEditSession(*tid, &es, TF_ES_SYNC | TF_ES_READ).ok()?.ok().ok()?;
            let v = out.borrow().clone();
            v
        }
    })
}

/// From the driver thread: the composition string, or None if there is no composition.
fn comp_text(ui: Ui) -> Option<String> {
    unsafe {
        SendMessageW(Ui::h(ui.main), WM_READ_COMP, None, None);
    }
    COMP.lock().unwrap().take()
}

/// Reads the shell's trace file (`TWIME_TRACE`) and checks the call sequence: every engine exchange comes from an
/// `OnKeyDown` that follows an `OnTestKeyDown` which answered "eaten"; a test call alone never touches the engine.
fn check_peek_log(r: &mut Report) {
    let Ok(path) = std::env::var("TWIME_TRACE") else {
        println!("INFO\tpeek: TWIME_TRACE not set; log check skipped");
        return;
    };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let (tests, downs, bad) = peek_log_violations(&text);
    r.check(&format!("peek: call sequence clean ({tests} test calls, {downs} key downs)"), bad == 0 && tests > 0, || format!("{bad} violations"));
}

/// (test calls, key downs, violations) of a shell trace. `OnTestKeyDown` writes `TestKeyDown-begin` on entry and
/// `TestKeyDown vk=.. eaten=..` on exit; no `exchange` or `handshake` (engine traffic) may fall in between, and every
/// key down must follow a test call that answered "eaten".
fn peek_log_violations(text: &str) -> (usize, usize, usize) {
    let (mut state, mut tests, mut downs, mut bad) = (0u8, 0usize, 0usize, 0usize);
    let mut in_test = false;
    // state 0: idle; 1: test said eaten; 2: inside OnKeyDown
    for l in text.lines() {
        if l.ends_with("TestKeyDown-begin") {
            in_test = true;
            continue;
        }
        if (l.ends_with("\texchange") || l.ends_with("\thandshake")) && in_test {
            bad += 1; // engine traffic inside the key-test callback
        }
        if l.contains("TestKeyDown vk") {
            in_test = false;
        }
        if l.contains("TestKeyDown") {
            tests += 1;
            if state == 1 {
                bad += 1; // a second test without OnKeyDown after "eaten" is legal for hosts, but not in this host
            }
            state = if l.contains("eaten=true") { 1 } else { 0 };
        } else if l.contains("KeyDown vk") && l.contains("begin") {
            downs += 1;
            if state != 1 {
                bad += 1;
            }
            state = 2;
        } else if l.ends_with("\texchange") {
            // An exchange between OnTestKeyDown and OnKeyDown would be a side effect of the test call; queued events
            // outside a key come from focus changes.
            if state == 1 {
                bad += 1;
            }
        } else if l.contains("KEYDOWN vk") {
            state = 0;
        }
    }
    (tests, downs, bad)
}

#[cfg(test)]
mod peek_log_tests {
    use super::peek_log_violations;

    const CLEAN: &str = "1\tpid=1\ttid=1\tTestKeyDown-begin\n2\tpid=1\ttid=1\tTestKeyDown vk=0x51 eaten=true\n3\tpid=1\ttid=1\tKeyDown vk=0x51 begin\n4\tpid=1\ttid=1\thandshake\n5\tpid=1\ttid=1\texchange\n6\tpid=1\ttid=1\tKEYDOWN vk=0x51 done\n";

    #[test]
    fn handshake_and_exchange_after_the_test_call_are_clean() {
        assert_eq!(peek_log_violations(CLEAN), (1, 1, 0));
    }

    #[test]
    fn traffic_inside_the_test_callback_is_a_violation() {
        for kind in ["handshake", "exchange"] {
            let bad = CLEAN.replacen("2\tpid=1\ttid=1\tTestKeyDown vk", &format!("1\tpid=1\ttid=1\t{kind}\n2\tpid=1\ttid=1\tTestKeyDown vk"), 1);
            assert_eq!(peek_log_violations(&bad).2, 1, "{kind} inside the callback must be reported");
        }
    }
}

fn cpu_times() -> (u64, u64) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u));
    }
    let q = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    (q(i), q(k) + q(u)) // idle, kernel (includes idle) + user
}

fn cpu_busy(a: (u64, u64), b: (u64, u64)) -> f64 {
    let (idle, total) = (b.0 - a.0, b.1 - a.1);
    if total == 0 { 0.0 } else { 100.0 * (1.0 - idle as f64 / total as f64) }
}

/// Path latency from the shell's trace file: first callback of the key to the end of the edit session.
fn report_latency() {
    let Ok(path) = std::env::var("TWIME_TRACE") else { return };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let (mut trig, mut other): (Vec<f64>, Vec<f64>) = (vec![], vec![]);
    let mut eng: Vec<f64> = vec![];
    for l in text.lines().filter(|l| l.contains("KEYDOWN vk")) {
        let get = |k: &str| l.split(k).nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).and_then(|s| s.parse::<f64>().ok());
        let (Some(t), Some(tr)) = (get("total_us="), get("trig=")) else { continue };
        if tr > 0.5 {
            trig.push(t / 1000.0);
            if let Some(e) = get("engine_us=") {
                eng.push(e / 1000.0);
            }
        } else {
            other.push(t / 1000.0);
        }
    }
    let line = |name: &str, mut v: Vec<f64>| {
        if v.is_empty() {
            println!("INFO\t{name}: n=0");
            return;
        }
        v.sort_by(f64::total_cmp);
        let q = |p: f64| v[((v.len() as f64 * p) as usize).min(v.len() - 1)];
        println!("INFO\t{name}: n={} p50={:.3} p95={:.3} p99={:.3} max={:.3} (ms)", v.len(), q(0.5), q(0.95), q(0.99), v[v.len() - 1]);
    };
    line("m4 path latency, conversion keys (first callback -> edit session done)", trig);
    line("m4 engine self-reported, same keys", eng);
    line("m4 path latency, other eaten keys", other);
}

/// After the user took the foreground: wait until they are idle again, then bring the test window back and go on.
fn resume(ui: Ui) {
    loop {
        if wait_user_idle(20, Duration::from_secs(3600)) {
            unsafe {
                SendMessageW(Ui::h(ui.main), WM_SHOWME, None, None);
            }
            if focus_a(ui) {
                ABORTED.store(false, std::sync::atomic::Ordering::SeqCst);
                println!("INFO\tresumed after the user released the foreground");
                return;
            }
        }
    }
}
// ---- UX1 scenarios ----------------------------------------------------

const VK_SHIFT_KEY: u16 = 0x10;
const VK_CAPITAL_KEY: u16 = 0x14;

fn nap(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Shift + key (Shift down, key down, key up, Shift up) as one batch.
fn press_shift(k: Key) {
    send_raw(&[(VK_SHIFT_KEY, false), (vk_of(k), false), (vk_of(k), true), (VK_SHIFT_KEY, true)]);
    nap(30);
}

/// The same without the pause (for the race between two quick keys).
fn press_shift_fast(k: Key) {
    send_raw(&[(VK_SHIFT_KEY, false), (vk_of(k), false), (vk_of(k), true), (VK_SHIFT_KEY, true)]);
}

fn press_fast(k: Key) {
    send_vk(vk_of(k));
}

/// A clean Shift tap: down, a short pause, up.
fn tap_shift() {
    send_raw(&[(VK_SHIFT_KEY, false)]);
    nap(40);
    send_raw(&[(VK_SHIFT_KEY, true)]);
    nap(120);
}

/// Shift held for `ms` milliseconds (a long press is no tap).
fn hold_shift(ms: u64) {
    send_raw(&[(VK_SHIFT_KEY, false)]);
    nap(ms);
    send_raw(&[(VK_SHIFT_KEY, true)]);
    nap(120);
}

/// English text typed key by key: lower case letters plain, capitals with Shift.
fn english(s: &str) {
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            press_shift(Key::Letter(c.to_ascii_lowercase() as u8));
        } else if c.is_ascii_lowercase() {
            press(Key::Letter(c as u8));
        } else if c == '.' {
            press(Key::Period);
        } else {
            panic!("english(): unsupported {c}");
        }
    }
    nap(60);
}

fn caps_is_on() -> bool {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(i32::from(VK_CAPITAL_KEY)) & 1 != 0 }
}

/// Toggles Caps Lock without the foreground check: only used to put the user's Caps Lock state back.
fn toggle_caps_unconditional() {
    let mk = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(VK_CAPITAL_KEY), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, time: 0, dwExtraInfo: 0 } },
    };
    unsafe {
        SendInput(&[mk(false), mk(true)], size_of::<INPUT>() as i32);
    }
}

fn trace_text() -> String {
    std::env::var("TWIME_TRACE").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()
}

fn trace_count(needle: &str) -> usize {
    trace_text().lines().filter(|l| l.contains(needle)).count()
}

fn label_window_visible() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, IsWindowVisible};
    unsafe {
        [w!("TwImeCandidate"), w!("TwImeLabel")].iter().any(|class| FindWindowW(*class, PCWSTR::null()).ok().is_some_and(|h| IsWindowVisible(h).as_bool()))
    }
}

// SETUI scenarios require the private stage and trace provided by setui-th-run.sh.
fn setui_user_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("TWIME_STAGE").map(|p| std::path::PathBuf::from(p).join("user"))
}

fn setui_settings(dir: &std::path::Path, toggle: &str, mode: &str) -> bool {
    let text = format!("version = 1\ntoggle_key = \"{toggle}\"\ndefault_mode = \"{mode}\"\npunctuation = \"full\"\nsymbol_table = true\ncache = false\n");
    std::fs::write(dir.join("settings.toml"), text).is_ok()
}

fn setui_shift(ui: Ui, r: &mut Report) {
    let Some(dir) = setui_user_dir() else {
        r.check("setui-shift: private stage required", false, || "TWIME_STAGE missing".into());
        return;
    };
    let caps = caps_is_on();
    if caps { send_vk(VK_CAPITAL_KEY); nap(100); }
    let initial = std::env::var("TWIME_SETUI_TOGGLE").unwrap_or_else(|_| "shift".into());
    let variants = [
        ("shift", "Shift", true, true),
        ("left_shift", "LeftShift", true, false),
        ("right_shift", "RightShift", false, true),
        ("none", "None", false, false),
    ];
    let Some(first) = variants.iter().copied().find(|v| v.0 == initial) else {
        r.check("setui-shift: valid initial setting required", false, || "invalid TWIME_SETUI_TOGGLE".into());
        if caps { toggle_caps_unconditional(); }
        return;
    };
    let mut cases = vec![first];
    if initial == "none" { cases.push(variants[0]); }
    let mut chinese = true;
    for (index, (setting, wire, left, right)) in cases.into_iter().enumerate() {
        let marker = format!("settings refresh toggle={wire}");
        let before = trace_count(&marker);
        // The first setting was written by the runner before engine startup. The none -> shift change is live.
        let wrote = index == 0 || setui_settings(&dir, setting, "chinese");
        r.check(&format!("setui-shift: settings {setting}"), wrote, || "settings write failed".into());
        let refreshed = wrote && wait_for(Duration::from_secs(10), || trace_count(&marker) > if index == 0 { 0 } else { before });
        r.check(&format!("setui-shift: refresh {setting} without focus change"), refreshed, || "no refresh within 10 seconds".into());
        for (vk, side, enabled) in [(0xA0, "left", left), (0xA1, "right", right)] {
            let before = trace_count("ToggleMode sent");
            send_raw(&[(vk, false)]);
            nap(40);
            send_raw(&[(vk, true)]);
            nap(200);
            let toggled = trace_count("ToggleMode sent") - before;
            r.check(&format!("setui-shift: {setting} {side}"), toggled == usize::from(enabled), || format!("toggles={toggled}"));
            if enabled { chinese = !chinese; }
            // Check the mode through host-visible input as well as the request trace.
            clear(ui);
            press(Key::Letter(b'a'));
            let mode_ok = if chinese { comp_text(ui).is_some() } else { comp_text(ui).is_none() && text_of(ui.a) == "a" };
            r.check(&format!("setui-shift: {setting} {side} mode"), mode_ok, || "host mode differs".into());
            if chinese { press(Key::Esc); }
            clear(ui);
        }
    }
    clear(ui);
    if caps { toggle_caps_unconditional(); }
}

/// EMOJI (EMOJI-plan 3.4, V4), named only: choose emoji from the candidate list in the host's edit control and check what
/// reaches the document (UTF-16 text, one composition end, the caret behind the text). The engine must run with `emoji=on` and
/// the table `TH_EMOJI_TSV` names (the engine's installed `emoji.tsv`). Each case types a keyword, opens the list, goes to the
/// end (the emoji are the last items) and up to the emoji, and commits it with Enter.
fn emoji_commit(ui: Ui, r: &mut Report) {
    const EM_GETSEL: u32 = 0x00B0; // Win32_UI_Controls is not among the enabled features
    let Ok(path) = std::env::var("TH_EMOJI_TSV") else {
        r.check("emoji: TH_EMOJI_TSV (the engine's emoji.tsv) is required", false, || "not set".into());
        return;
    };
    let table = std::fs::read_to_string(&path).unwrap_or_default();
    let row = |key: &str| -> Vec<String> {
        table.lines().find_map(|l| l.strip_prefix(key).and_then(|rest| rest.strip_prefix('\t'))).map(|rest| rest.split('\t').next().unwrap_or("").split(' ').map(String::from).collect()).unwrap_or_default()
    };
    // keyword readings of the representative keys; the checks also cover the three string classes of the plan when the table
    // row has them (a surrogate pair, a symbol with U+FE0F)
    let cases = [
        ("ㄞˋ-ㄒㄧㄣ", "ㄞˋㄒㄧㄣ "),          // ❤️ U+2764 U+FE0F: a BMP symbol with the selector (2 units)
        ("ㄒㄧㄠˋ-ㄌㄧㄢˇ", "ㄒㄧㄠˋㄌㄧㄢˇ"),  // 😊 😀 ...: surrogate pairs (2 units)
        ("ㄧㄢˇ-ㄐㄧㄥ", "ㄧㄢˇㄐㄧㄥ "),       // 👁️ U+1F441 U+FE0F: a surrogate pair with the selector (3 units)
        ("ㄇㄠ", "ㄇㄠ "),
    ];
    // the three string classes of the plan (3.4) must all have been committed, else the scenario fails
    let mut classes = std::collections::BTreeSet::new();
    for (key, keys) in cases {
        let emoji = row(key);
        r.check(&format!("emoji {key}: the table has a row"), !emoji.is_empty(), || path.clone());
        for (i, e) in emoji.iter().enumerate() {
            clear(ui);
            typ(keys);
            press(Key::Down);
            press(Key::End);
            press(Key::Up); // the typed bopomofo is the very last item (ZHUYIN-OUT); the emoji end just before it
            for _ in 0..(emoji.len() - 1 - i) {
                press(Key::Up);
            }
            press(Key::Enter);
            std::thread::sleep(Duration::from_millis(150));
            let t = text_of(ui.a);
            let units = e.encode_utf16().count();
            r.check(&format!("emoji {key} #{i}: committed text is the emoji"), t == *e, || format!("{t:?} want {e:?}"));
            r.check(&format!("emoji {key} #{i}: {units} UTF-16 units, no composition left"), t.encode_utf16().count() == units && comp_text(ui).is_none(), || format!("{:?}", comp_text(ui)));
            let (mut s, mut en) = (0u32, 0u32);
            unsafe {
                SendMessageW(Ui::h(ui.a), EM_GETSEL, Some(WPARAM(&mut s as *mut u32 as usize)), Some(LPARAM(&mut en as *mut u32 as isize)));
            }
            r.check(&format!("emoji {key} #{i}: the caret is behind the text"), en as usize == units && s as usize == units, || format!("sel {s}..{en}"));
            let (wide, fe0f) = (e.chars().any(|c| c as u32 > 0xFFFF), e.chars().any(|c| c as u32 == 0xFE0F));
            classes.insert((wide, fe0f, units));
        }
    }
    println!("INFO	emoji: classes (outside BMP, with U+FE0F, UTF-16 units) {classes:?}");
    for (name, want) in [("a surrogate pair (2 units)", (true, false, 2)), ("a BMP symbol with U+FE0F (2 units)", (false, true, 2)), ("a surrogate pair with U+FE0F (3 units)", (true, true, 3))] {
        r.check(&format!("emoji: {name} was committed"), classes.contains(&want), || format!("{classes:?}"));
    }
    // a second commit appends (the context after an emoji is usable) and Esc leaves nothing behind
    clear(ui);
    typ("ㄇㄠ ");
    press(Key::Down);
    press(Key::End);
    press(Key::Up);
    press(Key::Enter);
    nap(150);
    let first = text_of(ui.a);
    typ(SENTENCE);
    press(Key::Enter);
    nap(150);
    let t = text_of(ui.a);
    r.check("emoji: a sentence typed after an emoji is appended", t.starts_with(&first) && t.chars().count() > first.chars().count(), || format!("{t:?}"));
    clear(ui);
}

/// EMOJI, named only, with `TWIME_FAULT=commit-fail` (dev-mode stage) as for `ux-fault-commit`: the first commit is refused after the
/// composition ended. For an emoji the document must stay empty with no composition (the composition comes back in the engine,
/// not in the document), and choosing the emoji again afterwards works.
fn emoji_fault(ui: Ui, r: &mut Report) {
    let Ok(path) = std::env::var("TH_EMOJI_TSV") else {
        r.check("emoji-fault: TH_EMOJI_TSV (the engine's emoji.tsv) is required", false, || "not set".into());
        return;
    };
    // the emoji that End + Enter chooses for 愛心: the last of its row
    let table = std::fs::read_to_string(&path).unwrap_or_default();
    let want = table
        .lines()
        .find_map(|l| l.strip_prefix("ㄞˋ-ㄒㄧㄣ\t"))
        .and_then(|rest| rest.split('\t').next().and_then(|list| list.split(' ').next_back()))
        .unwrap_or("")
        .to_string();
    r.check("emoji-fault: the table has a row for 愛心", !want.is_empty(), || path.clone());
    let choose = || {
        typ("ㄞˋㄒㄧㄣ ");
        press(Key::Down);
        press(Key::End);
        press(Key::Up); // the typed bopomofo is the very last item (ZHUYIN-OUT)
        press(Key::Enter);
    };
    clear(ui);
    choose();
    nap(400);
    let (t, comp) = (text_of(ui.a), comp_text(ui));
    r.check("emoji-fault: a refused emoji commit leaves nothing in the document and no composition", t.is_empty() && comp.is_none(), || format!("{t:?} comp {comp:?}"));
    nap(300);
    clear(ui);
    choose();
    nap(200);
    let t = text_of(ui.a);
    r.check("emoji-fault: choosing the emoji again commits exactly that emoji once", t == want && comp_text(ui).is_none(), || format!("{t:?} want {want:?} comp {:?}", comp_text(ui)));
    let (mut sel_start, mut sel_end) = (0u32, 0u32);
    unsafe {
        SendMessageW(Ui::h(ui.a), 0x00B0, Some(WPARAM(&mut sel_start as *mut u32 as usize)), Some(LPARAM(&mut sel_end as *mut u32 as isize)));
    }
    let units = want.encode_utf16().count();
    r.check("emoji-fault: the caret is behind the emoji", sel_start as usize == units && sel_end as usize == units, || format!("sel {sel_start}..{sel_end}, {units} units"));
    clear(ui);
}

/// GetFocus is thread-local, so query it on the window thread as well as checking GUIThreadInfo.
fn edit_has_focus(ui: Ui) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO};
    unsafe {
        let focus = SendMessageW(Ui::h(ui.main), WM_QUERY_FOCUS, None, None).0;
        let tid = GetWindowThreadProcessId(Ui::h(ui.main), None);
        let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        focus == ui.a && GetGUIThreadInfo(tid, &mut info).is_ok() && info.hwndFocus == Ui::h(ui.a)
    }
}

fn setui_english_refresh(ui: Ui, r: &mut Report) {
    let Some(dir) = setui_user_dir() else {
        r.check("setui-english-refresh: private stage required", false, || "TWIME_STAGE missing".into());
        return;
    };
    let caps = caps_is_on();
    if caps { send_vk(VK_CAPITAL_KEY); nap(100); }
    let initial = std::fs::read_to_string(dir.join("settings.toml")).unwrap_or_default();
    r.check("setui-english-refresh: initial none / english settings", initial.lines().any(|l| l.trim() == "toggle_key = \"none\"") && initial.lines().any(|l| l.trim() == "default_mode = \"english\""), || initial);
    let ready = wait_for(Duration::from_secs(10), || trace_count("settings refresh toggle=None") > 0);
    r.check("setui-english-refresh: initial none refreshed", ready, || "no settings trace".into());
    clear(ui);
    english("a");
    r.check("setui-english-refresh: starts in English", text_of(ui.a) == "a" && comp_text(ui).is_none(), || format!("text={:?} comp={:?}", text_of(ui.a), comp_text(ui)));

    // No typing or focus changes from here until the first successful Shift tap.
    let before = trace_text();
    let count = |s: &str, needle: &str| s.lines().filter(|l| l.contains(needle)).count();
    let toggles = count(&before, "ToggleMode sent");
    tap_shift();
    let none_trace = trace_text();
    r.check("setui-english-refresh: none ignores Shift without input IPC", count(&none_trace, "ToggleMode sent") == toggles && count(&none_trace, "\texchange") == count(&before, "\texchange") && text_of(ui.a) == "a" && comp_text(ui).is_none(), || "Shift toggled, sent input IPC, or changed text".into());
    let wrote = setui_settings(&dir, "shift", "english");
    r.check("setui-english-refresh: write shift / english", wrote, || "settings write failed".into());
    let start = Instant::now();
    let mut switched = false;
    let mut kept_focus = unsafe { GetForegroundWindow() } == Ui::h(ui.main) && edit_has_focus(ui);
    let mut kept_text = true;
    for second in 1..=10 {
        if !wrote || aborted() { break; }
        // Include the tap's 160 ms in the ten-second budget.
        let due = Duration::from_millis(second * 1000 - 160);
        std::thread::sleep(due.saturating_sub(start.elapsed()));
        kept_focus &= unsafe { GetForegroundWindow() } == Ui::h(ui.main) && edit_has_focus(ui);
        if !kept_focus { break; }
        tap_shift();
        kept_focus &= unsafe { GetForegroundWindow() } == Ui::h(ui.main) && edit_has_focus(ui);
        kept_text &= text_of(ui.a) == "a" && comp_text(ui).is_none();
        if trace_count("ToggleMode sent") > toggles {
            switched = true;
            break;
        }
    }
    let after = trace_text();
    r.check("setui-english-refresh: Shift switches to Chinese within 10 seconds", switched && count(&after, "ToggleMode sent") == toggles + 1 && after.lines().rev().find(|l| l.contains("ToggleMode sent")).is_some_and(|l| l.contains("mode now Chinese")), || format!("elapsed={} ms", start.elapsed().as_millis()));
    r.check("setui-english-refresh: focus and text preserved during idle refresh", kept_focus && kept_text, || "focus or text changed".into());
    r.check("setui-english-refresh: periodic refresh observed", count(&after, "settings refresh toggle=Shift") > count(&before, "settings refresh toggle=Shift"), || "missing Shift refresh trace".into());
    // Context allocation, sync and input-link establishment are all traced. Only the successful ToggleMode may exchange.
    let stable = ["ctx new session=", "sync ok", "handshake", "link: connected", "doc focus changed"].iter().all(|n| count(&after, n) == count(&before, n));
    r.check("setui-english-refresh: no session rebuild or input reconnect", count(&before, "ctx new session=") > 0 && count(&before, "sync ok") > 0 && stable, || "context/sync/connection trace changed or initial trace missing".into());
    r.check("setui-english-refresh: no input IPC except successful toggle", count(&after, "\texchange") == count(&before, "\texchange") + usize::from(switched), || "unexpected exchange during idle refresh".into());
    if switched {
        press(Key::Letter(b'a'));
        r.check("setui-english-refresh: Chinese composition after preserved English text", comp_text(ui).is_some() && text_of(ui.a).starts_with('a'), || format!("text={:?} comp={:?}", text_of(ui.a), comp_text(ui)));
        press(Key::Esc);
    }
    clear(ui);
    if caps { toggle_caps_unconditional(); }
}

fn setui_add(ui: Ui, r: &mut Report) {
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::ClientToScreen,
        UI::{
            Input::KeyboardAndMouse::{INPUT_MOUSE, MOUSEINPUT, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP},
            WindowsAndMessaging::{FindWindowW, GetCursorPos, IsWindowVisible, SetCursorPos},
        },
    };
    let Some(dir) = setui_user_dir() else {
        r.check("setui-add: private stage required", false, || "TWIME_STAGE missing".into());
        return;
    };
    struct RestoreCursor(POINT);
    impl Drop for RestoreCursor {
        fn drop(&mut self) {
            unsafe { let _ = SetCursorPos(self.0.x, self.0.y); }
        }
    }
    let mut original = POINT::default();
    let saved_cursor = unsafe { GetCursorPos(&mut original).is_ok() };
    let restore_cursor = saved_cursor.then(|| RestoreCursor(original));
    r.check("setui-add: save cursor position", saved_cursor, || "GetCursorPos failed".into());
    let caps = caps_is_on();
    if caps { send_vk(VK_CAPITAL_KEY); nap(100); }
    clear(ui);
    let reading = "ㄐㄧㄣ ㄊㄧㄢ";
    let word = "今天";
    typ("ㄐㄧㄣ ㄊㄧㄢ ");
    r.check("setui-add: two-character composition", comp_text(ui).as_deref() == Some(word), || "unexpected conversion".into());
    press(Key::Down);
    nap(100);
    let cell = trace_text().lines().rev().find_map(|l| {
        let (_, coords) = l.split_once("cands add_cell=")?;
        let v: Vec<i32> = coords.trim().split(',').map(str::parse).collect::<Result<_, _>>().ok()?;
        (v.len() == 4).then(|| ((v[0] + v[2]) / 2, (v[1] + v[3]) / 2))
    });
    let candidate = unsafe { FindWindowW(w!("TwImeCandidate"), PCWSTR::null()).ok() };
    let notices = trace_count("notice shown");
    let foreground = unsafe { GetForegroundWindow() };
    let focused = edit_has_focus(ui);
    let composition = comp_text(ui);
    let document = text_of(ui.a);
    let visible = candidate.is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() });
    let before_ok = foreground == Ui::h(ui.main) && focused && composition.as_deref() == Some(word) && visible;
    r.check("setui-add: before click foreground is host", foreground == Ui::h(ui.main), || "unexpected foreground".into());
    r.check("setui-add: before click GetFocus and GUIThreadInfo identify edit", focused, || "edit lacks keyboard focus".into());
    r.check("setui-add: before click composition and candidates visible", composition.as_deref() == Some(word) && visible, || "composition or candidates missing".into());
    let mut clicked = false;
    if saved_cursor && before_ok && !aborted() && let (Some(h), Some((x, y))) = (candidate, cell) {
        let mut screen = POINT { x, y };
        // add_cell is in candidate client coordinates. Real mouse input exercises WM_MOUSEACTIVATE.
        let positioned = unsafe { ClientToScreen(h, &mut screen).as_bool() && SetCursorPos(screen.x, screen.y).is_ok() };
        if positioned && unsafe { GetForegroundWindow() } == foreground && edit_has_focus(ui) {
            let mouse = |flags| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } },
            };
            let sent = unsafe { SendInput(&[mouse(MOUSEEVENTF_LEFTDOWN), mouse(MOUSEEVENTF_LEFTUP)], size_of::<INPUT>() as i32) };
            if sent == 1 {
                // A partial injection must not leave the user's mouse button held.
                unsafe { SendInput(&[mouse(MOUSEEVENTF_LEFTUP)], size_of::<INPUT>() as i32); }
            }
            clicked = sent == 2;
        }
    }
    r.check("setui-add: real mouse down/up at add-cell screen center", clicked, || "missing cell, invalid preconditions, cursor move or SendInput failed".into());
    let notice = wait_for(Duration::from_secs(1), || trace_count("notice shown") > notices);
    r.check("setui-add: click shows notice", clicked && notice, || "missing button or notice".into());
    r.check("setui-add: candidate and composition survive notice", candidate.is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() }) && comp_text(ui) == composition && text_of(ui.a) == document, || "candidate, composition or document changed".into());
    r.check("setui-add: click keeps foreground", unsafe { GetForegroundWindow() } == foreground && foreground == Ui::h(ui.main), || "foreground moved".into());
    r.check("setui-add: after click GetFocus and GUIThreadInfo identify edit", edit_has_focus(ui), || "edit lost keyboard focus".into());
    let stored = wait_for(Duration::from_secs(3), || {
        std::fs::read_to_string(dir.join("userlex.tsv")).ok().is_some_and(|s| s.lines().any(|line| {
            let mut fields = line.split('\t');
            fields.next() == Some(word) && fields.next() == Some(reading)
        }))
    });
    r.check("setui-add: userlex contains 今天 / ㄐㄧㄣ ㄊㄧㄢ", stored, || "entry missing".into());
    nap(1600);
    let label = unsafe { FindWindowW(w!("TwImeLabel"), PCWSTR::null()).ok() };
    r.check("setui-add: notice expires, candidates remain", label.is_none_or(|h| unsafe { !IsWindowVisible(h).as_bool() }) && candidate.is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() }), || "notice or candidate lifetime differs".into());
    press(Key::Esc);
    press(Key::Esc);
    clear(ui);
    typ("ㄐㄧㄣ ㄊㄧㄢ ");
    press(Key::Enter);
    nap(150);
    r.check("setui-add: repeat reading converts to 今天", text_of(ui.a) == word && comp_text(ui).is_none(), || "conversion differs".into());
    clear(ui);
    if caps { toggle_caps_unconditional(); }
    drop(restore_cursor);
    let mut restored = POINT::default();
    r.check("setui-add: cursor restored", saved_cursor && unsafe { GetCursorPos(&mut restored).is_ok() } && restored == original, || "cursor restoration failed".into());
}

fn focus_b_then_a(ui: Ui) {
    unsafe {
        SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.b as usize)), None);
    }
    nap(200);
}

fn focus_a_again(ui: Ui) {
    focus_a(ui);
    nap(200);
}

fn ux_scenarios(ui: Ui, want: &dyn Fn(&str) -> bool, r: &mut Report) {
    if want("ux-punct") {
        let cases: [(Key, bool, &str); 12] = [
            (Key::Comma, true, "，"),
            (Key::Period, true, "。"),
            (Key::Slash, true, "？"),
            (Key::Semicolon, true, "："),
            (Key::Digit(1), true, "！"),
            (Key::Quote, false, "、"),
            (Key::LBracket, false, "「"),
            (Key::RBracket, false, "」"),
            (Key::Digit(9), true, "（"),
            (Key::Digit(0), true, "）"),
            (Key::Digit(6), true, "……"),
            (Key::Minus, true, "——"),
        ];
        let mut bad = vec![];
        for (k, shift, want_text) in cases {
            clear(ui);
            if shift {
                press_shift(k);
            } else {
                press(k);
            }
            nap(60);
            let t = text_of(ui.a);
            if t != want_text || comp_text(ui).is_some() {
                bad.push(format!("{k:?}: {t:?}"));
            }
        }
        r.check("ux U1/U2: full-width marks are committed at once, no composition", bad.is_empty(), || bad.join("; "));
        clear(ui);
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        press_shift(Key::Period);
        nap(80);
        let t = text_of(ui.a);
        r.check("ux U3: a convertible composition and the mark are one commit", t.chars().count() == 3 && t.ends_with('。') && comp_text(ui).is_none(), || format!("{t:?} comp {:?}", comp_text(ui)));
        clear(ui);
        typ("ㄐㄧㄣ ㄅㄅ ");
        press_shift(Key::Period);
        nap(80);
        let t = text_of(ui.a);
        r.check("ux U4: an unconvertible composition swallows the mark and stays", comp_text(ui).is_some_and(|c| c.chars().count() == 2 && c.ends_with('ㄅ')) && text_of(ui.a) == comp_text(ui).unwrap_or_default(), || format!("{t:?} comp {:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
        press(Key::NumpadDigit(3));
        press(Key::NumpadDecimal);
        press(Key::NumpadDigit(5));
        nap(60);
        let t = text_of(ui.a);
        r.check("ux U10: number pad 3 . 5 gives 3.5", t == "3.5" && comp_text(ui).is_none(), || format!("{t:?}"));
        clear(ui);
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        press(Key::NumpadDigit(3));
        nap(80);
        let t = text_of(ui.a);
        r.check("ux U11: number pad digit commits the composition with the digit", t.chars().count() == 3 && t.ends_with('3'), || format!("{t:?}"));
        clear(ui);
        typ("ㄐㄧㄣ ");
        press(Key::Backquote);
        nap(60);
        r.check("ux: backquote inside a composition is swallowed", comp_text(ui).is_some_and(|c| c.chars().count() == 1), || format!("{:?} {:?}", text_of(ui.a), comp_text(ui)));
        press(Key::Esc);
        // U5: with the candidate window open the highlighted candidate is taken and everything is committed with the mark.
        // (A focus change first: the engine forgets the context, as the fresh reference session has none.)
        focus_b_then_a(ui);
        focus_a_again(ui);
        clear(ui);
        typ("ㄑㄩㄢˊㄌㄧˋ");
        press(Key::Down);
        nap(150);
        press(Key::Space);
        nap(100);
        let open = label_window_visible();
        press_shift(Key::Comma);
        nap(150);
        let t = text_of(ui.a);
        // The same keys in an in-process session with the same assets: the committed sentence is the highlighted candidate's.
        let want = {
            let mut s = reference_engine().new_session();
            s.set_ux(true);
            for c in "ㄑㄩㄢˊㄌㄧˋ".chars() {
                s.on_key(twime_proto::KeyEvent::press(key_of(c).unwrap()));
            }
            s.on_key(twime_proto::KeyEvent::press(Key::Down));
            s.on_key(twime_proto::KeyEvent::press(Key::Space));
            s.on_key(twime_proto::KeyEvent { key: Key::Comma, mods: twime_proto::Mods(twime_proto::Mods::SHIFT), down: true }).commit.unwrap_or_default()
        };
        r.check("ux U5: a mark with the candidate window open commits the highlighted candidate's whole sentence once (equals the reference session), window closed", open && t == want && t.ends_with('，') && comp_text(ui).is_none() && !label_window_visible(), || format!("window {open}, {} chars vs {} chars, comp {:?}", t.chars().count(), want.chars().count(), comp_text(ui)));
        clear(ui);
        // The Chinese-mode keys without Shift that carry no bopomofo: Equals and Backslash are half-width.
        press(Key::Equals);
        press(Key::Backslash);
        nap(60);
        let t = text_of(ui.a);
        r.check("ux: = and backslash are typed half-width through the engine", t == "=\\", || format!("{t:?}"));
        clear(ui);
    }
    if want("ux-run") {
        press_shift(Key::Letter(b'y'));
        r.check("ux U6: Shift+letter starts an English run (shown as a composition)", comp_text(ui).as_deref() == Some("Y"), || format!("{:?}", comp_text(ui)));
        english("ou");
        press_shift(Key::Letter(b't'));
        english("ube");
        let shown = comp_text(ui);
        press(Key::Space);
        nap(80);
        let t = text_of(ui.a);
        r.check("ux U6: Space ends the run without a blank", shown.as_deref() == Some("YouTube") && t == "YouTube" && comp_text(ui).is_none(), || format!("shown {shown:?} text {t:?}"));
        typ("ㄐㄧㄣ ");
        r.check("ux U6: the next bopomofo key composes in Chinese again", comp_text(ui).is_some_and(|c| c.chars().count() == 1 && c != "YouTube"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
        english("You");
        press_shift(Key::Comma);
        nap(80);
        r.check("ux U7: a full-width mark ends the run (both committed)", text_of(ui.a) == "You，" && comp_text(ui).is_none(), || format!("{:?}", text_of(ui.a)));
        clear(ui);
        english("Node.js");
        let shown = comp_text(ui);
        press(Key::Enter);
        nap(60);
        r.check("ux U8: a dot inside the run does not end it", shown.as_deref() == Some("Node.js") && text_of(ui.a) == "Node.js", || format!("{shown:?} {:?}", text_of(ui.a)));
        clear(ui);
        press_shift(Key::Letter(b'a'));
        english("b");
        press(Key::Esc);
        nap(60);
        r.check("ux U9: Esc cancels the run", text_of(ui.a).is_empty() && comp_text(ui).is_none(), || format!("{:?} {:?}", text_of(ui.a), comp_text(ui)));
        press_shift(Key::Letter(b'a'));
        english("b");
        press(Key::Backspace);
        let one = comp_text(ui);
        press(Key::Backspace);
        nap(60);
        r.check("ux U9: Backspace deletes one character; the last one leaves the run", one.as_deref() == Some("A") && comp_text(ui).is_none() && text_of(ui.a).is_empty(), || format!("{one:?} {:?}", comp_text(ui)));
        press_shift(Key::Letter(b'a'));
        press(Key::Left);
        nap(100);
        r.check("ux U9: an arrow key commits the run and goes on to the host", text_of(ui.a) == "A" && comp_text(ui).is_none(), || format!("{:?} {:?}", text_of(ui.a), comp_text(ui)));
        clear(ui);
        // Commit Chinese, then start an English run (the split commit / asynchronous composition path).
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        press_shift(Key::Letter(b'y'));
        nap(100);
        let a = (text_of(ui.a), comp_text(ui));
        english("o");
        press(Key::Space);
        nap(100);
        r.check("ux: Chinese committed, then an English run composition", a.0.chars().count() == 3 && a.0.ends_with('Y') && a.1.as_deref() == Some("Y") && text_of(ui.a).chars().count() == 4 && text_of(ui.a).ends_with("Yo"), || format!("{a:?} then {:?}", text_of(ui.a)));
        clear(ui);
        // The next key overtakes the asynchronous composition: nothing lost, duplicated or left behind.
        let mut bad = vec![];
        for i in 0..25 {
            typ("ㄐㄧㄣ ");
            press_shift_fast(Key::Letter(b'y'));
            press_fast(Key::Letter(b'o'));
            nap(120);
            let during = comp_text(ui);
            press(Key::Space);
            nap(80);
            let t = text_of(ui.a);
            if t.chars().count() != 3 || !t.ends_with("Yo") || during.as_deref() != Some("Yo") || comp_text(ui).is_some() {
                bad.push(format!("#{i}: text {t:?} during {during:?}"));
            }
            clear(ui);
        }
        r.check("ux: quick Shift+Y then o after a committed word, 25 times: always 今Yo", bad.is_empty(), || bad.join("; "));
        // Focus leaving commits a running English text once.
        press_shift(Key::Letter(b'f'));
        english("oo");
        unsafe {
            SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.b as usize)), None);
        }
        nap(250);
        let t = text_of(ui.a);
        r.check("ux U21: leaving the field commits the English run once", t == "Foo" && comp_text(ui).is_none(), || format!("{t:?}"));
        focus_a(ui);
        clear(ui);
        // Rescue.
        for k in [b'h', b'e', b'l', b'l', b'o'] {
            press(Key::Letter(k));
        }
        let shown = comp_text(ui);
        send_raw(&[(VK_SHIFT_KEY, false), (0x0D, false), (0x0D, true), (VK_SHIFT_KEY, true)]);
        nap(100);
        r.check("ux U14: Shift+Enter turns keys typed as bopomofo into letters", shown.is_some() && text_of(ui.a) == "hello" && comp_text(ui).is_none(), || format!("{shown:?} {:?}", text_of(ui.a)));
        clear(ui);
        typ("ㄐㄧㄣ ");
        press(Key::Left);
        send_raw(&[(VK_SHIFT_KEY, false), (0x0D, false), (0x0D, true), (VK_SHIFT_KEY, true)]);
        nap(100);
        r.check("ux U14: after a cursor key the rescue does nothing (the composition is still the converted syllable)", comp_text(ui).is_some_and(|c| c.chars().count() == 1 && c != "r") && text_of(ui.a) == comp_text(ui).unwrap_or_default(), || format!("{:?} {:?}", text_of(ui.a), comp_text(ui)));
        press(Key::Esc);
        clear(ui);
    }
    if want("ux-mode") {
        let before = trace_count("mode label");
        tap_shift();
        nap(150);
        let shown = label_window_visible();
        r.check("ux: a Shift tap shows the mode label", shown, || "no label window".into());
        nap(900);
        r.check("ux: the label goes away by itself", !label_window_visible(), || "still visible after 1 s".into());
        if std::env::var("TWIME_TRACE").is_ok() {
            r.check("ux: the shell logged the label", trace_count("mode label") > before, || "no trace line".into());
        }
        english_raw_letters("abc");
        let t = text_of(ui.a);
        r.check("ux: in English mode keys reach the host untouched (no composition)", t == "abc" && comp_text(ui).is_none(), || format!("{t:?}"));
        press(Key::Comma);
        press_shift(Key::Comma);
        nap(60);
        let t = text_of(ui.a);
        r.check("ux: English mode marks are the host's own half-width ones", t == "abc,<", || format!("{t:?}"));
        clear(ui);
        tap_shift();
        typ("ㄐㄧㄣ ");
        r.check("ux: a second tap returns to Chinese (the next bopomofo key composes)", comp_text(ui).is_some_and(|c| c.chars().count() == 1), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
        // Toggle with a convertible composition: committed first.
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        tap_shift();
        nap(100);
        english_raw_letters("a");
        let t = text_of(ui.a);
        r.check("ux U12: a tap commits the composition, then English", t.chars().count() == 3 && t.ends_with('a') && comp_text(ui).is_none(), || format!("{t:?}"));
        tap_shift();
        clear(ui);
        // Not convertible but plainly typed: the keys become letters and the mode is English.
        for k in [b'h', b'e', b'l', b'l', b'o'] {
            press(Key::Letter(k));
        }
        tap_shift();
        nap(100);
        let t = text_of(ui.a);
        english_raw_letters("x");
        let t2 = text_of(ui.a);
        r.check("ux U13: a tap with an unconvertible composition gives the typed letters and English", t == "hello" && t2 == "hellox", || format!("{t:?} then {t2:?}"));
        tap_shift();
        clear(ui);
        // Long press and Shift+letter are no taps.
        hold_shift(700);
        typ("ㄐㄧㄣ ");
        r.check("ux: a long Shift press does not toggle", comp_text(ui).is_some_and(|c| c.chars().count() == 1), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        press_shift(Key::Letter(b'a'));
        press(Key::Esc);
        typ("ㄐㄧㄣ ");
        r.check("ux: Shift+letter does not toggle", comp_text(ui).is_some_and(|c| c.chars().count() == 1), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
        // The mode stays across a focus change (both edit controls of this host share one TSF context, so the isolation between
        // contexts is checked at the engine: tests/hello.rs, `the_mode_belongs_to_the_session_not_to_the_connection`).
        tap_shift();
        focus_b_then_a(ui);
        focus_a_again(ui);
        english_raw_letters("q");
        let t = text_of(ui.a);
        r.check("ux: the mode survives a focus change in the same context", t == "q", || format!("a {t:?}"));
        tap_shift();
        clear(ui);
        typ("ㄐㄧㄣ ");
        r.check("ux: back in Chinese", comp_text(ui).is_some_and(|c| c.chars().count() == 1), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
        // The host refuses the commit that a tap carries: the mode stays Chinese and nothing is duplicated.
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        unsafe {
            SendMessageW(Ui::h(ui.a), EM_SETREADONLY, Some(WPARAM(1)), None);
        }
        tap_shift();
        nap(200);
        unsafe {
            SendMessageW(Ui::h(ui.a), EM_SETREADONLY, Some(WPARAM(0)), None);
        }
        nap(200);
        let t1 = text_of(ui.a);
        press(Key::Letter(b'q'));
        nap(100);
        r.check("ux: refused commit on a tap: mode back to Chinese (the next letter composes), nothing duplicated", comp_text(ui).as_deref() == Some("ㄆ") && t1.matches('今').count() == 0, || format!("text {t1:?} comp {:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
    }
    if want("ux-sym") {
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect};
        press(Key::Backquote);
        nap(200);
        r.check("ux U24: the backquote opens the symbol table (anchor composition and window)", comp_text(ui).as_deref() == Some("`") && label_window_visible(), || format!("{:?}", comp_text(ui)));
        press(Key::Digit(4));
        nap(100);
        r.check("ux U24: digit 4 commits the fourth symbol and removes the anchor", text_of(ui.a) == "_" && comp_text(ui).is_none() && !label_window_visible(), || format!("{:?} {:?}", text_of(ui.a), comp_text(ui)));
        clear(ui);
        press(Key::Backquote);
        press(Key::Esc);
        nap(100);
        r.check("ux U24: Esc cancels the table without committing", text_of(ui.a).is_empty() && comp_text(ui).is_none(), || format!("{:?} {:?}", text_of(ui.a), comp_text(ui)));
        press(Key::Backquote);
        nap(200);
        let h = unsafe { FindWindowW(w!("TwImeCandidate"), PCWSTR::null()).ok() };
        if let Some(h) = h {
            let mut cr = windows::Win32::Foundation::RECT::default();
            unsafe {
                let _ = GetWindowRect(h, &mut cr);
                // The second of nine cells, about 3/18 of the width.
                let x = ((cr.right - cr.left) * 3) / 18;
                PostMessageW(Some(h), 0x0201, WPARAM(1), LPARAM((10 << 16) | x as isize)).ok();
            }
            nap(250);
        }
        let t = text_of(ui.a);
        r.check("ux U24: a click commits that symbol", (t == ">" || t == "<" || t == "\"") && comp_text(ui).is_none(), || format!("{t:?} {:?}", comp_text(ui)));
        clear(ui);
        // Page 2 (／ ± ％): PageDown, then the digit; the paging keys keep the table open.
        for (d, want) in [(1u8, "／"), (2, "±"), (3, "％")] {
            press(Key::Backquote);
            press(Key::PageDown);
            nap(200);
            let open = comp_text(ui).as_deref() == Some("`") && label_window_visible();
            press(Key::Digit(d));
            nap(100);
            r.check(&format!("ux U24: page 2, digit {d} commits {want} (one character)"), open && text_of(ui.a) == want && comp_text(ui).is_none() && !label_window_visible(), || format!("open {open} {:?} {:?}", text_of(ui.a), comp_text(ui)));
            clear(ui);
        }
        press(Key::Backquote);
        press(Key::Right);
        press(Key::Right); // wraps back to page 1
        press(Key::Digit(1));
        nap(100);
        r.check("ux U24: Right twice wraps to page 1 (digit 1 commits <)", text_of(ui.a) == "<" && comp_text(ui).is_none(), || format!("{:?} {:?}", text_of(ui.a), comp_text(ui)));
        clear(ui);
        press(Key::Backquote);
        press_shift(Key::Period);
        nap(100);
        r.check("ux U24: another key closes the table and is handled as without it (the anchor is never committed)", text_of(ui.a) == "。" && comp_text(ui).is_none(), || format!("{:?} {:?}", text_of(ui.a), comp_text(ui)));
        clear(ui);
    }
    if want("ux-caps") {
        if caps_is_on() {
            println!("INFO\tux-caps: Caps Lock is on already; not touching it");
        } else {
            typ("ㄐㄧㄣ ㄊㄧㄢ ");
            send_vk(VK_CAPITAL_KEY);
            nap(120);
            let on = caps_is_on();
            press(Key::Letter(b'a')); // Caps Lock on: the composition is committed and the key goes on (upper case)
            nap(100);
            let t = text_of(ui.a);
            let none = comp_text(ui).is_none();
            send_raw(&[(0x42, false), (0x42, true)]); // another letter at idle: untouched
            nap(60);
            let t2 = text_of(ui.a);
            send_raw(&[(VK_SHIFT_KEY, false), (0x0D, false), (0x0D, true), (VK_SHIFT_KEY, true)]);
            if caps_is_on() {
                toggle_caps_unconditional();
                nap(100);
            }
            r.check("ux U16: with Caps Lock on a letter commits the composition and passes on; idle keys pass", on && t.chars().count() == 3 && t.ends_with('A') && none && t2.chars().count() == 4 && t2.ends_with("AB"), || format!("caps {on} {t:?} {t2:?}"));
            clear(ui);
            r.check("ux: Caps Lock is back to off", !caps_is_on(), || "still on".into());
            typ("ㄐㄧㄣ ");
            r.check("ux: Chinese again after Caps Lock off", comp_text(ui).is_some_and(|c| c.chars().count() == 1), || format!("{:?}", comp_text(ui)));
            press(Key::Esc);
            clear(ui);
        }
    }
}

/// Letters typed in English mode, through the host (WM_CHAR).
fn english_raw_letters(s: &str) {
    for c in s.chars() {
        send_vk(u16::from(c as u8) - 32);
        nap(25);
    }
    nap(60);
}

/// A new connection or a restarted engine: until the context is synchronised every key passes through, and without any
/// focus change it becomes ready again by itself. `slow`: the engine stays away for
/// more than five retry periods (its executable is moved aside, `TWIME_STAGE` names the private stage).
fn ux_restart(ui: Ui, r: &mut Report, slow: bool) {
    #[derive(Clone, Copy, Debug)]
    enum First {
        ShiftTap,
        ShiftLetter,
        PlainLetter,
    }
    let stage = std::env::var("TWIME_STAGE").ok().map(std::path::PathBuf::from);
    if slow && stage.is_none() {
        println!("INFO\tux-restart-slow: TWIME_STAGE not set; skipped");
        return;
    }
    for first in [First::ShiftTap, First::ShiftLetter, First::PlainLetter] {
        let tag = format!("ux-restart{} {first:?}", if slow { "-slow" } else { "" });
        nap(11_000); // the shell starts an engine at most every 10 s: do not measure that throttle
        clear(ui);
        let oks = trace_count("sync ok");
        let fails = trace_count("sync: not connected") + trace_count("sync failed");
        let exe = stage.as_ref().map(|s| s.join(ids::ENGINE_NAME));
        let aside = stage.as_ref().map(|s| s.join("twime-engined.exe.aside"));
        if slow && let (Some(_e), Some(a)) = (&exe, &aside) {
            let _ = std::fs::remove_file(a);
        }
        let killed = kill_engine();
        if slow && let (Some(e), Some(a)) = (&exe, &aside) {
            nap(300);
            if std::fs::rename(e, a).is_err() {
                r.check(&format!("{tag}: engine executable moved aside"), false, || "rename failed".into());
                continue;
            }
        }
        let t0 = Instant::now();
        match first {
            First::ShiftTap => tap_shift(),
            First::ShiftLetter => press_shift(Key::Letter(b'a')),
            First::PlainLetter => press(Key::Letter(b'a')),
        }
        nap(120);
        let passed = text_of(ui.a);
        let want_first = match first {
            First::ShiftTap => "",
            First::ShiftLetter => "A",
            First::PlainLetter => "a",
        };
        r.check(&format!("{tag}: the first key passes straight through (engine gone, not synchronised)"), killed && passed == want_first && comp_text(ui).is_none(), || format!("killed {killed} text {passed:?}"));
        if slow {
            nap(8000);
            let failed_tries = trace_count("sync: not connected") + trace_count("sync failed") - fails;
            r.check(&format!("{tag}: the retry timer kept trying while the engine was away (more than 5 attempts)"), failed_tries >= 5, || format!("{failed_tries} failed attempts"));
            if let (Some(e), Some(a)) = (&exe, &aside) {
                let _ = std::fs::rename(a, e);
            }
        }
        // When did the engine become reachable, and when did the context become ready again?
        let (mut t_engine, mut t_sync) = (None::<Duration>, None::<Duration>);
        let limit = Duration::from_secs(if slow { 60 } else { 30 });
        while t0.elapsed() < limit {
            if t_engine.is_none() && engine_pid().is_some() {
                t_engine = Some(t0.elapsed());
            }
            if trace_count("sync ok") > oks {
                t_sync = Some(t0.elapsed());
                break;
            }
            nap(50);
        }
        let ok = t_sync.is_some();
        let took = t0.elapsed();
        r.check(&format!("{tag}: ready again without a focus change ({} ms after the first key)", took.as_millis()), ok, || "never synchronised again".into());
        if let (Some(e), Some(s)) = (t_engine, t_sync) {
            let lag = s.saturating_sub(e);
            let tries = trace_count("sync: not connected") + trace_count("sync failed") - fails;
            println!("INFO	{tag}: engine reachable after {} ms, ready {} ms later, failed attempts before {tries}", e.as_millis(), lag.as_millis());
            // One retry period (about 1 s for the first five attempts, about 5 s after that) plus the engine's own answer time.
            let allowed = if tries <= 5 { 2_000 } else { 6_500 };
            r.check(&format!("{tag}: ready within one retry period of the engine being back ({} ms, limit {allowed} ms)", lag.as_millis()), lag.as_millis() <= allowed, || format!("{} ms", lag.as_millis()));
        }
        println!("INFO\t{tag}: recovered after {} ms, failed attempts {}", took.as_millis(), trace_count("sync: not connected") + trace_count("sync failed") - fails);
        nap(200);
        clear(ui);
        typ("ㄆ");
        r.check(&format!("{tag}: input works again (Chinese mode, nothing toggled by the first key)"), comp_text(ui).as_deref() == Some("ㄆ"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        clear(ui);
    }
}

/// Engine started with `--default-english`: a new context is in English mode from the first key on.
fn ux_default_english(ui: Ui, r: &mut Report) {
    english_raw_letters("ab");
    let t = text_of(ui.a);
    r.check("ux: default English mode: keys reach the host untouched from the first one", t == "ab" && comp_text(ui).is_none(), || format!("{t:?}"));
    tap_shift();
    typ("ㄐㄧㄣ ");
    r.check("ux: a tap switches to Chinese (the next bopomofo key composes)", comp_text(ui).is_some_and(|c| c.chars().count() == 1), || format!("{:?}", comp_text(ui)));
    press(Key::Esc);
    clear(ui);
}

/// Latency of the commit-type keys: the shell's own timing from the first callback of
/// the key to the end of the host write, taken from the trace file (numbers only). Exploratory on a busy machine.
fn ux_lat(ui: Ui) {
    let n: usize = std::env::var("TH_LAT_N").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    let busy0 = cpu_times();
    let classes = ["punct", "punct+composition", "numpad", "run-end", "toggle"];
    for class in classes {
        clear(ui);
        let skip = trace_text().lines().count();
        for i in 0..n {
            if aborted() {
                resume(ui);
            }
            match class {
                "punct" => press_shift_fast(Key::Period),
                "punct+composition" => {
                    typ("ㄐㄧㄣ ");
                    press_shift_fast(Key::Period);
                }
                "numpad" => press_fast(Key::NumpadDigit(3)),
                "run-end" => {
                    press_shift_fast(Key::Letter(b'a'));
                    nap(8);
                    press_fast(Key::Space);
                }
                _ => {
                    tap_shift();
                    tap_shift();
                }
            }
            nap(14);
            if i % 20 == 19 {
                clear(ui);
            }
        }
        let text = trace_text();
        let mut v: Vec<f64> = vec![];
        let (needle, field) = match class {
            "punct" | "punct+composition" => ("KEYDOWN vk=0xbe ", "total_us="),
            "numpad" => ("KEYDOWN vk=0x63 ", "total_us="),
            "run-end" => ("KEYDOWN vk=0x20 ", "total_us="),
            _ => ("TOGGLE total_us=", "total_us="),
        };
        for l in text.lines().skip(skip).filter(|l| l.contains(needle)) {
            if let Some(x) = l.split(field).nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).and_then(|s| s.parse::<f64>().ok()) {
                v.push(x / 1000.0);
            }
        }
        if v.is_empty() {
            println!("INFO\tlat {class}: n=0");
            continue;
        }
        v.sort_by(f64::total_cmp);
        let q = |p: f64| v[((v.len() as f64 * p) as usize).min(v.len() - 1)];
        println!("INFO\tlat {class}: n={} p50={:.3} p95={:.3} p99={:.3} max={:.3} (ms)", v.len(), q(0.5), q(0.95), q(0.99), v[v.len() - 1]);
    }
    clear(ui);
    println!("INFO\tlat: machine CPU busy during the run: {:.0}%", cpu_busy(busy0, cpu_times()));
}

/// The engine the engine process builds (TRAIN T-model weights, prior, lambda 0.5; G1K-s07 before 2026-10-22), in this process, as the reference for comparisons.
fn reference_engine() -> Arc<twime_core::Engine> {
    use twime_core::{Config, Engine};
    use twime_model::{Assets, ModelPaths};
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let assets = Assets::load(&ModelPaths {
        data_txt: root.join("third_party/mcbopomofo/data.txt"),
        aliases: root.join("engine/data/common-readings.txt"),
        model: root.join("data/train/T-model.twg1"),
        prior: Some(root.join("data/vd/prior/freq.pack")),
    })
    .expect("assets");
    Engine::new(Arc::new(assets), Config::default())
}

/// A committed Chinese word followed by an English run whose composition write fails after the commit (one edit session;
/// `TWIME_FAULT=async-fail`, honoured in dev mode only): the committed word stays, nothing of the run is left
/// in the host, and input works again without a focus change.
fn ux_fault(ui: Ui, r: &mut Report, tag: &str) {
    for round in 0..3 {
        clear(ui);
        typ("ㄐㄧㄣ ㄊㄧㄢ ");
        let word = comp_text(ui).unwrap_or_default();
        press_shift(Key::Letter(b'y'));
        nap(300);
        let t = text_of(ui.a);
        let comp = comp_text(ui);
        r.check(&format!("{tag} #{round}: exactly the committed word stays, no run text or composition is left behind"), word.chars().count() == 2 && t == word && comp.is_none(), || format!("{t:?} vs {word:?}, comp {comp:?}"));
        // The shell cleans up (nothing to remove) and takes keys again; the next composition works.
        nap(300);
        typ("ㄆ");
        r.check(&format!("{tag} #{round}: input works again afterwards"), comp_text(ui).as_deref() == Some("ㄆ"), || format!("{:?}", comp_text(ui)));
        press(Key::Esc);
        nap(100);
    }
    clear(ui);
}

/// The host refuses the final text after the composition was ended (`TWIME_FAULT=commit-fail`, once): the interim text is taken
/// away again, nothing stays half committed, the shell recovers and the next commit works.
fn ux_fault_commit(ui: Ui, r: &mut Report) {
    typ("ㄐㄧㄣ ㄊㄧㄢ ");
    let word = comp_text(ui).unwrap_or_default();
    press(Key::Enter);
    nap(400);
    let (t, comp) = (text_of(ui.a), comp_text(ui));
    r.check("ux-fault-commit: a refused commit leaves nothing in the document and no composition", word.chars().count() == 2 && t.is_empty() && comp.is_none(), || format!("{t:?} comp {comp:?} (word {word:?})"));
    nap(300);
    typ("ㄐㄧㄣ ㄊㄧㄢ ");
    let again = comp_text(ui);
    press(Key::Enter);
    nap(200);
    let t = text_of(ui.a);
    r.check("ux-fault-commit: typing and committing work again afterwards", again.is_some_and(|c| c.chars().count() == 2) && t.chars().count() == 2 && comp_text(ui).is_none(), || format!("{t:?}"));
    clear(ui);
}

/// The commit of a Chinese word that also starts an English run is refused (`TWIME_FAULT=commit-fail`, once): nothing of the word
/// or the run stays in the document and input works again.
fn ux_fault_commit_run(ui: Ui, r: &mut Report) {
    typ("ㄐㄧㄣ ㄊㄧㄢ ");
    let word = comp_text(ui).unwrap_or_default();
    press_shift(Key::Letter(b'y'));
    // right away: the cleanup echo of the refused commit must not be taken for an external edit that drops the next composition
    typ("ㄆ");
    nap(200);
    let (t, comp) = (text_of(ui.a), comp_text(ui));
    r.check("ux-fault-commit-run: a refused commit leaves neither the word nor the run behind, and the next key composes", word.chars().count() == 2 && t == "ㄆ" && comp.as_deref() == Some("ㄆ"), || format!("{t:?} comp {comp:?} (word {word:?})"));
    press(Key::Esc);
    clear(ui);
}

// Direct-drive tests deliberately use the same COM event sinks as the system.
thread_local! {
    static HOST_SINK: std::cell::RefCell<Option<windows::Win32::UI::TextServices::ITfKeyEventSink>> = const { std::cell::RefCell::new(None) };
    static LATE_PUSH: std::cell::RefCell<Option<(windows::Win32::UI::TextServices::ITfDocumentMgr, Option<windows::Win32::UI::TextServices::ITfContext>, windows::Win32::UI::TextServices::ITfDocumentMgr)>> = const { std::cell::RefCell::new(None) };
    static STACK_PUSH: std::cell::RefCell<Option<(windows::Win32::UI::TextServices::ITfDocumentMgr, windows::Win32::UI::TextServices::ITfContext)>> = const { std::cell::RefCell::new(None) };
}

/// A synthetic context tests the empty-document -> delayed Push order without
/// transferring a CUAS-owned RichEdit context away from its document manager.
fn late_push_on_ui(stage: usize) -> windows::core::Result<()> {
    use windows::{core::IUnknown, Win32::{Foundation::E_FAIL, UI::TextServices::TF_POPF_ALL}};
    let (tm, tid) = HOST_TM.with(|h| h.borrow().clone()).ok_or(windows::core::Error::from(E_FAIL))?;
    unsafe {
        match stage {
            0 => {
                let old = tm.GetFocus()?;
                let doc = tm.CreateDocumentMgr()?;
                tm.SetFocus(&doc)?;
                if doc.GetTop().is_ok() { return Err(E_FAIL.into()); }
                LATE_PUSH.with(|h| *h.borrow_mut() = Some((doc, None, old)));
            }
            1 => {
                let (doc, _, old) = LATE_PUSH.with(|h| h.borrow_mut().take()).ok_or(windows::core::Error::from(E_FAIL))?;
                let mut ctx = None;
                let mut ec = 0;
                let result = doc.CreateContext(tid, 0, None::<&IUnknown>, &mut ctx, &mut ec)
                    .and_then(|()| doc.Push(ctx.as_ref().ok_or(windows::core::Error::from(E_FAIL))?));
                LATE_PUSH.with(|h| *h.borrow_mut() = Some((doc, ctx, old)));
                result?;
            }
            2 => {
                let ctx = LATE_PUSH.with(|h| h.borrow().as_ref().and_then(|v| v.1.clone())).ok_or(windows::core::Error::from(E_FAIL))?;
                let sink = HOST_SINK.with(|h| h.borrow().clone()).ok_or(windows::core::Error::from(E_FAIL))?;
                // Test-only: no edit session is requested from the synthetic context.
                if !sink.OnTestKeyDown(&ctx, WPARAM(0x41), LPARAM(0))?.as_bool() { return Err(E_FAIL.into()); }
            }
            3 => {
                if let Some((doc, _, old)) = LATE_PUSH.with(|h| h.borrow_mut().take()) {
                    tm.SetFocus(&old)?;
                    doc.Pop(TF_POPF_ALL)?;
                }
            }
            4 => {
                let doc = tm.GetFocus()?;
                let base = doc.GetTop()?;
                let mut ctx = None;
                let mut ec = 0;
                doc.CreateContext(tid, 0, None::<&IUnknown>, &mut ctx, &mut ec)?;
                doc.Push(ctx.as_ref().ok_or(windows::core::Error::from(E_FAIL))?)?;
                STACK_PUSH.with(|h| *h.borrow_mut() = Some((doc, base)));
            }
            5 => {
                use windows::core::Interface;
                let (doc, base) = STACK_PUSH.with(|h| h.borrow_mut().take()).ok_or(windows::core::Error::from(E_FAIL))?;
                // No SetFocus callback: only OnPopContext can restore tracking.
                doc.Pop(0)?;
                if doc.GetTop()?.cast::<IUnknown>()? != base.cast::<IUnknown>()? {
                    return Err(E_FAIL.into());
                }
            }
            _ => return Err(E_FAIL.into()),
        }
    }
    Ok(())
}

/// Seed only the TIP's cached document focus. The real thread-manager focus
/// remains on RichEdit; no document notification is delivered during recovery.
fn recover_key_focus_on_ui(stale: bool) -> windows::core::Result<()> {
    use windows::{core::{Interface, IUnknown}, Win32::{Foundation::E_FAIL,
        UI::TextServices::{ITfThreadMgrEventSink, TF_POPF_ALL}}};
    let (tm, tid) = HOST_TM.with(|h| h.borrow().clone()).ok_or(windows::core::Error::from(E_FAIL))?;
    let sink = HOST_SINK.with(|h| h.borrow().clone()).ok_or(windows::core::Error::from(E_FAIL))?;
    let events: ITfThreadMgrEventSink = sink.cast()?;
    unsafe {
        let actual = tm.GetFocus()?;
        let top = actual.GetTop()?;
        let other = if stale {
            let doc = tm.CreateDocumentMgr()?;
            let mut ctx = None;
            let mut cookie = 0;
            doc.CreateContext(tid, 0, None::<&IUnknown>, &mut ctx, &mut cookie)?;
            doc.Push(ctx.as_ref().ok_or(windows::core::Error::from(E_FAIL))?)?;
            Some(doc)
        } else { None };
        // Deliberately make the cache disagree with GetFocus/GetTop.
        events.OnSetFocus(other.as_ref(), None)?;
        if tm.GetFocus()?.cast::<IUnknown>()? != actual.cast::<IUnknown>()?
            || actual.GetTop()?.cast::<IUnknown>()? != top.cast::<IUnknown>()? {
            return Err(E_FAIL.into());
        }
        // No message pumping, SetFocus, or thread-manager focus notification
        // between poisoning the cache and this direct keyboard-focus callback.
        let result = sink.OnSetFocus(true.into());
        if let Some(other) = other { other.Pop(TF_POPF_ALL)?; }
        result?;
    }
    Ok(())
}

fn check_focus_input(ui: Ui, edit: isize, tag: &str, r: &mut Report) {
    // The runner starts these named scenarios in Chinese mode with Shift enabled.
    press(Key::Esc);
    set_text(edit, "");
    typ("\u{3105}");
    r.check(&format!("{tag}: composition"), comp_text(ui).as_deref() == Some("\u{3105}"), || "composition missing after focus return".into());
    press(Key::Esc);
    send_vk(VK_SHIFT_KEY);
    nap(150);
    press(Key::Letter(b'a'));
    r.check(&format!("{tag}: Shift selects English"), comp_text(ui).is_none() && text_of(edit) == "a", || "Shift did not select English".into());
    set_text(edit, "");
    send_vk(VK_SHIFT_KEY);
    nap(150);
    typ("\u{3105}");
    r.check(&format!("{tag}: Shift restores Chinese"), comp_text(ui).as_deref() == Some("\u{3105}"), || "Shift did not restore Chinese".into());
    press(Key::Esc);
    set_text(edit, "");
}

fn focus_cycle(ui: Ui, r: &mut Report) {
    let caps = caps_is_on();
    if caps { send_vk(VK_CAPITAL_KEY); nap(100); }
    focus_a(ui);
    clear(ui);
    for (stale, tag) in [(false, "zero cache"), (true, "other context cache")] {
        let before = trace_count("reason=key_focus");
        let docs = trace_count("doc focus changed");
        let ok = unsafe { SendMessageW(Ui::h(ui.main), WM_RECOVER_KEY_FOCUS,
            Some(WPARAM(usize::from(stale))), None).0 != 0 };
        r.check(&format!("focus-cycle {tag}: direct OnSetFocus(true)"), ok, || "focus recovery callback failed".into());
        if std::env::var("TWIME_TRACE").is_ok() {
            // Exactly one document notification seeds the cache; recovery must
            // take the old != key branch before any input can repair the cache.
            r.check(&format!("focus-cycle {tag}: keyboard callback reacquires context without document notification"),
                trace_count("reason=key_focus") == before + 1 && trace_count("doc focus changed") == docs + 1,
                || "keyboard callback did not replace the cached context".into());
        }
        nap(150);
        if ok { check_focus_input(ui, ui.a, &format!("focus-cycle {tag}"), r); }
    }
    for cycle in 1..=3 {
        focus_a(ui);
        clear(ui);
        typ("\u{3105}");
        r.check(&format!("focus-cycle {cycle}: composing before sink focus loss"), comp_text(ui).as_deref() == Some("\u{3105}"), || "initial composition missing".into());
        for foreground in [false, true] {
            let ok = unsafe { SendMessageW(Ui::h(ui.main), WM_KEY_FOCUS, Some(WPARAM(usize::from(foreground))), None).0 != 0 };
            r.check(&format!("focus-cycle {cycle}: sink OnSetFocus({foreground})"), ok, || "key sink callback failed".into());
            nap(150);
        }
        r.check(&format!("focus-cycle {cycle}: sink focus return preserves composition"), comp_text(ui).as_deref() == Some("\u{3105}"), || "ready context composition was reset".into());
        // Leave the incomplete phonetic composition active while changing documents.
        unsafe { SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.b as usize)), None); }
        nap(300);
        focus_a(ui);
        nap(300);
        r.check(&format!("focus-cycle {cycle}: document change cancels incomplete composition"), comp_text(ui).is_none() && text_of(ui.a).is_empty(), || "stale composition/text after focus return".into());
        check_focus_input(ui, ui.a, &format!("focus-cycle {cycle} composing return"), r);
        for (edit, name) in [(ui.b, "b"), (ui.a, "a")] {
            unsafe { SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(edit as usize)), None); }
            nap(300);
            check_focus_input(ui, edit, &format!("focus-cycle {cycle} edit {name}"), r);
        }
        typ("\u{3105}");
        r.check(&format!("focus-cycle {cycle}: composing before top-level change"), comp_text(ui).as_deref() == Some("\u{3105}"), || "initial composition missing".into());
        unsafe {
            SendMessageW(Ui::h(ui.other), WM_SHOWME, None, None);
            SendMessageW(Ui::h(ui.other), WM_FOCUS_EDIT, Some(WPARAM(ui.other_edit as usize)), None);
        }
        let left = wait_for(Duration::from_secs(3), || unsafe { GetForegroundWindow() } == Ui::h(ui.other));
        r.check(&format!("focus-cycle {cycle}: second top-level foreground"), left, || "second window did not receive foreground".into());
        nap(300);
        let back = focus_a(ui);
        r.check(&format!("focus-cycle {cycle}: main foreground restored"), back, || "main window did not regain foreground".into());
        if !back { break; }
        nap(300);
        r.check(&format!("focus-cycle {cycle}: top-level return clears incomplete composition"), comp_text(ui).is_none() && text_of(ui.a).is_empty(), || "stale composition/text after top-level return".into());
        check_focus_input(ui, ui.a, &format!("focus-cycle {cycle} top-level return"), r);
    }
    if caps { toggle_caps_unconditional(); }
}

fn late_push(ui: Ui, r: &mut Report) {
    let caps = caps_is_on();
    if caps { send_vk(VK_CAPITAL_KEY); nap(100); }
    let stage = |n| unsafe { SendMessageW(Ui::h(ui.main), WM_LATE_PUSH, Some(WPARAM(n)), None).0 != 0 };
    let empty = stage(0);
    r.check("late-push: focus an empty document", empty, || "could not create empty focused document".into());
    nap(200);
    let pushed = empty && stage(1);
    r.check("late-push: delayed context push", pushed, || "CreateContext or Push failed".into());
    nap(500);
    r.check("late-push: context becomes ready", pushed && stage(2), || "delayed context did not accept a key test".into());
    r.check("late-push: restore original document", stage(3), || "restore failed".into());
    focus_a(ui);
    nap(300);
    check_focus_input(ui, ui.a, "late-push RichEdit return", r);
    let stacked = stage(4);
    r.check("late-push: push temporary context above ready RichEdit", stacked, || "temporary Push failed".into());
    nap(500);
    let popped = stacked && stage(5);
    r.check("late-push: pop restores the same underlying ready context", popped, || "Pop or underlying context identity failed".into());
    nap(300);
    if popped {
        check_focus_input(ui, ui.a, "late-push ready context after pop", r);
    }
    if caps { toggle_caps_unconditional(); }
}
