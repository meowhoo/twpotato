//! Drives real hosts (a NEW Notepad window, an isolated Edge instance with its own profile directory) with the registered
//! TwIme input method. Never touches existing windows: it records which processes exist first, only works on windows of
//! processes it started, waits until the user is idle, and checks the foreground before every key.
//!
//!   hostdrive notepad | edge          run the checks, print PASS / FAIL / INFO lines
//!   hostdrive wholoads                list processes that loaded twime_tsf.dll with their token (integrity, AppContainer)
//!
//! The input method is switched for the session with ActivateProfile(TF_IPPMF_FORSESSION) and the previous profile is
//! restored at the end (also on errors).

#![cfg(windows)]

use std::{collections::HashSet, process::Command, time::{Duration, Instant}};

use twime_core::keymap::key_of;
use twime_proto::{Key, ids};
use windows::Win32::System::Variant::VARIANT;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, WPARAM},
        Security::{GetTokenInformation, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel, TokenIsAppContainer},
        System::{
            Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx},
            Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess},
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationValuePattern,
                TreeScope_Descendants, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
                UIA_TextPatternId, UIA_ValuePatternId,
            },
            Input::KeyboardAndMouse::{GetLastInputInfo, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, LASTINPUTINFO, SendInput, VIRTUAL_KEY},
            TextServices::{CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD, ITfInputProcessorProfileMgr, TF_INPUTPROCESSORPROFILE},
            WindowsAndMessaging::{
                EnumWindows, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                SetForegroundWindow, WM_CLOSE, GetWindowRect,
            },
        },
    },
    core::{GUID, w, PCWSTR},
};

const FORSESSION: u32 = 0x2000_0000;

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

fn clsid() -> GUID {
    GUID::from_u128(ids::CLSID_TIP)
}
fn profile() -> GUID {
    GUID::from_u128(ids::PROFILE)
}

// ---- input -------------------------------------------------------------------------------------------------------------

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
        Key::Down => 0x28,
        Key::Up => 0x26,
        Key::Left => 0x25,
        Key::Right => 0x27,
        Key::PageUp => 0x21,
        Key::PageDown => 0x22,
        Key::Backquote => 0xC0,
        Key::LBracket => 0xDB,
        Key::RBracket => 0xDD,
        Key::Quote => 0xDE,
        Key::Backslash => 0xDC,
        Key::Equals => 0xBB,
        Key::NumpadDecimal => 0x6E,
        Key::NumpadDigit(d) => 0x60 + u16::from(d),
        _ => 0,
    }
}

const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_CAPITAL: u16 = 0x14;

struct Driver {
    target: HWND,
    aborted: bool,
}

impl Driver {
    /// Injects `(virtual key, is key-up)` events as one batch, only while the target window is in the foreground.
    fn raw(&mut self, events: &[(u16, bool)]) {
        unsafe {
            if self.aborted || GetForegroundWindow() != self.target {
                self.aborted = true;
                return;
            }
            let inputs: Vec<INPUT> = events
                .iter()
                .map(|&(vk, up)| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, time: 0, dwExtraInfo: 0 } },
                })
                .collect();
            SendInput(&inputs, size_of::<INPUT>() as i32);
        }
    }
    fn press(&mut self, k: Key) {
        let v = vk_of(k);
        self.raw(&[(v, false), (v, true)]);
        std::thread::sleep(Duration::from_millis(40));
    }
    fn shift_press(&mut self, k: Key) {
        let v = vk_of(k);
        self.raw(&[(VK_SHIFT, false), (v, false), (v, true), (VK_SHIFT, true)]);
        std::thread::sleep(Duration::from_millis(60));
    }
    /// A clean Shift tap: down, a short pause, up.
    fn tap_shift(&mut self) {
        self.raw(&[(VK_SHIFT, false)]);
        std::thread::sleep(Duration::from_millis(60));
        self.raw(&[(VK_SHIFT, true)]);
        std::thread::sleep(Duration::from_millis(200));
    }
    fn hold_shift(&mut self, ms: u64) {
        self.raw(&[(VK_SHIFT, false)]);
        std::thread::sleep(Duration::from_millis(ms));
        self.raw(&[(VK_SHIFT, true)]);
        std::thread::sleep(Duration::from_millis(200));
    }
    fn typ(&mut self, s: &str) {
        for c in s.chars() {
            if c == ' ' {
                self.press(Key::Space);
            } else {
                self.press(key_of(c).expect("key"));
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    /// English text: capitals with Shift, lower case plain, `.` the period key.
    fn english(&mut self, s: &str) {
        for c in s.chars() {
            if c.is_ascii_uppercase() {
                self.shift_press(Key::Letter(c.to_ascii_lowercase() as u8));
            } else if c.is_ascii_lowercase() {
                self.press(Key::Letter(c as u8));
            } else if c == '.' {
                self.press(Key::Period);
            } else {
                panic!("english(): unsupported {c}");
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn wait_user_idle(secs: u32) {
    loop {
        let mut li = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        let idle = unsafe {
            if GetLastInputInfo(&mut li).as_bool() {
                windows::Win32::System::SystemInformation::GetTickCount().wrapping_sub(li.dwTime)
            } else {
                0
            }
        };
        if idle >= secs * 1000 {
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn fg_lock_timeout(set: Option<u32>) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{SPI_GETFOREGROUNDLOCKTIMEOUT, SPI_SETFOREGROUNDLOCKTIMEOUT, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
    unsafe {
        let mut cur = 0u32;
        let _ = SystemParametersInfoW(SPI_GETFOREGROUNDLOCKTIMEOUT, 0, Some(&mut cur as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        if let Some(v) = set {
            // fWinIni = 0: only the running session, nothing is written to the user profile
            let _ = SystemParametersInfoW(SPI_SETFOREGROUNDLOCKTIMEOUT, 0, Some(v as usize as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        cur
    }
}

fn grab_foreground(hwnd: HWND) -> bool {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
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
        if GetForegroundWindow() != hwnd {
            windows::Win32::UI::WindowsAndMessaging::SwitchToThisWindow(hwnd, true);
        }
        if GetForegroundWindow() != hwnd
            && let Ok(ua) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            && let Ok(el) = ua.ElementFromHandle(hwnd)
        {
            let r = el.SetFocus();
            println!("INFO	UI Automation SetFocus: {r:?}");
        }
        std::thread::sleep(Duration::from_millis(400));
        let fg2 = GetForegroundWindow();
        if fg2 != hwnd {
            let mut c = [0u16; 64];
            let n = GetClassNameW(fg2, &mut c) as usize;
            println!("INFO	foreground is {:?} instead of the target", String::from_utf16_lossy(&c[..n]));
        }
        fg2 == hwnd
    }
}

// ---- windows / processes ----------------------------------------------------------------------------------------------------

fn top_windows() -> Vec<(HWND, u32, String)> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> windows::core::BOOL {
        unsafe {
            let v = &mut *(lp.0 as *mut Vec<(HWND, u32, String)>);
            if IsWindowVisible(h).as_bool() {
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                let mut c = [0u16; 64];
                let n = GetClassNameW(h, &mut c) as usize;
                v.push((h, pid, String::from_utf16_lossy(&c[..n])));
            }
        }
        true.into()
    }
    let mut v: Vec<(HWND, u32, String)> = vec![];
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

fn new_window(before: &HashSet<isize>, class: &str, limit: Duration) -> Option<(HWND, u32)> {
    let t = Instant::now();
    while t.elapsed() < limit {
        for (h, pid, c) in top_windows() {
            if c == class && !before.contains(&(h.0 as isize)) {
                return Some((h, pid));
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    None
}

fn pids_loading_dll() -> Vec<u32> {
    let out = Command::new("tasklist").args(["/m", "twime_tsf-*", "/fo", "csv", "/nh"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    out.lines().filter_map(|l| l.split("\",\"").nth(1).and_then(|p| p.trim_matches('"').parse().ok())).collect()
}

fn describe(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return format!("pid {pid}: cannot open") };
        let name = twime_winpipe::image_path_of_pid(pid).unwrap_or_default();
        let mut tok = Default::default();
        let mut s = format!("pid {pid} {name}");
        if OpenProcessToken(h, TOKEN_QUERY, &mut tok).is_ok() {
            let mut len = 0u32;
            let _ = GetTokenInformation(tok, TokenIntegrityLevel, None, 0, &mut len);
            let mut b = vec![0u8; len as usize + 8];
            if GetTokenInformation(tok, TokenIntegrityLevel, Some(b.as_mut_ptr() as _), b.len() as u32, &mut len).is_ok() {
                let l = &*(b.as_ptr() as *const TOKEN_MANDATORY_LABEL);
                let sid = l.Label.Sid;
                let cnt = *windows::Win32::Security::GetSidSubAuthorityCount(sid) as u32;
                let rid = *windows::Win32::Security::GetSidSubAuthority(sid, cnt - 1);
                s += &format!(" integrity=0x{rid:x}");
            }
            let mut ac = 0u32;
            let mut l2 = 0u32;
            if GetTokenInformation(tok, TokenIsAppContainer, Some(&mut ac as *mut _ as _), 4, &mut l2).is_ok() {
                s += &format!(" appcontainer={ac}");
            }
            let _ = CloseHandle(tok);
        }
        let _ = CloseHandle(h);
        s
    }
}

// ---- reading text through UI Automation -----------------------------------------------------------------------------------

fn read_text(ua: &IUIAutomation, hwnd: HWND) -> Option<String> {
    unsafe {
        let root = ua.ElementFromHandle(hwnd).ok()?;
        let mut first: Option<String> = None;
        for ct in [UIA_DocumentControlTypeId.0, UIA_EditControlTypeId.0] {
            let cond = ua.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(ct)).ok()?;
            let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) else { continue };
            for i in 0..all.Length().unwrap_or(0) {
                let Ok(el) = all.GetElement(i) else { continue };
                let Some(t) = element_text(&el) else { continue };
                // The element that has the keyboard focus is the one being typed into (not the address bar).
                if el.CurrentHasKeyboardFocus().map(|b| b.as_bool()).unwrap_or(false) {
                    return Some(t);
                }
                first.get_or_insert(t);
            }
        }
        first
    }
}

fn element_text(el: &IUIAutomationElement) -> Option<String> {
    unsafe {
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            && let Ok(v) = p.CurrentValue()
        {
            return Some(v.to_string());
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            && let Ok(r) = p.DocumentRange()
            && let Ok(t) = r.GetText(-1)
        {
            return Some(t.to_string().trim_end_matches(['\r', '\n']).to_string());
        }
        None
    }
}

// ---- profile switching --------------------------------------------------------------------------------------------------------

struct Switched {
    mgr: ITfInputProcessorProfileMgr,
    old: TF_INPUTPROCESSORPROFILE,
}

fn switch_to_ours() -> Result<Switched, String> {
    unsafe {
        let mgr: ITfInputProcessorProfileMgr = CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        let mut old = TF_INPUTPROCESSORPROFILE::default();
        mgr.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut old).map_err(|e| format!("GetActiveProfile: {e}"))?;
        println!("INFO\tprevious profile: type {} lang {:#06x} clsid {:?}", old.dwProfileType, old.langid, old.clsid);
        mgr.ActivateProfile(1, ids::LANGID_ZH_TW, &clsid(), &profile(), HKL::default(), FORSESSION).map_err(|e| format!("ActivateProfile: {e}"))?;
        Ok(Switched { mgr, old })
    }
}

impl Switched {
    fn restore(&self) {
        unsafe {
            let o = &self.old;
            let r = self.mgr.ActivateProfile(o.dwProfileType, o.langid, &o.clsid, &o.guidProfile, o.hkl, FORSESSION);
            println!("INFO\trestored previous profile: {r:?}");
        }
    }
}

// ---- scenarios ------------------------------------------------------------------------------------------------------------------

const SENTENCE: &str = "ㄐㄧㄣ ㄊㄧㄢ ㄊㄧㄢ ㄑㄧˋㄏㄣˇㄏㄠˇ";

fn run_checks(ua: &IUIAutomation, name: &str, hwnd: HWND, r: &mut Report, engine: &std::sync::Arc<twime_core::Engine>, facts: &mut ShiftFacts) {
    let mut d = Driver { target: hwnd, aborted: false };
    let old_timeout = fg_lock_timeout(Some(0));
    let mut got = false;
    for _ in 0..6 {
        wait_user_idle(10);
        if grab_foreground(hwnd) {
            got = true;
            break;
        }
    }
    if !got {
        r.check(&format!("{name}: window in the foreground"), false, || "could not take the foreground".into());
        fg_lock_timeout(Some(old_timeout));
        return;
    }
    std::thread::sleep(Duration::from_millis(800));
    if name == "Edge-unused" {
        focus_edge_textarea(ua, hwnd);
    }
    // warm-up: the first key may race the engine start
    d.typ("ㄅ");
    d.press(Key::Esc);
    std::thread::sleep(Duration::from_secs(2));
    let loaded = pids_loading_dll();
    r.check(&format!("{name}: the input method DLL is loaded in the host process"), !loaded.is_empty(), || "no process has twime_tsf.dll".into());
    for p in &loaded {
        println!("INFO\t{name}: loaded in {}", describe(*p));
    }
    // typing
    d.typ(SENTENCE);
    d.press(Key::Enter);
    std::thread::sleep(Duration::from_millis(500));
    let t = read_text(ua, hwnd).unwrap_or_default();
    r.check(&format!("{name}: type, convert and commit with Enter"), t.trim() == "今天天氣很好", || format!("text {t:?}{}", if d.aborted { " (aborted: foreground lost)" } else { "" }));
    // Backspace and Esc
    d.typ("ㄐㄧㄣ ㄊㄧㄢ ");
    d.press(Key::Backspace);
    d.press(Key::Enter);
    std::thread::sleep(Duration::from_millis(400));
    let t2 = read_text(ua, hwnd).unwrap_or_default();
    let added: String = t2.chars().skip(t.chars().count()).collect();
    r.check(&format!("{name}: Backspace removes the last syllable"), added.chars().count() == 1, || format!("added {added:?}"));
    d.typ("ㄐㄧㄣ ㄊㄧㄢ ");
    d.press(Key::Esc);
    std::thread::sleep(Duration::from_millis(300));
    let t3 = read_text(ua, hwnd).unwrap_or_default();
    r.check(&format!("{name}: Esc cancels the composition"), t3 == t2, || format!("{t3:?} vs {t2:?}"));
    // candidates
    d.typ("ㄏㄠˇ");
    d.press(Key::Down);
    std::thread::sleep(Duration::from_millis(400));
    let cand = unsafe { FindWindowW(w!("TwImeCandidate"), PCWSTR::null()).ok() };
    r.check(&format!("{name}: candidate window opens"), cand.is_some(), || "no TwImeCandidate window".into());
    if let Some(c) = cand {
        let mut rc = windows::Win32::Foundation::RECT::default();
        unsafe {
            let _ = GetWindowRect(c, &mut rc);
            let mut hr = windows::Win32::Foundation::RECT::default();
            let _ = GetWindowRect(hwnd, &mut hr);
            println!("INFO\t{name}: candidate window {:?} host window {:?}", (rc.left, rc.top, rc.right, rc.bottom), (hr.left, hr.top, hr.right, hr.bottom));
            r.check(&format!("{name}: candidate window is inside the host window area"), rc.left >= hr.left - 50 && rc.top >= hr.top - 50 && rc.top <= hr.bottom, || "outside".into());
            r.check(&format!("{name}: candidate window did not take the foreground"), GetForegroundWindow() == hwnd, || "foreground changed".into());
        }
    }
    d.press(Key::Digit(2));
    std::thread::sleep(Duration::from_millis(300));
    d.press(Key::Enter);
    std::thread::sleep(Duration::from_millis(400));
    let t4 = read_text(ua, hwnd).unwrap_or_default();
    let added4: String = t4.chars().skip(t3.chars().count()).collect();
    println!("INFO\t{name}: after picking candidate 2 the committed character count is {}", added4.chars().count());
    r.check(&format!("{name}: picking a candidate commits exactly one character"), added4.chars().count() == 1 && cand.is_some(), || format!("added {added4:?}"));
    if !d.aborted {
        run_ux(ua, name, hwnd, &mut d, r, engine, facts);
    }
    fg_lock_timeout(Some(old_timeout));
    if d.aborted {
        println!("ABORTED\tforeground lost during {name}");
        r.fail += 1;
    }
}

// ---- UX1 / N3b / N3 scenarios in a real host ------------------------------------------------------------------------------------------

fn text_now(ua: &IUIAutomation, hwnd: HWND) -> String {
    read_text(ua, hwnd).unwrap_or_default().trim_end_matches(['\r', '\n']).to_string()
}

fn caps_on() -> bool {
    // GetKeyState in this process is stale for other threads' input; ask a fresh process.
    Command::new("powershell").args(["-NoProfile", "-Command", "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Control]::IsKeyLocked('CapsLock')"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "True").unwrap_or(false)
}

/// Puts the user's Caps Lock state back (no foreground check: it only undoes our own toggle).
fn caps_toggle() {
    let mk = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(VK_CAPITAL), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, time: 0, dwExtraInfo: 0 } },
    };
    unsafe {
        SendInput(&[mk(false), mk(true)], size_of::<INPUT>() as i32);
    }
}

fn select_all_delete(d: &mut Driver) {
    d.raw(&[(VK_CONTROL, false), (0x41, false), (0x41, true), (VK_CONTROL, true)]);
    std::thread::sleep(Duration::from_millis(150));
    d.press(Key::Delete);
    std::thread::sleep(Duration::from_millis(250));
}

/// Empty text field, no composition, Chinese mode. False if that could not be reached.
fn reset(d: &mut Driver, ua: &IUIAutomation, hwnd: HWND) -> bool {
    for _ in 0..3 {
        d.press(Key::Esc);
        select_all_delete(d);
        if d.aborted || text_now(ua, hwnd).is_empty() {
            break;
        }
    }
    // Which mode is it in? A plain letter shows as "a" in English mode and as a composition in Chinese mode.
    d.press(Key::Letter(b'a'));
    std::thread::sleep(Duration::from_millis(250));
    let english = text_now(ua, hwnd) == "a";
    d.press(Key::Esc);
    if english {
        d.tap_shift();
    }
    select_all_delete(d);
    !d.aborted && text_now(ua, hwnd).is_empty()
}

/// A small window of our own on its own thread with a message loop (without one Windows soon marks it as hung and a
/// "Ghost" window takes over). Closed with WM_CLOSE.
fn own_window() -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DispatchMessageW, GetMessageW, MSG, TranslateMessage, WINDOW_EX_STYLE, WS_POPUP, WS_VISIBLE};
    let (tx, rx) = std::sync::mpsc::channel::<isize>();
    std::thread::spawn(move || unsafe {
        let h = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("twime-focus-away"), WS_POPUP | WS_VISIBLE, 200, 200, 300, 120, None, None, None, None).unwrap_or_default();
        let _ = tx.send(h.0 as isize);
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    });
    HWND(rx.recv().unwrap_or(0) as _)
}

/// Edge opens with the address bar focused; put the keyboard focus into the text area of the page.
fn focus_edge_textarea(ua: &IUIAutomation, hwnd: HWND) {
    unsafe {
        let Ok(root) = ua.ElementFromHandle(hwnd) else { return };
        let Ok(cond) = ua.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_DocumentControlTypeId.0)) else { return };
        let Ok(doc) = root.FindFirst(TreeScope_Descendants, &cond) else { return };
        let _ = doc.SetFocus();
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(c2) = ua.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_EditControlTypeId.0))
            && let Ok(ed) = doc.FindFirst(TreeScope_Descendants, &c2)
        {
            let _ = ed.SetFocus();
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// What the Shift experiments showed (plan UX-EN-PUNC risk 3: does every host send the Shift release?).
#[derive(Default)]
struct ShiftFacts {
    release_delivered: Option<bool>,
}

fn find_cand() -> Option<HWND> {
    unsafe { FindWindowW(w!("TwImeCandidate"), PCWSTR::null()).ok() }.filter(|h| unsafe { IsWindowVisible(*h).as_bool() })
}

fn run_ux(ua: &IUIAutomation, name: &str, hwnd: HWND, d: &mut Driver, r: &mut Report, engine: &std::sync::Arc<twime_core::Engine>, facts: &mut ShiftFacts) {
    use twime_proto::{KeyEvent, Update};
    let lost = |d: &Driver| if d.aborted { " (aborted: foreground lost)" } else { "" }.to_string();
    let settle = || std::thread::sleep(Duration::from_millis(300));
    macro_rules! chk {
        ($what:expr, $ok:expr, $detail:expr) => {
            r.check(&format!("{name}: {}", $what), $ok, || $detail)
        };
    }

    // N3: leaving the field with a convertible composition commits it once.
    if reset(d, ua, hwnd) {
        d.typ("ㄐㄧㄣ ㄊㄧㄢ ");
        let helper = own_window();
        // Plain SetForegroundWindow (no AttachThreadInput: attaching shares the input state with the host thread and hides the focus change).
        unsafe {
            let _ = SetForegroundWindow(helper);
        }
        std::thread::sleep(Duration::from_millis(300));
        let got = unsafe { GetForegroundWindow() } == helper;
        std::thread::sleep(Duration::from_millis(500));
        let t = text_now(ua, hwnd);
        unsafe {
            let _ = PostMessageW(Some(helper), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let back = grab_foreground(hwnd);
        std::thread::sleep(Duration::from_millis(300));
        let t2 = text_now(ua, hwnd);
        chk!("N3 focus leaves with a convertible composition: committed once", got && t.chars().count() == 2 && t2 == t && back, format!("left {got} text {t:?} after return {t2:?} back {back}"));
    } else {
        chk!("N3 focus-away scenario: could not reach an empty field", false, lost(d));
    }

    // ---- UX1 ----
    if reset(d, ua, hwnd) {
        d.shift_press(Key::Comma);
        d.shift_press(Key::Period);
        d.shift_press(Key::Slash);
        d.shift_press(Key::Digit(1));
        d.press(Key::Quote);
        d.press(Key::LBracket);
        d.press(Key::RBracket);
        d.shift_press(Key::Digit(9));
        d.shift_press(Key::Digit(0));
        d.shift_press(Key::Digit(6));
        d.shift_press(Key::Minus);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U1/U2 Shift+symbol keys give full-width marks at once", t == "，。？！、「」（）……——", format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        d.typ("ㄐㄧㄣ ㄊㄧㄢ ");
        d.shift_press(Key::Period);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U3 convertible composition + Shift+. is one commit ending in the full-width period", t.chars().count() == 3 && t.ends_with('。'), format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        d.press(Key::NumpadDigit(3));
        d.press(Key::NumpadDecimal);
        d.press(Key::NumpadDigit(5));
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U10 number pad 3 . 5 gives half-width 3.5", t == "3.5", format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        d.shift_press(Key::Letter(b'y'));
        d.english("ou");
        d.shift_press(Key::Letter(b't'));
        d.english("ube");
        d.press(Key::Space);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U6 Shift+Y ... Space: English run YouTube, no blank", t == "YouTube", format!("{t:?}{}", lost(d)));
        d.typ("ㄐㄧㄣ ");
        settle();
        let t2 = text_now(ua, hwnd);
        chk!("UX1 U6 the next bopomofo key composes in Chinese again", t2.chars().count() == 8 && t2.starts_with("YouTube") && !t2.is_ascii(), format!("{t2:?}"));
        d.press(Key::Esc);
    }
    if reset(d, ua, hwnd) {
        d.english("Node.js");
        d.press(Key::Enter);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U8 Node.js stays one run; Enter commits it", t == "Node.js", format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        d.english("You");
        d.shift_press(Key::Comma);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U7 a full-width mark ends the run (both committed)", t == "You，", format!("{t:?}{}", lost(d)));
    }
    // Shift tap: the Shift release is what completes the gesture.
    if reset(d, ua, hwnd) {
        d.tap_shift();
        d.press(Key::Letter(b'a'));
        d.press(Key::Letter(b'b'));
        d.press(Key::Letter(b'c'));
        settle();
        let t = text_now(ua, hwnd);
        let toggled = t == "abc";
        facts.release_delivered = Some(toggled);
        chk!("UX1 a Shift tap switches to English (the host sends the Shift release)", toggled, format!("{t:?}{}", lost(d)));
        d.shift_press(Key::Comma);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 in English mode Shift+, is the host's own less-than sign", t == "abc<", format!("{t:?}"));
        d.tap_shift();
        d.typ("ㄐㄧㄣ ");
        d.press(Key::Enter);
        settle();
        let t = text_now(ua, hwnd);
        let back = t.chars().count() == 5 && t.starts_with("abc<") && !t.is_ascii();
        chk!("UX1 a second tap returns to Chinese", back, format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        d.typ("ㄐㄧㄣ ㄊㄧㄢ ");
        d.tap_shift();
        d.press(Key::Letter(b'a'));
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U12 a tap commits a convertible composition, then English", t.chars().count() == 3 && t.ends_with('a'), format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        for k in [b'h', b'e', b'l', b'l', b'o'] {
            d.press(Key::Letter(k));
        }
        d.tap_shift();
        d.press(Key::Letter(b'x'));
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U13 a tap with an unconvertible composition gives the typed letters, then English", t == "hellox", format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        d.hold_shift(700);
        d.typ("ㄐㄧㄣ ");
        d.press(Key::Enter);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 a long Shift press does not toggle", t.chars().count() == 1 && !t.is_ascii(), format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        for k in [b'h', b'e', b'l', b'l', b'o'] {
            d.press(Key::Letter(k));
        }
        d.raw(&[(VK_SHIFT, false), (0x0D, false), (0x0D, true), (VK_SHIFT, true)]);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX1 U14 Shift+Enter turns keys typed as bopomofo into letters", t == "hello", format!("{t:?}{}", lost(d)));
    }
    if reset(d, ua, hwnd) {
        if caps_on() {
            println!("INFO\t{name}: Caps Lock is on already; the Caps Lock scenario is skipped");
        } else {
            d.typ("ㄐㄧㄣ ㄊㄧㄢ ");
            d.raw(&[(VK_CAPITAL, false), (VK_CAPITAL, true)]);
            std::thread::sleep(Duration::from_millis(200));
            let on = caps_on();
            d.press(Key::Letter(b'a'));
            std::thread::sleep(Duration::from_millis(200));
            d.press(Key::Letter(b'b'));
            settle();
            let t = text_now(ua, hwnd);
            if caps_on() {
                caps_toggle();
                std::thread::sleep(Duration::from_millis(200));
            }
            chk!("UX1 U16 Caps Lock on: a letter commits the composition and passes on; idle letters pass", on && t.chars().count() == 4 && t.ends_with("AB"), format!("caps {on} {t:?}{}", lost(d)));
            chk!("UX1 Caps Lock is back to off", !caps_on(), "still on".to_string());
        }
    }
    if reset(d, ua, hwnd) {
        d.press(Key::Backquote);
        std::thread::sleep(Duration::from_millis(400));
        let open = find_cand().is_some();
        d.press(Key::Digit(4));
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX3 U24 backquote opens the symbol table; digit 4 commits the underscore (no anchor left)", open && t == "_", format!("window {open} text {t:?}{}", lost(d)));
    }
    for (dg, want) in [(1u8, "／"), (2, "±"), (3, "％")] {
        if reset(d, ua, hwnd) {
            d.press(Key::Backquote);
            std::thread::sleep(Duration::from_millis(300));
            d.press(Key::PageDown);
            std::thread::sleep(Duration::from_millis(200));
            let open = find_cand().is_some();
            d.press(Key::Digit(dg));
            settle();
            let t = text_now(ua, hwnd);
            chk!(&format!("UX3 U24 symbol table page 2, digit {dg} commits {want}"), open && t == want, format!("window {open} text {t:?}{}", lost(d)));
        }
    }
    if reset(d, ua, hwnd) {
        d.press(Key::Backquote);
        std::thread::sleep(Duration::from_millis(300));
        d.press(Key::Esc);
        settle();
        let t = text_now(ua, hwnd);
        chk!("UX3 U24 Esc cancels the symbol table without committing", t.is_empty(), format!("{t:?}{}", lost(d)));
    }

    // ---- N3b: word candidates (an in-process session with the same assets is the reference) ----
    if reset(d, ua, hwnd) {
        let mut refs = engine.new_session();
        refs.set_ux(true);
        let mut feed = |k: Key| -> Update { refs.on_key(KeyEvent::press(k)) };
        for c in "ㄑㄩㄢˊㄌㄧˋ".chars() {
            let k = key_of(c).unwrap();
            d.press(k);
            feed(k);
        }
        let shown = text_now(ua, hwnd);
        d.press(Key::Down);
        let u = feed(Key::Down);
        std::thread::sleep(Duration::from_millis(400));
        chk!("N3b Down opens the candidate window on a two-syllable composition", find_cand().is_some() && shown.chars().count() == 2, format!("window {} shown {shown:?}", find_cand().is_some()));
        if let Some(c) = u.candidates.clone() {
            let mut all: Vec<String> = c.items.clone();
            for _ in 1..(c.total as usize).div_ceil(9) {
                all.extend(feed(Key::PageDown).candidates.unwrap().items);
            }
            feed(Key::PageDown);
            let target = (1..all.len()).find(|&i| all[i].chars().count() >= 2);
            chk!("N3b the reference list contains a multi-character word to choose", target.is_some(), format!("{} items", all.len()));
            if let Some(t) = target {
                let (page, idx) = (t / 9, t % 9);
                println!("INFO\t{name}: N3b {} candidates, choosing item {} (page {})", all.len(), t + 1, page + 1);
                d.press(Key::Tab);
                feed(Key::Tab);
                std::thread::sleep(Duration::from_millis(250));
                chk!("N3b Tab pages forward and the window stays open", find_cand().is_some(), "window closed".to_string());
                d.press(Key::PageUp);
                feed(Key::PageUp);
                std::thread::sleep(Duration::from_millis(200));
                chk!("N3b PageUp pages back", find_cand().is_some(), "window closed".to_string());
                for _ in 0..page {
                    d.press(Key::PageDown);
                    feed(Key::PageDown);
                }
                std::thread::sleep(Duration::from_millis(200));
                d.press(Key::Digit(idx as u8 + 1));
                let want = feed(Key::Digit(idx as u8 + 1)).composition.as_ref().map(twime_proto::Comp::text).unwrap_or_default();
                std::thread::sleep(Duration::from_millis(300));
                let closed = find_cand().is_none();
                d.press(Key::Enter);
                settle();
                let t = text_now(ua, hwnd);
                chk!("N3b the number key chooses the word; window closes; the committed text equals the reference", closed && t == want && t == all[target.unwrap()], format!("closed {closed} text {t:?} reference {want:?}{}", lost(d)));
            }
        }
        d.press(Key::Esc);
    }
    let _ = reset(d, ua, hwnd);
}

fn kill_new(pids: &[u32], before: &HashSet<u32>) {
    for p in pids {
        if before.contains(p) {
            continue;
        }
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, *p) {
                let _ = TerminateProcess(h, 0);
                let _ = CloseHandle(h);
            }
        }
    }
}

struct HostWin {
    kind: &'static str,
    label: &'static str,
    class: &'static str,
    hwnd: HWND,
    tmp: std::path::PathBuf,
    notepad_file: Option<std::path::PathBuf>,
}

fn loaded_count(tag: &str) -> usize {
    let p = pids_loading_dll();
    println!("INFO\tprocesses with twime_tsf.dll loaded {tag}: {}", p.len());
    p.len()
}

fn open_host(kind: &'static str, before_w: &HashSet<isize>) -> Result<HostWin, String> {
    let tmp = std::env::temp_dir().join(format!("twime-edge-{}", std::process::id()));
    if kind == "notepad" {
        // Never share an instance with the user: if a Notepad is running already, do nothing.
        let running = Command::new("tasklist").args(["/fi", "imagename eq Notepad.exe", "/fo", "csv", "/nh"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("Notepad.exe")).unwrap_or(true);
        if running {
            return Err("a Notepad is already running; not touching it".into());
        }
        // Notepad restores earlier tabs, so open a brand-new empty file and check that the text is empty before typing.
        let file = std::env::temp_dir().join(format!("twime-note-{}.txt", std::process::id()));
        std::fs::write(&file, "").map_err(|e| e.to_string())?;
        Command::new("notepad.exe").arg(&file).spawn().map_err(|e| e.to_string())?;
        let (hwnd, pid) = new_window(before_w, "Notepad", Duration::from_secs(25)).ok_or("Notepad: no new window")?;
        println!("INFO\tNotepad: new window {:#x} of process {pid}", hwnd.0 as usize);
        Ok(HostWin { kind, label: "Notepad", class: "Notepad", hwnd, tmp, notepad_file: Some(file) })
    } else {
        let exe = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
        let page = "data:text/html,<title>twime</title><textarea id=t rows=6 cols=50 autofocus></textarea>";
        Command::new(exe).arg(format!("--user-data-dir={}", tmp.display())).args(["--no-first-run", "--no-default-browser-check", &format!("--app={page}")]).spawn().map_err(|e| e.to_string())?;
        let (hwnd, pid) = new_window(before_w, "Chrome_WidgetWin_1", Duration::from_secs(25)).ok_or("Edge: no new window")?;
        println!("INFO\tEdge: new window {:#x} of process {pid}", hwnd.0 as usize);
        Ok(HostWin { kind, label: "Edge", class: "Chrome_WidgetWin_1", hwnd, tmp, notepad_file: None })
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let mut r = Report { pass: 0, fail: 0 };
    let kinds: Vec<&'static str> = match a.get(1).map(String::as_str) {
        Some("wholoads") => {
            for p in pids_loading_dll() {
                println!("{}", describe(p));
            }
            return;
        }
        Some("notepad") => vec!["notepad"],
        Some("edge") => vec!["edge"],
        Some("both") => vec!["notepad", "edge"],
        _ => {
            eprintln!("usage: hostdrive notepad | edge | both | wholoads");
            std::process::exit(2);
        }
    };
    let ua: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.expect("UI Automation");
    // The reference session for the candidate scenario (same assets and defaults as the installed engine).
    let engine = {
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
        Engine::new(std::sync::Arc::new(assets), Config::default())
    };
    wait_user_idle(20);
    let before_w: HashSet<isize> = top_windows().into_iter().map(|(h, _, _)| h.0 as isize).collect();
    let before_p: HashSet<u32> = top_windows().into_iter().map(|(_, p, _)| p).collect();
    let _ = loaded_count("before anything (earlier leftovers)");
    let mut hosts: Vec<HostWin> = vec![];
    for k in &kinds {
        match open_host(k, &before_w) {
            Ok(h) => hosts.push(h),
            Err(e) => {
                println!("ABORTED\t{k}: {e}");
                r.fail += 1;
            }
        }
    }
    let mut facts_all: Vec<(&str, Option<bool>)> = vec![];
    if r.fail == 0 {
        std::thread::sleep(Duration::from_secs(3));
        match switch_to_ours() {
            Err(e) => r.check("select the input method", false, || e),
            Ok(sw) => {
                std::thread::sleep(Duration::from_secs(1));
                let _ = loaded_count("1 s after the session-wide activation (before any host was driven)");
                for h in &hosts {
                    let mut facts = ShiftFacts::default();
                    let initial = read_text(&ua, h.hwnd);
                    if h.kind == "notepad" && initial.as_deref().map(str::trim) != Some("") {
                        println!("ABORTED\tthe new Notepad window does not show an empty file ({initial:?}); typing nothing");
                        r.fail += 1;
                        continue;
                    }
                    run_checks(&ua, h.label, h.hwnd, &mut r, &engine, &mut facts);
                    facts_all.push((h.label, facts.release_delivered));
                }
                let _ = loaded_count("at the end of the checks (still activated)");
                sw.restore();
                std::thread::sleep(Duration::from_secs(3));
                let _ = loaded_count("3 s after restoring the previous profile");
            }
        }
    }
    for (host, v) in &facts_all {
        println!("INFO\tShift release event reaches the input method in {host}: {}", match v { Some(true) => "yes (a tap toggled the mode)", Some(false) => "NO (a tap did not toggle)", None => "not measured" });
    }
    // close the windows we opened (only ours), then end every process we started
    for h in &hosts {
        unsafe {
            let _ = PostMessageW(Some(h.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    std::thread::sleep(Duration::from_secs(2));
    for h in &hosts {
        let now: Vec<u32> = top_windows().into_iter().map(|(_, p, c)| (p, c)).filter(|(_, c)| c == h.class).map(|(p, _)| p).collect();
        kill_new(&now, &before_p);
        if h.kind == "edge" {
            let _ = Command::new("powershell")
                .args(["-NoProfile", "-Command", &format!("Get-CimInstance Win32_Process -Filter \"Name='msedge.exe'\" | Where-Object {{ $_.CommandLine -like '*{}*' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}", h.tmp.display())])
                .output();
            std::thread::sleep(Duration::from_secs(1));
            let _ = std::fs::remove_dir_all(&h.tmp);
        }
        if let Some(f) = &h.notepad_file {
            let _ = std::fs::remove_file(f);
        }
    }
    println!("SUMMARY\tpass={}\tfail={}", r.pass, r.fail);
    std::process::exit(i32::from(r.fail != 0));
}
