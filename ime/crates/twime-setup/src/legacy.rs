//! Read-only installation selection and post-commit legacy cleanup/migration.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Manifest { Missing, Installed, Other }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Active { Fresh, Current, Legacy }

pub(super) fn manifest(root: &Path) -> Result<Manifest, String> {
    match fs::read_to_string(root.join(MANIFEST)) {
        Ok(s) => Ok(if s.lines().filter(|l| l.starts_with("state=")).collect::<Vec<_>>() == ["state=installed"] {
            Manifest::Installed
        } else { Manifest::Other }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::Missing),
        Err(e) => Err(format!("read {}: {e}", root.join(MANIFEST).display())),
    }
}

fn known_install_entry(name: &str, kind: fs::FileType) -> bool {
    if name == ids::UNINSTALL_MARKER { return kind.is_file(); }
    let base = name.strip_suffix(".stuck").unwrap_or(name);
    if base == "assets" { return kind.is_dir(); }
    kind.is_file() && ((base.starts_with("twime_tsf") && base.ends_with(".dll")) ||
        [ids::ENGINE_NAME, SETTINGS_EXE, "user-data.on", "dev-mode", MANIFEST].contains(&base))
}

/// Only known installation entries remain under the protection marker.
pub(super) fn residue(root: &Path) -> Result<bool, String> {
    if !root.join(ids::UNINSTALL_MARKER).is_file() || manifest(root)? != Manifest::Missing { return Ok(false); }
    only_known_residue(root)
}

/// Read-only check, including unmarked leftovers and absent roots.
fn only_known_residue(root: &Path) -> Result<bool, String> {
    if manifest(root)? != Manifest::Missing { return Ok(false); }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name(); let name = name.to_string_lossy();
        if !known_install_entry(&name, entry.file_type().map_err(|e| e.to_string())?) { return Ok(false); }
    }
    Ok(true)
}

pub(super) fn cleanup_inactive(current: &Path, old: &Path, com: Option<&str>) -> Result<(), String> {
    // Validate BOTH roots before cleanup can write even a protection marker.
    let active = active_in(current, old, com, false)?;
    if active != Active::Current && (!only_known_residue(current)? ||
        (active == Active::Fresh && !only_known_residue(old)?)) {
        return Err(format!("拒絕清理：不是純殘留；HKCU COM={com:?}; 新根 {} manifest={:?}; 舊根 {} manifest={:?}; active={active:?}",
            current.display(), manifest(current)?, old.display(), manifest(old)?));
    }
    if active != Active::Legacy { cleanup(old, com)?; }
    if active != Active::Current { cleanup(current, com)?; }
    Ok(())
}

pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

pub(super) fn path_key(path: &Path) -> String {
    let spelling = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let spelling = if let Some(unc) = spelling.strip_prefix(r"\\?\unc\") { format!(r"\\{unc}") }
        else { spelling.strip_prefix(r"\\?\").unwrap_or(&spelling).to_owned() };
    Path::new(&spelling).components().filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect::<PathBuf>().to_string_lossy().into_owned()
}

pub(super) fn points_to(com: Option<&str>, root: &Path) -> bool {
    com.and_then(|s| Path::new(s).parent()).is_some_and(|p| same_path(p, root))
}

/// No filesystem/registry operations: callers obtain both manifests and transaction status first.
pub(super) fn active(com: Option<&str>, current: &Path, old: &Path,
    new_manifest: Manifest, old_manifest: Manifest, unfinished: bool, machine: bool,
) -> Result<Active, String> {
    let result = if machine || unfinished || [new_manifest, old_manifest].contains(&Manifest::Other) { None }
        else if points_to(com, old) && old_manifest == Manifest::Installed && new_manifest == Manifest::Missing { Some(Active::Legacy) }
        else if points_to(com, current) && new_manifest == Manifest::Installed { Some(Active::Current) }
        else if com.is_none() && new_manifest == Manifest::Missing && old_manifest == Manifest::Missing { Some(Active::Fresh) }
        else { None };
    result.ok_or_else(|| format!("拒絕安裝，未做任何變更：HKCU COM={com:?}; 新根 {} manifest={new_manifest:?}; 舊根 {} manifest={old_manifest:?}; 未完成交易={unfinished}; 全機模式={machine}", current.display(), old.display()))
}

pub(super) fn active_in(current: &Path, old: &Path, com: Option<&str>, machine: bool) -> Result<Active, String> {
    let new_manifest = manifest(current)?;
    let old_manifest = manifest(old)?;
    let unfinished = [install::reject_unfinished_install(current), install::reject_unfinished_install(old)];
    active(com, current, old, new_manifest, old_manifest, unfinished.iter().any(Result::is_err), machine)
        .map_err(|e| format!("{e}\n{}", unfinished.into_iter().filter_map(Result::err).collect::<Vec<_>>().join("\n")))
}

pub(super) fn settings_running(root: &Path) -> Result<bool, String> {
    use windows::Win32::{System::{Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION}}, Foundation::ERROR_NO_MORE_FILES};
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.to_string())?;
        struct Guard(windows::Win32::Foundation::HANDLE);
        impl Drop for Guard { fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } } }
        let _guard = Guard(snapshot);
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut next = Process32FirstW(snapshot, &mut entry);
        loop {
            if let Err(e) = next {
                if e.code() == windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0) { return Ok(false); }
                return Err(format!("enumerate settings process: {e}"));
            }
            let name = String::from_utf16_lossy(&entry.szExeFile[..entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len())]);
            if name.eq_ignore_ascii_case(SETTINGS_EXE) {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID)
                    .map_err(|e| format!("cannot inspect settings process {}: {e}", entry.th32ProcessID))?;
                let _process = Guard(process);
                let mut image = vec![0u16; 32768];
                let mut len = image.len() as u32;
                QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(image.as_mut_ptr()), &mut len)
                    .map_err(|e| format!("cannot read settings image: {e}"))?;
                if same_path(Path::new(&String::from_utf16_lossy(&image[..len as usize])), &root.join(SETTINGS_EXE)) { return Ok(true); }
            }
            next = Process32NextW(snapshot, &mut entry);
        }
    }
}

/// Only known installation files are removed. Unknown files and locked leftovers keep the marker.
/// Never touches registration or schedules a reboot-time deletion.
/// Transaction-internal primitive; standalone cleanup must use cleanup_inactive's preflight.
pub(super) fn cleanup(root: &Path, com: Option<&str>) -> Result<(), String> {
    if points_to(com, root) { return Err(format!("COM 仍指向舊根 {}，拒絕清理", root.display())); }
    if !root.exists() { return Ok(()); }
    // Protect hosts still holding the old DLL even when cleaning a previously unmarked residue.
    fs::write(root.join(ids::UNINSTALL_MARKER), b"1\n").map_err(|e| e.to_string())?;
    let residue = residue(root)?;
    let mut problems = vec![];
    // A later cleanup-legacy also retries leftovers after the DLL host exits.
    let mut paths = vec![];
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != ids::UNINSTALL_MARKER && known_install_entry(&name, entry.file_type().map_err(|e| e.to_string())?) { paths.push(entry.path()); }
    }
    paths.sort(); paths.dedup();
    // Remove pre-existing .stuck files first, so a newly renamed DLL stays protected until the next run.
    paths.sort_by_key(|p| !p.to_string_lossy().ends_with(".stuck"));
    for p in paths {
        let result = if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
        if let Err(e) = result {
            if e.kind() == std::io::ErrorKind::NotFound { continue; }
            if p.to_string_lossy().ends_with(".stuck") {
                if !residue { problems.push(format!("cannot remove {}: {e}", p.display())); }
            } else {
                let mut stuck = p.as_os_str().to_os_string(); stuck.push(".stuck");
                if let Err(rename) = fs::rename(&p, PathBuf::from(stuck)) {
                    problems.push(format!("cannot remove/rename {}: {e}; {rename}", p.display()));
                }
            }
        }
    }
    let marker = root.join(ids::UNINSTALL_MARKER);
    let left = fs::read_dir(root).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    if left.iter().all(|e| e.file_name() == ids::UNINSTALL_MARKER) {
        remove_settings_shortcut(&marker)?;
        fs::remove_dir(root).map_err(|e| e.to_string())?;
    } else {
        fs::write(&marker, b"1\n").map_err(|e| e.to_string())?;
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

fn equal_files(a: &Path, b: &Path) -> std::io::Result<bool> {
    use std::io::{BufReader, Read};
    if fs::metadata(a)?.len() != fs::metadata(b)?.len() { return Ok(false); }
    let mut a = BufReader::new(fs::File::open(a)?);
    let mut b = BufReader::new(fs::File::open(b)?);
    let mut aa = [0; 8192]; let mut bb = [0; 8192];
    loop {
        let n = a.read(&mut aa)?;
        if n == 0 { return Ok(b.read(&mut bb)? == 0); }
        b.read_exact(&mut bb[..n])?;
        if aa[..n] != bb[..n] { return Ok(false); }
    }
}

fn publish(src: &Path, dst: &Path) -> std::io::Result<()> {
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};
    unsafe { MoveFileExW(PCWSTR(wide(&src.to_string_lossy()).as_ptr()), PCWSTR(wide(&dst.to_string_lossy()).as_ptr()), MOVE_FILE_FLAGS(0)) }
        .map_err(std::io::Error::from)
}

/// Arguments and callbacks keep tests inside temporary directories, including publication races.
pub(super) fn copy_data_in(old: &Path, new: &Path, running: &dyn Fn() -> Result<bool, String>,
    copy: &dyn Fn(&Path, &mut fs::File) -> std::io::Result<()>,
    before_publish: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Vec<String> {
    match running() {
        Ok(false) => {},
        result => return vec![format!("不複製使用者資料：舊設定程式執行中或無法確認 ({result:?})")],
    }
    let mut warnings = vec![];
    for name in ["settings.toml", "userlex.tsv"] {
        let src = old.join(name); let dst = new.join(name); let tmp = new.join(format!("{name}.migrating"));
        let result = (|| -> std::io::Result<()> {
            match fs::metadata(&src) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                result => { result?; },
            }
            if dst.try_exists()? {
                warnings.push(format!("{name}：新目錄已有同名檔，兩份都保留，請回報"));
                return Ok(());
            }
            fs::create_dir_all(new)?;
            let mut out = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            copy(&src, &mut out)?;
            out.sync_all()?;
            drop(out);
            if !equal_files(&src, &tmp)? { return Err(std::io::Error::other("複製後大小或逐位元組比對不符")); }
            before_publish(&tmp, &dst)?;
            publish(&tmp, &dst)
        })();
        if let Err(e) = result {
            // .migrating is reserved for this one-installer migration, including stale attempts.
            if let Err(cleanup) = fs::remove_file(&tmp) {
                if cleanup.kind() != std::io::ErrorKind::NotFound { warnings.push(format!("remove {}: {cleanup}", tmp.display())); }
            }
            warnings.push(format!("{name} 複製失敗，舊檔保留，請回報：{e}"));
        }
    }
    warnings
}

pub(super) fn copy_data(old: &Path, new: &Path, old_root: &Path) -> Vec<String> {
    copy_data_in(old, new, &|| settings_running(old_root), &|src, out| {
        std::io::copy(&mut fs::File::open(src)?, out).map(|_| ())
    }, &|_, _| Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(test_temp_dir("twpotato-legacy-test"))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) { assert!(self.0.starts_with(std::env::temp_dir())); let _ = fs::remove_dir_all(&self.0); }
    }
    fn copy(src: &Path, dst: &mut fs::File) -> std::io::Result<()> { std::io::copy(&mut fs::File::open(src)?, dst).map(|_| ()) }

    #[test]
    fn active_installation_selection_covers_all_com_and_manifest_states() {
        use Manifest::*;
        let new = Path::new(r"C:\private\TwPotato"); let old = Path::new(r"C:\private\TwIme");
        let current = r"c:\PRIVATE\twpOTATO\twime_tsf-new.dll";
        let legacy = r"C:\private\TwIme\twime_tsf-old.dll";
        let other = r"C:\private\TwIme-extra\twime_tsf.dll";
        let cases = [
            (None, Missing, Missing, Some(Active::Fresh)),
            (Some(legacy), Missing, Installed, Some(Active::Legacy)),
            (Some(current), Installed, Missing, Some(Active::Current)),
            (Some(current), Installed, Installed, Some(Active::Current)),
            (None, Installed, Missing, None), (None, Missing, Installed, None), (None, Installed, Installed, None),
            (Some(legacy), Missing, Missing, None), (Some(legacy), Installed, Installed, None),
            (Some(legacy), Installed, Missing, None), (Some(current), Missing, Installed, None),
            (Some(current), Missing, Missing, None), (Some(other), Missing, Missing, None),
            (Some(other), Installed, Installed, None), (Some(""), Missing, Missing, None),
        ];
        for (com, nm, om, expected) in cases { assert_eq!(active(com, new, old, nm, om, false, false).ok(), expected, "{com:?} {nm:?} {om:?}"); }
        for com in [None, Some(current), Some(legacy), Some(other)] {
            for nm in [Missing, Installed, Other] {
                for om in [Missing, Installed, Other] {
                    for (unfinished, machine) in [(true, false), (false, true), (true, true)] {
                        assert!(active(com, new, old, nm, om, unfinished, machine).is_err());
                    }
                    if nm == Other || om == Other { assert!(active(com, new, old, nm, om, false, false).is_err()); }
                }
            }
        }
        assert!(same_path(Path::new(r"C:\Private\TwIme\TWIME-SETTINGS.EXE"), &old.join(SETTINGS_EXE)));
    }

    #[test]
    fn manifest_requires_one_installed_state() {
        let t = Temp::new();
        assert_eq!(manifest(&t.0).unwrap(), Manifest::Missing);
        for (s, expected) in [("state=installed\r\n", Manifest::Installed), ("state=installing\n", Manifest::Other),
            ("state=installed\nstate=installing\n", Manifest::Other), ("corrupt", Manifest::Other)] {
            fs::write(t.0.join(MANIFEST), s).unwrap(); assert_eq!(manifest(&t.0).unwrap(), expected);
        }
    }

    #[test]
    fn preflight_rejects_unfinished_transactions_in_either_root_without_changing_files() {
        let t = Temp::new(); let current = t.0.join("new"); let old = t.0.join("old");
        for root in [&current, &old] {
            fs::create_dir_all(root.join("assets")).unwrap();
            fs::write(root.join(MANIFEST), b"state=installed\n").unwrap();
            fs::write(root.join(ids::UNINSTALL_MARKER), b"previous protection").unwrap();
        }
        let com = current.join("twime_tsf-new.dll").to_string_lossy().into_owned();
        for root in [&current, &old] {
            let backup = root.join("assets").join("model.bin.old-install"); fs::write(&backup, b"backup").unwrap();
            let error = active_in(&current, &old, Some(&com), false).unwrap_err();
            assert!(error.contains(&backup.display().to_string())); assert!(error.contains("HKCU COM="));
            assert_eq!(fs::read(&backup).unwrap(), b"backup");
            for r in [&current, &old] { assert_eq!(fs::read(r.join(MANIFEST)).unwrap(), b"state=installed\n"); assert_eq!(fs::read(r.join(ids::UNINSTALL_MARKER)).unwrap(), b"previous protection"); }
            fs::remove_file(backup).unwrap();
        }
    }

    #[test]
    fn data_copy_missing_old_directory_does_not_create_new_directory() {
        let t = Temp::new(); let old = t.0.join("old"); let new = t.0.join("new");
        assert!(copy_data_in(&old, &new, &|| Ok(false), &copy, &|_, _| Ok(())).is_empty());
        assert!(!old.exists() && !new.exists());
    }

    #[test]
    fn data_copy_preserves_sources_and_copies_only_the_two_user_files() {
        let t = Temp::new(); let new = t.0.join("new");
        for name in ["settings.toml", "userlex.tsv", "running.lock", "unconfirmed.json", "other.txt"] { fs::write(t.0.join(name), format!("source {name} 地瓜")).unwrap(); }
        assert!(copy_data_in(&t.0, &new, &|| Ok(false), &copy, &|_, _| Ok(())).is_empty());
        for name in ["settings.toml", "userlex.tsv", "running.lock", "unconfirmed.json", "other.txt"] {
            assert_eq!(fs::read(t.0.join(name)).unwrap(), format!("source {name} 地瓜").as_bytes());
            if ["settings.toml", "userlex.tsv"].contains(&name) { assert!(equal_files(&t.0.join(name), &new.join(name)).unwrap()); }
            else { assert!(!new.join(name).exists()); }
        }
        assert_eq!(fs::read_dir(new).unwrap().count(), 2);
    }

    #[test]
    fn data_copy_never_replaces_existing_files_or_a_publication_race() {
        for race in [false, true] {
            let t = Temp::new(); let new = t.0.join("new"); fs::create_dir(&new).unwrap();
            fs::write(t.0.join("settings.toml"), b"source").unwrap();
            if !race { fs::write(new.join("settings.toml"), b"other writer").unwrap(); }
            let warnings = copy_data_in(&t.0, &new, &|| Ok(false), &copy, &|_, dst| fs::write(dst, b"other writer"));
            assert_eq!(warnings.len(), 1); assert!(warnings[0].contains("settings.toml"));
            assert_eq!(fs::read(new.join("settings.toml")).unwrap(), b"other writer");
            assert_eq!(fs::read(t.0.join("settings.toml")).unwrap(), b"source");
            assert!(!new.join("settings.toml.migrating").exists());
        }
    }

    #[test]
    fn data_copy_running_settings_or_unverifiable_process_skips_everything() {
        for error in [false, true] {
            let t = Temp::new(); let new = t.0.join("new"); fs::write(t.0.join("settings.toml"), b"source").unwrap();
            let warnings = copy_data_in(&t.0, &new, &|| if error { Err("process query failed".into()) } else { Ok(true) }, &copy, &|_, _| Ok(()));
            assert_eq!(warnings.len(), 1); assert!(!new.exists()); assert_eq!(fs::read(t.0.join("settings.toml")).unwrap(), b"source");
        }
    }

    #[test]
    fn data_copy_failures_remove_owned_temporary_file_and_leave_source_unchanged() {
        for failure in ["copy", "compare", "publish"] {
            let t = Temp::new(); let new = t.0.join("new"); fs::write(t.0.join("settings.toml"), b"source").unwrap();
            let warnings = copy_data_in(&t.0, &new, &|| Ok(false), &|src, out| {
                if failure == "copy" { out.write_all(b"partial")?; return Err(std::io::Error::other("copy failure")); }
                if failure == "compare" { return out.write_all(b"wrong!"); }
                copy(src, out)
            }, &|_, _| Err(std::io::Error::other("publication failure")));
            assert_eq!(warnings.len(), 1); assert!(!new.join("settings.toml.migrating").exists());
            assert!(!new.join("settings.toml").exists()); assert_eq!(fs::read(t.0.join("settings.toml")).unwrap(), b"source");
        }
    }

    #[test]
    fn data_copy_removes_a_stale_migrating_file_on_failure() {
        let t = Temp::new(); let new = t.0.join("new"); fs::create_dir(&new).unwrap();
        fs::write(t.0.join("settings.toml"), b"source").unwrap(); fs::write(new.join("settings.toml.migrating"), b"other installer").unwrap();
        assert_eq!(copy_data_in(&t.0, &new, &|| Ok(false), &copy, &|_, _| Ok(())).len(), 1);
        assert!(!new.join("settings.toml.migrating").exists());
        assert_eq!(fs::read(t.0.join("settings.toml")).unwrap(), b"source");
    }

    #[test]
    fn standalone_cleanup_rejects_old_backups_without_com_before_any_write() {
        for name in ["twime_tsf-old.dll.old-install", "assets/model.bin.old-install"] {
            let t = Temp::new(); let current = t.0.join("new"); let old = t.0.join("old");
            for root in [&current, &old] {
                fs::create_dir_all(root.join("assets")).unwrap();
                fs::write(root.join(ids::UNINSTALL_MARKER), b"original marker").unwrap();
                fs::write(root.join(ids::ENGINE_NAME), b"original engine").unwrap();
                fs::write(root.join("assets/model.bin"), b"original model").unwrap();
            }
            let backup = old.join(name.replace('/', "\\")); fs::write(&backup, b"backup").unwrap();
            let error = cleanup_inactive(&current, &old, None).unwrap_err();
            assert!(error.contains("HKCU COM=None") && error.contains(&backup.display().to_string()), "{error}");
            assert_eq!(fs::read(&backup).unwrap(), b"backup");
            for root in [&current, &old] {
                assert_eq!(fs::read(root.join(ids::UNINSTALL_MARKER)).unwrap(), b"original marker");
                assert_eq!(fs::read(root.join(ids::ENGINE_NAME)).unwrap(), b"original engine");
                assert_eq!(fs::read(root.join("assets/model.bin")).unwrap(), b"original model");
                assert!(!root.join(MANIFEST).exists());
                assert_eq!(fs::read_dir(root).unwrap().count(), 3 + usize::from(root == &old && !name.starts_with("assets/")));
            }
        }
    }

    #[test]
    fn standalone_cleanup_allows_the_three_consistent_states() {
        for active in [Active::Current, Active::Legacy, Active::Fresh] {
            let t = Temp::new(); let current = t.0.join("new"); let old = t.0.join("old");
            for root in [&current, &old] {
                fs::create_dir_all(root.join("assets")).unwrap();
                fs::write(root.join(ids::ENGINE_NAME), b"engine").unwrap();
                fs::write(root.join("assets/model.bin"), b"model").unwrap();
            }
            let selected = match active { Active::Current => Some(&current), Active::Legacy => Some(&old), Active::Fresh => None };
            if let Some(root) = selected { fs::write(root.join(MANIFEST), b"state=installed\n").unwrap(); }
            // Current COM also permits a leftover installed manifest in the old root.
            if active == Active::Current { fs::write(old.join(MANIFEST), b"state=installed\n").unwrap(); }
            let com = selected.map(|root| root.join("twime_tsf.dll").to_string_lossy().into_owned());
            cleanup_inactive(&current, &old, com.as_deref()).unwrap();
            for root in [&current, &old] {
                if Some(root) == selected {
                    assert_eq!(fs::read(root.join(MANIFEST)).unwrap(), b"state=installed\n");
                    assert_eq!(fs::read(root.join(ids::ENGINE_NAME)).unwrap(), b"engine");
                    assert_eq!(fs::read(root.join("assets/model.bin")).unwrap(), b"model");
                    assert!(!root.join(ids::UNINSTALL_MARKER).exists());
                } else { assert!(!root.exists()); }
            }
        }
    }

    #[test]
    fn standalone_cleanup_rejects_inconsistent_states_and_impure_residue_without_marking() {
        use Manifest::*;
        for com_root in [None, Some("new"), Some("old"), Some("other")] {
            for nm in [Missing, Installed, Other] {
                for om in [Missing, Installed, Other] {
                    let t = Temp::new(); let current = t.0.join("new"); let old = t.0.join("old");
                    for (root, state) in [(&current, nm), (&old, om)] {
                        fs::create_dir(root).unwrap();
                        fs::write(root.join(ids::ENGINE_NAME), b"keep").unwrap();
                        if state != Missing { fs::write(root.join(MANIFEST), if state == Installed { "state=installed\n" } else { "invalid" }).unwrap(); }
                    }
                    let com = com_root.map(|name| t.0.join(name).join("twime_tsf.dll").to_string_lossy().into_owned());
                    if active(com.as_deref(), &current, &old, nm, om, false, false).is_ok() { continue; }
                    let error = cleanup_inactive(&current, &old, com.as_deref()).unwrap_err();
                    assert!(error.contains("HKCU COM=") && error.contains("manifest="), "{error}");
                    for (root, state) in [(&current, nm), (&old, om)] {
                        assert!(!root.join(ids::UNINSTALL_MARKER).exists());
                        assert_eq!(fs::read(root.join(ids::ENGINE_NAME)).unwrap(), b"keep");
                        assert_eq!(manifest(root).unwrap(), state);
                    }
                }
            }
        }
        for legacy in [false, true] {
            let t = Temp::new(); let current = t.0.join("new"); let old = t.0.join("old");
            fs::create_dir(&current).unwrap(); fs::create_dir(&old).unwrap();
            fs::write(current.join("unknown.txt"), b"keep").unwrap();
            fs::write(old.join(ids::ENGINE_NAME), b"keep old").unwrap();
            let com = if legacy {
                fs::write(old.join(MANIFEST), b"state=installed\n").unwrap();
                Some(old.join("twime_tsf.dll").to_string_lossy().into_owned())
            } else { None };
            assert!(cleanup_inactive(&current, &old, com.as_deref()).unwrap_err().contains("HKCU COM="));
            assert_eq!(fs::read(current.join("unknown.txt")).unwrap(), b"keep");
            assert_eq!(fs::read(old.join(ids::ENGINE_NAME)).unwrap(), b"keep old");
            assert!(!current.join(ids::UNINSTALL_MARKER).exists() && !old.join(ids::UNINSTALL_MARKER).exists());
        }
    }

    #[test]
    fn cleanup_refuses_active_com_then_cleans_known_files_and_retries_stuck_files() {
        let t = Temp::new(); let old = t.0.join("old"); fs::create_dir_all(old.join("assets")).unwrap();
        let names = ["twime_tsf-old.dll", "twime_tsf.dll.stuck", ids::ENGINE_NAME, SETTINGS_EXE, "user-data.on", "dev-mode", MANIFEST, ids::UNINSTALL_MARKER];
        for name in names { fs::write(old.join(name), b"old").unwrap(); }
        fs::write(old.join("assets/model.bin"), b"model").unwrap();
        let com = old.join("twime_tsf-old.dll").to_string_lossy().into_owned();
        assert!(cleanup(&old, Some(&com)).is_err());
        for name in names { assert_eq!(fs::read(old.join(name)).unwrap(), b"old"); }
        fs::write(old.join("unknown.txt"), b"keep").unwrap();
        cleanup(&old, None).unwrap();
        assert_eq!(fs::read_dir(&old).unwrap().count(), 2); assert!(old.join(ids::UNINSTALL_MARKER).exists());
        assert_eq!(fs::read(old.join("unknown.txt")).unwrap(), b"keep");
        fs::remove_file(old.join("unknown.txt")).unwrap(); cleanup(&old, None).unwrap(); assert!(!old.exists());
        cleanup(&old, None).unwrap();
    }

    #[test]
    fn unknown_stuck_file_is_not_residue_and_survives_cleanup() {
        let t = Temp::new(); let root = t.0.join("private-root"); fs::create_dir(&root).unwrap();
        fs::write(root.join(ids::UNINSTALL_MARKER), b"1").unwrap();
        fs::write(root.join("twime_tsf-old.dll.stuck"), b"known").unwrap();
        assert!(residue(&root).unwrap());
        for name in ["backup.stuck", "twime_tsf.dll.stuck.stuck", "twime_tsf-backup.stuck"] {
            fs::write(root.join(name), b"keep").unwrap();
        }
        assert!(!residue(&root).unwrap());
        cleanup(&root, None).unwrap();
        assert!(root.join(ids::UNINSTALL_MARKER).is_file());
        assert!(!root.join("twime_tsf-old.dll.stuck").exists());
        for name in ["backup.stuck", "twime_tsf.dll.stuck.stuck", "twime_tsf-backup.stuck"] {
            assert_eq!(fs::read(root.join(name)).unwrap(), b"keep");
        }
        assert!(!residue(&root).unwrap());
    }

    #[test]
    fn residue_and_cleanup_accept_only_known_installation_entries() {
        let t = Temp::new(); let root = t.0.join("private-root"); fs::create_dir(&root).unwrap();
        fs::write(root.join(ids::UNINSTALL_MARKER), b"1").unwrap();
        for name in ["twime_tsf.dll", "twime_tsf-old.dll", ids::ENGINE_NAME, SETTINGS_EXE, "user-data.on", "dev-mode"] {
            for suffix in ["", ".stuck"] { fs::write(root.join(format!("{name}{suffix}")), b"known").unwrap(); }
        }
        fs::write(root.join("install.manifest.stuck"), b"known").unwrap();
        for name in ["assets", "assets.stuck"] { fs::create_dir(root.join(name)).unwrap(); }
        assert!(residue(&root).unwrap());
        cleanup(&root, None).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn cleanup_locked_file_keeps_marker_until_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = Temp::new(); let old = t.0.join("old"); fs::create_dir(&old).unwrap();
        fs::write(old.join("twime_tsf-old.dll"), b"locked").unwrap();
        let lock = fs::OpenOptions::new().read(true).share_mode(1).open(old.join("twime_tsf-old.dll")).unwrap();
        assert!(cleanup(&old, None).is_err()); assert!(old.join(ids::UNINSTALL_MARKER).exists());
        drop(lock); cleanup(&old, None).unwrap(); assert!(!old.exists());
    }
}
