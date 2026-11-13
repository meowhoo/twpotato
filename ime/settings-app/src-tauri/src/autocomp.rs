//! SETTINGS-AUTOCOMP (SETTINGS-AUTOCOMP-plan): the settings page's completion switches. The values live in the
//! user-level `autocomp.cfg` (shared parser in twime-userdata); the engine reads it at start-up, so a change takes effect
//! after the engine restarts (`restart_with`) or at the next sign-in.
use serde::Serialize;
use std::{path::Path, time::Duration};
use twime_userdata::autocomp_cfg::{self, Loaded};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AutocompDto {
    pub list: bool,
    pub pop: bool,
    /// The user file exists but is unusable (ignored by the engine as a whole).
    pub file_error: bool,
}

/// `autocomp_list` / `autocomp_pop` of the assets model.cfg, non-strict (the page only needs the fallback value):
/// anything unreadable or malformed counts as off.
fn model_keys(assets_cfg: &Path) -> (bool, bool) {
    let Ok(text) = std::fs::read_to_string(assets_cfg) else { return (false, false) };
    let (mut list, mut pop) = (false, false);
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) else { continue };
        match k {
            "autocomp_list" => list = v == "on",
            "autocomp_pop" => pop = v == "on",
            _ => {}
        }
    }
    (list, pop)
}

/// What the engine will do after its next start: the user file per key, else model.cfg, else off.
pub fn get(user_dir: &Path, root: &Path) -> AutocompDto {
    let loaded = autocomp_cfg::load(user_dir);
    let user = loaded.effective();
    let (ml, mp) = model_keys(&root.join("assets").join("model.cfg"));
    AutocompDto { list: user.list.unwrap_or(ml), pop: user.pop.unwrap_or(mp), file_error: loaded == Loaded::Bad }
}

/// Writes both keys (the popup is never on without the list). On error the caller does not restart anything.
pub fn set(user_dir: &Path, list: bool, pop: bool) -> Result<(), String> {
    autocomp_cfg::save(user_dir, list, list && pop).map_err(|e| format!("無法儲存自動完成設定：{e:?}"))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Probe {
    Up(u32),
    NotFound,
    Other(String),
}
/// Process-control boundary of the engine restart (real on Windows, fake in tests).
pub trait EngineCtl {
    fn probe(&self) -> Probe;
    fn kill(&self, pid: u32) -> Result<(), String>;
    fn alive(&self, pid: u32) -> bool;
    /// Starts the engine and returns the new process id.
    fn spawn(&self) -> Result<u32, String>;
    fn pause(&self, d: Duration);
}
#[derive(Debug, PartialEq)]
pub enum Restart {
    /// Restarted and answering as a new process.
    Done,
    /// Not confirmed within the wait (our new process is not the one answering, or nothing answered): the settings are
    /// saved and apply when the engine next starts (sign-in).
    Unconfirmed,
}
pub const KILL_WAIT_MS: u64 = 5_000;
pub const READY_WAIT_MS: u64 = 20_000;
const STEP_MS: u64 = 200;

pub fn restart_with(ctl: &dyn EngineCtl) -> Result<Restart, String> {
    let old = match ctl.probe() {
        Probe::Up(pid) => Some(pid),
        Probe::NotFound => None,
        Probe::Other(e) => return Err(format!("無法確認目前的引擎：{e}")),
    };
    if let Some(pid) = old {
        ctl.kill(pid).map_err(|e| format!("無法結束引擎：{e}"))?;
        let mut waited = 0;
        while ctl.alive(pid) {
            if waited >= KILL_WAIT_MS {
                return Err("引擎沒有在時限內結束".into());
            }
            ctl.pause(Duration::from_millis(STEP_MS));
            waited += STEP_MS;
        }
    }
    let mine = ctl.spawn().map_err(|e| format!("無法啟動引擎：{e}"))?;
    let mut waited = 0;
    loop {
        match ctl.probe() {
            Probe::Up(pid) if pid == mine => return Ok(Restart::Done),
            Probe::Other(e) if waited >= READY_WAIT_MS => return Err(format!("引擎狀態不明：{e}")),
            _ => {}
        }
        if waited >= READY_WAIT_MS {
            return Ok(Restart::Unconfirmed);
        }
        ctl.pause(Duration::from_millis(STEP_MS));
        waited += STEP_MS;
    }
}

/// Overrides that make a real restart unsafe: the settings page and the engine could then disagree on the user directory
/// (`TWIME_USER_DIR`) or the pipe belongs to a test engine (`TWIME_PIPE`).
pub fn restart_blocked() -> Option<&'static str> {
    if std::env::var_os("TWIME_PIPE").is_some() || std::env::var_os("TWIME_USER_DIR").is_some() {
        Some("目前為測試／目錄覆寫模式，未重啟引擎；設定已儲存")
    } else {
        None
    }
}
pub fn restart_message(r: Restart) -> &'static str {
    match r {
        Restart::Done => "引擎已重新啟動，設定生效",
        Restart::Unconfirmed => "設定已儲存，但尚未確認引擎已套用；請重新登入後生效",
    }
}

#[cfg(windows)]
pub struct RealCtl {
    pub pipe: String,
    pub root: std::path::PathBuf,
}
#[cfg(windows)]
impl RealCtl {
    fn exe(&self) -> std::path::PathBuf {
        self.root.join("twime-engined.exe")
    }
}
/// A connected pipe is `Up` only after a Hello exchange (protocol 5) is answered, within a bounded wait.
#[cfg(windows)]
fn hello(c: &twime_winpipe::Client) -> Probe {
    use twime_proto::{Event, Reply, Request};
    let bytes = Request { session: 0, seq: 1, ack: None, event: Event::Hello(5) }.encode();
    match c.call(&bytes, 3000) {
        Ok(b) => match Reply::decode(&b) {
            Some(Reply::HelloCaps { version, .. }) if version >= 5 => Probe::Up(c.server_pid),
            _ => Probe::Other("引擎回覆格式或版本不符".into()),
        },
        Err(e) => Probe::Other(format!("引擎沒有回應 Hello：{e:?}")),
    }
}
#[cfg(windows)]
impl EngineCtl for RealCtl {
    fn probe(&self) -> Probe {
        let image = self.exe().to_string_lossy().into_owned();
        match twime_winpipe::Client::connect(&self.pipe, Some(&image), 200) {
            Ok(c) => hello(&c),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Probe::NotFound,
            Err(e) => Probe::Other(e.to_string()),
        }
    }
    fn kill(&self, pid: u32) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() { Ok(()) } else { Err(format!("taskkill 失敗（{}）", out.status)) }
    }
    fn alive(&self, pid: u32) -> bool {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .creation_flags(0x0800_0000)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
            .unwrap_or(true)
    }
    fn spawn(&self) -> Result<u32, String> {
        use std::os::windows::process::CommandExt;
        let exe = self.exe();
        if !exe.is_file() {
            return Err(format!("找不到 {}", exe.display()));
        }
        // CREATE_NO_WINDOW | DETACHED_PROCESS, fixed path from the install root (never PATH), like the shell's spawn.
        std::process::Command::new(&exe)
            .arg("--assets-dir")
            .arg(self.root.join("assets"))
            .arg("--candidates")
            .current_dir(&self.root)
            .creation_flags(0x0800_0000 | 0x0000_0008)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|c| c.id())
            .map_err(|e| e.to_string())
    }
    fn pause(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("twime-acset-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    #[test]
    fn unset_is_off_and_model_cfg_is_the_fallback() {
        let root = tmp("r1");
        let user = root.join("u");
        assert_eq!(get(&user, &root), AutocompDto { list: false, pop: false, file_error: false });
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/model.cfg"), "int8=on\nautocomp_list=on\nautocomp_pop=on\n").unwrap();
        assert_eq!(get(&user, &root), AutocompDto { list: true, pop: true, file_error: false });
        set(&user, true, false).unwrap(); // first save creates the directory
        assert_eq!(get(&user, &root), AutocompDto { list: true, pop: false, file_error: false });
        set(&user, false, true).unwrap(); // pop is forced off without the list
        assert_eq!(get(&user, &root), AutocompDto { list: false, pop: false, file_error: false });
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn bad_user_file_shows_like_absent_with_a_flag() {
        let root = tmp("r2");
        let user = root.join("u");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/model.cfg"), "int8=on\nautocomp_list=on\n").unwrap();
        for junk in [&b"\xff\xfe"[..], b"nonsense\n", b"autocomp_list=on\nautocomp_list=off\n"] {
            std::fs::write(user.join("autocomp.cfg"), junk).unwrap();
            assert_eq!(get(&user, &root), AutocompDto { list: true, pop: false, file_error: true });
        }
        set(&user, false, false).unwrap(); // saving rebuilds it
        assert_eq!(get(&user, &root), AutocompDto { list: false, pop: false, file_error: false });
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn write_failure_is_an_error() {
        let root = tmp("r3");
        std::fs::create_dir_all(root.join("autocomp.cfg")).unwrap();
        assert!(set(&root, true, true).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    struct Fake {
        probes: RefCell<Vec<Probe>>,
        last: RefCell<Option<Probe>>,
        kill_ok: bool,
        alive_for: Cell<u32>,
        spawn_ok: bool,
        child: u32,
        killed: Cell<bool>,
        spawned: Cell<bool>,
        slept: Cell<u64>,
    }
    impl Fake {
        fn new(probes: Vec<Probe>) -> Self {
            Self { probes: RefCell::new(probes), last: RefCell::new(None), kill_ok: true, alive_for: Cell::new(0), spawn_ok: true, child: 11, killed: Cell::new(false), spawned: Cell::new(false), slept: Cell::new(0) }
        }
    }
    impl EngineCtl for Fake {
        fn probe(&self) -> Probe {
            let mut p = self.probes.borrow_mut();
            let next = if p.is_empty() { self.last.borrow().clone().unwrap() } else { p.remove(0) };
            *self.last.borrow_mut() = Some(next.clone());
            next
        }
        fn kill(&self, _pid: u32) -> Result<(), String> {
            self.killed.set(true);
            if self.kill_ok { Ok(()) } else { Err("denied".into()) }
        }
        fn alive(&self, _pid: u32) -> bool {
            let n = self.alive_for.get();
            if n > 0 { self.alive_for.set(n - 1); true } else { false }
        }
        fn spawn(&self) -> Result<u32, String> {
            self.spawned.set(true);
            if self.spawn_ok { Ok(self.child) } else { Err("no exe".into()) }
        }
        fn pause(&self, d: Duration) {
            self.slept.set(self.slept.get() + d.as_millis() as u64);
        }
    }
    #[test]
    fn restart_happy_path_and_not_running() {
        let f = Fake::new(vec![Probe::Up(10), Probe::NotFound, Probe::Up(11)]);
        assert_eq!(restart_with(&f), Ok(Restart::Done));
        assert!(f.killed.get() && f.spawned.get());
        let f = Fake::new(vec![Probe::NotFound, Probe::Up(11)]);
        assert_eq!(restart_with(&f), Ok(Restart::Done));
        assert!(!f.killed.get() && f.spawned.get());
        // a different engine (started by the shell before the save, so on the old settings) answers: not confirmed
        let f = Fake::new(vec![Probe::NotFound, Probe::Up(5)]);
        assert_eq!(restart_with(&f), Ok(Restart::Unconfirmed));
        // connectable but no Hello answer yet: keeps waiting, then reports an error rather than success
        let f = Fake::new(vec![Probe::NotFound, Probe::Other("no hello".into())]);
        assert!(restart_with(&f).is_err());
        assert!(!f.killed.get());
    }
    #[test]
    fn restart_other_probe_error_starts_nothing() {
        let f = Fake::new(vec![Probe::Other("busy".into())]);
        assert!(restart_with(&f).is_err());
        assert!(!f.killed.get() && !f.spawned.get());
    }
    #[test]
    fn restart_kill_failure_and_slow_exit() {
        let mut f = Fake::new(vec![Probe::Up(10)]);
        f.kill_ok = false;
        assert!(restart_with(&f).is_err());
        assert!(!f.spawned.get());
        let f = Fake::new(vec![Probe::Up(10)]);
        f.alive_for.set(u32::MAX);
        assert!(restart_with(&f).unwrap_err().contains("時限"));
        assert!(!f.spawned.get());
        let f = Fake::new(vec![Probe::Up(10), Probe::Up(11)]);
        f.alive_for.set(3);
        assert_eq!(restart_with(&f), Ok(Restart::Done));
    }
    #[test]
    fn restart_spawn_failure_timeout_and_same_pid() {
        let mut f = Fake::new(vec![Probe::Up(10)]);
        f.spawn_ok = false;
        assert!(restart_with(&f).is_err());
        // never comes up: not claimed as success
        let f = Fake::new(vec![Probe::Up(10), Probe::NotFound]);
        assert_eq!(restart_with(&f), Ok(Restart::Unconfirmed));
        assert!(f.slept.get() >= READY_WAIT_MS);
        // the old engine answering again is not a restart
        let f = Fake::new(vec![Probe::Up(10), Probe::Up(10)]);
        assert_eq!(restart_with(&f), Ok(Restart::Unconfirmed));
    }
    #[test]
    fn override_modes_block_restart() {
        // the env is process-global: this is the only test touching it
        unsafe {
            std::env::remove_var("TWIME_PIPE");
            std::env::remove_var("TWIME_USER_DIR");
        }
        assert!(restart_blocked().is_none());
        unsafe { std::env::set_var("TWIME_USER_DIR", "x") };
        assert!(restart_blocked().is_some());
        unsafe {
            std::env::remove_var("TWIME_USER_DIR");
            std::env::set_var("TWIME_PIPE", "p");
        }
        assert!(restart_blocked().is_some());
        unsafe { std::env::remove_var("TWIME_PIPE") };
    }
}
