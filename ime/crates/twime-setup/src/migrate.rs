//! Moving one user's own user-mode installation into the per-machine MSI (release plan 2.4a).
//! The three commands run from the MSI Binary table as the installing user (Impersonate=yes):
//! `user-migrate-prepare` (deferred) renames the user-mode registrations away, `user-migrate-restore` (rollback) renames
//! them back, `user-migrate-finish` (commit, and on a later repair) deletes what is no longer referenced.
//! Only the caller's own HKCU, LocalAppData and Start menu are touched; never HKLM, never a TSF API, never user data.
use super::*;
use windows::Win32::{
    Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW},
    System::{Com::CoTaskMemFree, Registry::{KEY_ALL_ACCESS, RRF_RT_REG_SZ, RegGetValueW, RegRenameKey}},
    UI::Shell::{FOLDERID_LocalAppData, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
};

/// Name suffix of a registration that the migration took out of use.
pub(super) const SUFFIX: &str = ".twpotato-migrating";
const SERVICE_SIDS: [&str; 3] = ["S-1-5-18", "S-1-5-19", "S-1-5-20"];

/// Everything a migration touches. `real()` uses the caller's known folders and HKCU; the tests use private
/// directories and a private prefix below HKCU.
pub(super) struct Mig {
    /// Prefix of every registry path below HKCU ("" for the real keys).
    reg: String,
    local: PathBuf,
    /// The shortcut, at the same place `twime-setup install` creates it (Roaming AppData\...\Start Menu\Programs).
    shortcut: PathBuf,
    owner: String,
    /// State directories the engine confirmation reads, and the pipes of the user-mode engine (current and legacy name).
    data: Vec<PathBuf>,
    pipes: Vec<String>,
    /// Test-only fault injection: the named step fails.
    #[cfg(test)]
    fail_at: Option<&'static str>,
}

/// The migration record `Software\TwPotato\Migrate` (all REG_SZ).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record { owner: String, dll: String, root: String, marker: String, state: String, token: String }

fn known_folder(id: &GUID) -> Result<PathBuf, String> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).map_err(|e| format!("SHGetKnownFolderPath: {e}"))?;
        let s = p.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

fn present(p: &Path) -> Result<bool, String> { p.try_exists().map_err(|e| format!("cannot inspect {}: {e}", p.display())) }

impl Mig {
    fn real() -> Result<Mig, String> {
        let owner = twime_winpipe::current_identity().map_err(|e| e.to_string())?.user;
        Ok(Self::from_known(&known_folder(&FOLDERID_LocalAppData)?, &known_folder(&FOLDERID_RoamingAppData)?, owner,
            vec![engine_pipe(false)?, engine_pipe(true)?]))
    }
    /// Every path from the known folders, none from the environment (the custom-action environment need not be the
    /// user's). Same places as `twime-setup install`: the root and the two state directories below LocalAppData, the
    /// shortcut below Roaming AppData\Microsoft\Windows\Start Menu\Programs.
    fn from_known(local: &Path, roaming: &Path, owner: String, pipes: Vec<String>) -> Mig {
        Mig {
            reg: String::new(), local: local.into(),
            shortcut: roaming.join("Microsoft/Windows/Start Menu/Programs/地瓜輸入法 設定.lnk"),
            owner, data: state_dirs_in(local.join("TwPotato"), local.join("TwIme"), None), pipes,
            #[cfg(test)] fail_at: None,
        }
    }
    fn step(&self, _name: &str) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_at == Some(_name) { return Err(format!("injected failure at {_name}")); }
        Ok(())
    }
    fn root(&self) -> PathBuf { self.local.join("Programs").join("TwPotato") }
    fn com_parent(&self) -> String { format!(r"{}Software\Classes\CLSID", self.reg) }
    fn tip_parent(&self) -> String { format!(r"{}Software\Microsoft\CTF\TIP", self.reg) }
    fn record_parent(&self) -> String { format!(r"{}Software\TwPotato", self.reg) }
    fn record_key(&self) -> String { format!(r"{}\Migrate", self.record_parent()) }
    fn record_tmp(&self) -> String { format!(r"{}\Migrate.tmp", self.record_parent()) }
    fn name() -> String { ids::guid_string(ids::CLSID_TIP) }
    fn com_value(&self) -> Result<Option<String>, String> {
        reg_get(&format!(r"{}\{}\InprocServer32", self.com_parent(), Self::name()), None)
    }
    /// (parent key, name, step name) of the two registrations the migration renames.
    fn keys(&self) -> [(String, String, &'static str); 2] {
        [(self.com_parent(), Self::name(), "com"), (self.tip_parent(), Self::name(), "tip")]
    }
    /// Common to the three commands: never a service identity.
    fn check_identity(&self) -> Result<(), String> {
        if SERVICE_SIDS.contains(&self.owner.as_str()) { return Err(format!("service identity {} is not migrated", self.owner)); }
        Ok(())
    }

    fn log(&self, msg: &str) {
        use std::io::Write;
        let dir = self.local.join("Temp");
        let _ = fs::create_dir_all(&dir);
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("twpotato-migrate.log")) {
            let _ = writeln!(f, "{} {msg}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
        }
    }

    fn read_record(&self) -> Result<Option<Record>, String> {
        let key = self.record_key();
        if !reg_exists_checked(HKEY_CURRENT_USER, &key)? { return Ok(None); }
        let get = |n: &str| reg_get(&key, Some(n)).map(|v| v.unwrap_or_default());
        Ok(Some(Record { owner: get("owner")?, dll: get("dll")?, root: get("root")?, marker: get("marker")?, state: get("state")?, token: get("token")? }))
    }
    fn set_state(&self, state: &str) -> Result<(), String> { reg_set(&self.record_key(), Some("state"), state) }
    /// The record belongs to this user and points at this user's root.
    fn owned(&self, r: &Record) -> Result<(), String> {
        if r.owner != self.owner || !legacy::same_path(Path::new(&r.root), &self.root()) {
            return Err(format!("migration record belongs to {} / {}, not to {} / {}", r.owner, r.root, self.owner, self.root().display()));
        }
        Ok(())
    }
}

fn reg_get(sub: &str, name: Option<&str>) -> Result<Option<String>, String> {
    unsafe {
        let w = wide(sub);
        let n = name.map(wide);
        let n = n.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
        let mut len = 0u32;
        let e = RegGetValueW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()), n, RRF_RT_REG_SZ, None, None, Some(&mut len));
        if e.0 == 2 { return Ok(None); }
        if e.0 != 0 { return Err(format!("read {sub}: error {}", e.0)); }
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        let e = RegGetValueW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()), n, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len));
        if e.0 != 0 { return Err(format!("read {sub}: error {}", e.0)); }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16(&buf[..end]).map(Some).map_err(|e| e.to_string())
    }
}

/// Atomic rename of `parent\from` to `parent\to` (both plain names).
fn rename_key(parent: &str, from: &str, to: &str) -> Result<(), String> {
    unsafe {
        let mut h = HKEY::default();
        let e = RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide(parent).as_ptr()), None, KEY_ALL_ACCESS, &mut h);
        if e.0 != 0 { return Err(format!("open {parent}: error {}", e.0)); }
        let e = RegRenameKey(h, PCWSTR(wide(from).as_ptr()), PCWSTR(wide(to).as_ptr()));
        let _ = RegCloseKey(h);
        if e.0 != 0 { Err(format!("rename {parent}\\{from} -> {to}: error {}", e.0)) } else { Ok(()) }
    }
}

/// Rename that never replaces an existing file.
fn rename_file(from: &Path, to: &Path) -> Result<(), String> {
    unsafe {
        MoveFileExW(PCWSTR(wide(&from.to_string_lossy()).as_ptr()), PCWSTR(wide(&to.to_string_lossy()).as_ptr()), MOVE_FILE_FLAGS(0))
            .map_err(|e| format!("rename {} -> {}: {e}", from.display(), to.display()))
    }
}

fn renamed(p: &Path) -> PathBuf { let mut s = p.as_os_str().to_os_string(); s.push(SUFFIX); PathBuf::from(s) }
fn exists(parent: &str, name: &str) -> Result<bool, String> { reg_exists_checked(HKEY_CURRENT_USER, &format!(r"{parent}\{name}")) }

/// Step (1): every check before the first change. Errors mean nothing was changed.
fn check_prepare(m: &Mig, expect_dll: &str, engine: &mut dyn UninstallEngine) -> Result<Vec<(String, Vec<String>)>, String> {
    m.check_identity()?;
    let root = m.root();
    let com = m.com_value()?;
    if !com.as_deref().is_some_and(|c| legacy::same_path(Path::new(c), Path::new(expect_dll))) {
        return Err(format!("HKCU COM {com:?} is not the detected user-mode DLL {expect_dll}"));
    }
    if !legacy::points_to(com.as_deref(), &root) { return Err(format!("user-mode DLL {expect_dll} is not in {}", root.display())); }
    if legacy::manifest(&root)? != legacy::Manifest::Installed { return Err(format!("{} is not a completed user-mode installation", root.display())); }
    install::reject_unfinished_install(&root)?;
    if present(&root.join(ids::UNINSTALL_MARKER))? { return Err(format!("{} carries an uninstall marker", root.display())); }
    let flag = root.join("user-data.on");
    if !(present(&flag)? && fs::metadata(&flag).map_err(|e| e.to_string())?.is_file()) {
        return Err(format!("{} has no user-data.on; migrating would change the user-data setting", root.display()));
    }
    if reg_exists_checked(HKEY_CURRENT_USER, &m.record_key())? { return Err("an earlier migration record exists".into()); }
    for (parent, name, _) in m.keys() {
        if exists(&parent, &format!("{name}{SUFFIX}"))? { return Err(format!("leftover {parent}\\{name}{SUFFIX}")); }
    }
    if present(&renamed(&m.shortcut))? { return Err(format!("leftover {}", renamed(&m.shortcut).display())); }
    let engines = candidate_engines(&[root.as_path()], &m.pipes);
    engine.confirm(&m.data, &engines)?;
    Ok(engines)
}

fn prepare(m: &Mig, expect_dll: &str, token: &str, engine: &mut dyn UninstallEngine) -> Result<(), String> {
    let engines = check_prepare(m, expect_dll, engine).inspect_err(|e| m.log(&format!("prepare refused, nothing changed: {e}")))?;
    m.log("prepare: checks passed");
    let root = m.root();
    // (2) The record is written completely under a temporary name and then published by one rename.
    reg_delete_tree(&m.record_tmp())?;
    let tmp = m.record_tmp();
    for (k, v) in [("owner", m.owner.as_str()), ("dll", expect_dll), ("root", &root.to_string_lossy()), ("marker", "1"), ("state", "prepared"), ("token", token)] {
        m.step(&format!("record-{k}")).map_err(|e| e.to_string())?;
        reg_set(&tmp, Some(k), v)?;
    }
    m.step("publish")?;
    rename_key(&m.record_parent(), "Migrate.tmp", "Migrate")?;
    m.log("prepare: record published");
    // (3) Old DLLs no longer start the user-mode engine.
    m.step("marker")?;
    fs::write(root.join(ids::UNINSTALL_MARKER), b"1\n").map_err(|e| format!("write marker: {e}"))?;
    m.step("marker-written")?;
    // (4)
    for (pipe, images) in &engines { engine.shutdown(pipe, images)?; }
    m.log("prepare: user-mode engine stopped");
    // (5)
    for (parent, name, step) in m.keys() {
        if exists(&parent, &name)? {
            m.step(&format!("rename-{step}"))?;
            rename_key(&parent, &name, &format!("{name}{SUFFIX}"))?;
        }
    }
    if present(&m.shortcut)? {
        m.step("rename-lnk")?;
        rename_file(&m.shortcut, &renamed(&m.shortcut))?;
    }
    m.log("prepare: user-mode registrations renamed");
    Ok(())
}

fn restore(m: &Mig, token: &str) -> Result<(), String> {
    m.check_identity()?;
    let Some(r) = m.read_record()? else { m.log("restore: no record, nothing to do"); return Ok(()); };
    if r.token != token || r.state != "prepared" { m.log(&format!("restore: record of another transaction or state {}, untouched", r.state)); return Ok(()); }
    m.owned(&r)?;
    let mut problems = vec![];
    for (parent, name, step) in m.keys() {
        let gone = format!("{name}{SUFFIX}");
        match (exists(&parent, &name), exists(&parent, &gone)) {
            (Ok(false), Ok(true)) => if let Err(e) = m.step(&format!("restore-{step}")).and_then(|()| rename_key(&parent, &gone, &name)) { problems.push(e); },
            (Ok(_), Ok(_)) => {},
            (Err(e), _) | (_, Err(e)) => problems.push(e),
        }
    }
    let lnk = &m.shortcut;
    match (present(lnk), present(&renamed(lnk))) {
        (Ok(false), Ok(true)) => if let Err(e) = m.step("restore-lnk").and_then(|()| rename_file(&renamed(lnk), lnk)) { problems.push(e); },
        (Ok(_), Ok(_)) => {},
        (Err(e), _) | (_, Err(e)) => problems.push(e),
    }
    if r.marker == "1" {
        match fs::remove_file(Path::new(&r.root).join(ids::UNINSTALL_MARKER)) {
            Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(e) => problems.push(format!("remove marker: {e}")),
        }
    }
    if problems.is_empty() {
        reg_delete_tree(&m.record_key())?;
        m.log("restore: user-mode installation restored");
        Ok(())
    } else {
        let _ = m.set_state("restore-failed");
        m.log(&format!("restore failed: {}", problems.join("; ")));
        Err(problems.join("; "))
    }
}

/// `token`: the committing transaction; None: a later repair retrying a committed cleanup.
fn finish(m: &Mig, token: Option<&str>) -> Result<(), String> {
    m.check_identity()?;
    let Some(r) = m.read_record()? else { m.log("finish: no record, nothing to do"); return Ok(()); };
    let allowed = match token {
        Some(t) => r.token == t && (r.state == "prepared" || r.state == "committed"),
        None => r.state == "committed",
    };
    if !allowed { m.log(&format!("finish: record state {} / token not ours, untouched", r.state)); return Ok(()); }
    m.owned(&r)?;
    let root = PathBuf::from(&r.root);
    let com = m.com_value()?;
    if legacy::points_to(com.as_deref(), &root) { return Err("HKCU COM points at the user-mode root again; not cleaned".into()); }
    m.step("commit-state")?;
    m.set_state("committed")?;
    let mut problems = vec![];
    for (parent, name, _) in m.keys() {
        if let Err(e) = reg_delete_tree(&format!(r"{parent}\{name}{SUFFIX}")) { problems.push(e); }
    }
    match fs::remove_file(renamed(&m.shortcut)) {
        Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(e) => problems.push(format!("remove shortcut: {e}")),
    }
    if let Err(e) = legacy::cleanup(&root, com.as_deref()) { problems.push(e); }
    // The record goes only when everything is verifiably gone; anything that cannot be inspected counts as left.
    let mut left = present(&root).unwrap_or(true) || present(&renamed(&m.shortcut)).unwrap_or(true);
    for (parent, name, _) in m.keys() { left |= exists(&parent, &format!("{name}{SUFFIX}")).unwrap_or(true); }
    if left || !problems.is_empty() {
        m.log(&format!("finish: leftovers remain, record kept for a later repair ({})", problems.join("; ")));
    } else {
        reg_delete_tree(&m.record_key())?;
        m.log("finish: user-mode installation removed");
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

pub(super) fn run(a: &[String]) -> Result<(), String> {
    let m = Mig::real()?;
    m.log(&format!("{} {}", a.join(" "), machine::identity_line()));
    let token = arg(a, "--token");
    let result = match a[0].as_str() {
        "user-migrate-prepare" => {
            let dll = arg(a, "--expect-dll").ok_or("--expect-dll missing")?;
            prepare(&m, &dll, &token.ok_or("--token missing")?, &mut SystemUninstallEngine)
        }
        "user-migrate-restore" => restore(&m, &token.ok_or("--token missing")?),
        "user-migrate-finish" => finish(&m, token.as_deref()),
        _ => Err("unknown migrate command".into()),
    };
    if let Err(e) = &result { m.log(&format!("error: {e}")); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake { confirm: Result<(), String>, shutdown: Result<(), String>, calls: RefCell<Vec<String>> }
    impl UninstallEngine for Fake {
        fn confirm(&mut self, _: &[PathBuf], _: &[(String, Vec<String>)]) -> Result<(), String> { self.calls.borrow_mut().push("confirm".into()); self.confirm.clone() }
        fn shutdown(&mut self, _: &str, _: &[String]) -> Result<(), String> { self.calls.borrow_mut().push("shutdown".into()); self.shutdown.clone() }
    }
    fn ok() -> Fake { Fake { confirm: Ok(()), shutdown: Ok(()), calls: RefCell::new(vec![]) } }

    /// A private user-mode installation: files below a temp dir, registrations below an HKCU test prefix.
    struct Env { m: Mig, dir: PathBuf, dll: String }
    impl Drop for Env {
        fn drop(&mut self) {
            let _ = reg_delete_tree(self.m.reg.trim_end_matches('\\'));
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
    fn env(tag: &str) -> Env {
        let dir = std::env::temp_dir().join(format!("twime-migrate-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let reg = format!(r"Software\TwImeSetupTest\Migrate-{}-{tag}\", std::process::id());
        let m = Mig {
            reg, local: dir.join("local"), shortcut: dir.join("programs").join("地瓜輸入法 設定.lnk"),
            owner: "S-1-5-21-1-2-3-1001".into(), data: vec![], pipes: vec!["twime-migrate-test-pipe".into()], fail_at: None,
        };
        let root = m.root();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::create_dir_all(m.shortcut.parent().unwrap()).unwrap();
        let dll = root.join("twime_tsf-1-2.dll");
        for f in [dll.clone(), root.join("twime-engined.exe"), root.join("user-data.on"), root.join("assets").join("model.bin")] { fs::write(f, b"x").unwrap(); }
        fs::write(root.join(MANIFEST), b"state=installed\n").unwrap();
        fs::write(&m.shortcut, b"lnk-content").unwrap();
        let com = format!(r"{}\{}", m.com_parent(), Mig::name());
        reg_set(&com, None, "TwPotato TSF").unwrap();
        reg_set(&format!(r"{com}\InprocServer32"), None, &dll.to_string_lossy()).unwrap();
        reg_set(&format!(r"{com}\InprocServer32"), Some("ThreadingModel"), "Apartment").unwrap();
        reg_set(&format!(r"{}\{}\LanguageProfile\0x00000404", m.tip_parent(), Mig::name()), Some("Description"), "tip-value").unwrap();
        Env { dll: dll.to_string_lossy().into_owned(), m, dir }
    }
    /// Snapshot of everything the migration may change, value by value.
    fn state(e: &Env) -> Vec<String> {
        let m = &e.m;
        let com = format!(r"{}\{}", m.com_parent(), Mig::name());
        let tip = format!(r"{}\{}\LanguageProfile\0x00000404", m.tip_parent(), Mig::name());
        let mut v = vec![
            format!("com default={:?}", reg_get(&com, None).unwrap()),
            format!("com dll={:?}", m.com_value().unwrap()),
            format!("com threading={:?}", reg_get(&format!(r"{com}\InprocServer32"), Some("ThreadingModel")).unwrap()),
            format!("tip={:?}", reg_get(&tip, Some("Description")).unwrap()),
        ];
        for (parent, name, _) in m.keys() { v.push(format!("{name} renamed {}", exists(&parent, &format!("{name}{SUFFIX}")).unwrap())); }
        v.push(format!("lnk={:?} renamed {}", fs::read(&m.shortcut).ok(), renamed(&m.shortcut).exists()));
        v.push(format!("marker {}", m.root().join(ids::UNINSTALL_MARKER).exists()));
        v.push(format!("record {:?} tmp {}", m.read_record().unwrap(), reg_exists_checked(HKEY_CURRENT_USER, &m.record_tmp()).unwrap()));
        v.push(format!("root {:?}", fs::read_dir(m.root()).map(|d| d.count()).ok()));
        v
    }

    #[test]
    fn every_refused_check_changes_nothing_also_after_the_rollback_runs() {
        let cases: Vec<(&str, Box<dyn Fn(&Env)>)> = vec![
            ("dll", Box::new(|_| {})),
            ("no-user-data", Box::new(|e| fs::remove_file(e.m.root().join("user-data.on")).unwrap())),
            ("marker", Box::new(|e| fs::write(e.m.root().join(ids::UNINSTALL_MARKER), b"1\n").unwrap())),
            ("record", Box::new(|e| for (k, v) in [("owner", e.m.owner.as_str()), ("root", &e.m.root().to_string_lossy()), ("marker", "1"), ("state", "prepared"), ("token", "t1")] { reg_set(&e.m.record_key(), Some(k), v).unwrap(); })),
            ("leftover", Box::new(|e| fs::write(renamed(&e.m.shortcut), b"old").unwrap())),
            ("leftover-key", Box::new(|e| reg_set(&format!(r"{}\{}{SUFFIX}", e.m.tip_parent(), Mig::name()), Some("x"), "y").unwrap())),
            ("manifest", Box::new(|e| fs::write(e.m.root().join(MANIFEST), b"state=installing\n").unwrap())),
            ("service", Box::new(|_| {})),
        ];
        for (tag, setup) in cases {
            let mut e = env(tag);
            setup(&e);
            if tag == "service" { e.m.owner = "S-1-5-18".into(); }
            let before = state(&e);
            let dll = if tag == "dll" { "C:\\elsewhere\\twime_tsf.dll".to_string() } else { e.dll.clone() };
            assert!(prepare(&e.m, &dll, "t1", &mut ok()).is_err(), "{tag}");
            assert_eq!(state(&e), before, "{tag}");
            // The rollback of the same transaction runs next; it must not touch anything either (for "record": the
            // pre-existing record of token t1 has no renamed resources, and the earlier record stays intact).
            let _ = restore(&e.m, if tag == "record" { "t-other" } else { "t1" });
            assert_eq!(state(&e), before, "{tag} after restore");
        }
        let e = env("busy");
        let before = state(&e);
        let mut busy = Fake { confirm: Err("engine busy".into()), ..ok() };
        assert!(prepare(&e.m, &e.dll, "t1", &mut busy).is_err());
        assert_eq!(*busy.calls.borrow(), ["confirm"]);
        assert_eq!(state(&e), before);
    }

    #[test]
    fn prepare_failing_at_any_step_is_undone_exactly_by_restore() {
        let steps = ["record-owner", "record-token", "publish", "marker", "marker-written", "shutdown", "rename-com", "rename-tip", "rename-lnk", "none"];
        for step in steps {
            let mut e = env(step);
            let before = state(&e);
            e.m.fail_at = if step == "shutdown" || step == "none" { None } else { Some(step) };
            let mut engine = Fake { shutdown: if step == "shutdown" { Err("cannot stop".into()) } else { Ok(()) }, ..ok() };
            let r = prepare(&e.m, &e.dll, "t1", &mut engine);
            assert_eq!(r.is_err(), step != "none", "{step}");
            if step == "none" {
                let s = state(&e);
                assert!(s.iter().any(|x| x == "com dll=None"), "{s:?}");
                assert!(s.iter().any(|x| x == "lnk=None renamed true"), "{s:?}");
                assert!(s.iter().any(|x| x == "marker true"), "{s:?}");
            }
            e.m.fail_at = None;
            // Another transaction's rollback leaves it alone.
            restore(&e.m, "t2").unwrap();
            restore(&e.m, "t1").unwrap();
            let after = state(&e);
            if step == "record-owner" || step == "record-token" || step == "publish" {
                // Never published: the temporary key may remain, but nothing else changed and no record exists.
                assert_eq!(after[..after.len() - 2], before[..before.len() - 2], "{step}");
                assert!(after[after.len() - 2].starts_with("record None"), "{step}");
            } else {
                assert_eq!(after, before, "{step}");
            }
            restore(&e.m, "t1").unwrap(); // idempotent
            assert_eq!(state(&e), after, "{step} second restore");
        }
    }

    #[test]
    fn a_failing_restore_marks_the_record_and_blocks_every_cleanup() {
        let mut e = env("rs-fail");
        prepare(&e.m, &e.dll, "t1", &mut ok()).unwrap();
        e.m.fail_at = Some("restore-tip");
        assert!(restore(&e.m, "t1").is_err());
        e.m.fail_at = None;
        assert_eq!(e.m.read_record().unwrap().unwrap().state, "restore-failed");
        let before = state(&e);
        finish(&e.m, Some("t1")).unwrap();
        finish(&e.m, None).unwrap();
        restore(&e.m, "t1").unwrap();
        assert_eq!(state(&e), before);
        assert!(e.m.root().join("twime-engined.exe").exists());
    }

    #[test]
    fn finish_deletes_only_what_is_no_longer_referenced_and_keeps_the_record_while_leftovers_remain() {
        let mut e = env("fin");
        prepare(&e.m, &e.dll, "t1", &mut ok()).unwrap();
        // A repair without token does not clean an uncommitted record.
        finish(&e.m, None).unwrap();
        assert_eq!(e.m.read_record().unwrap().unwrap().state, "prepared");
        // Writing `committed` fails: nothing is deleted, the record stays prepared.
        let before = state(&e);
        e.m.fail_at = Some("commit-state");
        assert!(finish(&e.m, Some("t1")).is_err());
        e.m.fail_at = None;
        assert_eq!(state(&e), before);
        // An unknown file keeps the root (cleanup removes known entries only): record stays committed.
        fs::write(e.m.root().join("unknown.txt"), b"keep").unwrap();
        finish(&e.m, Some("t1")).unwrap();
        let r = e.m.read_record().unwrap().unwrap();
        assert_eq!(r.state, "committed");
        assert!(e.m.root().join("unknown.txt").exists());
        assert!(!e.m.root().join("twime-engined.exe").exists());
        assert!(!renamed(&e.m.shortcut).exists());
        for (parent, name, _) in e.m.keys() { assert!(!exists(&parent, &format!("{name}{SUFFIX}")).unwrap()); }
        // A committed record is never touched by a rollback.
        restore(&e.m, "t1").unwrap();
        assert_eq!(e.m.read_record().unwrap().unwrap().state, "committed");
        // The later repair finishes the cleanup and deletes the record.
        fs::remove_file(e.m.root().join("unknown.txt")).unwrap();
        finish(&e.m, None).unwrap();
        assert!(!e.m.root().exists());
        assert!(e.m.read_record().unwrap().is_none());
    }

    #[test]
    fn finish_refuses_while_com_points_back_at_the_user_root() {
        let e = env("fin-back");
        prepare(&e.m, &e.dll, "t1", &mut ok()).unwrap();
        let com = format!(r"{}\{}\InprocServer32", e.m.com_parent(), Mig::name());
        reg_set(&com, None, &e.dll).unwrap();
        assert!(finish(&e.m, Some("t1")).is_err());
        assert_eq!(e.m.read_record().unwrap().unwrap().state, "prepared");
        assert!(e.m.root().join("twime-engined.exe").exists());
    }

    #[test]
    fn paths_come_from_the_known_folders_never_from_the_environment() {
        let local = PathBuf::from(r"X:\known\Local");
        let roaming = PathBuf::from(r"X:\known\Roaming");
        let m = Mig::from_known(&local, &roaming, "S-1-5-21-1-2-3-1001".into(), vec![]);
        assert_eq!(m.root(), PathBuf::from(r"X:\known\Local\Programs\TwPotato"));
        assert!(legacy::same_path(&m.shortcut, Path::new(r"X:\known\Roaming\Microsoft\Windows\Start Menu\Programs\地瓜輸入法 設定.lnk")));
        assert_eq!(m.data, vec![local.join("TwPotato"), local.join("TwIme")]);
        // The real constructor uses SHGetKnownFolderPath, whatever the process environment says.
        let real = Mig::real().unwrap();
        assert!(legacy::same_path(&real.root(), &known_folder(&FOLDERID_LocalAppData).unwrap().join("Programs").join("TwPotato")));
        assert!(real.shortcut.starts_with(known_folder(&FOLDERID_RoamingAppData).unwrap()));
        assert_eq!(real.owner, twime_winpipe::current_identity().unwrap().user);
    }

    #[test]
    fn the_current_user_passes_the_owner_checks_through_prepare_restore_and_finish() {
        let mut e = env("me");
        e.m.owner = twime_winpipe::current_identity().unwrap().user;
        let before = state(&e);
        prepare(&e.m, &e.dll, "t1", &mut ok()).unwrap();
        restore(&e.m, "t1").unwrap();
        assert_eq!(state(&e), before);
        prepare(&e.m, &e.dll, "t2", &mut ok()).unwrap();
        finish(&e.m, Some("t2")).unwrap();
        assert!(!e.m.root().exists());
        assert!(e.m.read_record().unwrap().is_none());
        assert!(!e.m.shortcut.exists() && !renamed(&e.m.shortcut).exists());
    }

    #[test]
    fn records_of_another_user_or_root_and_service_identities_are_refused() {
        for (tag, key, value) in [("owner", "owner", "S-1-5-21-9-9-9-1002"), ("root", "root", r"X:\other\AppData\Local\Programs\TwPotato")] {
            let e = env(tag);
            prepare(&e.m, &e.dll, "t1", &mut ok()).unwrap();
            reg_set(&e.m.record_key(), Some(key), value).unwrap();
            let before = state(&e);
            assert!(restore(&e.m, "t1").is_err(), "{tag}");
            assert!(finish(&e.m, Some("t1")).is_err(), "{tag}");
            assert_eq!(state(&e), before, "{tag}");
        }
        let mut e = env("svc");
        prepare(&e.m, &e.dll, "t1", &mut ok()).unwrap();
        e.m.owner = "S-1-5-18".into();
        let before = state(&e);
        assert!(restore(&e.m, "t1").is_err());
        assert!(finish(&e.m, Some("t1")).is_err());
        assert_eq!(state(&e), before);
    }
}
