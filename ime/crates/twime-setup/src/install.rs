use super::*;

struct Backup {
    dst: PathBuf,
    old: PathBuf,
}

fn suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", path.display())),
    }
}

fn collect(problems: &mut Vec<String>, result: Result<(), String>) {
    if let Err(e) = result {
        eprintln!("twime-setup: restoration/cleanup: {e}");
        problems.push(e);
    }
}

/// Only successful renames enter `backups`: a failed rename must never delete the original.
pub(super) struct Files {
    root: PathBuf,
    dll: String,
    backups: Vec<Backup>,
    added: Vec<PathBuf>,
    temps: Vec<PathBuf>,
    stuck: bool,
}

impl Files {
    fn backup(&mut self, dst: &Path) -> Result<(), String> {
        if dst.exists() {
            let old = suffix(dst, ".old-install");
            if old.exists() { return Err(format!("backup already exists: {}", old.display())); }
            fs::rename(dst, &old).map_err(|e| format!("backup {}: {e}", dst.display()))?;
            self.backups.push(Backup { dst: dst.into(), old });
        }
        Ok(())
    }

    fn prepare(&mut self, dst: &Path) -> Result<(), String> {
        self.backup(dst)?;
        self.added.push(dst.into());
        Ok(())
    }

    fn put(&mut self, src: &Path, dst: &Path, fresh_time: bool) -> Result<(), String> {
        self.prepare(dst)?;
        let tmp = suffix(dst, ".tmp-install");
        self.temps.push(tmp.clone());
        if fresh_time {
            fs::read(src).and_then(|b| fs::write(&tmp, b)).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        } else {
            fs::copy(src, &tmp).map_err(|e| format!("copy {}: {e}", src.display()))?;
        }
        fs::rename(&tmp, dst).map_err(|e| format!("rename {}: {e}", dst.display()))
    }

    fn write(&mut self, dst: &Path, bytes: &[u8]) -> Result<(), String> {
        self.prepare(dst)?;
        fs::write(dst, bytes).map_err(|e| format!("write {}: {e}", dst.display()))
    }

    fn rollback(&mut self, problems: &mut Vec<String>) {
        for path in self.temps.iter().chain(self.added.iter()).rev() {
            let result = remove(path).or_else(|e| {
                if self.added.contains(path) && path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("twime_tsf") && n.to_string_lossy().ends_with(".dll")) {
                    fs::rename(path, suffix(path, ".stuck")).map_err(|rename| format!("{e}; rename: {rename}"))?;
                    self.stuck = true;
                    eprintln!("twime-setup: locked DLL retained at {}; keep the root marker until cleanup", suffix(path, ".stuck").display());
                    Ok(())
                } else { Err(e) }
            });
            collect(problems, result);
        }
        for backup in self.backups.iter().rev() {
            // Rename a temporary link; retain every backup until ALL restores succeed.
            let tmp = suffix(&backup.dst, ".tmp-install");
            let result = fs::hard_link(&backup.old, &tmp).and_then(|()| fs::rename(&tmp, &backup.dst));
            collect(problems, result.map_err(|e| format!("restore {}; backup retained at {}: {e}", backup.dst.display(), backup.old.display())));
            collect(problems, remove(&tmp));
        }
        if !problems.is_empty() {
            for backup in &self.backups {
                eprintln!("twime-setup: backup retained at {}", backup.old.display());
            }
        }
    }

    fn discard_backups(&self, problems: &mut Vec<String>) {
        for backup in &self.backups { collect(problems, remove(&backup.old)); }
    }

    fn cleanup(&mut self) -> Result<(), String> {
        let mut problems = vec![];
        // Includes leftovers made by the previous copy_into (with_extension) implementation.
        for dir in [self.root.clone(), self.root.join("assets")] {
            for entry in fs::read_dir(&dir).map_err(|e| format!("list {}: {e}", dir.display()))? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_name().to_string_lossy().ends_with(".tmp-install") {
                    collect(&mut problems, remove(&entry.path()));
                }
            }
        }
        self.discard_backups(&mut problems);
        if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
    }
}

pub(super) trait Hooks {
    fn is_overwrite(&self) -> bool { false }
    fn apply(&mut self, files: &mut Files) -> Result<(), String>;
    fn rollback(&mut self, problems: &mut Vec<String>);
    fn after_commit(&mut self) -> Result<(), String> { Ok(()) }
    fn after_rollback(&mut self, _registration_restored: bool, _problems: &mut Vec<String>) {}
    // No-op in production; tests inspect ordering and inject finalization failures here.
    fn checkpoint(&mut self, _step: &str, _files: &Files) -> Result<(), String> { Ok(()) }
}

/// Read-only preflight: never let a new transaction clear an earlier transaction's protection.
pub(super) fn reject_unfinished_install(root: &Path) -> Result<(), String> {
    fn scan(dir: &Path, leftovers: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("list {}: {e}", dir.display())),
        };
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name().to_string_lossy().ends_with(".old-install") {
                leftovers.push(entry.path());
            } else if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                scan(&entry.path(), leftovers)?;
            }
        }
        Ok(())
    }
    let mut leftovers = vec![];
    scan(root, &mut leftovers)?;
    let manifest = root.join(MANIFEST);
    match fs::read(&manifest) {
        Ok(bytes) if String::from_utf8_lossy(&bytes).lines().any(|line| line.trim() == "state=installing") => leftovers.push(manifest),
        Ok(_) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(format!("read manifest: {e}")),
    }
    if leftovers.is_empty() { return Ok(()); }
    let marker = root.join(ids::UNINSTALL_MARKER);
    if marker.exists() { leftovers.push(marker); }
    leftovers.sort();
    Err(format!("偵測到前次未完成的安裝交易，拒絕安裝；未變更任何檔案或標記。\n前次留下的檔案：\n{}\n請先關閉使用安裝檔案的程式，手動由 .old-install 備份還原對應原檔並確認檔案與登錄均已還原，再刪除備份；若 manifest 為 state=installing，請先完成還原。全部處理完成後才可移除 uninstalling 標記並重新安裝。",
        leftovers.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")))
}

/// The test caller supplies a private root, fake stop and hooks with no registry/TSF operations.
pub(super) fn install_files_in(
    root: &Path, bin: &Path, assets: &Path, settings_exe: Option<&Path>,
    stop: &dyn Fn() -> Result<bool, String>, hooks: &mut dyn Hooks,
) -> Result<(), String> {
    reject_unfinished_install(root)?;
    if legacy::residue(root)? { legacy::cleanup(root, None)?; }
    let root_existed = root.exists();
    let assets_existed = root.join("assets").exists();
    let manifest = root.join(MANIFEST);
    let previous = match fs::read(&manifest) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("read manifest: {e}")),
    };
    let marker = root.join(ids::UNINSTALL_MARKER);
    let mut files = Files {
        root: root.into(),
        dll: format!("twime_tsf-{}-{}.dll", std::process::id(), std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())),
        backups: vec![], added: vec![], temps: vec![], stuck: false,
    };
    let mut hooks_started = false;
    let steps = if previous.is_some() || hooks.is_overwrite() { "dirs,files,com,tsf" } else { "dirs,files,com,tsf,enable" };
    let result: Result<(), String> = (|| {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        // Block restarts and sessions throughout installation and restoration.
        fs::write(&marker, b"1\n").map_err(|e| format!("write marker: {e}"))?;
        // 配合引擎「取得鎖後再檢查標記」：寫入標記後，若 pipe 與 running.lock 都不存在，
        // 任何仍在啟動的引擎取得鎖後都會因標記退出，不會載入資產。
        stop()?; // Not found is OK. Any identification/termination error aborts before replacement.
        fs::create_dir_all(root.join("assets")).map_err(|e| e.to_string())?;
        files.write(&manifest, format!("state=installing\ndll={}\n", files.dll).as_bytes())?;
        files.write(&root.join("user-data.on"), b"")?;
        files.put(&bin.join(ids::DLL_NAME), &root.join(files.dll.clone()), false)?;
        files.put(&bin.join(ids::ENGINE_NAME), &root.join(ids::ENGINE_NAME), false)?;
        if let Some(exe) = settings_exe { files.put(exe, &root.join(SETTINGS_EXE), false)?; }
        for name in ["data.txt", "common-readings.txt", "model.bin", "prior.pack"] {
            files.put(&assets.join(name), &root.join("assets").join(name), false)?;
        }
        // Disable old optional settings first and publish new settings last with a fresh mtime.
        let pn = root.join("assets").join(PN_FILES[2]);
        files.backup(&pn)?;
        if assets.join(PN_FILES[2]).exists() {
            for name in &PN_FILES[..2] { files.put(&assets.join(name), &root.join("assets").join(name), false)?; }
            files.put(&assets.join(PN_FILES[2]), &pn, true)?;
        }
        // SRCH mode 0 general-word layer: the engine loads assets\gen.{fst,lst} only as a complete pair at startup (the
        // marker blocks starts during installation). A source without the complete pair removes an installed one.
        for name in GEN_FILES { files.backup(&root.join("assets").join(name))?; }
        if GEN_FILES.iter().all(|name| assets.join(name).is_file()) {
            for name in GEN_FILES { files.put(&assets.join(name), &root.join("assets").join(name), false)?; }
        }
        // EMOJI default-on: emoji.tsv is optional; a source without it removes an installed one (like gen.*).
        let emoji = root.join("assets").join(EMOJI_FILE);
        files.backup(&emoji)?;
        if assets.join(EMOJI_FILE).is_file() { files.put(&assets.join(EMOJI_FILE), &emoji, false)?; }
        // WORDBONUS: both files optional, a source without them removes installed ones; they are written before model.cfg,
        // which publishes the `wordbonus=` key last (the engine never sees the key without the file).
        for name in WORDBONUS_FILES {
            let dst = root.join("assets").join(name);
            files.backup(&dst)?;
            if assets.join(name).is_file() { files.put(&assets.join(name), &dst, false)?; }
        }
        let cfg = root.join("assets/model.cfg");
        files.backup(&cfg)?;
        if assets.join("model.cfg").exists() { files.put(&assets.join("model.cfg"), &cfg, true)?; }
        match fs::read(assets.join("dev-mode")) {
            Ok(bytes) => files.write(&root.join("dev-mode"), &bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("read dev-mode: {e}")),
        }
        hooks_started = true;
        hooks.apply(&mut files)?;
        fs::write(&manifest, format!("state=installed\ndll={}\nroot={}\nsteps={steps}\n", files.dll, root.display()))
            .map_err(|e| format!("write installed manifest: {e}"))?;
        // Commit: no subsequent failure may trigger restoration.
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("install failed ({e}); restoring previous installation");
        let mut problems = vec![];
        if hooks_started { hooks.rollback(&mut problems); }
        let registration_restored = problems.is_empty();
        if registration_restored {
            files.rollback(&mut problems);
            collect(&mut problems, hooks.checkpoint("restoration complete", &files));
        }
        hooks.after_rollback(registration_restored, &mut problems);
        if problems.is_empty() {
            // Restoration is complete; disposal is only cleanup.
            let mut cleanup_problems = vec![];
            files.discard_backups(&mut cleanup_problems);
            for e in cleanup_problems { eprintln!("twime-setup: warning: {e}"); }
            if !files.stuck { collect(&mut problems, remove(&marker)); }
            if !assets_existed { let _ = fs::remove_dir(root.join("assets")); }
            if !root_existed && problems.is_empty() {
                // Only empty directories created by this transaction; never remove unknown files.
                let _ = fs::remove_dir(root);
            }
        }
        if !problems.is_empty() {
            eprintln!("twime-setup: restoration incomplete; keep {} in place to block engine startup. Resolve the restoration errors above before removing this marker.", marker.display());
            for backup in &files.backups {
                eprintln!("twime-setup: manually restore {} from {}; close processes using the target first. Keep backups until all files and registration are restored.", backup.dst.display(), backup.old.display());
            }
        }
        return Err(if problems.is_empty() { e } else { format!("{e}; restoration errors: {}", problems.join("; ")) });
    }
    for result in [hooks.checkpoint("installed manifest", &files), files.cleanup(),
        hooks.checkpoint("cleanup complete", &files)] {
        if let Err(e) = result { eprintln!("twime-setup: warning: installation committed; {e}"); }
    }
    if let Err(e) = hooks.after_commit() { eprintln!("twime-setup: warning: installation committed; {e}"); }
    remove(&marker).map_err(|e| format!("安裝已完成，但 uninstalling 標記無法刪除；請手動刪除 {} 後即可使用（{e}）", marker.display()))?;
    println!("installed: {steps}");
    Ok(())
}

// Preserve the exact registry type/bytes, including absence. Read errors abort before file changes.
type RawValue = (windows::Win32::System::Registry::REG_VALUE_TYPE, Vec<u8>);

struct SavedValue {
    sub: String,
    name: Option<String>,
    value: Option<RawValue>,
}

impl SavedValue {
    fn read(sub: String, name: Option<&str>) -> Result<Self, String> {
        Self::read_in(HKEY_CURRENT_USER, sub, name)
    }

    fn read_in(hive: HKEY, sub: String, name: Option<&str>) -> Result<Self, String> {
        use windows::Win32::System::Registry::RegQueryValueExW;
        let mut saved = Self { sub, name: name.map(String::from), value: None };
        unsafe {
            let mut h = HKEY::default();
            let e = RegOpenKeyExW(hive, PCWSTR(wide(&saved.sub).as_ptr()), None, KEY_READ, &mut h);
            if e.0 == 2 { return Ok(saved); }
            if e.0 != 0 { return Err(format!("snapshot {}: error {}", saved.sub, e.0)); }
            let n = name.map(wide);
            let n = n.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
            let mut kind = REG_SZ;
            let mut len = 0;
            let e = RegQueryValueExW(h, n, None, Some(&mut kind), None, Some(&mut len));
            if e.0 == 2 { let _ = RegCloseKey(h); return Ok(saved); }
            if e.0 != 0 { let _ = RegCloseKey(h); return Err(format!("snapshot {}: error {}", saved.sub, e.0)); }
            let mut bytes = vec![0; len as usize];
            let e = RegQueryValueExW(h, n, None, Some(&mut kind), Some(bytes.as_mut_ptr()), Some(&mut len));
            let _ = RegCloseKey(h);
            if e.0 != 0 { return Err(format!("snapshot {}: error {}", saved.sub, e.0)); }
            bytes.truncate(len as usize);
            saved.value = Some((kind, bytes));
        }
        Ok(saved)
    }

    fn restore(&self) -> Result<(), String> {
        use windows::Win32::System::Registry::RegDeleteValueW;
        unsafe {
            let mut h = HKEY::default();
            let e = RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide(&self.sub).as_ptr()), None, KEY_WRITE, &mut h);
            if e.0 == 2 && self.value.is_none() { return Ok(()); }
            if e.0 != 0 { return Err(format!("restore {}: error {}", self.sub, e.0)); }
            let n = self.name.as_deref().map(wide);
            let n = n.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
            let e = match &self.value {
                Some((kind, bytes)) => RegSetValueExW(h, n, None, *kind, Some(bytes)),
                None => RegDeleteValueW(h, n),
            };
            let _ = RegCloseKey(h);
            if e.0 == 0 || (e.0 == 2 && self.value.is_none()) { Ok(()) }
            else { Err(format!("restore {}: error {}", self.sub, e.0)) }
        }
    }
}

fn text_value(value: Option<&RawValue>) -> Result<Option<String>, String> {
    let Some((kind, bytes)) = value else { return Ok(None); };
    if *kind != REG_SZ || bytes.len() % 2 != 0 { return Err("registry string has unexpected type/length".into()); }
    let words: Vec<_> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let end = words.iter().position(|c| *c == 0).unwrap_or(words.len());
    String::from_utf16(&words[..end]).map(Some).map_err(|e| e.to_string())
}

pub(super) fn com_target() -> Result<Option<String>, String> {
    text_value(SavedValue::read(format!(r"{}\InprocServer32", clsid_key()), None)?.value.as_ref())
}

#[derive(Debug, PartialEq, Eq)]
struct ProfileParameters { description: String, icon: String, index: u32 }

/// Pure conversion; absent values remain absent in the snapshot and become empty/zero API arguments.
fn profile_parameters(machine: &[Option<RawValue>; 3], user: &[Option<RawValue>; 3]) -> Result<ProfileParameters, String> {
    use windows::Win32::System::Registry::REG_DWORD;
    let get = |i: usize| machine[i].as_ref().or(user[i].as_ref());
    let index = match get(2) {
        None => 0,
        Some((kind, b)) if *kind == REG_DWORD && b.len() == 4 => u32::from_le_bytes(b.as_slice().try_into().unwrap()),
        _ => return Err("IconIndex has unexpected type/length".into()),
    };
    Ok(ProfileParameters { description: text_value(get(0))?.unwrap_or_default(), icon: text_value(get(1))?.unwrap_or_default(), index })
}

struct ProfileSnapshot { parameters: ProfileParameters, user: Vec<SavedValue> }
impl ProfileSnapshot {
    fn read(tip: &str) -> Result<Self, String> {
        Self::read_in(HKEY_LOCAL_MACHINE, tip, tip)
    }
    fn read_in(machine_hive: HKEY, machine_tip: &str, user_tip: &str) -> Result<Self, String> {
        let path = |tip: &str| format!(r"{tip}\LanguageProfile\0x00000404\{}", ids::guid_string(ids::PROFILE));
        let mut user = vec![];
        let mut machine = vec![];
        for name in ["Description", "IconFile", "IconIndex"] {
            machine.push(SavedValue::read_in(machine_hive, path(machine_tip), Some(name))?.value);
            user.push(SavedValue::read(path(user_tip), Some(name))?);
        }
        let effective = profile_parameters(&machine.try_into().unwrap(), &std::array::from_fn(|i| user[i].value.clone()))?;
        Ok(Self { parameters: effective, user })
    }
}

/// Wraps the existing transaction; stop writes the old marker before touching the old engine.
pub(super) struct LegacyHooks<'a> {
    pub inner: &'a mut dyn Hooks,
    pub old_root: &'a Path,
    pub old_shortcut: Option<PathBuf>,
    pub marker_written: &'a std::cell::Cell<bool>,
    pub com: &'a dyn Fn() -> Result<Option<String>, String>,
    pub migrate: &'a dyn Fn() -> Vec<String>,
}
impl Hooks for LegacyHooks<'_> {
    fn is_overwrite(&self) -> bool { self.inner.is_overwrite() }
    fn apply(&mut self, files: &mut Files) -> Result<(), String> {
        if let Some(path) = &self.old_shortcut { files.backup(path)?; }
        self.inner.apply(files)
    }
    fn rollback(&mut self, problems: &mut Vec<String>) { self.inner.rollback(problems); }
    fn checkpoint(&mut self, step: &str, files: &Files) -> Result<(), String> { self.inner.checkpoint(step, files) }
    fn after_rollback(&mut self, restored: bool, problems: &mut Vec<String>) {
        self.inner.after_rollback(restored, problems);
        if self.marker_written.get() {
            if restored { collect(problems, remove(&self.old_root.join(ids::UNINSTALL_MARKER))); }
            else { eprintln!("twime-setup: registration restoration failed; keep {}", self.old_root.join(ids::UNINSTALL_MARKER).display()); }
        }
    }
    fn after_commit(&mut self) -> Result<(), String> {
        let mut problems = vec![];
        collect(&mut problems, self.inner.after_commit());
        collect(&mut problems, (self.com)().and_then(|com| legacy::cleanup(self.old_root, com.as_deref())));
        problems.extend((self.migrate)());
        if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
    }
}

pub(super) struct SystemInstall {
    key: String,
    overwrite: bool,
    shortcut: Option<PathBuf>,
    values: Vec<SavedValue>,
    class_existed: bool,
    inproc_existed: bool,
    com_started: bool,
    tsf_started: bool,
    enable_started: bool,
    profile: ProfileSnapshot,
}

impl SystemInstall {
    pub(super) fn new(overwrite: bool, settings: bool) -> Result<Self, String> {
        let key = clsid_key();
        let inproc = format!(r"{key}\InprocServer32");
        Ok(Self {
            key: key.clone(),
            overwrite,
            shortcut: if settings { Some(settings_shortcut()?) } else { None },
            values: vec![SavedValue::read(key.clone(), None)?, SavedValue::read(inproc.clone(), None)?,
                SavedValue::read(inproc.clone(), Some("ThreadingModel"))?],
            class_existed: reg_exists(&key), inproc_existed: reg_exists(&inproc),
            com_started: false, tsf_started: false, enable_started: false,
            profile: ProfileSnapshot::read(&tip_path(HKCU_TIP_PREFIX))?,
        })
    }
}

fn enable_for_fresh_install(
    overwrite: bool, started: &mut bool, enable: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if overwrite { return Ok(()); }
    *started = true;
    enable()
}

impl Hooks for SystemInstall {
    fn is_overwrite(&self) -> bool { self.overwrite }
    fn apply(&mut self, files: &mut Files) -> Result<(), String> {
        self.apply_in(files, &mut tsf_register, &mut || install_layout_or_tip(0))
    }

    fn rollback(&mut self, problems: &mut Vec<String>) {
        let machine = match machine_installed() {
            Ok(machine) => machine,
            Err(e) => { problems.push(e); true }, // Unknown machine state must protect global TSF registration.
        };
        self.rollback_in(machine, &mut SystemRegistration, &mut |p| register_profile(&p.description, &p.icon, p.index), problems);
    }
}

impl SystemInstall {
    fn apply_in(&mut self, files: &mut Files, register: &mut dyn FnMut(&Path) -> Result<(), String>,
        enable: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        if let Some(shortcut) = &self.shortcut {
            files.prepare(shortcut)?;
            create_settings_shortcut(&files.root, shortcut)?;
        }
        let key = &self.key;
        self.com_started = true;
        reg_set(key, None, "TwPotato TSF text input processor")?;
        reg_set(&format!(r"{key}\InprocServer32"), None, &files.root.join(&files.dll).to_string_lossy())?;
        reg_set(&format!(r"{key}\InprocServer32"), Some("ThreadingModel"), "Apartment")?;
        self.tsf_started = true; // Undo partial TSF registration too, for a fresh install.
        register(&files.root.join(&files.dll))?;
        enable_for_fresh_install(self.overwrite, &mut self.enable_started, enable)
    }
}

impl SystemInstall {
    fn rollback_in(&mut self, machine: bool, registration: &mut dyn RegistrationRemoval,
        restore_profile: &mut dyn FnMut(&ProfileParameters) -> Result<(), String>, problems: &mut Vec<String>,
    ) {
        if !self.overwrite {
            problems.extend(remove_registration(machine, self.enable_started, self.tsf_started,
                &if self.tsf_started { tip_records() } else { vec![] }, registration));
        } else if self.tsf_started {
            if !machine { collect(problems, restore_profile(&self.profile.parameters)); }
            for value in &self.profile.user { collect(problems, value.restore()); }
        }
        if self.com_started {
            if !self.class_existed {
                collect(problems, reg_delete_tree(&self.key));
            } else {
                for value in self.values.iter().rev() { collect(problems, value.restore()); }
                if !self.inproc_existed {
                    collect(problems, reg_delete_tree(&format!(r"{}\InprocServer32", self.key)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::BTreeMap, os::windows::fs::OpenOptionsExt};

    fn raw_text(s: &str) -> Option<RawValue> { Some((REG_SZ, wide(s).iter().flat_map(|c| c.to_le_bytes()).collect())) }

    #[test]
    fn profile_rollback_parameters_prefer_machine_values_and_default_missing_values() {
        use windows::Win32::System::Registry::REG_DWORD;
        let absent = [None, None, None];
        assert_eq!(profile_parameters(&absent, &absent).unwrap(), ProfileParameters { description: "".into(), icon: "".into(), index: 0 });
        let user = [raw_text("舊輸入法"), raw_text(r"C:\private\old.dll"), Some((REG_DWORD, 7u32.to_le_bytes().into()))];
        let machine = [raw_text("全機輸入法"), None, Some((REG_DWORD, 3u32.to_le_bytes().into()))];
        assert_eq!(profile_parameters(&absent, &user).unwrap(), ProfileParameters { description: "舊輸入法".into(), icon: r"C:\private\old.dll".into(), index: 7 });
        assert_eq!(profile_parameters(&machine, &user).unwrap(), ProfileParameters { description: "全機輸入法".into(), icon: r"C:\private\old.dll".into(), index: 3 });
        assert!(profile_parameters(&[None, None, raw_text("bad index")], &user).is_err());
        assert!(profile_parameters(&[Some((REG_SZ, vec![1])), None, None], &absent).is_err());
        let icon = wide(r"C:\private\地瓜\twime_tsf-new.dll");
        assert_eq!(icon.last(), Some(&0)); assert!(!icon[..icon.len()-1].contains(&0));
        assert_eq!(String::from_utf16(&icon[..icon.len()-1]).unwrap(), r"C:\private\地瓜\twime_tsf-new.dll");
    }

    fn mock_system(overwrite: bool) -> SystemInstall {
        SystemInstall { key: format!(r"Software\TwPotatoPrivateRollbackTest{}\CLSID", std::process::id()), overwrite, shortcut: None, values: vec![],
            class_existed: false, inproc_existed: false, com_started: false, tsf_started: true, enable_started: true,
            profile: ProfileSnapshot { parameters: ProfileParameters { description: "old".into(), icon: "old.dll".into(), index: 9 }, user: vec![] } }
    }

    #[test]
    fn rollback_machine_mode_never_calls_tsf_or_deletes_hklm() {
        for overwrite in [false, true] {
            let mut system = mock_system(overwrite);
            let mut ops = crate::tests::RecordingRegistration::default(); let mut problems = vec![];
            system.rollback_in(true, &mut ops, &mut |_| panic!("machine mode must never call RegisterProfile"), &mut problems);
            assert!(problems.is_empty());
            if overwrite { assert!(ops.calls.is_empty()); }
            else { assert_eq!(ops.calls, [format!("HKCU {}", tip_path(HKCU_TIP_PREFIX))]); }
        }
    }

    #[test]
    fn overwrite_rollback_calls_register_profile_with_saved_parameters_and_reports_failure() {
        for fail in [false, true] {
            let mut system = mock_system(true); let mut ops = crate::tests::RecordingRegistration::default(); let mut problems = vec![];
            let mut calls = 0;
            system.rollback_in(false, &mut ops, &mut |p| {
                calls += 1; assert_eq!(p, &ProfileParameters { description: "old".into(), icon: "old.dll".into(), index: 9 });
                if fail { Err("injected RegisterProfile restoration failure".into()) } else { Ok(()) }
            }, &mut problems);
            assert_eq!(calls, 1); assert_eq!(problems.len(), if fail { 1 } else { 0 }); assert!(ops.calls.is_empty());
        }
    }

    #[test]
    fn overwrite_profile_and_com_restore_exact_values_under_private_hkcu_prefix() {
        let f = Fixture::new(true);
        let before = snapshot(&f.root);
        let old_dll = f.root.join("twime_tsf-old.dll").to_string_lossy().into_owned();
        let base = format!(r"Software\TwPotatoProfileUnitTest{}", std::process::id());
        struct RegistryGuard(String);
        impl Drop for RegistryGuard { fn drop(&mut self) { let _ = reg_delete_tree(&self.0); } }
        // Never constructs SystemInstall::new or calls a real registration API.
        let _guard = RegistryGuard(base.clone());
        let key = format!(r"{base}\CLSID"); let inproc = format!(r"{key}\InprocServer32");
        let tip = format!(r"{base}\TIP"); let profile_key = format!(r"{tip}\LanguageProfile\0x00000404\{}", ids::guid_string(ids::PROFILE));
        reg_set(&key, None, "old class").unwrap(); reg_set(&inproc, None, &old_dll).unwrap();
        reg_set(&profile_key, Some("Description"), "舊描述").unwrap();
        // IconFile and IconIndex are intentionally absent: rollback must remove new values.
        let mut system = SystemInstall { key: key.clone(), overwrite: true, shortcut: None,
            values: vec![SavedValue::read(key.clone(), None).unwrap(), SavedValue::read(inproc.clone(), None).unwrap(), SavedValue::read(inproc.clone(), Some("ThreadingModel")).unwrap()],
            class_existed: true, inproc_existed: true, com_started: true, tsf_started: true, enable_started: false,
            profile: ProfileSnapshot::read_in(HKEY_CURRENT_USER, &format!(r"{base}\AbsentMachine"), &tip).unwrap() };
        struct PrivateSystem<'a> { system: &'a mut SystemInstall, profile_key: &'a str, registered: bool, restored: bool }
        impl Hooks for PrivateSystem<'_> {
            fn is_overwrite(&self) -> bool { true }
            fn apply(&mut self, files: &mut Files) -> Result<(), String> {
                let inproc = format!(r"{}\InprocServer32", self.system.key);
                self.system.apply_in(files, &mut |dll| {
                    assert!(dll.is_file());
                    assert_eq!(text_value(SavedValue::read(inproc.clone(), None)?.value.as_ref())?.as_deref(), Some(dll.to_string_lossy().as_ref()));
                    reg_set(self.profile_key, Some("Description"), ids::DESCRIPTION)?;
                    reg_set(self.profile_key, Some("IconFile"), &dll.to_string_lossy())?;
                    SavedValue { sub: self.profile_key.into(), name: Some("IconIndex".into()),
                        value: Some((windows::Win32::System::Registry::REG_DWORD, 0u32.to_le_bytes().into())) }.restore()?;
                    self.registered = true;
                    Ok(())
                }, &mut || panic!("overwrite must not enable a profile"))?;
                Err("injected failure after RegisterProfile before commit".into())
            }
            fn rollback(&mut self, problems: &mut Vec<String>) {
                self.system.rollback_in(false, &mut crate::tests::RecordingRegistration::default(), &mut |p| {
                    assert_eq!(p, &ProfileParameters { description: "舊描述".into(), icon: "".into(), index: 0 });
                    // A fake RegisterProfile writes values; SavedValue must restore even absence afterwards.
                    reg_set(self.profile_key, Some("Description"), &p.description)?;
                    reg_set(self.profile_key, Some("IconFile"), &p.icon)?;
                    self.restored = true; Ok(())
                }, problems);
            }
        }
        let mut hooks = PrivateSystem { system: &mut system, profile_key: &profile_key, registered: false, restored: false };
        let error = install_files_in(&f.root, &f.bin, &f.assets, None, &|| Ok(false), &mut hooks).unwrap_err();
        assert!(error.contains("injected failure after RegisterProfile"));
        assert!(hooks.registered && hooks.restored); assert_eq!(snapshot(&f.root), before);
        assert_eq!(text_value(SavedValue::read(inproc.clone(), None).unwrap().value.as_ref()).unwrap().as_deref(), Some(old_dll.as_str()));
        assert!(Path::new(&old_dll).is_file());
        assert!(SavedValue::read(inproc, Some("ThreadingModel")).unwrap().value.is_none());
        assert_eq!(text_value(SavedValue::read(profile_key.clone(), Some("Description")).unwrap().value.as_ref()).unwrap().as_deref(), Some("舊描述"));
        for name in ["IconFile", "IconIndex"] { assert!(SavedValue::read(profile_key.clone(), Some(name)).unwrap().value.is_none()); }
    }

    struct UpgradeMock {
        shortcut: PathBuf, com: std::rc::Rc<std::cell::RefCell<String>>, old_com: String,
        fail: bool, fail_restore: bool,
    }
    impl Hooks for UpgradeMock {
        fn is_overwrite(&self) -> bool { true }
        fn apply(&mut self, files: &mut Files) -> Result<(), String> {
            files.write(&self.shortcut, b"new shortcut")?;
            *self.com.borrow_mut() = files.root.join(&files.dll).to_string_lossy().into_owned();
            assert!(Path::new(&*self.com.borrow()).is_file());
            if self.fail { Err("injected failure after COM/profile switch before commit".into()) } else { Ok(()) }
        }
        fn rollback(&mut self, problems: &mut Vec<String>) {
            if self.fail_restore { problems.push("injected registration restoration failure".into()); }
            else { *self.com.borrow_mut() = self.old_com.clone(); }
        }
    }

    fn run_upgrade(fail: bool, fail_restore: bool, stop_failure: bool) {
        let f = Fixture::new(false); fs::remove_dir_all(&f.root).unwrap();
        let old = f.dir.join("legacy"); fs::create_dir_all(old.join("assets")).unwrap();
        for name in ["twime_tsf-old.dll", ids::ENGINE_NAME, SETTINGS_EXE, MANIFEST, "user-data.on"] { fs::write(old.join(name), b"old").unwrap(); }
        fs::write(old.join("assets/model.bin"), b"old model").unwrap();
        let old_before = snapshot(&old);
        let old_shortcut = f.dir.join("TwIme 設定.lnk"); let new_shortcut = f.dir.join("地瓜輸入法 設定.lnk");
        fs::write(&old_shortcut, b"old shortcut").unwrap();
        let old_com = old.join("twime_tsf-old.dll").to_string_lossy().into_owned();
        let com = std::rc::Rc::new(std::cell::RefCell::new(old_com.clone()));
        let mut inner = UpgradeMock { shortcut: new_shortcut.clone(), com: com.clone(), old_com: old_com.clone(), fail, fail_restore };
        let marked = Cell::new(false); let migrated = Cell::new(false);
        let get_com = || Ok(Some(com.borrow().clone()));
        let migrate = || {
            assert!(f.root.join(ids::UNINSTALL_MARKER).exists()); assert!(!old.join(ids::ENGINE_NAME).exists());
            migrated.set(true); vec!["injected post-commit copy warning".into()]
        };
        let mut hooks = LegacyHooks { inner: &mut inner, old_root: &old, old_shortcut: Some(old_shortcut.clone()),
            marker_written: &marked, com: &get_com, migrate: &migrate };
        // Simulates a host retaining an old DLL: cleanup still removes the engine/settings/manifest.
        let lock = fs::OpenOptions::new().read(true).share_mode(1).open(old.join("twime_tsf-old.dll")).unwrap();
        let result = install_files_in(&f.root, &f.bin, &f.assets, Some(&f.settings), &|| {
            assert!(f.root.join(ids::UNINSTALL_MARKER).exists());
            fs::write(old.join(ids::UNINSTALL_MARKER), b"1\n").unwrap(); marked.set(true);
            if stop_failure { Err("injected stop failure".into()) } else { Ok(true) }
        }, &mut hooks);
        if fail || stop_failure {
            assert!(result.is_err()); assert!(!migrated.get());
            if fail_restore && !stop_failure {
                assert!(legacy::points_to(Some(&com.borrow()), &f.root));
                assert!(Path::new(&*com.borrow()).is_file());
                for name in TARGETS { assert!(f.root.join(name).is_file(), "{name}"); }
                assert!(f.root.join(MANIFEST).exists());
                assert_eq!(fs::read(&new_shortcut).unwrap(), b"new shortcut");
                assert!(suffix(&old_shortcut, ".old-install").exists());
                assert!(f.root.join(ids::UNINSTALL_MARKER).exists());
                assert!(old.join(ids::UNINSTALL_MARKER).exists());
                assert_eq!(legacy::manifest(&f.root).unwrap(), legacy::Manifest::Other);
                assert!(fs::read_to_string(f.root.join(MANIFEST)).unwrap().contains("state=installing"));
                // The command entry point must preserve both roots, shortcuts and all backups.
                let before_cleanup = snapshot(&f.dir);
                let error = legacy::cleanup_inactive(&f.root, &old, Some(&com.borrow())).unwrap_err();
                assert!(error.contains("HKCU COM=") && error.contains("state=installing"), "{error}");
                assert_eq!(snapshot(&f.dir), before_cleanup);
            } else {
                assert_eq!(*com.borrow(), old_com);
                assert!(!new_shortcut.exists()); assert_eq!(fs::read(&old_shortcut).unwrap(), b"old shortcut");
                assert!(!old.join(ids::UNINSTALL_MARKER).exists());
                assert_eq!(snapshot(&old), old_before);
                assert!(!f.root.exists());
            }
        } else {
            result.unwrap(); assert!(migrated.get()); assert!(!f.root.join(ids::UNINSTALL_MARKER).exists());
            assert!(fs::read_to_string(f.root.join(MANIFEST)).unwrap().contains("steps=dirs,files,com,tsf\n"));
            assert!(old.join(ids::UNINSTALL_MARKER).exists());
            assert!(!old.join(MANIFEST).exists() && !old.join("assets").exists());
            assert!(f.root.join(ids::ENGINE_NAME).exists()); assert_eq!(fs::read(&new_shortcut).unwrap(), b"new shortcut");
            assert!(!old_shortcut.exists() && !suffix(&old_shortcut, ".old-install").exists());
            assert!(legacy::points_to(Some(&com.borrow()), &f.root));
            assert_eq!(legacy::active(Some(&com.borrow()), &f.root, &old, legacy::manifest(&f.root).unwrap(), legacy::manifest(&old).unwrap(), false, false).unwrap(), legacy::Active::Current);
        }
        drop(lock);
    }

    #[test]
    fn legacy_upgrade_commits_cleans_old_root_and_switches_shortcuts_under_markers() { run_upgrade(false, false, false); }
    #[test]
    fn legacy_upgrade_failure_after_com_switch_restores_shortcut_and_removes_new_root() { run_upgrade(true, false, false); }
    #[test]
    fn legacy_upgrade_failed_registration_restore_keeps_both_markers() { run_upgrade(true, true, false); }
    #[test]
    fn legacy_upgrade_stop_failure_removes_old_marker_even_before_hooks_start() { run_upgrade(false, false, true); }

    const OLD_MANIFEST: &[u8] = b"state=installed\r\ndll=twime_tsf-old.dll\r\ncustom=keep verbatim\r\n";
    #[test]
    fn loaded_new_dll_rollback_residue_can_be_cleaned_or_upgraded_while_loaded() {
        use windows::Win32::Foundation::{FreeLibrary, HMODULE};
        struct Loaded { module: Option<HMODULE> }
        impl Drop for Loaded {
            fn drop(&mut self) { if let Some(module) = self.module.take() { unsafe { FreeLibrary(module).unwrap(); } } }
        }
        impl Hooks for Loaded {
            fn apply(&mut self, files: &mut Files) -> Result<(), String> {
                self.module = Some(unsafe { LoadLibraryW(PCWSTR(wide(&files.root.join(&files.dll).to_string_lossy()).as_ptr())).unwrap() });
                Err("failure after loading new DLL".into())
            }
            fn rollback(&mut self, _: &mut Vec<String>) {}
        }
        for cleanup_first in [false, true] {
            let f = Fixture::new(false); fs::remove_dir_all(&f.root).unwrap();
            // Use a harmless system DLL; never load the TSF implementation or call TSF APIs.
            fs::copy(PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/version.dll"), f.bin.join(ids::DLL_NAME)).unwrap();
            let old = f.dir.join("legacy"); fs::create_dir(&old).unwrap();
            fs::write(old.join(MANIFEST), b"state=installed\n").unwrap();
            let com = old.join("twime_tsf-old.dll").to_string_lossy().into_owned();
            let mut loaded = Loaded { module: None };
            let error = install_files_in(&f.root, &f.bin, &f.assets, Some(&f.settings), &|| Ok(true), &mut loaded).unwrap_err();
            assert!(!error.contains("restoration errors"), "{error}");
            assert!(legacy::residue(&f.root).unwrap());
            let entries = fs::read_dir(&f.root).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(entries.len(), 2);
            assert!(entries.iter().any(|e| e.file_name().to_string_lossy().ends_with(".dll.stuck")));
            assert_eq!(legacy::active_in(&f.root, &old, Some(&com), false).unwrap(), legacy::Active::Legacy);
            assert_eq!(legacy::active_in(&f.root, &f.dir.join("missing"), None, false).unwrap(), legacy::Active::Fresh);
            if cleanup_first {
                drop(loaded);
                legacy::cleanup_inactive(&f.root, &old, Some(&com)).unwrap();
                assert!(!f.root.exists());
            } else {
                legacy::cleanup_inactive(&f.root, &old, Some(&com)).unwrap();
                assert!(legacy::residue(&f.root).unwrap());
            }
            let mut hooks = FakeSystem::default();
            f.run(&|| Ok(true), &mut hooks).unwrap();
            assert_eq!(legacy::manifest(&f.root).unwrap(), legacy::Manifest::Installed);
            assert!(!f.root.join(ids::UNINSTALL_MARKER).exists());
        }
    }

    const TARGETS: &[&str] = &[
        "user-data.on", ids::ENGINE_NAME, SETTINGS_EXE,
        "assets/data.txt", "assets/common-readings.txt", "assets/model.bin", "assets/prior.pack",
        "assets/pn.fst", "assets/pn.lst", "assets/pn.cfg", "assets/model.cfg", "dev-mode",
        "assets/gen.fst", "assets/gen.lst", "assets/emoji.tsv", "assets/wordbonus.tsv", "assets/wordbonus.bin", "assets/lm-f32.bin", "assets/lm-vocab.json",
    ];

    struct Fixture { dir: PathBuf, root: PathBuf, bin: PathBuf, assets: PathBuf, settings: PathBuf }
    impl Fixture {
        fn new(overwrite: bool) -> Self {
            let dir = test_temp_dir("twime-install-test");
            let f = Self { root: dir.join("installed"), bin: dir.join("bin"), assets: dir.join("source-assets"),
                settings: dir.join("settings.exe"), dir };
            for dir in [&f.bin, &f.assets, &f.root.join("assets")] { fs::create_dir_all(dir).unwrap(); }
            for name in [ids::DLL_NAME, ids::ENGINE_NAME] { fs::write(f.bin.join(name), b"new binary").unwrap(); }
            for name in ["data.txt", "common-readings.txt", "model.bin", "prior.pack", "pn.fst", "pn.lst", "pn.cfg", "model.cfg", "dev-mode", "gen.fst", "gen.lst", "emoji.tsv", "wordbonus.tsv", "wordbonus.bin", "lm-f32.bin", "lm-vocab.json"] {
                fs::write(f.assets.join(name), format!("new {name}")).unwrap();
            }
            fs::write(&f.settings, b"new settings").unwrap();
            if overwrite {
                for name in TARGETS { fs::write(f.root.join(name), format!("old {name}")).unwrap(); }
                fs::write(f.root.join("twime_tsf-old.dll"), b"old DLL").unwrap();
                fs::write(f.root.join(MANIFEST), OLD_MANIFEST).unwrap();
            }
            f
        }
        fn run(&self, stop: &dyn Fn() -> Result<bool, String>, hooks: &mut FakeSystem) -> Result<(), String> {
            install_files_in(&self.root, &self.bin, &self.assets, Some(&self.settings), stop, hooks)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert!(self.dir.is_absolute() && self.dir.starts_with(std::env::temp_dir()));
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[derive(Default)]
    struct FakeSystem {
        shortcut: Option<PathBuf>, fail: bool, applied: bool, undone: bool,
        fail_finalize: bool, manifest_lock: Option<fs::File>,
        fail_restore: bool, restore_manifest: bool, restore_lock: Option<fs::File>,
        fail_checkpoint: Option<&'static str>, checkpoints: Vec<String>,
        marker_lock: Option<fs::File>, fail_marker: bool,
        fail_cleanup: bool, cleanup_lock: Option<fs::File>,
        root: Option<PathBuf>,
    }
    impl Hooks for FakeSystem {
        fn apply(&mut self, files: &mut Files) -> Result<(), String> {
            assert!(files.root.join(ids::UNINSTALL_MARKER).exists());
            self.root = Some(files.root.clone());
            self.applied = true;
            if let Some(path) = &self.shortcut { files.write(path, b"new shortcut")?; }
            if self.fail_finalize {
                // Fail publication while the marker and all backups are still present.
                // rollback releases this lock before restoring the manifest.
                self.manifest_lock = Some(fs::OpenOptions::new().read(true).share_mode(1)
                    .open(files.root.join(MANIFEST)).unwrap());
            }
            if self.fail_restore {
                self.restore_lock = Some(fs::OpenOptions::new().read(true).share_mode(1)
                    .open(files.root.join(if self.restore_manifest { MANIFEST } else { ids::ENGINE_NAME })).unwrap());
            }
            if self.fail || self.fail_restore { Err("injected registration failure".into()) } else { Ok(()) }
        }
        fn rollback(&mut self, _: &mut Vec<String>) {
            assert!(self.root.as_ref().unwrap().join(ids::UNINSTALL_MARKER).exists());
            self.undone = true;
            self.manifest_lock.take();
            self.marker_lock.take();
            self.cleanup_lock.take();
        }
        fn checkpoint(&mut self, step: &str, files: &Files) -> Result<(), String> {
            assert!(files.root.join(ids::UNINSTALL_MARKER).exists(), "{step}");
            self.checkpoints.push(step.into());
            match step {
                "installed manifest" => {
                    assert!(fs::read_to_string(files.root.join(MANIFEST)).unwrap().starts_with("state=installed\n"));
                    assert!(files.backups.iter().all(|b| b.old.exists()));
                    if self.fail_cleanup {
                        self.cleanup_lock = Some(fs::OpenOptions::new().read(true).share_mode(1)
                            .open(&files.backups.last().unwrap().old).unwrap());
                    }
                }
                "cleanup complete" => {
                    assert!(files.backups.iter().all(|b| !b.old.exists() || self.fail_cleanup));
                    if self.fail_marker {
                        self.marker_lock = Some(fs::OpenOptions::new().read(true).share_mode(1)
                            .open(files.root.join(ids::UNINSTALL_MARKER)).unwrap());
                    }
                }
                _ => {},
            }
            if self.fail_checkpoint == Some(step) { Err(format!("injected failure at {step}")) }
            else { Ok(()) }
        }
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { walk(root, &path, result); }
                else { result.insert(path.strip_prefix(root).unwrap().into(), fs::read(path).unwrap()); }
            }
        }
        let mut result = BTreeMap::new();
        walk(root, root, &mut result);
        result
    }

    #[test]
    fn overwrite_stops_locked_engine_before_replacing_files_and_commits() {
        let f = Fixture::new(true);
        let handle = std::cell::RefCell::new(Some(fs::OpenOptions::new().read(true).share_mode(1).open(f.root.join(ids::ENGINE_NAME)).unwrap()));
        // Legacy temporary files must also disappear on success.
        fs::write(f.root.join("assets/model.tmp-install"), b"stale").unwrap();
        let shortcut = f.dir.join("settings.lnk");
        fs::write(&shortcut, b"old shortcut").unwrap();
        let mut hooks = FakeSystem { shortcut: Some(shortcut.clone()), ..Default::default() };
        let stopped = Cell::new(false);
        f.run(&|| {
            assert!(f.root.join(ids::UNINSTALL_MARKER).exists());
            assert_eq!(fs::read(f.root.join(MANIFEST)).unwrap(), OLD_MANIFEST);
            drop(handle.borrow_mut().take());
            stopped.set(true);
            Ok(true)
        }, &mut hooks).unwrap();
        assert!(stopped.get() && hooks.applied && !hooks.undone);
        assert_eq!(hooks.checkpoints, ["installed manifest", "cleanup complete"]);
        for name in TARGETS {
            let bytes = fs::read(f.root.join(name)).unwrap();
            if *name == "user-data.on" { assert!(bytes.is_empty()); }
            else { assert!(bytes.starts_with(b"new"), "{name}"); }
        }
        assert_eq!(fs::read(&shortcut).unwrap(), b"new shortcut");
        assert!(!suffix(&shortcut, ".old-install").exists());
        let all = snapshot(&f.root);
        assert!(all.keys().all(|p| !p.to_string_lossy().ends_with(".old-install") && !p.to_string_lossy().ends_with(".tmp-install")));
        assert!(!f.root.join(ids::UNINSTALL_MARKER).exists());
        assert!(fs::read_to_string(f.root.join(MANIFEST)).unwrap().starts_with("state=installed\n"));
        assert_eq!(installed_dlls(&f.root).len(), 2);
    }

    #[test]
    fn locked_kth_file_restores_every_original_and_manifest() {
        for name in TARGETS {
            let f = Fixture::new(true);
            let before = snapshot(&f.root);
            // FILE_SHARE_READ only: excludes FILE_SHARE_DELETE, like a running executable.
            let lock = fs::OpenOptions::new().read(true).share_mode(1).open(f.root.join(name)).unwrap();
            let mut hooks = FakeSystem::default();
            let error = f.run(&|| Ok(false), &mut hooks).unwrap_err();
            assert!(error.contains("backup"), "{name}: {error}");
            assert!(!hooks.applied);
            assert_eq!(snapshot(&f.root), before, "locked {name}");
            drop(lock);
        }
    }

    #[test]
    fn stop_error_leaves_all_files_unchanged_and_clears_marker() {
        for overwrite in [false, true] {
            let f = Fixture::new(overwrite);
            let before = snapshot(&f.root);
            let mut hooks = FakeSystem::default();
            let error = f.run(&|| {
                assert!(f.root.join(ids::UNINSTALL_MARKER).exists());
                Err("unknown engine".into())
            }, &mut hooks).unwrap_err();
            assert_eq!(error, "unknown engine");
            assert!(!hooks.applied);
            assert_eq!(snapshot(&f.root), before);
        }
    }

    #[test]
    fn unfinished_transaction_refuses_before_stop_without_changing_files_or_marker() {
        for leftover in ["engine.old-install", "assets/model.bin.old-install", "install.manifest.old-install", MANIFEST] {
            for with_marker in [false, true] {
                let f = Fixture::new(true);
                let path = leftover.split('/').fold(f.root.clone(), |path, part| path.join(part));
                fs::write(&path, if leftover == MANIFEST { b"state=installing\r\ndll=partial.dll\r\n".as_slice() } else { b"previous backup" }).unwrap();
                if with_marker { fs::write(f.root.join(ids::UNINSTALL_MARKER), b"previous protection").unwrap(); }
                let before = snapshot(&f.root);
                let mut hooks = FakeSystem::default();
                let error = f.run(&|| panic!("must reject before stopping engine"), &mut hooks).unwrap_err();
                assert!(error.contains("前次未完成的安裝交易"), "{error}");
                assert!(error.contains(&path.display().to_string()), "{error}");
                assert!(error.contains("手動") && error.contains("全部處理完成後才可移除"), "{error}");
                assert!(!hooks.applied && !hooks.undone && hooks.checkpoints.is_empty());
                assert_eq!(snapshot(&f.root), before);
            }
        }
    }

    #[derive(Default)]
    struct FakeEngineStop {
        ticks: u64,
        forever_locked: bool,
        probes: Vec<&'static str>,
    }

    impl EngineStopHooks for FakeEngineStop {
        fn elapsed(&self) -> std::time::Duration { std::time::Duration::from_millis(self.ticks * 200) }
        fn stop_pipe(&mut self, _: std::time::Duration) -> Result<bool, String> {
            self.probes.push("pipe");
            Ok(!self.forever_locked && self.ticks == 1)
        }
        fn lock_held(&mut self) -> Result<bool, String> {
            self.probes.push("lock");
            Ok(self.forever_locked || self.ticks == 0)
        }
        fn wait(&mut self) { self.probes.push("wait"); self.ticks += 1; }
    }

    #[test]
    fn starting_engine_with_only_lock_is_stopped_when_pipe_appears_before_file_changes() {
        let f = Fixture::new(true);
        let before = snapshot(&f.root);
        let engine = std::cell::RefCell::new(FakeEngineStop::default());
        f.run(&|| {
            assert!(f.root.join(ids::UNINSTALL_MARKER).exists());
            let mut during = snapshot(&f.root);
            during.remove(Path::new(ids::UNINSTALL_MARKER));
            assert_eq!(during, before);
            let result = wait_engine_absent(&mut *engine.borrow_mut());
            let mut after = snapshot(&f.root);
            after.remove(Path::new(ids::UNINSTALL_MARKER));
            assert_eq!(after, before);
            result
        }, &mut FakeSystem::default()).unwrap();
        assert_eq!(engine.borrow().probes, ["pipe", "lock", "wait", "pipe", "wait", "pipe", "lock"]);
        assert_eq!(fs::read(f.root.join(ids::ENGINE_NAME)).unwrap(), b"new binary");
    }

    #[test]
    fn held_lock_times_out_without_touching_installation_files() {
        for overwrite in [false, true] {
            let f = Fixture::new(overwrite);
            fs::write(f.root.join("assets/untouched.tmp-install"), b"keep on stop timeout").unwrap();
            let before = snapshot(&f.root);
            let engine = std::cell::RefCell::new(FakeEngineStop { forever_locked: true, ..Default::default() });
            let mut hooks = FakeSystem::default();
            let error = f.run(&|| wait_engine_absent(&mut *engine.borrow_mut()), &mut hooks).unwrap_err();
            assert!(error.contains("10 s"), "{error}");
            assert_eq!(engine.borrow().ticks, 50);
            assert!(!hooks.applied && !hooks.undone);
            assert_eq!(snapshot(&f.root), before);
        }
    }

    #[test]
    fn running_lock_probe_detects_sharing_violation_in_private_directory() {
        let f = Fixture::new(false);
        let mut engine = SystemEngineStop { engines: vec![],
            data: vec![f.dir.clone()], started: std::time::Instant::now() };
        assert!(!engine.lock_held().unwrap());
        let path = f.dir.join("running.lock");
        fs::write(&path, b"test").unwrap();
        assert!(!engine.lock_held().unwrap());
        let lock = fs::OpenOptions::new().read(true).share_mode(1).open(&path).unwrap();
        assert!(engine.lock_held().unwrap());
        drop(lock);
        assert!(!engine.lock_held().unwrap());
    }

    #[test]
    fn engine_stop_probes_both_state_directories_and_skips_pipe_for_missing_image() {
        let f = Fixture::new(false); let old_data = f.dir.join("old-state"); let new_data = f.dir.join("new-state");
        for data in [&old_data, &new_data] { fs::create_dir(data).unwrap(); fs::write(data.join("running.lock"), b"private").unwrap(); }
        let mut engine = SystemEngineStop { engines: group_engines([("must-never-be-opened".into(), f.dir.join("absent.exe").to_string_lossy().into_owned())]),
            data: vec![new_data.clone(), old_data.clone()], started: std::time::Instant::now() };
        assert!(!engine.stop_pipe(std::time::Duration::from_secs(1)).unwrap());
        for data in [&old_data, &new_data] {
            let lock = fs::OpenOptions::new().read(true).share_mode(1).open(data.join("running.lock")).unwrap();
            assert!(engine.lock_held().unwrap()); drop(lock); assert!(!engine.lock_held().unwrap());
        }
    }

    #[test]
    fn current_overwrite_treats_a_leftover_old_manifest_only_as_post_commit_cleanup() {
        let f = Fixture::new(true); let old = f.dir.join("legacy"); fs::create_dir(&old).unwrap();
        fs::write(old.join(MANIFEST), b"state=installed\n").unwrap();
        fs::write(old.join(ids::ENGINE_NAME), b"old engine").unwrap();
        let old_lock = fs::OpenOptions::new().read(true).share_mode(1).open(old.join(MANIFEST)).unwrap();
        let target = f.root.join("twime_tsf-old.dll").to_string_lossy().into_owned();
        assert_eq!(legacy::active_in(&f.root, &old, Some(&target), false).unwrap(), legacy::Active::Current);
        let marked = Cell::new(false); let stops = Cell::new(0); let mut inner = FakeSystem::default();
        let com = || Ok(Some(target.clone()));
        let mut hooks = LegacyHooks { inner: &mut inner, old_root: &old, old_shortcut: None, marker_written: &marked,
            com: &com, migrate: &|| vec![] };
        install_files_in(&f.root, &f.bin, &f.assets, Some(&f.settings), &|| {
            assert!(!old.join(ids::UNINSTALL_MARKER).exists()); stops.set(stops.get()+1); Ok(false)
        }, &mut hooks).unwrap();
        assert_eq!(stops.get(), 1); assert!(!marked.get());
        assert!(!old.join(ids::ENGINE_NAME).exists()); assert!(old.join(MANIFEST).exists());
        assert!(old.join(ids::UNINSTALL_MARKER).exists()); assert!(!f.root.join(ids::UNINSTALL_MARKER).exists());
        assert_eq!(legacy::active_in(&f.root, &old, Some(&target), false).unwrap(), legacy::Active::Current);
        drop(old_lock);
    }

    #[test]
    fn registration_failure_restores_shortcut_and_files_or_removes_fresh_install() {
        for overwrite in [false, true] {
            for existing_shortcut in [false, true] {
                let f = Fixture::new(overwrite);
                let before = snapshot(&f.root);
                let shortcut = f.dir.join("settings.lnk");
                if existing_shortcut { fs::write(&shortcut, b"old shortcut").unwrap(); }
                let mut hooks = FakeSystem { shortcut: Some(shortcut.clone()), fail: true, ..Default::default() };
                assert!(f.run(&|| Ok(false), &mut hooks).is_err());
                assert!(hooks.applied && hooks.undone);
                assert_eq!(snapshot(&f.root), before);
                assert_eq!(shortcut.exists(), existing_shortcut);
                if existing_shortcut { assert_eq!(fs::read(&shortcut).unwrap(), b"old shortcut"); }
                assert!(!suffix(&shortcut, ".old-install").exists());
            }
        }
    }

    #[test]
    fn fresh_copy_failure_clears_stale_marker_and_partial_files() {
        let f = Fixture::new(false);
        fs::write(f.root.join(ids::UNINSTALL_MARKER), b"stale").unwrap();
        fs::remove_file(f.assets.join("model.bin")).unwrap();
        assert!(f.run(&|| Ok(false), &mut FakeSystem::default()).is_err());
        assert!(!f.root.exists() || snapshot(&f.root).is_empty());
    }

    #[test]
    fn final_manifest_failure_restores_originals_under_marker() {
        for overwrite in [false, true] {
            let f = Fixture::new(overwrite);
            let before = snapshot(&f.root);
            let mut hooks = FakeSystem { fail_finalize: true, ..Default::default() };
            let error = f.run(&|| Ok(false), &mut hooks).unwrap_err();
            assert!(error.contains("write installed manifest"), "{error}");
            assert!(hooks.undone);
            assert_eq!(hooks.checkpoints, ["restoration complete"]);
            assert_eq!(snapshot(&f.root), before);
        }
    }

    #[test]
    fn failed_restore_rename_retains_all_backups_and_reports_path() {
        for restore_manifest in [false, true] {
            let f = Fixture::new(true);
            let mut hooks = FakeSystem { fail_restore: true, restore_manifest, ..Default::default() };
            let error = f.run(&|| Ok(false), &mut hooks).unwrap_err();
            let target = if restore_manifest { MANIFEST } else { ids::ENGINE_NAME };
            let backup = suffix(&f.root.join(target), ".old-install");
            assert!(error.contains("restore "), "{error}");
            assert!(error.contains(&format!("backup retained at {}", backup.display())), "{error}");
            assert_eq!(fs::read(suffix(&f.root.join(MANIFEST), ".old-install")).unwrap(), OLD_MANIFEST);
            for name in TARGETS {
                assert_eq!(fs::read(suffix(&f.root.join(name), ".old-install")).unwrap(), format!("old {name}").as_bytes());
            }
            if !restore_manifest { assert_eq!(fs::read(f.root.join(MANIFEST)).unwrap(), OLD_MANIFEST); }
            assert_eq!(hooks.checkpoints.last().unwrap(), "restoration complete");
            assert!(f.root.join(ids::UNINSTALL_MARKER).exists());
            hooks.restore_lock.take();
            let before_retry = snapshot(&f.root);
            let mut retry_hooks = FakeSystem::default();
            let error = f.run(&|| panic!("must reject retry before stopping engine"), &mut retry_hooks).unwrap_err();
            assert!(error.contains(&backup.display().to_string()), "{error}");
            for path in before_retry.keys().filter(|p| p.to_string_lossy().ends_with(".old-install")) {
                assert!(error.contains(&f.root.join(path).display().to_string()), "{error}");
            }
            assert!(!retry_hooks.applied && !retry_hooks.undone);
            assert_eq!(snapshot(&f.root), before_retry);
        }
    }

    #[test]
    fn partial_backup_cleanup_failure_keeps_committed_installation() {
        let f = Fixture::new(true);
        let mut hooks = FakeSystem { fail_cleanup: true, ..Default::default() };
        f.run(&|| Ok(false), &mut hooks).unwrap();
        assert!(!hooks.undone);
        assert_eq!(hooks.checkpoints, ["installed manifest", "cleanup complete"]);
        assert!(fs::read_to_string(f.root.join(MANIFEST)).unwrap().starts_with("state=installed\n"));
        assert_eq!(fs::read(f.root.join(ids::ENGINE_NAME)).unwrap(), b"new binary");
        assert_eq!(fs::read(suffix(&f.root.join("dev-mode"), ".old-install")).unwrap(), b"old dev-mode");
        assert!(!f.root.join(ids::UNINSTALL_MARKER).exists());
    }

    #[test]
    fn locked_temporary_cleanup_only_warns_after_commit() {
        let f = Fixture::new(true);
        let temp = f.root.join("assets/model.tmp-install");
        fs::write(&temp, b"stale temporary").unwrap();
        let _lock = fs::OpenOptions::new().read(true).share_mode(1).open(&temp).unwrap();
        let mut hooks = FakeSystem::default();
        f.run(&|| Ok(false), &mut hooks).unwrap();
        assert!(!hooks.undone);
        assert_eq!(fs::read(&temp).unwrap(), b"stale temporary");
        assert_eq!(fs::read(f.root.join(ids::ENGINE_NAME)).unwrap(), b"new binary");
        assert!(fs::read_to_string(f.root.join(MANIFEST)).unwrap().starts_with("state=installed\n"));
        assert!(!f.root.join(ids::UNINSTALL_MARKER).exists());
    }

    #[test]
    fn post_commit_failures_never_restore() {
        for overwrite in [false, true] {
            for step in ["installed manifest", "cleanup complete", "marker removal"] {
                let f = Fixture::new(overwrite);
                let mut hooks = FakeSystem {
                    fail_marker: step == "marker removal",
                    fail_checkpoint: if step == "marker removal" { None } else { Some(step) },
                    ..Default::default()
                };
                let result = f.run(&|| Ok(false), &mut hooks);
                if step == "marker removal" {
                    let error = result.unwrap_err();
                    assert!(error.contains("安裝已完成，但 uninstalling 標記無法刪除；請手動刪除"), "{error}");
                    assert!(error.contains(&f.root.join(ids::UNINSTALL_MARKER).display().to_string()), "{error}");
                    assert!(error.contains("後即可使用"), "{error}");
                } else {
                    result.unwrap();
                }
                assert!(!hooks.undone);
                assert_eq!(hooks.checkpoints, ["installed manifest", "cleanup complete"]);
                assert!(fs::read_to_string(f.root.join(MANIFEST)).unwrap().starts_with("state=installed\n"));
                assert_eq!(fs::read(f.root.join(ids::ENGINE_NAME)).unwrap(), b"new binary");
                assert_eq!(f.root.join(ids::UNINSTALL_MARKER).exists(), step == "marker removal");
            }
        }
    }

    #[test]
    fn overwrite_skips_enable_hook_and_fresh_install_tracks_partial_enable() {
        for overwrite in [false, true] {
            for fail in [false, true] {
                let mut started = false;
                let mut calls = 0;
                let result = enable_for_fresh_install(overwrite, &mut started, &mut || {
                    calls += 1;
                    if fail { Err("enable failure".into()) } else { Ok(()) }
                });
                assert_eq!(calls, if overwrite { 0 } else { 1 });
                assert_eq!(started, !overwrite);
                assert_eq!(result.is_err(), !overwrite && fail);
            }
        }
    }

    #[test]
    fn overwrite_without_optional_sources_disables_configs_and_retains_settings_exe() {
        let f = Fixture::new(true);
        for name in ["pn.cfg", "model.cfg", "dev-mode", "gen.lst", "emoji.tsv", "wordbonus.tsv", "wordbonus.bin", "lm-f32.bin", "lm-vocab.json"] { fs::remove_file(f.assets.join(name)).unwrap(); }
        install_files_in(&f.root, &f.bin, &f.assets, None, &|| Ok(false), &mut FakeSystem::default()).unwrap();
        for name in ["pn.cfg", "model.cfg", "gen.fst", "gen.lst", "emoji.tsv", "wordbonus.tsv", "wordbonus.bin", "lm-f32.bin", "lm-vocab.json"] { assert!(!f.root.join("assets").join(name).exists()); }
        assert_eq!(fs::read(f.root.join(SETTINGS_EXE)).unwrap(), format!("old {SETTINGS_EXE}").as_bytes());
        assert_eq!(fs::read(f.root.join("dev-mode")).unwrap(), b"old dev-mode");
        assert!(snapshot(&f.root).keys().all(|p| !p.to_string_lossy().contains("-install")));
    }
    #[test]
    fn install_copies_emoji_tsv() {
        for overwrite in [false, true] {
            let f = Fixture::new(overwrite);
            install_files_in(&f.root, &f.bin, &f.assets, None, &|| Ok(false), &mut FakeSystem::default()).unwrap();
            assert_eq!(fs::read(f.root.join("assets").join(EMOJI_FILE)).unwrap(), format!("new {EMOJI_FILE}").as_bytes());
            assert!(snapshot(&f.root).keys().all(|p| !p.to_string_lossy().contains("-install")));
        }
    }
    #[test]
    fn install_copies_wordbonus_files_and_a_source_without_them_removes_old_ones() {
        for overwrite in [false, true] {
            let f = Fixture::new(overwrite);
            install_files_in(&f.root, &f.bin, &f.assets, None, &|| Ok(false), &mut FakeSystem::default()).unwrap();
            for name in WORDBONUS_FILES {
                assert_eq!(fs::read(f.root.join("assets").join(name)).unwrap(), format!("new {name}").as_bytes());
            }
            assert!(snapshot(&f.root).keys().all(|p| !p.to_string_lossy().contains("-install")));
        }
        let f = Fixture::new(true);
        for name in WORDBONUS_FILES { fs::remove_file(f.assets.join(name)).unwrap(); }
        install_files_in(&f.root, &f.bin, &f.assets, None, &|| Ok(false), &mut FakeSystem::default()).unwrap();
        for name in WORDBONUS_FILES { assert!(!f.root.join("assets").join(name).exists()); }
    }
    #[test]
    fn failed_install_restores_old_wordbonus_files_and_leaves_none_on_a_fresh_root() {
        for overwrite in [false, true] {
            let f = Fixture::new(overwrite);
            let before = snapshot(&f.root);
            let mut hooks = FakeSystem { fail: true, ..Default::default() };
            assert!(f.run(&|| Ok(false), &mut hooks).is_err());
            if overwrite {
                for name in WORDBONUS_FILES {
                    assert_eq!(fs::read(f.root.join("assets").join(name)).unwrap(), format!("old assets/{name}").as_bytes());
                }
                assert_eq!(snapshot(&f.root), before);
            } else {
                for name in WORDBONUS_FILES { assert!(!f.root.join("assets").join(name).exists()); }
            }
        }
    }
    #[test]
    fn install_copies_the_complete_gen_pair() {
        for overwrite in [false, true] {
            let f = Fixture::new(overwrite);
            install_files_in(&f.root, &f.bin, &f.assets, None, &|| Ok(false), &mut FakeSystem::default()).unwrap();
            for name in GEN_FILES {
                assert_eq!(fs::read(f.root.join("assets").join(name)).unwrap(), format!("new {name}").as_bytes());
            }
            assert!(snapshot(&f.root).keys().all(|p| !p.to_string_lossy().contains("-install")));
        }
    }
}
