//! twime-setup: per-user install / uninstall of 地瓜輸入法 (TwPotato), RELEASE plan 2.4–2.5.
//!
//!   twime-setup plan                      what install would write (read only; the default)
//!   twime-setup verify                    which parts are present
//!   twime-setup install --confirm --bin-dir D --assets-src A [--settings-exe FILE]
//!   twime-setup uninstall                 undo everything this tool added (idempotent)
//!   twime-setup cleanup-legacy            clean inactive TwIme installation leftovers
//!   twime-setup stop-engine               end the engine process that serves this user's pipe
//!
//! MSI helpers: ident, machine-register/unregister/rollback-register/stop/release (see machine.rs).
//! The commands above are per user: files under %LOCALAPPDATA%\Programs\TwPotato, the COM class under HKCU\Software\Classes, the
//! TSF profile through the TSF registration API, enabling through InstallLayoutOrTip. Nothing is added to the default
//! input method order by this tool. Only the fixed CLSID / profile GUID and files under the install root are touched.
//! Both TwIme and TwPotato state directories participate in fail-closed confirmation.

mod confirm;
mod diag;
mod install;
mod legacy;
mod machine;
mod migrate;

#[cfg(test)]
fn test_temp_dir(prefix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        match fs::create_dir(&path) {
            Ok(()) => return path,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("create test directory {}: {e}", path.display()),
        }
    }
}

use std::{
    fs,
    path::{Path, PathBuf},
    process::exit,
};

use twime_proto::ids;
use windows::{
    Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::{
            Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx},
            LibraryLoader::{GetProcAddress, LoadLibraryW},
            Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW},
            Threading::{OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject},
        },
        UI::{
            Input::KeyboardAndMouse::HKL,
            TextServices::{
                CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
                ITfCategoryMgr, ITfInputProcessorProfileMgr,
            },
        },
    },
    core::{GUID, PCWSTR},
};

const MANIFEST: &str = "install.manifest";
const SETTINGS_EXE: &str = "twime-settings.exe";

fn settings_shortcut() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA").map(PathBuf::from)
        .map(|p| p.join("Microsoft/Windows/Start Menu/Programs/地瓜輸入法 設定.lnk"))
        .ok_or("APPDATA missing; cannot locate Start Menu shortcut".into())
}

/// Also used by the unit test with a shortcut under a private temporary directory.
fn create_settings_shortcut(install_root: &Path, shortcut: &Path) -> Result<(), String> {
    use windows::{core::Interface, Win32::{System::Com::{CoUninitialize, IPersistFile}, UI::Shell::{IShellLinkW, ShellLink}}};
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().map_err(|e| e.to_string())?;
        struct ComGuard;
        impl Drop for ComGuard { fn drop(&mut self) { unsafe { CoUninitialize() } } }
        let _guard = ComGuard;
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        link.SetPath(PCWSTR(wide(&install_root.join(SETTINGS_EXE).to_string_lossy()).as_ptr())).map_err(|e| e.to_string())?;
        link.SetWorkingDirectory(PCWSTR(wide(&install_root.to_string_lossy()).as_ptr())).map_err(|e| e.to_string())?;
        link.SetDescription(PCWSTR(wide("地瓜輸入法 設定").as_ptr())).map_err(|e| e.to_string())?;
        let persist: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        if let Some(parent) = shortcut.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        persist.Save(PCWSTR(wide(&shortcut.to_string_lossy()).as_ptr()), true).map_err(|e| e.to_string())
    }
}

fn remove_settings_shortcut(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove shortcut {}: {e}", path.display())),
    }
}
/// Optional PN assets (made by ime/scripts/make-assets.sh); `pn.cfg` last.
const PN_FILES: [&str; 3] = ["pn.fst", "pn.lst", "pn.cfg"];
/// Optional SRCH general-word layer (make-assets.sh gen=); installed only as a complete pair.
const GEN_FILES: [&str; 2] = ["gen.fst", "gen.lst"];
/// Optional EMOJI table (emoji/emoji.tsv copied into the assets by make-assets.sh); the engine reads it when `emoji=on`.
const EMOJI_FILE: &str = "emoji.tsv";
/// Optional plain asset files that are copied when the source has them and removed when it does not: the WORDBONUS table
/// (WORDBONUS-plan): the tsv source and its built bin, both made by make-assets.sh; the engine reads the bin when model.cfg has `wordbonus=wordbonus.bin`. SETTINGS-AUTOCOMP: also the completion-popup LM `lm-f32.bin` + `lm-vocab.json`
/// (read only when the user switches the popup on in the settings page; the engine degrades to popup-off when they are missing).
const WORDBONUS_FILES: [&str; 4] = ["wordbonus.tsv", "wordbonus.bin", "lm-f32.bin", "lm-vocab.json"];
const SYSTRAY_CAP: u128 = 0x25504fb4_7bab_4bc1_9c69_cf81890f0ef5; // GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn root() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("Programs").join("TwPotato")
}

fn legacy_root() -> PathBuf { root().with_file_name("TwIme") }
fn legacy_data() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("TwIme")
}
fn state_dirs() -> Vec<PathBuf> {
    state_dirs_in(twime_winpipe::state::data_dir(), legacy_data(), std::env::var_os("TWIME_DATA_DIR").map(PathBuf::from))
}
fn state_dirs_in(current: PathBuf, old: PathBuf, extra: Option<PathBuf>) -> Vec<PathBuf> {
    let mut unique: Vec<PathBuf> = vec![];
    for dir in [Some(current), Some(old), extra].into_iter().flatten() {
        let key = |p: &Path| legacy::path_key(&fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
        if !unique.iter().any(|p| key(p) == key(&dir)) { unique.push(dir); }
    }
    unique
}
fn legacy_shortcut() -> Result<PathBuf, String> {
    settings_shortcut().map(|p| p.with_file_name("TwIme 設定.lnk"))
}
fn machine_installed() -> Result<bool, String> { reg_exists_checked(HKEY_LOCAL_MACHINE, &clsid_key()) }

fn engine_pipe(old: bool) -> Result<String, String> {
    let id = twime_winpipe::current_identity().map_err(|e| e.to_string())?;
    Ok(if old { twime_winpipe::legacy_pipe_name(&id) }
        else { twime_winpipe::pipe_name(&id) })
}

fn clsid() -> GUID {
    GUID::from_u128(ids::CLSID_TIP)
}
fn profile() -> GUID {
    GUID::from_u128(ids::PROFILE)
}
fn clsid_key() -> String {
    format!(r"Software\Classes\CLSID\{}", ids::guid_string(ids::CLSID_TIP))
}
fn profile_string() -> String {
    format!("{:04X}:{}{}", ids::LANGID_ZH_TW, ids::guid_string(ids::CLSID_TIP), ids::guid_string(ids::PROFILE))
}

fn categories() -> [GUID; 4] {
    [GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, GUID::from_u128(SYSTRAY_CAP), windows::Win32::UI::TextServices::GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER]
}

// ---- registry ----------------------------------------------------------------------------------------------------

fn reg_exists(sub: &str) -> bool {
    reg_exists_in(HKEY_CURRENT_USER, sub)
}

fn reg_exists_in(hive: HKEY, sub: &str) -> bool {
    reg_exists_checked(hive, sub).unwrap_or(false)
}

fn reg_exists_checked(hive: HKEY, sub: &str) -> Result<bool, String> {
    unsafe {
        let mut h = HKEY::default();
        let w = wide(sub);
        let e = RegOpenKeyExW(hive, PCWSTR(w.as_ptr()), None, KEY_READ, &mut h);
        if e.0 == 0 {
            let _ = RegCloseKey(h);
            Ok(true)
        } else if e.0 == 2 {
            Ok(false)
        } else {
            Err(format!("cannot inspect registry key {sub}: error {}", e.0))
        }
    }
}

fn reg_set(sub: &str, name: Option<&str>, value: &str) -> Result<(), String> {
    unsafe {
        let mut h = HKEY::default();
        let w = wide(sub);
        let e = RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut h, None);
        if e.0 != 0 {
            return Err(format!("RegCreateKeyEx {sub}: error {}", e.0));
        }
        let n = name.map(wide);
        let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
        let e = RegSetValueExW(h, n.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())), None, REG_SZ, Some(&data));
        let _ = RegCloseKey(h);
        if e.0 != 0 {
            return Err(format!("RegSetValueEx {sub}: error {}", e.0));
        }
        Ok(())
    }
}

fn reg_delete_tree(sub: &str) -> Result<(), String> {
    if !reg_exists(sub) {
        return Ok(());
    }
    unsafe {
        let w = wide(sub);
        let e = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()));
        if e.0 != 0 { Err(format!("RegDeleteTree {sub}: error {}", e.0)) } else { Ok(()) }
    }
}

/// Deletes a whole registry tree, but only if `path` is exactly one of the `allowed` paths (compared case-insensitively).
/// `UnregisterProfile` / `UnregisterCategory` remove the leaf values only and leave the empty container keys
/// (`Category\Category\{cat}`, `Category\Item\{clsid}`, `LanguageProfile\0x00000404`, ...); this is the one place that
/// removes them. Returns whether something was there.
fn delete_guarded(hive: HKEY, path: &str, allowed: &[String]) -> Result<bool, String> {
    delete_guarded_in(hive, path, allowed, false)
}

fn delete_guarded_in(hive: HKEY, path: &str, allowed: &[String], checked: bool) -> Result<bool, String> {
    if !allowed.iter().any(|a| a.eq_ignore_ascii_case(path)) {
        return Err(format!("refusing to delete unexpected registry path {path}"));
    }
    // Machine mode checks RegDeleteTreeW's result directly; a failed existence probe must not hide access errors.
    if !checked && !reg_exists_in(hive, path) {
        return Ok(false);
    }
    unsafe {
        let w = wide(path);
        let e = RegDeleteTreeW(hive, PCWSTR(w.as_ptr()));
        if checked && (e.0 == 2 || e.0 == 3) { return Ok(false); }
        if e.0 != 0 { Err(format!("RegDeleteTree {path}: error {}", e.0)) } else { Ok(true) }
    }
}

/// `<prefix>\{our CLSID}`.
fn tip_path(prefix: &str) -> String {
    format!(r"{prefix}\{}", ids::guid_string(ids::CLSID_TIP))
}

/// Where TSF keeps its per-machine and per-user records of this TIP.
const HKLM_TIP_PREFIX: &str = r"SOFTWARE\Microsoft\CTF\TIP";
const HKCU_TIP_PREFIX: &str = r"Software\Microsoft\CTF\TIP";
/// The test run of the uninstall uses this prefix below HKCU instead (never HKLM).
const TEST_TIP_PREFIX: &str = r"Software\TwImeSetupTest\CTF\TIP";

fn tip_records() -> Vec<(HKEY, String)> {
    vec![(HKEY_LOCAL_MACHINE, tip_path(HKLM_TIP_PREFIX)), (HKEY_CURRENT_USER, tip_path(HKCU_TIP_PREFIX))]
}

trait RegistrationRemoval {
    fn disable(&mut self) -> Result<(), String>;
    fn unregister(&mut self) -> Vec<String>;
    fn delete(&mut self, hive: HKEY, path: &str) -> Result<(), String>;
}
struct SystemRegistration;
impl RegistrationRemoval for SystemRegistration {
    fn disable(&mut self) -> Result<(), String> { install_layout_or_tip(1) }
    fn unregister(&mut self) -> Vec<String> { tsf_unregister() }
    fn delete(&mut self, hive: HKEY, path: &str) -> Result<(), String> {
        delete_guarded(hive, path, &[path.into()]).map(|_| ())
    }
}
/// The same injectable machine-mode policy is used by uninstall and fresh-install rollback.
fn remove_registration(machine: bool, disable: bool, unregister: bool, paths: &[(HKEY, String)], ops: &mut dyn RegistrationRemoval) -> Vec<String> {
    let mut problems = vec![];
    if !machine {
        if disable { if let Err(e) = ops.disable() { problems.push(e); } }
        if unregister { problems.extend(ops.unregister()); }
    }
    for (hive, path) in paths {
        if machine && *hive == HKEY_LOCAL_MACHINE { continue; }
        if let Err(e) = ops.delete(*hive, path) { problems.push(e); }
    }
    problems
}

// ---- TSF -----------------------------------------------------------------------------------------------------------

fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

fn install_layout_or_tip(flags: u32) -> Result<(), String> {
    unsafe {
        let lib = LoadLibraryW(PCWSTR(wide("input.dll").as_ptr())).map_err(|e| format!("input.dll: {e}"))?;
        let f = GetProcAddress(lib, windows::core::s!("InstallLayoutOrTip")).ok_or("InstallLayoutOrTip not found")?;
        let f: extern "system" fn(PCWSTR, u32) -> windows::core::BOOL = std::mem::transmute(f);
        let s = wide(&profile_string());
        if f(PCWSTR(s.as_ptr()), flags).as_bool() { Ok(()) } else { Err(format!("InstallLayoutOrTip({flags}) failed: {}", std::io::Error::last_os_error())) }
    }
}

fn register_profile(description: &str, icon_file: &str, icon_index: u32) -> Result<(), String> {
    com_init();
    unsafe {
        let mgr: ITfInputProcessorProfileMgr = CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("profile mgr: {e}"))?;
        let desc = wide(description);
        let icon = wide(icon_file);
        // bEnabledByDefault = false: the user turns it on in the language settings; the default order is not touched.
        mgr.RegisterProfile(&clsid(), ids::LANGID_ZH_TW, &profile(), &desc[..desc.len()-1], &icon[..icon.len()-1], icon_index, HKL::default(), 0, false, 0).map_err(|e| format!("RegisterProfile: {e}"))?;
    }
    Ok(())
}

fn tsf_register(dll: &Path) -> Result<(), String> {
    let dll = std::path::absolute(dll).map_err(|e| format!("absolute DLL path: {e}"))?;
    register_profile(ids::DESCRIPTION, &dll.to_string_lossy(), 0)?;
    unsafe {
        let cat: ITfCategoryMgr = CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("category mgr: {e}"))?;
        for c in categories() {
            cat.RegisterCategory(&clsid(), &c, &clsid()).map_err(|e| format!("RegisterCategory: {e}"))?;
        }
    }
    Ok(())
}

fn tsf_unregister() -> Vec<String> {
    com_init();
    let mut errs = vec![];
    unsafe {
        match CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) {
            Ok(cat) => {
                for c in categories() {
                    if let Err(e) = cat.UnregisterCategory(&clsid(), &c, &clsid()) {
                        errs.push(format!("UnregisterCategory: {e}"));
                    }
                }
            }
            Err(e) => errs.push(format!("category mgr: {e}")),
        }
        match CoCreateInstance::<_, ITfInputProcessorProfileMgr>(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) {
            Ok(mgr) => {
                if let Err(e) = mgr.UnregisterProfile(&clsid(), ids::LANGID_ZH_TW, &profile(), 0) {
                    errs.push(format!("UnregisterProfile: {e}"));
                }
            }
            Err(e) => errs.push(format!("profile mgr: {e}")),
        }
    }
    errs
}

// ---- engine ---------------------------------------------------------------------------------------------------------

trait EngineStopHooks {
    fn elapsed(&self) -> std::time::Duration;
    fn stop_pipe(&mut self, remaining: std::time::Duration) -> Result<bool, String>;
    fn lock_held(&mut self) -> Result<bool, String>;
    fn wait(&mut self);
}

/// The marker blocks new starts; an engine already starting may hold only running.lock.
fn wait_engine_absent(hooks: &mut dyn EngineStopHooks) -> Result<bool, String> {
    let timeout = std::time::Duration::from_secs(10);
    let mut stopped = false;
    loop {
        let remaining = timeout.checked_sub(hooks.elapsed()).filter(|d| !d.is_zero())
            .ok_or("engine still present after 10 s (pipe or running.lock); installation aborted")?;
        if hooks.stop_pipe(remaining)? {
            stopped = true;
        } else if !hooks.lock_held()? {
            return Ok(stopped);
        }
        hooks.wait();
    }
}

struct SystemEngineStop {
    engines: Vec<(String, Vec<String>)>,
    data: Vec<PathBuf>,
    started: std::time::Instant,
}

impl EngineStopHooks for SystemEngineStop {
    fn elapsed(&self) -> std::time::Duration { self.started.elapsed() }

    fn stop_pipe(&mut self, remaining: std::time::Duration) -> Result<bool, String> {
        let attempt = std::time::Instant::now();
        let mut stopped = false;
        for (pipe, images) in &self.engines {
            let Some(client) = confirm::connect_verified(pipe, images)? else { continue; };
            let pid = client.server_pid;
            unsafe {
                let h = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid).map_err(|e| e.to_string())?;
                let terminated = TerminateProcess(h, 0).map_err(|e| e.to_string());
                let wait_ms = remaining.saturating_sub(attempt.elapsed()).as_millis() as u32;
                let w = if terminated.is_ok() { WaitForSingleObject(h, wait_ms) } else { WAIT_OBJECT_0 };
                let _ = CloseHandle(h);
                terminated?;
                if w != WAIT_OBJECT_0 { return Err("engine did not exit before stop deadline".into()); }
            }
            stopped = true;
        }
        Ok(stopped)
    }

    fn lock_held(&mut self) -> Result<bool, String> {
        use std::os::windows::fs::OpenOptionsExt;
        let mut held = false;
        for data in &self.data {
            let lock = data.join("running.lock");
            match fs::OpenOptions::new().read(true).write(true).share_mode(7).open(&lock) {
                Ok(_) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) if e.raw_os_error() == Some(32) => held = true,
                Err(e) => return Err(format!("cannot examine {}: {e}", lock.display())),
            }
        }
        Ok(held)
    }

    fn wait(&mut self) { std::thread::sleep(std::time::Duration::from_millis(200)); }
}

/// Waits until both the verified engine pipe and held running.lock are absent.
fn stop_engine(root: &Path) -> Result<bool, String> {
    stop_engine_in(root, false)
}

fn stop_engine_in(root: &Path, old: bool) -> Result<bool, String> {
    let other = if old { self::root() } else { legacy_root() };
    let engines = candidate_engines(&[root, &other], &[engine_pipe(false)?, engine_pipe(true)?]);
    wait_engine_absent(&mut SystemEngineStop {
        engines,
        data: state_dirs(),
        started: std::time::Instant::now(),
    })
}

// ---- commands ---------------------------------------------------------------------------------------------------------

fn plan() {
    let r = root();
    println!("地瓜輸入法 TwPotato：使用者模式安裝；全機模式存在時拒絕安裝");
    println!("legacy root           {} (cleanup-legacy)", legacy_root().display());
    println!("state directories     {:?}", state_dirs());
    println!("settings (optional)   {} (--settings-exe <path>)", r.join(SETTINGS_EXE).display());
    println!("settings shortcut     {}", settings_shortcut().map(|p| p.display().to_string()).unwrap_or_else(|e| e));
    println!("install root          {}", r.display());
    println!("files                 {}, {}, assets\\{{data.txt, common-readings.txt, model.bin, prior.pack, optional pn.fst pn.lst pn.cfg model.cfg gen.fst gen.lst emoji.tsv wordbonus.tsv wordbonus.bin lm-f32.bin lm-vocab.json}}, user-data.on, {MANIFEST}", "twime_tsf-<install time>.dll (a fresh name per install)", ids::ENGINE_NAME);
    println!("registry (HKCU)       {}  (default = DLL path, ThreadingModel = Apartment)", clsid_key());
    println!("TSF profile           RegisterProfile  clsid={}  profile={}  langid=0x{:04X}  enabled-by-default=false", ids::guid_string(ids::CLSID_TIP), ids::guid_string(ids::PROFILE), ids::LANGID_ZH_TW);
    println!("TSF categories        keyboard TIP, immersive support, systray support");
    println!("enable                InstallLayoutOrTip(\"{}\")", profile_string());
    println!("upgrade               每次更新 profile 描述與 DLL 圖示；改根升級不呼叫 InstallLayoutOrTip");
    println!("not touched           default input method order, other profiles, system files");
    println!("ident                 顯示目前 SID、完整性層級與提升狀態（唯讀）");
    println!("diag                  唯讀診斷：ctfmon、輸入服務、作用中 profile、本 TIP 註冊、引擎 pipe、簽章、Smart App Control（不改任何設定）");
    println!("MSI machine commands  由 MSI 以 SYSTEM 執行；只動 HKLM 與明確指定的目錄");
    println!("machine-register      --dll <absolute-path> [--test-fail-after-categories <0..3>]");
    println!("machine-unregister    無參數；移除本 TIP 的 HKLM 註冊");
    println!("machine-rollback-register --tip-present <0|1> --old-dll <absolute-path-or-empty> --dll <absolute-path>");
    println!("machine-stop          --root <absolute-path>；標記並停止該目錄中的引擎");
    println!("machine-release       --root <absolute-path>；移除標記與空目錄");
}

fn verify() {
    let r = root();
    let yes = |b: bool| if b { "present" } else { "absent" };
    println!("地瓜輸入法 TwPotato");
    println!("legacy root           {} {}", yes(legacy_root().exists()), legacy_root().display());
    match install::com_target() {
        Ok(Some(target)) => {
            println!("HKCU COM target       {target}");
            println!("HKCU COM directory    {}", Path::new(&target).parent().map(|p| p.display().to_string()).unwrap_or_default());
        }
        Ok(None) => println!("HKCU COM target       absent"),
        Err(e) => println!("HKCU COM target       cannot read: {e}"),
    }
    println!("state directories     {:?}", state_dirs());
    println!("settings exe          {} {}", yes(r.join(SETTINGS_EXE).exists()), r.join(SETTINGS_EXE).display());
    match settings_shortcut() {
        Ok(p) => println!("settings shortcut     {} {}", yes(p.exists()), p.display()),
        Err(e) => println!("settings shortcut     {e}"),
    }
    if let Ok(p) = legacy_shortcut() { println!("legacy shortcut       {} {}", yes(p.exists()), p.display()); }
    // WebView2 Evergreen per-user / per-machine runtime registration.
    // Microsoft distribution guidance: read pv in HKCU and the 32-bit HKLM view.
    // https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution
    let webview = r"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let webview_present = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].iter().any(|hive| {
        use windows::Win32::System::Registry::{KEY_WOW64_32KEY, RegQueryValueExW};
        let mut key = HKEY::default();
        let status = unsafe { RegOpenKeyExW(*hive, PCWSTR(wide(webview).as_ptr()), None, KEY_READ | KEY_WOW64_32KEY, &mut key) };
        if status.is_err() { return false; }
        let mut value = [0u16; 128]; let mut len = (value.len() * 2) as u32; let mut kind = REG_SZ;
        let result = unsafe { RegQueryValueExW(key, PCWSTR(wide("pv").as_ptr()), None, Some(&mut kind), Some(value.as_mut_ptr().cast()), Some(&mut len)) };
        unsafe { let _ = RegCloseKey(key); }
        let version = String::from_utf16_lossy(&value[..value.iter().position(|c| *c == 0).unwrap_or(value.len())]);
        result.is_ok() && kind == REG_SZ && version.split('.').any(|s| s.parse::<u32>().is_ok_and(|v| v > 0))
    });
    println!("WebView2 runtime      {} (settings requires Microsoft Edge WebView2 Runtime)", if webview_present { "detected" } else { "not detected; install runtime if settings cannot open" });
    println!("install root          {}", yes(r.exists()));
    for f in installed_dlls(&r).iter().map(String::as_str).chain([ids::ENGINE_NAME, "assets\\data.txt", "assets\\common-readings.txt", "assets\\model.bin", "assets\\prior.pack", "assets\\pn.cfg", "assets\\model.cfg", "assets\\gen.fst", "assets\\gen.lst", "assets\\emoji.tsv", "assets\\wordbonus.tsv", "assets\\wordbonus.bin", "user-data.on", MANIFEST, ids::UNINSTALL_MARKER]) {
        println!("file {f:<28} {}", yes(r.join(f).exists()));
    }
    println!("HKCU CLSID key        {}", yes(reg_exists(&clsid_key())));
    if reg_exists(&clsid_key()) && reg_exists_in(HKEY_LOCAL_MACHINE, &clsid_key()) {
        println!("警告：HKCU 與 HKLM 同時存在 CLSID；改用全機模式前，每個帳號請先執行 `twime-setup uninstall`。");
    }
    println!("engine process        {}", match twime_winpipe::Client::connect(&twime_winpipe::pipe_name(&twime_winpipe::current_identity().unwrap()), None, 0) {
        Ok(_) => "running",
        Err(_) => "not reachable",
    });
}

fn arg(a: &[String], flag: &str) -> Option<String> {
    a.iter().position(|x| x == flag).and_then(|i| a.get(i + 1)).cloned()
}

/// Our DLL files in the install root (`twime_tsf.dll`, `twime_tsf-<n>.dll`; not the `.stuck` leftovers).
fn installed_dlls(r: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(r)
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("twime_tsf") && n.ends_with(".dll")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
fn copy_into(src: &Path, dst: &Path) -> Result<(), String> {
    let tmp = dst.with_extension("tmp-install");
    fs::copy(src, &tmp).map_err(|e| format!("copy {} -> {}: {e}", src.display(), tmp.display()))?;
    fs::rename(&tmp, dst).map_err(|e| format!("rename {}: {e}", dst.display()))
}

fn install(a: &[String]) -> Result<(), String> {
    // All installation-state checks happen before constructing or mutating a transaction.
    let r = root();
    let old = legacy_root();
    let com = install::com_target()?;
    let active = legacy::active_in(&r, &old, com.as_deref(), machine_installed()?)?;
    let upgrade = active == legacy::Active::Legacy;
    if upgrade {
        if !legacy::residue(&r)? && r.exists() && fs::read_dir(&r).map_err(|e| e.to_string())?.next().is_some() {
            return Err(format!("改根升級的新根必須是全新目錄：{}", r.display()));
        }
        if legacy::settings_running(&old)? { return Err("舊設定程式正在執行，請先關閉".into()); }
        println!("升級期間請勿開啟設定程式；只執行一個安裝器");
    }
    if !a.iter().any(|x| x == "--confirm") {
        return Err("install needs --confirm (see `plan`)".into());
    }
    let bin = PathBuf::from(arg(a, "--bin-dir").ok_or("--bin-dir missing")?);
    let assets = PathBuf::from(arg(a, "--assets-src").ok_or("--assets-src missing")?);
    if a.iter().any(|s| s == "--settings-exe") && arg(a, "--settings-exe").is_none() {
        return Err("--settings-exe missing path".into());
    }
    let settings_exe = arg(a, "--settings-exe").map(PathBuf::from);
    if let Some(exe) = &settings_exe {
        if !exe.is_file() { return Err(format!("settings exe not found: {}", exe.display())); }
    }
    let mut system = install::SystemInstall::new(active != legacy::Active::Fresh, settings_exe.is_some() || (upgrade && old.join(SETTINGS_EXE).exists()))?;
    let marked = std::cell::Cell::new(false);
    // userdata::user_dir() currently shares state::data_dir()'s default TwPotato path;
    // TWIME_DATA_DIR only redirects state, never the user's settings/lexicon destination.
    let migrate = || if upgrade { legacy::copy_data(&legacy_data(), &twime_winpipe::state::data_dir(), &old) } else { vec![] };
    let mut hooks = install::LegacyHooks { inner: &mut system, old_root: &old,
        old_shortcut: if upgrade { Some(legacy_shortcut()?) } else { None }, marker_written: &marked,
        com: &install::com_target, migrate: &migrate };
    // Keep an existing settings program usable even if --settings-exe was omitted.
    let settings_source = settings_exe.or_else(|| if upgrade && old.join(SETTINGS_EXE).is_file() { Some(old.join(SETTINGS_EXE)) } else { None });
    install::install_files_in(&r, &bin, &assets, settings_source.as_deref(), &|| {
        if upgrade {
            marked.set(true); // Also clean up a partially written marker if writing it fails.
            fs::write(old.join(ids::UNINSTALL_MARKER), b"1\n").map_err(|e| e.to_string())?;
            stop_engine_in(&old, true)?;
        }
        stop_engine(&r)
    }, &mut hooks)
}

/// Everything an uninstall touches. The real one is built by `Ctx::real()`; `uninstall-test` builds one that points at a
/// private directory, a private pipe and a private registry prefix below HKCU, and skips the steps that would change the
/// system (TSF registration calls and the real HKLM / HKCU records).
struct Ctx {
    root: PathBuf,
    data: Vec<PathBuf>,
    pipe: String,
    image: String,
    other: Option<(PathBuf, String, String)>,
    real: bool,
    allowed: Vec<(HKEY, String)>,
    com: Option<String>,
    shortcuts: Vec<PathBuf>,
}

impl Ctx {
    fn real() -> Result<Ctx, String> {
        let root = root();
        let old = legacy_root();
        Ok(Ctx {
            image: root.join(ids::ENGINE_NAME).to_string_lossy().into_owned(),
            data: state_dirs(), pipe: engine_pipe(false)?, root,
            other: Some((old.clone(), engine_pipe(true)?, old.join(ids::ENGINE_NAME).to_string_lossy().into_owned())),
            real: true, allowed: tip_records(), com: Some(clsid_key()),
            shortcuts: vec![settings_shortcut()?, legacy_shortcut()?],
        })
    }

    /// Private roots, data and pipe only; no TSF calls or real registration changes.
    fn test(a: &[String]) -> Result<Ctx, String> {
        let root = PathBuf::from(arg(a, "--root").ok_or("--root missing")?);
        let data = PathBuf::from(arg(a, "--data-dir").ok_or("--data-dir missing")?);
        // Resolve existing ancestors too, so relative spelling/junctions cannot bypass the guard.
        fn absolute(p: &Path) -> Result<PathBuf, String> {
            let p = std::path::absolute(p).map_err(|e| e.to_string())?;
            if p.exists() { return fs::canonicalize(p).map_err(|e| e.to_string()); }
            let parent = p.parent().ok_or("invalid private path")?;
            Ok(absolute(parent)?.join(p.file_name().ok_or("invalid private path")?))
        }
        let protected = [self::root(), legacy_root(), legacy_data(), twime_winpipe::state::data_dir()];
        for path in [&root, &data] {
            let path = absolute(path)?;
            for protected in &protected {
                let protected = absolute(protected)?;
                let a = path.to_string_lossy().to_lowercase();
                let b = protected.to_string_lossy().to_lowercase();
                if Path::new(&a).starts_with(Path::new(&b)) || Path::new(&b).starts_with(Path::new(&a)) {
                    return Err("uninstall-test refuses real install/state paths and their parents".into());
                }
            }
        }
        Ok(Ctx {
            image: root.join(ids::ENGINE_NAME).to_string_lossy().into_owned(),
            data: vec![data], pipe: twime_winpipe::qualify(&arg(a, "--pipe").ok_or("--pipe missing")?), root,
            other: None, real: false, allowed: vec![(HKEY_CURRENT_USER, tip_path(TEST_TIP_PREFIX))],
            com: None, shortcuts: vec![],
        })
    }
}

/// One verified engine per pipe, allowing every installed image that uses it.
fn candidate_engines(roots: &[&Path], pipes: &[String]) -> Vec<(String, Vec<String>)> {
    group_engines(roots.iter().flat_map(|root| pipes.iter().map(move |pipe|
        (pipe.clone(), root.join(ids::ENGINE_NAME).to_string_lossy().into_owned()))))
}

fn group_engines(engines: impl IntoIterator<Item = (String, String)>) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = vec![];
    for (pipe, image) in engines {
        if !Path::new(&image).is_file() { continue; }
        if let Some((_, images)) = groups.iter_mut().find(|(p, _)| p.eq_ignore_ascii_case(&pipe)) {
            if !confirm::image_allowed(&image, images) { images.push(image); }
        } else { groups.push((pipe, vec![image])); }
    }
    groups
}

trait UninstallEngine {
    fn status(&mut self, pipe: &str, image: &[String]) -> Result<Option<(u16, u16, u16)>, String> {
        confirm::engine_status(pipe, image)
    }
    fn confirm(&mut self, data: &[PathBuf], engines: &[(String, Vec<String>)]) -> Result<(), String> {
        confirm::check_many_with(data, std::time::Duration::from_secs(10), &mut || self.statuses(engines))
    }
    fn statuses(&mut self, engines: &[(String, Vec<String>)]) -> Result<Vec<(u16, u16, u16)>, String> {
        let mut statuses = vec![];
        for (pipe, images) in engines {
            if let Some(status) = self.status(pipe, images)? { statuses.push(status); }
        }
        Ok(statuses)
    }
    fn shutdown(&mut self, pipe: &str, image: &[String]) -> Result<(), String>;
}
struct SystemUninstallEngine;
impl UninstallEngine for SystemUninstallEngine {
    fn shutdown(&mut self, pipe: &str, image: &[String]) -> Result<(), String> {
        confirm::shutdown_engine(pipe, image)
    }
}

fn uninstall() -> Result<(), String> { uninstall_in(&Ctx::real()?) }
fn uninstall_in(c: &Ctx) -> Result<(), String> {
    uninstall_with(c, if c.real { machine_installed()? } else { false }, &mut SystemRegistration, &mut SystemUninstallEngine)
}

/// All confirmations precede the first marker or registry/file mutation.
fn uninstall_with(c: &Ctx, machine: bool, registration: &mut dyn RegistrationRemoval, engine: &mut dyn UninstallEngine) -> Result<(), String> {
    let mut roots = vec![(&c.root, &c.pipe, &c.image)];
    if let Some((root, pipe, image)) = &c.other { roots.push((root, pipe, image)); }
    let engines = if c.real {
        candidate_engines(&roots.iter().map(|(root, _, _)| root.as_path()).collect::<Vec<_>>(),
            &roots.iter().map(|(_, pipe, _)| (*pipe).clone()).collect::<Vec<_>>())
    } else {
        group_engines(roots.iter().map(|(_, pipe, image)| ((*pipe).clone(), (*image).clone())))
    };
    engine.confirm(&c.data, &engines)?;
    println!("step confirm: confirmed");
    for (r, _, _) in &roots {
        if r.exists() { fs::write(r.join(ids::UNINSTALL_MARKER), b"1\n").map_err(|e| e.to_string())?; }
    }
    for (pipe, image) in &engines { engine.shutdown(pipe, image)?; }
    let mut problems = remove_registration(machine, c.real, c.real, &c.allowed, registration);
    if let Some(key) = &c.com {
        if let Err(e) = reg_delete_tree(key) { problems.push(e); }
    }
    for shortcut in &c.shortcuts {
        if let Err(e) = remove_settings_shortcut(shortcut) { problems.push(e); }
    }
    for (r, _, _) in roots {
        if let Err(e) = legacy::cleanup(r, None) { problems.push(e); }
    }
    if problems.is_empty() { println!("uninstalled"); Ok(()) } else { Err(problems.join("; ")) }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = match a.first().map(String::as_str) {
        Some("ident") => machine::ident(),
        Some("diag") => diag::run(),
        Some("machine-register" | "machine-unregister" | "machine-rollback-register" | "machine-stop" | "machine-release") => machine::run(&a),
        Some("user-migrate-prepare" | "user-migrate-restore" | "user-migrate-finish") => migrate::run(&a),
        None | Some("plan") => {
            plan();
            Ok(())
        }
        Some("verify") => {
            verify();
            Ok(())
        }
        Some("install") => install(&a),
        Some("uninstall") => uninstall(),
        Some("cleanup-legacy") => install::com_target().and_then(|com| legacy::cleanup_inactive(&root(), &legacy_root(), com.as_deref())),
        // Test: the whole uninstall against a private root / data dir / pipe / HKCU test prefix (no system change).
        Some("uninstall-test") => Ctx::test(&a).and_then(|c| uninstall_in(&c)),
        // Test-only, per user: COM class under HKCU pointing at a staged DLL (no TSF profile, nothing system wide).
        Some("com-register") => {
            let dll = a.get(1).cloned().ok_or("com-register <dll path>".to_string());
            dll.and_then(|dll| {
                let key = clsid_key();
                reg_set(&key, None, "TwPotato TSF text input processor")?;
                reg_set(&format!(r"{key}\InprocServer32"), None, &dll)?;
                reg_set(&format!(r"{key}\InprocServer32"), Some("ThreadingModel"), "Apartment")
            })
        }
        Some("com-unregister") => reg_delete_tree(&clsid_key()),
        // Test: only the confirmation step of an uninstall (no changes).
        Some("shutdown") => (|| {
            let id = twime_winpipe::current_identity().map_err(|e| e.to_string())?;
            let pipe = std::env::var("TWIME_PIPE").map(|p| twime_winpipe::qualify(&p)).unwrap_or_else(|_| twime_winpipe::pipe_name(&id));
            let image = std::env::var("TWIME_ENGINE_IMAGE").unwrap_or_else(|_| root().join(ids::ENGINE_NAME).to_string_lossy().into_owned());
            let mut engines = candidate_engines(&[&root(), &legacy_root()], &[engine_pipe(false)?, engine_pipe(true)?]);
            for (p, images) in group_engines([(pipe, image)]) {
                if let Some((_, allowed)) = engines.iter_mut().find(|(existing, _)| existing.eq_ignore_ascii_case(&p)) {
                    for image in images { if !confirm::image_allowed(&image, allowed) { allowed.push(image); } }
                } else { engines.push((p, images)); }
            }
            for (p, images) in engines {
                confirm::shutdown_engine(&p, &images)?;
            }
            println!("engine ended"); Ok(())
        })(),
        Some("confirm") => (|| {
            let id = twime_winpipe::current_identity().map_err(|e| e.to_string())?;
            let pipe = std::env::var("TWIME_PIPE").map(|p| twime_winpipe::qualify(&p)).unwrap_or_else(|_| twime_winpipe::pipe_name(&id));
            let image = std::env::var("TWIME_ENGINE_IMAGE").unwrap_or_else(|_| root().join(ids::ENGINE_NAME).to_string_lossy().into_owned());
            let mut engines: Vec<_> = candidate_engines(&[&root(), &legacy_root()], &[engine_pipe(false)?, engine_pipe(true)?])
                .into_iter().flat_map(|(p, images)| images.into_iter().map(move |image| (p.clone(), image))).collect();
            engines.push((pipe, image));
            SystemUninstallEngine.confirm(&state_dirs(), &group_engines(engines))?;
            println!("confirmed"); Ok(())
        })(),
        Some("stop-engine") => stop_engine(&root()).map(|b| println!("{}", if b { "engine stopped" } else { "engine not running" })),
        Some(x) => Err(format!("unknown command {x}")),
    };
    if let Err(e) = r {
        eprintln!("twime-setup: {e}");
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    pub(super) struct RecordingRegistration { pub calls: Vec<String> }
    impl RegistrationRemoval for RecordingRegistration {
        fn disable(&mut self) -> Result<(), String> { self.calls.push("disable".into()); Ok(()) }
        fn unregister(&mut self) -> Vec<String> { self.calls.push("unregister".into()); vec![] }
        fn delete(&mut self, hive: HKEY, path: &str) -> Result<(), String> {
            self.calls.push(format!("{} {path}", if hive == HKEY_LOCAL_MACHINE { "HKLM" } else { "HKCU" })); Ok(())
        }
    }

    struct PrivateFiles(PathBuf);
    impl PrivateFiles {
        fn new() -> Self {
            Self(test_temp_dir("twpotato-uninstall-test"))
        }
    }
    impl Drop for PrivateFiles { fn drop(&mut self) { assert!(self.0.starts_with(std::env::temp_dir())); let _ = fs::remove_dir_all(&self.0); } }

    struct FakeUninstallEngine { roots: Vec<PathBuf>, confirmed: Vec<PathBuf>, ended: Vec<(String, String)>, fail: bool }
    impl UninstallEngine for FakeUninstallEngine {
        fn confirm(&mut self, data: &[PathBuf], engines: &[(String, Vec<String>)]) -> Result<(), String> {
            assert!(self.roots.iter().all(|r| !r.join(ids::UNINSTALL_MARKER).exists()));
            assert_eq!(engines.len(), 2);
            for (_, images) in engines {
                assert_eq!(images.len(), self.roots.iter().filter(|r| r.join(ids::ENGINE_NAME).exists()).count());
                for root in &self.roots {
                    let image = root.join(ids::ENGINE_NAME);
                    assert_eq!(confirm::image_allowed(&image.to_string_lossy(), images), image.is_file());
                }
            }
            self.confirmed.extend_from_slice(data);
            if self.fail { Err("injected cannot confirm".into()) } else { Ok(()) }
        }
        fn shutdown(&mut self, pipe: &str, image: &[String]) -> Result<(), String> {
            assert_eq!(self.confirmed.len(), 2);
            assert!(self.roots.iter().all(|r| r.join(ids::UNINSTALL_MARKER).exists()));
            self.ended.push((pipe.into(), image[0].clone())); Ok(())
        }
    }

    #[test]
    fn state_directories_always_include_both_defaults_and_deduplicate_override() {
        let current = PathBuf::from(r"C:\private\TwPotato");
        let old = PathBuf::from(r"C:\private\TwIme");
        assert_eq!(state_dirs_in(current.clone(), old.clone(), None), [current.clone(), old.clone()]);
        for extra in [current.clone(), old.clone(), PathBuf::from(r"c:\PRIVATE\TWPOTATO"),
            PathBuf::from(r"C:\private\TwPotato\"), PathBuf::from(r"c:/private/./TWPOTATO/.")] {
            assert_eq!(state_dirs_in(current.clone(), old.clone(), Some(extra)), [current.clone(), old.clone()]);
        }
        let extra = PathBuf::from(r"C:\private\override");
        assert_eq!(state_dirs_in(current.clone(), old.clone(), Some(extra.clone())), [current, old, extra]);
    }

    #[test]
    fn existing_state_directories_are_deduplicated_by_canonical_path() {
        let t = PrivateFiles::new(); let current = t.0.join("current"); let old = t.0.join("old");
        fs::create_dir_all(current.join("child")).unwrap(); fs::create_dir(&old).unwrap();
        for alias in [fs::canonicalize(&current).unwrap(), current.join("child").join(".."), current.join(".")] {
            assert_eq!(state_dirs_in(current.clone(), old.clone(), Some(alias)), [current.clone(), old.clone()]);
        }
    }

    #[test]
    fn shared_pipe_checks_once_and_accepts_either_existing_root_image() {
        struct Probe { image: String, calls: usize }
        impl UninstallEngine for Probe {
            fn status(&mut self, _: &str, images: &[String]) -> Result<Option<(u16, u16, u16)>, String> {
                self.calls += 1;
                if !confirm::image_allowed(&self.image, images) { return Err("image rejected".into()); }
                Ok(Some((0, 0, 0)))
            }
            fn shutdown(&mut self, _: &str, _: &[String]) -> Result<(), String> { unreachable!() }
        }
        let t = PrivateFiles::new(); let new = t.0.join("new.exe"); let old = t.0.join("old.exe");
        fs::write(&old, b"old").unwrap();
        for both_exist in [false, true] {
            if both_exist { fs::write(&new, b"new").unwrap(); }
            let groups = group_engines([("private-pipe".into(), new.to_string_lossy().into_owned()),
                ("private-pipe".into(), old.to_string_lossy().into_owned()),
                ("never-query".into(), t.0.join("missing.exe").to_string_lossy().into_owned())]);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].1.len(), if both_exist { 2 } else { 1 });
            let mut probe = Probe { image: old.to_string_lossy().into_owned(), calls: 0 };
            assert_eq!(probe.statuses(&groups).unwrap(), [(0, 0, 0)]);
            assert_eq!(probe.calls, 1);
            probe.image = new.to_string_lossy().into_owned();
            assert_eq!(probe.statuses(&groups).is_ok(), both_exist);
            probe.image = t.0.join("untrusted.exe").to_string_lossy().into_owned();
            assert!(probe.statuses(&groups).is_err());
        }
        assert!(confirm::image_allowed(r"c:/PRIVATE/./old.exe", &[r"C:\private\old.exe".into()]));
        assert!(confirm::image_allowed(r"\\?\C:\private\old.exe", &[r"C:\private\old.exe".into()]));
        assert!(!confirm::image_allowed(r"C:\private\old.exe.bak", &[r"C:\private\old.exe".into()]));
        assert!(!confirm::image_allowed(r"C:\private\old.exe", &[]));
    }

    #[test]
    fn stop_finds_new_root_image_on_legacy_pipe_and_accepts_both_absent() {
        // Inject pipe discovery and stopping: never open or terminate a real process.
        struct Stop {
            groups: Vec<(String, Vec<String>)>,
            running: Option<(String, String)>,
            stopped: bool,
        }
        impl EngineStopHooks for Stop {
            fn elapsed(&self) -> std::time::Duration { std::time::Duration::ZERO }
            fn stop_pipe(&mut self, _: std::time::Duration) -> Result<bool, String> {
                let Some((pipe, image)) = &self.running else { return Ok(false); };
                let (_, allowed) = self.groups.iter().find(|(p, _)| p == pipe).ok_or("pipe not found")?;
                if !confirm::image_allowed(image, allowed) { return Err("image rejected".into()); }
                self.running = None;
                self.stopped = true;
                Ok(true)
            }
            fn lock_held(&mut self) -> Result<bool, String> { Ok(false) }
            fn wait(&mut self) {}
        }
        let t = PrivateFiles::new();
        let new = t.0.join("TwPotato"); let old = t.0.join("TwIme");
        fs::create_dir(&new).unwrap(); fs::create_dir(&old).unwrap();
        let image = new.join(ids::ENGINE_NAME);
        fs::write(&image, b"private mock engine").unwrap();
        let pipes = ["private-new-pipe".into(), "private-legacy-pipe".into()];
        for both_roots in [false, true] {
            if both_roots { fs::write(old.join(ids::ENGINE_NAME), b"private old engine").unwrap(); }
            let groups = candidate_engines(&[&new, &old], &pipes);
            assert_eq!(groups.len(), 2);
            assert!(groups.iter().all(|(_, images)| images.len() == if both_roots { 2 } else { 1 }));
            let mut stop = Stop { groups, running: Some((pipes[1].clone(), image.to_string_lossy().into_owned())), stopped: false };
            assert!(wait_engine_absent(&mut stop).unwrap());
            assert!(stop.stopped);
            stop.stopped = false;
            assert!(!wait_engine_absent(&mut stop).unwrap());
            assert!(!stop.stopped);
            stop.running = Some((pipes[1].clone(), t.0.join("untrusted.exe").to_string_lossy().into_owned()));
            assert!(wait_engine_absent(&mut stop).is_err());
            assert!(!stop.stopped);
        }
    }

    #[test]
    fn confirmation_requires_enough_verified_engines_and_every_engine_idle_and_normal() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = PrivateFiles::new();
        let dirs = vec![t.0.join("one"), t.0.join("two")];
        let locks: Vec<_> = dirs.iter().map(|dir| {
            fs::create_dir_all(dir).unwrap();
            fs::OpenOptions::new().write(true).create_new(true).share_mode(0).open(dir.join("running.lock")).unwrap()
        }).collect();
        for (statuses, allowed) in [
            (vec![], false), (vec![(0, 0, 0)], false),
            (vec![(0, 0, 0), (0, 0, 0)], true),
            (vec![(0, 0, 0), (1, 0, 0)], false),
            (vec![(0, 0, 0), (0, 0, 1)], false),
            (vec![(0, 0, 0), (0, 0, 0), (1, 1, 0)], false),
        ] {
            assert_eq!(confirm::check_many_with(&dirs, std::time::Duration::ZERO,
                &mut || Ok(statuses.clone())).is_ok(), allowed, "{statuses:?}");
        }
        drop(locks);
    }

    #[test]
    fn uninstall_rejects_two_held_locks_with_only_one_verified_zero_engine_without_mutation() {
        use std::os::windows::fs::OpenOptionsExt;
        struct PartialEngine;
        impl UninstallEngine for PartialEngine {
            fn status(&mut self, pipe: &str, _: &[String]) -> Result<Option<(u16, u16, u16)>, String> {
                Ok(if pipe == "private-new" { Some((0, 0, 0)) } else { None })
            }
            fn shutdown(&mut self, _: &str, _: &[String]) -> Result<(), String> { panic!("must reject before shutdown"); }
        }
        let t = PrivateFiles::new(); let root = t.0.join("new"); let old = t.0.join("old");
        let data = vec![t.0.join("new-state"), t.0.join("old-state")];
        for r in [&root, &old] {
            fs::create_dir_all(r).unwrap();
            fs::write(r.join(MANIFEST), b"state=installed\n").unwrap();
            fs::write(r.join(ids::ENGINE_NAME), b"engine").unwrap();
            fs::write(r.join(ids::UNINSTALL_MARKER), b"original marker").unwrap();
        }
        let locks: Vec<_> = data.iter().map(|d| {
            fs::create_dir_all(d).unwrap();
            fs::OpenOptions::new().write(true).create_new(true).share_mode(0).open(d.join("running.lock")).unwrap()
        }).collect();
        let c = Ctx { root: root.clone(), data, pipe: "private-new".into(), image: root.join(ids::ENGINE_NAME).to_string_lossy().into_owned(),
            other: Some((old.clone(), "private-old".into(), old.join(ids::ENGINE_NAME).to_string_lossy().into_owned())), real: false,
            allowed: vec![], com: None, shortcuts: vec![] };
        let mut registration = RecordingRegistration::default();
        assert!(uninstall_with(&c, false, &mut registration, &mut PartialEngine).unwrap_err().contains("not enough verified engines"));
        assert!(registration.calls.is_empty());
        for r in [&root, &old] {
            assert_eq!(fs::read(r.join(ids::UNINSTALL_MARKER)).unwrap(), b"original marker");
            assert_eq!(fs::read(r.join(MANIFEST)).unwrap(), b"state=installed\n");
            assert_eq!(fs::read(r.join(ids::ENGINE_NAME)).unwrap(), b"engine");
        }
        drop(locks);
    }

    #[test]
    fn uninstall_checks_both_state_directories_before_mutation_and_guards_machine_registration() {
        for machine in [false, true] {
            for missing_old_engine in [false, true] {
                for fail in [false, true] {
                    let t = PrivateFiles::new(); let root = t.0.join("new"); let old = t.0.join("old");
                    for r in [&root, &old] { fs::create_dir_all(r.join("assets")).unwrap(); fs::write(r.join(MANIFEST), b"state=installed\n").unwrap(); }
                    fs::write(root.join(ids::ENGINE_NAME), b"new").unwrap();
                    if !missing_old_engine { fs::write(old.join(ids::ENGINE_NAME), b"old").unwrap(); }
                    let shortcuts = vec![t.0.join("new.lnk"), t.0.join("old.lnk")];
                    for p in &shortcuts { fs::write(p, b"shortcut").unwrap(); }
                    let c = Ctx { root: root.clone(), data: vec![t.0.join("new-state"), t.0.join("old-state")], pipe: "private-new".into(),
                        image: root.join(ids::ENGINE_NAME).to_string_lossy().into_owned(),
                        other: Some((old.clone(), "private-old".into(), old.join(ids::ENGINE_NAME).to_string_lossy().into_owned())),
                        real: true, allowed: vec![(HKEY_LOCAL_MACHINE, "private-machine-tip".into()), (HKEY_CURRENT_USER, "private-user-tip".into())],
                        com: None, shortcuts: shortcuts.clone() };
                    let mut ops = RecordingRegistration::default();
                    let mut engine = FakeUninstallEngine { roots: vec![root.clone(), old.clone()], confirmed: vec![], ended: vec![], fail };
                    let result = uninstall_with(&c, machine, &mut ops, &mut engine);
                    if fail {
                        assert!(result.is_err()); assert!(ops.calls.is_empty() && engine.ended.is_empty());
                        assert!(root.join(MANIFEST).exists() && old.join(MANIFEST).exists());
                        assert!(shortcuts.iter().all(|p| p.exists()));
                    } else {
                        result.unwrap(); assert_eq!(engine.confirmed, c.data);
                        assert_eq!(engine.ended[0], (c.pipe.clone(), c.image.clone()));
                        assert_eq!(engine.ended.len(), 2);
                        assert_eq!(engine.ended[1], ("private-old".into(), c.image.clone()));
                        assert!(!root.exists() && !old.exists()); assert!(shortcuts.iter().all(|p| !p.exists()));
                        if machine { assert_eq!(ops.calls, ["HKCU private-user-tip"]); }
                        else { assert_eq!(ops.calls, ["disable", "unregister", "HKLM private-machine-tip", "HKCU private-user-tip"]); }
                    }
                }
            }
        }
    }

    #[test]
    fn uninstall_test_uses_only_private_hkcu_prefix_and_removes_its_files() {
        let t = PrivateFiles::new(); let root = t.0.join("installed"); fs::create_dir(&root).unwrap();
        fs::write(root.join(MANIFEST), b"state=installed\n").unwrap();
        let c = Ctx::test(&["--root".into(), root.to_string_lossy().into_owned(), "--data-dir".into(), t.0.join("data").to_string_lossy().into_owned(),
            "--pipe".into(), format!("twpotato-private-uninstall-test-{}", std::process::id())]).unwrap();
        assert!(c.allowed.iter().all(|(hive, path)| *hive == HKEY_CURRENT_USER && path.starts_with(TEST_TIP_PREFIX)));
        let mut ops = RecordingRegistration::default();
        uninstall_with(&c, true, &mut ops, &mut SystemUninstallEngine).unwrap();
        assert_eq!(ops.calls.len(), 1); assert!(ops.calls[0].starts_with("HKCU Software\\TwImeSetupTest\\"));
        assert!(!root.exists());
    }

    #[test]
    fn uninstall_test_refuses_both_real_roots_and_state_directories() {
        for path in [root(), legacy_root(), legacy_data(), twime_winpipe::state::data_dir()] {
            let args = ["--root".into(), path.to_string_lossy().into_owned(), "--data-dir".into(), "private-state".into(), "--pipe".into(), "private".into()];
            assert!(Ctx::test(&args).is_err());
        }
    }

    #[test]
    fn settings_shortcut_roundtrip_in_private_directory() {
        use windows::{core::Interface, Win32::{System::Com::{CoUninitialize, IPersistFile, STGM_READ}, UI::Shell::{IShellLinkW, ShellLink}}};
        let dir = test_temp_dir("twime-settings-shortcut-設定");
        struct Temp(PathBuf);
        impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _temp = Temp(dir.clone());
        let source = dir.join("source.exe");
        fs::write(&source, b"test-only executable placeholder").unwrap();
        copy_into(&source, &dir.join(SETTINGS_EXE)).unwrap();
        assert_eq!(fs::read(dir.join(SETTINGS_EXE)).unwrap(), fs::read(&source).unwrap());
        let shortcut = dir.join("Programs/地瓜輸入法 設定.lnk");
        create_settings_shortcut(&dir, &shortcut).unwrap();
        assert!(shortcut.is_file());
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                let persist: IPersistFile = link.cast().unwrap();
                persist.Load(PCWSTR(wide(&shortcut.to_string_lossy()).as_ptr()), STGM_READ).unwrap();
                let mut target = [0u16; 1024]; let mut working = [0u16; 1024];
                let mut description = [0u16; 1024];
                link.GetPath(&mut target, std::ptr::null_mut(), 0).unwrap();
                link.GetWorkingDirectory(&mut working).unwrap();
                link.GetDescription(&mut description).unwrap();
                let decode = |v: &[u16]| String::from_utf16_lossy(&v[..v.iter().position(|c| *c == 0).unwrap()]);
                assert_eq!(PathBuf::from(decode(&target)), dir.join(SETTINGS_EXE));
                assert_eq!(PathBuf::from(decode(&working)), dir);
                assert_eq!(decode(&description), "地瓜輸入法 設定");
            }
            CoUninitialize();
        }
        remove_settings_shortcut(&shortcut).unwrap();
        remove_settings_shortcut(&shortcut).unwrap();
        fs::remove_file(dir.join(SETTINGS_EXE)).unwrap();
        assert!(!shortcut.exists()); assert!(!dir.join(SETTINGS_EXE).exists());
    }

    #[test]
    fn delete_guarded_only_touches_whitelisted_paths_and_removes_the_whole_tree() {
        let base = format!(r"Software\TwImeSetupUnitTest{}", std::process::id());
        let ok = tip_path(&base);
        let other = format!(r"{base}\Other");
        for sub in ["", r"\Category\Category\{13A016DF-560B-46CD-947A-4C3AF1E0E35D}", r"\LanguageProfile\0x00000404"] {
            reg_set(&format!("{ok}{sub}"), Some("v"), "x").unwrap();
        }
        reg_set(&other, Some("v"), "x").unwrap();
        let allowed = vec![ok.clone()];
        assert!(delete_guarded(HKEY_CURRENT_USER, &other, &allowed).is_err(), "a path outside the whitelist is refused");
        assert!(reg_exists(&other));
        assert!(delete_guarded(HKEY_CURRENT_USER, &base, &allowed).is_err(), "a parent of the whitelisted key is refused");
        assert_eq!(delete_guarded(HKEY_CURRENT_USER, &ok.to_uppercase(), &allowed), Ok(true));
        assert!(!reg_exists(&ok));
        assert_eq!(delete_guarded(HKEY_CURRENT_USER, &ok, &allowed), Ok(false), "absent is fine");
        assert!(reg_exists(&other), "siblings stay");
        reg_delete_tree(&base).unwrap();
    }

    #[test]
    fn real_whitelist_is_exactly_the_two_tip_records() {
        let c = Ctx::real().unwrap();
        let paths: Vec<_> = c.allowed.iter().map(|(_, p)| p.as_str()).collect();
        assert_eq!(paths, [r"SOFTWARE\Microsoft\CTF\TIP\{C2DD0C9B-F6E9-489D-9ADF-F97658502774}", r"Software\Microsoft\CTF\TIP\{C2DD0C9B-F6E9-489D-9ADF-F97658502774}"]);
        let t = Ctx::test(&["--root".into(), "x".into(), "--data-dir".into(), "y".into(), "--pipe".into(), "z".into()]).unwrap();
        assert!(t.allowed.iter().all(|(h, _)| *h == HKEY_CURRENT_USER), "the test run never names HKLM");
    }
}
