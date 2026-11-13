//! `twime-setup diag`: read-only report to tell "the system dropped to English" from "our engine is not up".
//! Reads registry values, process and service state, the TSF profile registration and the active profile, and opens
//! (then drops) one connection to the engine pipe. Writes nothing, changes no profile, restarts nothing, sends no
//! request to the engine. Output is plain text: states, numbers, GUIDs, file names; no typed text and no SIDs; the
//! per-user data directory prefix is shown as `%LOCALAPPDATA%`.
use super::*;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND},
        Security::WinTrust::{
            WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE,
            WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WinVerifyTrust,
        },
        System::{
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
            Registry::{RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW},
            RemoteDesktop::ProcessIdToSessionId,
            Services::{OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_STATUS_PROCESS},
        },
        UI::{
            Input::KeyboardAndMouse::GetKeyboardLayout,
            TextServices::{TF_INPUTPROCESSORPROFILE, TF_IPP_FLAG_ENABLED},
            WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
        },
    },
    core::w,
};

/// Services that carry the text input stack. Verified on Windows 11 24H2 (`Get-Service`): the name below exists.
const SERVICES: [&str; 1] = ["TextInputManagementService"];

fn local_appdata_masked(p: &str) -> String {
    // Longest prefix first: %LOCALAPPDATA% sits inside the profile directory. Anything else under the profile
    // (a custom install in Downloads, say) still must not show the account name.
    for (var, shown) in [("LOCALAPPDATA", "%LOCALAPPDATA%"), ("USERPROFILE", "%USERPROFILE%")] {
        if let Ok(base) = std::env::var(var)
            && !base.is_empty()
            && p.len() >= base.len()
            && p.is_char_boundary(base.len())
            && p[..base.len()].eq_ignore_ascii_case(&base)
        {
            return format!("{shown}{}", &p[base.len()..]);
        }
    }
    p.to_string()
}

fn masked(p: &Path) -> String { local_appdata_masked(&p.display().to_string()) }

fn reg_string(hive: HKEY, sub: &str, name: Option<&str>) -> Option<String> {
    let mut buf = [0u16; 520];
    let mut len = (buf.len() * 2) as u32;
    let n = name.map(wide);
    let r = unsafe {
        RegGetValueW(hive, PCWSTR(wide(sub).as_ptr()), n.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len))
    };
    if r.0 != 0 { return None; }
    let chars = (len as usize / 2).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..chars]).trim_end_matches('\0').to_string())
}

fn reg_dword(hive: HKEY, sub: &str, name: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut len = 4u32;
    let r = unsafe { RegGetValueW(hive, PCWSTR(wide(sub).as_ptr()), PCWSTR(wide(name).as_ptr()), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut len)) };
    (r.0 == 0).then_some(v)
}

/// `(pid, session)` of every process whose image name is `exe` (case-insensitive).
fn processes_named(exe: &str) -> Vec<(u32, Option<u32>)> {
    let mut out = vec![];
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let snap = SnapGuard(snap);
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap.0, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(exe) {
                let mut session = 0u32;
                out.push((e.th32ProcessID, ProcessIdToSessionId(e.th32ProcessID, &mut session).is_ok().then_some(session)));
            }
            ok = Process32NextW(snap.0, &mut e).is_ok();
        }
    }
    out
}

fn state_name(state: u32) -> &'static str {
    match state {
        1 => "stopped", 2 => "start-pending", 3 => "stop-pending", 4 => "running", 5 => "continue-pending", 6 => "pause-pending", 7 => "paused",
        _ => "unknown",
    }
}

/// `Ok((state, pid))`, or the Win32 error text.
fn service_state(name: &str) -> Result<(u32, u32), String> {
    unsafe {
        let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).map_err(|e| format!("OpenSCManager: {e}"))?;
        let svc = OpenServiceW(scm, PCWSTR(wide(name).as_ptr()), SERVICE_QUERY_STATUS);
        let result = match &svc {
            Ok(svc) => {
                let mut st = SERVICE_STATUS_PROCESS::default();
                let mut need = 0u32;
                let buf = std::slice::from_raw_parts_mut((&mut st as *mut SERVICE_STATUS_PROCESS).cast::<u8>(), size_of::<SERVICE_STATUS_PROCESS>());
                QueryServiceStatusEx(*svc, SC_STATUS_PROCESS_INFO, Some(buf), &mut need).map(|()| (st.dwCurrentState.0, st.dwProcessId)).map_err(|e| format!("QueryServiceStatus: {e}"))
            }
            Err(e) => Err(format!("OpenService: {e}")),
        };
        if let Ok(svc) = svc { let _ = windows::Win32::System::Services::CloseServiceHandle(svc); }
        let _ = windows::Win32::System::Services::CloseServiceHandle(scm);
        result
    }
}

/// Label for a `WinVerifyTrust` result (HRESULT as returned).
fn trust_label(code: i32) -> String {
    match code as u32 {
        0 => "signed, trusted".into(),
        0x800B0100 => "not signed".into(),
        0x800B0109 => "signed, root not trusted".into(),
        0x800B0101 | 0x800B0004 => "signed, certificate not valid / not trusted".into(),
        0x80096010 => "signature does not match the file".into(),
        0x800B0111 => "signature blocked by policy".into(),
        0x80092026 => "blocked by code integrity policy".into(),
        other => format!("not trusted (0x{other:08X})"),
    }
}

fn verify_file(path: &Path) -> String {
    if !path.exists() { return "file absent".into(); }
    let wpath = wide(&path.display().to_string());
    unsafe {
        let mut file = WINTRUST_FILE_INFO { cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: PCWSTR(wpath.as_ptr()), ..Default::default() };
        let mut data = WINTRUST_DATA {
            cbStruct: size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
            ..Default::default()
        };
        // WINTRUST_ACTION_GENERIC_VERIFY_V2 {00AAC56B-CD44-11d0-8CC2-00C04FC295EE}
        let mut action = GUID::from_u128(0x00AAC56B_CD44_11d0_8CC2_00C04FC295EE);
        let r = WinVerifyTrust(HWND::default(), &mut action, (&mut data as *mut WINTRUST_DATA).cast());
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(HWND::default(), &mut action, (&mut data as *mut WINTRUST_DATA).cast());
        trust_label(r)
    }
}

/// Smart App Control state from `VerifiedAndReputablePolicyState`: 0 off, 1 on (enforced), 2 evaluation.
fn sac_label(v: Option<u32>) -> String {
    match v {
        Some(0) => "off".into(),
        Some(1) => "ON (enforced): unsigned or untrusted binaries can be blocked".into(),
        Some(2) => "evaluation mode".into(),
        Some(n) => format!("unknown value {n}"),
        None => "not present (not available on this Windows)".into(),
    }
}

/// `CodeIntegrityOptions` bits from `NtQuerySystemInformation(SystemCodeIntegrityInformation)`.
fn ci_label(opts: u32) -> String {
    let mut v = vec![];
    for (bit, name) in [(0x1, "code-integrity enabled"), (0x2, "test-signing"), (0x4, "user-mode policy (WDAC/UMCI) enforced"), (0x8, "user-mode policy audit-only"), (0x400, "HVCI")] {
        if opts & bit != 0 { v.push(name); }
    }
    if v.is_empty() { "no code-integrity options set".into() } else { v.join(", ") }
}

fn code_integrity_options() -> Option<u32> {
    unsafe {
        let lib = LoadLibraryW(w!("ntdll.dll")).ok()?;
        let f = GetProcAddress(lib, windows::core::s!("NtQuerySystemInformation"))?;
        let f: unsafe extern "system" fn(u32, *mut u32, u32, *mut u32) -> i32 = std::mem::transmute(f);
        let mut info = [8u32, 0u32]; // Length, CodeIntegrityOptions
        let mut ret = 0u32;
        (f(103, info.as_mut_ptr(), 8, &mut ret) >= 0).then_some(info[1])
    }
}


/// A numeric registry value (DWORD or QWORD), `None` when absent or of another type.
fn reg_number(hive: HKEY, sub: &str, name: &str) -> Option<u64> {
    use windows::Win32::System::Registry::RRF_RT_REG_QWORD;
    let mut q = 0u64;
    let mut len = 8u32;
    let r = unsafe { RegGetValueW(hive, PCWSTR(wide(sub).as_ptr()), PCWSTR(wide(name).as_ptr()), RRF_RT_REG_QWORD, None, Some((&mut q as *mut u64).cast()), Some(&mut len)) };
    if r.0 == 0 { Some(q) } else { reg_dword(hive, sub, name).map(u64::from) }
}

/// Builds named in the reports on the touch keyboard / first-character problem (Windows 11 22H2/23H2 with the 2025-01
/// update, 24H2/25H2, LTSC 2024). Only says whether the build is in the family; it does not say the problem exists.
fn build_hint(build: Option<u32>) -> &'static str {
    match build {
        Some(22621 | 22631) => "build in the reported family (22H2/23H2); reports named 2025-01 and later updates",
        Some(26100 | 26200) => "build in the reported family (24H2/25H2/LTSC 2024)",
        Some(b) if b > 26200 => "newer than the builds in the reports; unknown",
        Some(_) => "build not named in the reports",
        None => "build unknown",
    }
}
fn guid_hex(g: &GUID) -> String { format!("{g:?}") }

fn row(label: &str, value: impl std::fmt::Display) { println!("{label:<28}{value}"); }

pub(super) fn run() -> Result<(), String> {
    println!("地瓜輸入法 TwPotato diag（唯讀；不含輸入內容）");
    // -- input stack of the system
    println!("[系統輸入堆疊]");
    let ctf = processes_named("ctfmon.exe");
    let mine = twime_winpipe::current_session_id();
    let in_session: Vec<u32> = ctf.iter().filter(|(_, s)| *s == Some(mine)).map(|(p, _)| *p).collect();
    row("ctfmon.exe", if in_session.is_empty() { "NOT running in this session".to_string() } else { format!("running, pid {:?} (this session)", in_session) });
    if ctf.len() != in_session.len() { row("ctfmon other sessions", ctf.len() - in_session.len()); }
    for name in SERVICES {
        row(name, match service_state(name) { Ok((st, pid)) => format!("{} (pid {pid})", state_name(st)), Err(e) => e });
    }
    unsafe {
        let fg = GetForegroundWindow();
        let tid = GetWindowThreadProcessId(fg, None);
        if tid != 0 { row("HKL (foreground thread)", format!("0x{:08X}", GetKeyboardLayout(tid).0 as usize as u32)); }
        else { row("HKL (foreground thread)", "no foreground window"); }
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        match CoCreateInstance::<_, ITfInputProcessorProfileMgr>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
            Ok(mgr) => {
                let mut p = TF_INPUTPROCESSORPROFILE::default();
                // The wrapper turns S_FALSE into Ok; keep the raw HRESULT (S_FALSE = no active profile).
                use windows::core::Interface as _;
                let hr = (mgr.vtable().GetActiveProfile)(mgr.as_raw(), &GUID_TFCAT_TIP_KEYBOARD, &mut p);
                if hr.0 == 1 {
                    row("active profile (diag proc)", "none (S_FALSE)");
                } else if hr.is_err() {
                    row("active profile (diag proc)", format!("GetActiveProfile failed: 0x{:08X}", hr.0 as u32));
                } else {
                    row("active profile (diag proc)", format!("type={} langid=0x{:04X} clsid={} profile={} hkl=0x{:08X} ours={}", p.dwProfileType, p.langid, guid_hex(&p.clsid), guid_hex(&p.guidProfile), p.hkl.0 as usize as u32, p.clsid == clsid()));
                }
                let mut p = TF_INPUTPROCESSORPROFILE::default();
                match mgr.GetProfile(1, ids::LANGID_ZH_TW as u16, &clsid(), &profile(), HKL::default(), &mut p) {
                    Ok(()) => row("our TIP profile", format!("registered, {}", if p.dwFlags & TF_IPP_FLAG_ENABLED != 0 { "enabled for this user" } else { "NOT enabled for this user" })),
                    Err(e) => row("our TIP profile", format!("not registered or not readable: {e}")),
                }
            }
            Err(e) => row("TSF profile manager", format!("cannot create: {e}")),
        }
    }
    // -- our install
    println!("[安裝]");
    let user = reg_exists_in(HKEY_CURRENT_USER, &clsid_key());
    let machine = reg_exists_in(HKEY_LOCAL_MACHINE, &clsid_key());
    row("install mode", match (user, machine) { (true, false) => "per-user (HKCU)", (false, true) => "per-machine (HKLM)", (true, true) => "BOTH HKCU and HKLM (conflict)", (false, false) => "not registered" });
    let inproc = format!(r"{}\InprocServer32", clsid_key());
    let dll = [(HKEY_CURRENT_USER, user), (HKEY_LOCAL_MACHINE, machine)].iter()
        .find_map(|(h, present)| present.then(|| reg_string(*h, &inproc, None)).flatten()).map(PathBuf::from);
    let dir = dll.as_ref().and_then(|d| d.parent().map(Path::to_path_buf)).unwrap_or_else(root);
    match &dll {
        Some(d) => {
            row("TIP DLL", masked(d));
            row("TIP DLL signature", verify_file(d));
        }
        None => row("TIP DLL", "no COM registration"),
    }
    let exe = dir.join(ids::ENGINE_NAME);
    row("engine exe", format!("{} {}", masked(&exe), if exe.exists() { "present" } else { "absent" }));
    row("engine exe signature", verify_file(&exe));
    row("uninstall marker", if dir.join(ids::UNINSTALL_MARKER).exists() { "PRESENT (TIP passes every key through)" } else { "absent" });
    row("trace flag (trace.on)", if twime_winpipe::state::data_dir().join("trace.on").is_file() { "present (TIP trace on)" } else { "absent (TIP trace off)" });
    // -- engine
    println!("[引擎]");
    let engines = processes_named(ids::ENGINE_NAME);
    row("engine processes", engines.iter().filter(|(_, s)| *s == Some(mine)).count());
    let pipe = engine_pipe(false)?;
    let expect = exe.to_string_lossy().into_owned();
    row("engine pipe", match twime_winpipe::Client::connect(&pipe, Some(&expect), 0) {
        Ok(_) => "connectable (one connection opened and dropped, no request sent)".to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "no pipe: engine not running or still loading (the pipe appears after the model is loaded)".to_string(),
        Err(e) => format!("exists but connect failed: {:?} (os error {})", e.kind(), e.raw_os_error().unwrap_or(0)),
    });
    // -- Windows 11 touch keyboard family (IME-FAILURE-MODES 13.3): read only, no conclusion drawn
    println!("[Windows 版本與觸控鍵盤]");
    let cv = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = reg_string(HKEY_LOCAL_MACHINE, cv, Some("CurrentBuildNumber")).unwrap_or_default();
    let ubr = reg_dword(HKEY_LOCAL_MACHINE, cv, "UBR");
    row("Windows build", format!("{build}.{} ({})", ubr.map_or("?".into(), |u| u.to_string()), reg_string(HKEY_LOCAL_MACHINE, cv, Some("DisplayVersion")).unwrap_or_default()));
    row("touch keyboard reports", build_hint(build.parse().ok()));
    let tih = processes_named("TextInputHost.exe");
    let tih: Vec<u32> = tih.iter().filter(|(_, s)| *s == Some(mine)).map(|(p, _)| *p).collect();
    row("TextInputHost.exe", if tih.is_empty() { "NOT running in this session".to_string() } else { format!("running, pid {tih:?}") });
    let common = r"Software\Microsoft\InputMethod\Settings\Common";
    for name in ["TouchKeyboardHasEverShown", "InputPanelPageLastOpenTime"] {
        row(name, reg_number(HKEY_CURRENT_USER, common, name).map_or("absent".to_string(), |v| v.to_string()));
    }
    // -- code integrity
    println!("[程式碼完整性]");
    row("Smart App Control", sac_label(reg_dword(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\CI\Policy", "VerifiedAndReputablePolicyState")));
    row("code integrity", code_integrity_options().map_or("cannot query".to_string(), ci_label));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_cover_the_known_values_and_never_panic_on_others() {
        assert_eq!(sac_label(Some(0)), "off");
        assert!(sac_label(Some(1)).starts_with("ON"));
        assert!(sac_label(None).contains("not present"));
        assert!(sac_label(Some(9)).contains('9'));
        assert_eq!(state_name(4), "running");
        assert_eq!(state_name(99), "unknown");
        assert_eq!(trust_label(0), "signed, trusted");
        assert_eq!(trust_label(0x800B0100u32 as i32), "not signed");
        assert!(trust_label(0x12345678).contains("12345678"));
        assert_eq!(ci_label(0), "no code-integrity options set");
        assert!(ci_label(0x5).contains("enforced"));
    }

    #[test]
    fn local_appdata_prefix_is_masked_case_insensitively() {
        let base = std::env::var("LOCALAPPDATA").unwrap();
        assert_eq!(local_appdata_masked(&format!("{}\\Programs\\TwPotato", base.to_ascii_uppercase())), "%LOCALAPPDATA%\\Programs\\TwPotato");
        assert_eq!(local_appdata_masked(r"C:\Program Files\X"), r"C:\Program Files\X");
        let profile = std::env::var("USERPROFILE").unwrap();
        assert_eq!(local_appdata_masked(&format!("{profile}\\Downloads\\x.dll")), "%USERPROFILE%\\Downloads\\x.dll");
    }

    #[test]
    fn running_the_report_writes_nothing_and_does_not_fail() {
        assert!(run().is_ok());
    }
}

struct SnapGuard(HANDLE);
impl Drop for SnapGuard {
    fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } }
}
