//! MSI Binary-table helpers. No user registry, environment paths, or input-method enabling.
use super::*;
use std::time::{Duration, Instant};
use windows::{core::{HRESULT, PWSTR}, Win32::{
    Foundation::{GetLastError, HANDLE, ERROR_NO_MORE_FILES, WAIT_EVENT, WAIT_FAILED, WAIT_TIMEOUT},
    Security::{GetTokenInformation, GetSidSubAuthority, GetSidSubAuthorityCount, TokenIntegrityLevel,
        TokenElevation, TokenElevationType, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_ELEVATION,
        TOKEN_ELEVATION_TYPE, TOKEN_QUERY},
    System::{Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        Threading::{GetCurrentProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_NAME_WIN32}},
}};

struct Handle(HANDLE);
impl Drop for Handle { fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } } }

fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>, String> {
    unsafe {
        let mut len = 0;
        let _ = GetTokenInformation(token, class, None, 0, &mut len);
        let mut buf = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
        // Fixed-size classes (TokenElevation, TokenElevationType) reject any length other than the exact one.
        GetTokenInformation(token, class, Some(buf.as_mut_ptr().cast()), len, &mut len)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }
}

fn integrity_name(rid: u32) -> &'static str {
    match rid { 0..0x2000 => "low", 0x2000..0x2100 => "medium", 0x2100..0x3000 => "medium-plus", 0x3000..0x4000 => "high", _ => "system" }
}

/// One line for the migration log: user SID, logon SID, session and integrity level of this process.
pub(super) fn identity_line() -> String {
    let id = twime_winpipe::current_identity().map(|id| (id.user, id.logon)).map_err(|e| e.to_string());
    let integrity = unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|e| e.to_string()).and_then(|()| {
            let token = Handle(token);
            let label = token_info(token.0, TokenIntegrityLevel)?;
            let sid = (*(label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>())).Label.Sid;
            Ok(*GetSidSubAuthority(sid, u32::from(*GetSidSubAuthorityCount(sid)) - 1))
        })
    };
    format!("identity={id:?} session={} integrity={}", twime_winpipe::current_session_id(),
        integrity.map(|rid| format!("{rid} ({})", integrity_name(rid))).unwrap_or_else(|e| e))
}

pub(super) fn ident() -> Result<(), String> {
    let id = twime_winpipe::current_identity().map_err(|e| e.to_string())?;
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|e| e.to_string())?;
        let token = Handle(token);
        let label = token_info(token.0, TokenIntegrityLevel)?;
        let sid = (*(label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>())).Label.Sid;
        let rid = *GetSidSubAuthority(sid, u32::from(*GetSidSubAuthorityCount(sid)) - 1);
        let elevation = token_info(token.0, TokenElevation)?;
        let elevated = (*(elevation.as_ptr().cast::<TOKEN_ELEVATION>())).TokenIsElevated != 0;
        let kind = token_info(token.0, TokenElevationType)?;
        let kind = (*(kind.as_ptr().cast::<TOKEN_ELEVATION_TYPE>())).0;
        let name = match kind { 1 => "Default", 2 => "Full", 3 => "Limited", _ => "unknown" };
        println!("user SID              {}", id.user);
        println!("logon SID             {}", id.logon);
        println!("integrity level       {rid} ({})", integrity_name(rid));
        println!("elevated              {elevated}");
        println!("TokenElevationType    {kind} ({name})");
    }
    Ok(())
}

fn absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || value.contains('\0') { return Err(format!("expected absolute path: {value}")); }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {},
            std::path::Component::ParentDir => { normalized.pop(); },
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

#[derive(Debug, PartialEq, Eq)]
enum Rollback { Register(PathBuf), Unregister }

fn rollback_action(present: &str, old: &str, dll: &str) -> Result<Rollback, String> {
    let fallback = absolute_path(dll)?;
    match present {
        "0" => Ok(Rollback::Unregister),
        "1" => Ok(Rollback::Register(if old.is_empty() { fallback } else { absolute_path(old)? })),
        _ => Err("--tip-present must be 0 or 1".into()),
    }
}

// The injected callbacks also let tests verify partial failures without calling TSF.
fn register_with(fail_after: Option<usize>, mut profile: impl FnMut() -> Result<(), String>,
    mut category: impl FnMut(GUID) -> Result<(), String>) -> Result<(), String> {
    profile()?;
    for (n, c) in categories().into_iter().enumerate() {
        if fail_after == Some(n) { return Err(format!("test failure after {n} categories")); }
        category(c)?;
    }
    if fail_after == Some(categories().len()) { return Err("test failure after all categories".into()); }
    Ok(())
}

fn register(dll: &Path, fail_after: Option<usize>) -> Result<(), String> {
    // RegisterProfile / RegisterCategory are machine registration APIs; no HKCU helpers or InstallLayoutOrTip.
    register_with(fail_after, || register_profile(ids::DESCRIPTION, &dll.to_string_lossy(), 0), |c| unsafe {
        let cat: ITfCategoryMgr = CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        cat.RegisterCategory(&clsid(), &c, &clsid()).map_err(|e| format!("RegisterCategory: {e}"))
    })
}

// Pure, lazy selection keeps each registry check immediately before its API call.
// Some(category) denotes a category; None denotes the language profile.
fn unregister_items<T>(items: impl Iterator<Item = (T, Result<bool, String>)>)
    -> impl Iterator<Item = Result<T, String>> {
    items.filter_map(|(item, present)| match present {
        Ok(true) => Some(Ok(item)), Ok(false) => None, Err(e) => Some(Err(e)),
    })
}

fn unregister_with(mut category: impl FnMut(GUID) -> windows::core::Result<()>,
    mut profile: impl FnMut() -> windows::core::Result<()>, mut delete: impl FnMut() -> Result<(), String>,
    mut tip_present: impl FnMut() -> Result<bool, String>,
    mut item_present: impl FnMut(Option<GUID>) -> Result<bool, String>) -> Result<(), String> {
    if !tip_present()? { return Ok(()); }
    let mut errors = vec![];
    let items = categories().into_iter().map(Some).chain(std::iter::once(None));
    for item in unregister_items(items.map(|item| (item, item_present(item)))) {
        match item {
            Ok(Some(c)) => if let Err(e) = category(c) { errors.push(format!("UnregisterCategory: {e}")); },
            Ok(None) => if let Err(e) = profile() { errors.push(format!("UnregisterProfile: {e}")); },
            Err(e) => errors.push(e),
        }
    }
    if let Err(e) = delete() { errors.push(e); }
    if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
}

fn unregister() -> Result<(), String> {
    unregister_with(|c| unsafe {
        com_init();
        let cat: ITfCategoryMgr = CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        cat.UnregisterCategory(&clsid(), &c, &clsid())
    }, || unsafe {
        com_init();
        let mgr: ITfInputProcessorProfileMgr = CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        mgr.UnregisterProfile(&clsid(), ids::LANGID_ZH_TW, &profile(), 0)
    }, || {
        let path = tip_path(HKLM_TIP_PREFIX);
        // The checked delete_guarded variant propagates access errors and tolerates an absent tree.
        delete_guarded_in(HKEY_LOCAL_MACHINE, &path, &[tip_path(HKLM_TIP_PREFIX)], true).map(|_| ())
    }, || {
        reg_exists_checked(HKEY_LOCAL_MACHINE, &tip_path(HKLM_TIP_PREFIX))
    }, |item| {
        let tip = tip_path(HKLM_TIP_PREFIX);
        let path = match item {
            Some(c) => format!(r"{tip}\Category\Item\{}\{{{c:?}}}", ids::guid_string(ids::CLSID_TIP)),
            None => format!(r"{tip}\LanguageProfile\0x00000404\{}", ids::guid_string(ids::PROFILE)),
        };
        reg_exists_checked(HKEY_LOCAL_MACHINE, &path)
    })
}

fn mark(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("create {}: {e}", root.display()))?;
    fs::write(root.join(ids::UNINSTALL_MARKER), b"1\n").map_err(|e| format!("write marker: {e}"))
}

fn release(root: &Path) -> Result<(), String> {
    match fs::remove_file(root.join(ids::UNINSTALL_MARKER)) {
        Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(e) => return Err(format!("remove marker: {e}")),
    }
    match fs::read_dir(root) {
        Ok(mut entries) => if entries.next().transpose().map_err(|e| e.to_string())?.is_none() {
            fs::remove_dir(root).map_err(|e| format!("remove empty root: {e}"))?;
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(format!("inspect root: {e}")),
    }
    Ok(())
}

struct Process { pid: u32, name: String, image: Result<PathBuf, String> }

fn image_key(path: &Path) -> String {
    let spelling = legacy::path_key(path);
    let mut normalized = PathBuf::new();
    for component in Path::new(&spelling).components() {
        match component {
            std::path::Component::ParentDir => { normalized.pop(); },
            std::path::Component::CurDir => {},
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized.to_string_lossy().into_owned()
}

fn remaining_stop_time(elapsed: Duration) -> Result<u32, String> {
    Duration::from_secs(10).checked_sub(elapsed).filter(|d| !d.is_zero())
        .map(|d| d.as_millis().min(10_000) as u32).filter(|ms| *ms > 0).ok_or("machine-stop total timeout".into())
}

// Capture the thread's last error immediately, before formatting or any other API call.
fn wait_result(pid: u32, outcome: WAIT_EVENT, last_error: impl FnOnce() -> u32) -> Result<bool, String> {
    match outcome {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        WAIT_FAILED => {
            let error = last_error();
            Err(format!("wait for engine PID {pid}: WaitForSingleObject failed (GetLastError={error})"))
        },
        _ => Err(format!("wait for engine PID {pid}: unexpected wait result {}", outcome.0)),
    }
}

fn stop_process_with(pid: u32, mut terminate: impl FnMut() -> Result<(), String>,
    mut wait: impl FnMut(u32) -> Result<bool, String>,
    mut elapsed: impl FnMut() -> Duration) -> Result<(), String> {
    if wait(0)? { return Ok(()); }
    remaining_stop_time(elapsed())?;
    let terminated = terminate();
    // Both callbacks capture the original handle; never reopen a potentially reused PID.
    let remaining = match remaining_stop_time(elapsed()) {
        Ok(ms) => ms,
        Err(e) => return Err(terminated.err().unwrap_or(e)),
    };
    match wait(remaining) {
        Ok(true) => Ok(()),
        Ok(false) => Err(terminated.err().unwrap_or_else(|| format!("engine PID {pid} did not exit within 10 s total"))),
        Err(e) => Err(terminated.err().map_or(e.clone(), |t| format!("{t}; {e}"))),
    }
}

/// Pure selection: matching names alone never authorize termination.
fn select_processes(root: &Path, processes: &[Process]) -> Result<Vec<u32>, String> {
    let target = image_key(&root.join(ids::ENGINE_NAME));
    let mut selected = vec![];
    for p in processes {
        match &p.image {
            Ok(image) if image_key(image) == target => selected.push(p.pid),
            Err(e) if p.name.eq_ignore_ascii_case(ids::ENGINE_NAME) => return Err(format!("cannot inspect engine PID {}: {e}", p.pid)),
            _ => {},
        }
    }
    Ok(selected)
}

fn stop(root: &Path) -> Result<(), String> {
    mark(root)?;
    let started = Instant::now();
    unsafe {
        let snapshot = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.to_string())?);
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut next = Process32FirstW(snapshot.0, &mut entry);
        let mut processes = vec![];
        let mut handles = vec![];
        loop {
            if let Err(e) = next {
                if e.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) { break; }
                return Err(format!("enumerate processes: {e}"));
            }
            let name = String::from_utf16_lossy(&entry.szExeFile[..entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len())]);
            let pid = entry.th32ProcessID;
            let image = (|| {
                let h = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid).map_err(|e| e.to_string())?);
                let mut buf = vec![0u16; 32768]; let mut len = buf.len() as u32;
                QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).map_err(|e| e.to_string())?;
                let image = PathBuf::from(String::from_utf16(&buf[..len as usize]).map_err(|e| e.to_string())?);
                handles.push((pid, h));
                Ok(image)
            })();
            processes.push(Process { pid, name, image });
            next = Process32NextW(snapshot.0, &mut entry);
        }
        let selected = select_processes(root, &processes)?;
        remaining_stop_time(started.elapsed())?;
        for pid in selected {
            let h = &handles.iter().find(|(id, _)| *id == pid).ok_or("missing process handle")?.1;
            stop_process_with(pid, || TerminateProcess(h.0, 0).map_err(|e| format!("terminate PID {pid}: {e}")),
                |ms| wait_result(pid, WaitForSingleObject(h.0, ms), || GetLastError().0), || started.elapsed())?;
        }
    }
    Ok(())
}

fn options(a: &[String], allowed: &[&str]) -> Result<(), String> {
    if (a.len() - 1) % 2 != 0 { return Err("expected option/value pairs".into()); }
    let mut seen: Vec<&str> = vec![];
    for pair in a[1..].chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str()) || seen.contains(&pair[0].as_str()) { return Err(format!("unexpected or duplicate option {}", pair[0])); }
        seen.push(pair[0].as_str());
    }
    Ok(())
}

pub(super) fn run(a: &[String]) -> Result<(), String> {
    let required = |flag| arg(a, flag).ok_or_else(|| format!("{flag} missing"));
    match a[0].as_str() {
        "machine-register" => {
            options(a, &["--dll", "--test-fail-after-categories"])?;
            let dll = absolute_path(&required("--dll")?)?;
            if !dll.is_file() { return Err(format!("DLL not found: {}", dll.display())); }
            let fail = arg(a, "--test-fail-after-categories").map(|s| s.parse::<usize>().ok().filter(|n| *n <= categories().len()).ok_or("test category count must be 0..4")).transpose()?;
            register(&dll, fail)
        },
        "machine-unregister" => { options(a, &[])?; unregister() },
        "machine-rollback-register" => {
            options(a, &["--tip-present", "--old-dll", "--dll"])?;
            match rollback_action(&required("--tip-present")?, &required("--old-dll")?, &required("--dll")?)? {
                Rollback::Register(dll) => register(&dll, None), Rollback::Unregister => unregister(),
            }
        },
        "machine-stop" | "machine-release" => {
            options(a, &["--root"])?;
            let root = absolute_path(&required("--root")?)?;
            if a[0] == "machine-stop" { stop(&root) } else { release(&root) }
        },
        _ => Err("unknown machine command".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn rollback_selects_old_fallback_or_removal_without_checking_files() {
        let dll = r"C:\private\missing.dll";
        let old = r"D:\private\old.dll";
        assert_eq!(rollback_action("1", old, dll).unwrap(), Rollback::Register(old.into()));
        assert_eq!(rollback_action("1", "", dll).unwrap(), Rollback::Register(dll.into()));
        assert_eq!(rollback_action("0", old, dll).unwrap(), Rollback::Unregister);
        for value in ["", "true", "2"] { assert!(rollback_action(value, old, dll).is_err()); }
        assert!(rollback_action("1", "relative.dll", dll).is_err());
        assert!(absolute_path("relative").is_err());
        assert!(absolute_path(r"C:relative").is_err());
    }

    #[test]
    fn machine_paths_normalize_dot_components_without_changing_case() {
        let root = absolute_path(r"C:\X\TwPotato").unwrap();
        for value in [r"C:\X\TwPotato\.", r"C:\X\.\unused\..\TwPotato"] {
            let normalized = absolute_path(value).unwrap();
            assert_eq!(normalized.as_os_str(), root.as_os_str());
            assert_eq!(normalized.join(ids::ENGINE_NAME).as_os_str(), root.join(ids::ENGINE_NAME).as_os_str());
            assert_eq!(normalized.join(ids::UNINSTALL_MARKER).as_os_str(), root.join(ids::UNINSTALL_MARKER).as_os_str());
        }
        assert_eq!(root.to_str(), Some(r"C:\X\TwPotato"));
        let dll = r"C:\X\TwPotato\twime_tsf.dll";
        let dotted = r"C:\X\TwPotato\.\unused\..\twime_tsf.dll";
        assert_eq!(absolute_path(dotted).unwrap().as_os_str(), Path::new(dll).as_os_str());
        assert_eq!(rollback_action("1", dotted, dll).unwrap(), Rollback::Register(dll.into()));
        assert_eq!(rollback_action("1", "", dotted).unwrap(), Rollback::Register(dll.into()));
    }

    #[test]
    fn registration_order_and_injection_only_use_fake_callbacks() {
        for fail in [None, Some(0), Some(1), Some(2), Some(3), Some(4)] {
            let calls = RefCell::new(vec![]);
            let result = register_with(fail, || { calls.borrow_mut().push("profile".to_string()); Ok(()) },
                |c| { calls.borrow_mut().push(format!("{c:?}")); Ok(()) });
            assert_eq!(result.is_err(), fail.is_some());
            let expected: Vec<_> = ["profile".to_string()].into_iter().chain(categories().into_iter().take(fail.unwrap_or(categories().len())).map(|c| format!("{c:?}"))).collect();
            assert_eq!(*calls.borrow(), expected);
        }
        let mut called = false;
        assert!(register_with(None, || Err("profile failed".into()), |_| { called = true; Ok(()) }).is_err());
        assert!(!called);
        let mut n = 0;
        assert!(register_with(None, || Ok(()), |_| { n += 1; Err("category failed".into()) }).is_err());
        assert_eq!(n, 1);
    }

    #[test]
    fn removal_checks_presence_before_api_calls_and_preserves_every_error() {
        // REGDB_E_CLASSNOTREG, E_NOINTERFACE, E_INVALIDARG, E_FAIL, E_ACCESSDENIED.
        for code in [0x80040154u32, 0x80004002, 0x80070057, 0x80004005, 0x80070005] {
            // Each COM creation/API step must independently fail the operation.
            for failed_step in 0..=categories().len() {
                let calls = RefCell::new(vec![]);
                let mut category_index = 0;
                let result = unregister_with(|c| {
                    assert_eq!(c, categories()[category_index]);
                    calls.borrow_mut().push("category");
                    let failed = category_index == failed_step;
                    category_index += 1;
                    if failed { Err(HRESULT(code as i32).into()) } else { Ok(()) }
                }, || {
                    calls.borrow_mut().push("profile");
                    if failed_step == categories().len() { Err(HRESULT(code as i32).into()) } else { Ok(()) }
                }, || { calls.borrow_mut().push("delete"); Ok(()) },
                    || { calls.borrow_mut().push("inspect"); Ok(true) }, |_| Ok(true));
                assert_eq!(*calls.borrow(), ["inspect", "category", "category", "category", "category", "profile", "delete"]);
                let error = result.unwrap_err();
                assert!(error.contains(if failed_step == categories().len() { "UnregisterProfile" } else { "UnregisterCategory" }));
                assert!(error.contains(&format!("{code:08X}")), "{error}");
            }
        }
        let calls = RefCell::new(vec![]);
        assert_eq!(unregister_with(|_| { calls.borrow_mut().push("category"); Ok(()) },
            || { calls.borrow_mut().push("profile"); Ok(()) },
            || { calls.borrow_mut().push("delete"); Ok(()) },
            || { calls.borrow_mut().push("inspect"); Ok(true) }, |_| Ok(true)), Ok(()));
        assert_eq!(*calls.borrow(), ["inspect", "category", "category", "category", "category", "profile", "delete"]);
    }

    #[test]
    fn removal_skips_absent_tree_and_stops_on_inspection_error() {
        for present in [Ok(false), Err("inspect denied".to_string())] {
            let expected = present.clone().map(|_| ());
            assert_eq!(unregister_with(|_| panic!("must not call category API"),
                || panic!("must not call profile API"), || panic!("must not delete"),
                || present.clone(), |_| panic!("must not inspect items")), expected);
        }
    }

    #[test]
    fn removal_accumulates_api_and_deletion_errors() {
        let error = unregister_with(|_| Err(HRESULT(0x80004005u32 as i32).into()),
            || Err(HRESULT(0x80070005u32 as i32).into()),
            || Err("delete failed".into()), || Ok(true), |_| Ok(true)).unwrap_err();
        assert_eq!(error.matches("UnregisterCategory:").count(), categories().len());
        assert!(error.contains("UnregisterProfile:"));
        assert!(error.ends_with("; delete failed"));
        assert_eq!(unregister_with(|_| Ok(()), || Ok(()), || Err("delete failed".into()),
            || Ok(true), |_| Ok(true)), Err("delete failed".into()));
    }

    #[test]
    fn removal_selects_only_registered_items_and_succeeds_for_partial_registration() {
        let first = categories()[0];
        let items = || categories().into_iter().map(Some).chain(std::iter::once(None));
        let present = |item| Ok(item == Some(first) || item == None);
        assert_eq!(unregister_items(items().map(|item| (item, present(item)))).collect::<Vec<_>>(),
            [Ok(Some(first)), Ok(None)]);
        for failed_step in [None, Some(0), Some(1)] {
            let calls = RefCell::new(vec![]);
            let result = unregister_with(|c| {
                assert_eq!(c, first);
                calls.borrow_mut().push("category");
                if failed_step == Some(0) { Err(HRESULT(0x80004005u32 as i32).into()) } else { Ok(()) }
            }, || {
                calls.borrow_mut().push("profile");
                if failed_step == Some(1) { Err(HRESULT(0x80004005u32 as i32).into()) } else { Ok(()) }
            }, || { calls.borrow_mut().push("delete"); Ok(()) }, || Ok(true), |item| {
                calls.borrow_mut().push("inspect item");
                present(item)
            });
            assert_eq!(result.is_err(), failed_step.is_some());
            assert_eq!(*calls.borrow(), ["inspect item", "category", "inspect item", "inspect item",
                "inspect item", "inspect item", "profile", "delete"]);
        }
    }

    #[test]
    fn removal_skips_absent_items_and_preserves_item_inspection_errors() {
        for state in [Ok(false), Err("inspect item denied".to_string())] {
            let mut deleted = false;
            let result = unregister_with(|_| panic!("must not call category API"),
                || panic!("must not call profile API"), || { deleted = true; Ok(()) },
                || Ok(true), |_| state.clone());
            assert!(deleted);
            match state {
                Ok(false) => assert_eq!(result, Ok(())),
                Err(e) => assert_eq!(result.unwrap_err().matches(&e).count(), categories().len() + 1),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn stop_accepts_exit_between_probe_and_failed_termination() {
        let mut waits = vec![];
        let mut elapsed = [Duration::from_secs(2), Duration::from_secs(3)].into_iter();
        let result = stop_process_with(7, || Err("access denied".into()), |ms| {
            waits.push(ms);
            Ok(ms != 0)
        }, || elapsed.next().unwrap());
        assert_eq!(result, Ok(()));
        assert_eq!(waits, [0, 7_000]);
    }

    #[test]
    fn stop_rejects_wait_failure_and_timeout_and_preserves_termination_error() {
        let wait_error = "wait for engine PID 7: WaitForSingleObject failed (GetLastError=6)";
        for outcome in [Err(wait_error.to_string()), Ok(false)] {
            for terminated in [Ok(()), Err("original access denied".to_string())] {
                let result = stop_process_with(7, || terminated.clone(),
                    |ms| if ms == 0 { Ok(false) } else { outcome.clone() }, || Duration::from_secs(1));
                let expected = match (&terminated, &outcome) {
                    (Err(t), Err(w)) => format!("{t}; {w}"),
                    (Ok(()), Err(w)) => w.clone(),
                    (Err(t), Ok(false)) => t.clone(),
                    _ => "engine PID 7 did not exit within 10 s total".into(),
                };
                assert_eq!(result, Err(expected));
            }
        }
    }

    #[test]
    fn wait_classifies_exit_timeout_and_failure_with_last_error() {
        assert_eq!(wait_result(7, WAIT_OBJECT_0, || panic!("not failed")), Ok(true));
        assert_eq!(wait_result(7, WAIT_TIMEOUT, || panic!("not failed")), Ok(false));
        for code in [5, 6] {
            assert_eq!(wait_result(7, WAIT_FAILED, || code),
                Err(format!("wait for engine PID 7: WaitForSingleObject failed (GetLastError={code})")));
        }
        assert_eq!(wait_result(7, WAIT_EVENT(123), || panic!("not WAIT_FAILED")),
            Err("wait for engine PID 7: unexpected wait result 123".into()));
    }

    #[test]
    fn stop_does_not_terminate_when_initial_probe_fails() {
        assert_eq!(stop_process_with(7, || panic!("must not terminate"), |ms| {
            assert_eq!(ms, 0);
            wait_result(7, WAIT_FAILED, || 6)
        }, || panic!("must stop immediately")),
            Err("wait for engine PID 7: WaitForSingleObject failed (GetLastError=6)".into()));
    }

    #[test]
    fn stop_skips_exited_process_and_enforces_deadline_after_termination() {
        assert_eq!(stop_process_with(7, || panic!("already exited"), |_| Ok(true),
            || panic!("already exited")), Ok(()));
        for terminated in [Ok(()), Err("original access denied".to_string())] {
            let mut elapsed = [Duration::from_secs(9), Duration::from_secs(10)].into_iter();
            let result = stop_process_with(7, || terminated.clone(), |ms| {
                assert_eq!(ms, 0); Ok(false)
            }, || elapsed.next().unwrap());
            assert_eq!(result, Err(terminated.err().unwrap_or("machine-stop total timeout".into())));
        }
    }

    #[test]
    fn process_selection_requires_exact_normalized_image_and_fails_closed() {
        let root = Path::new(r"C:\private\TwPotato");
        let process = |pid, name: &str, image: &str| Process { pid, name: name.into(), image: Ok(image.into()) };
        let mut rows = vec![
            process(1, ids::ENGINE_NAME, r"\\?\C:\PRIVATE\twpotato\twime-engined.exe"),
            process(2, ids::ENGINE_NAME, r"C:\other\twime-engined.exe"),
            process(3, "other.exe", r"C:\private\TwPotato\other.exe"),
            process(4, ids::ENGINE_NAME, r"C:\private\TwPotato-extra\twime-engined.exe"),
            Process { pid: 5, name: "other.exe".into(), image: Err("access denied".into()) },
            process(6, ids::ENGINE_NAME, "c:/private/twpotato/./twime-engined.exe"),
            process(8, ids::ENGINE_NAME, r"C:\private\TwPotato\sub\..\twime-engined.exe"),
        ];
        assert_eq!(select_processes(root, &rows).unwrap(), [1, 6, 8]);
        rows.push(Process { pid: 7, name: "TWIME-ENGINED.EXE".into(), image: Err("access denied".into()) });
        assert!(select_processes(root, &rows).is_err());
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(test_temp_dir("twime-machine-test"))
        }
    }
    impl Drop for Temp { fn drop(&mut self) { assert!(self.0.starts_with(std::env::temp_dir())); let _ = fs::remove_dir_all(&self.0); } }

    #[test]
    fn marker_lifecycle_only_uses_private_temp_directories() {
        let t = Temp::new(); let root = t.0.join("root");
        release(&root).unwrap();
        mark(&root).unwrap(); mark(&root).unwrap();
        assert_eq!(fs::read(root.join(ids::UNINSTALL_MARKER)).unwrap(), b"1\n");
        release(&root).unwrap(); release(&root).unwrap(); assert!(!root.exists());
        mark(&root).unwrap(); fs::write(root.join("keep"), b"keep").unwrap();
        release(&root).unwrap(); assert_eq!(fs::read(root.join("keep")).unwrap(), b"keep");
        assert!(!root.join(ids::UNINSTALL_MARKER).exists());
        release(&root).unwrap();
        fs::create_dir(root.join(ids::UNINSTALL_MARKER)).unwrap();
        assert!(release(&root).is_err());
        let file = t.0.join("file"); fs::write(&file, b"x").unwrap();
        assert!(mark(&file).is_err()); assert!(release(&file).is_err());
    }

    #[test]
    fn option_validation_rejects_missing_unknown_and_duplicate_arguments() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(options(&args(&["machine-register", "--dll"]), &["--dll"]).is_err());
        assert!(options(&args(&["machine-register", "--unknown", "x"]), &["--dll"]).is_err());
        assert!(options(&args(&["machine-register", "--dll", "x", "--dll", "y"]), &["--dll"]).is_err());
        assert!(options(&args(&["machine-rollback-register", "--old-dll", ""]), &["--old-dll"]).is_ok());
    }

    #[test]
    fn integrity_labels_cover_windows_boundaries() {
        for (rid, name) in [(0x1000, "low"), (0x2000, "medium"), (0x2100, "medium-plus"), (0x3000, "high"), (0x4000, "system")] {
            assert_eq!(integrity_name(rid), name);
        }
    }

    #[test]
    fn stop_deadline_caps_each_wait_and_enforces_total_budget() {
        assert_eq!(remaining_stop_time(Duration::ZERO).unwrap(), 10_000);
        assert_eq!(remaining_stop_time(Duration::from_secs(6)).unwrap(), 4_000);
        assert!(remaining_stop_time(Duration::from_secs(10)).is_err());
        assert!(remaining_stop_time(Duration::from_secs(11)).is_err());
    }
}
