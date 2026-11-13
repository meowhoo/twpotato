//! twime-shell against the real engine (Linux, macOS). Every test has its own directory, socket and engine process.
//!
//!     TWIME_TEST_ENGINE=<twime-engined> TWIME_TEST_ASSETS=<assets dir> cargo test -p twime-shell --test engine -- --ignored
//!
//! The time limit of a call is the product's 30 ms everywhere.
#![cfg(unix)]

use std::{
    ffi::OsString,
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    time::{Duration, Instant},
};

use twime_proto::{Event, Key, Mode, Mods, Reply as Wire, Request};
use twime_shell::{CALL_TIMEOUT_MS, Config, Context, Input, KeyCode, Shell};
use twime_winpipe::Client;

struct Stage {
    dir: PathBuf,
    socket: String,
    cfg: Config,
}

fn env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("set {name}")))
}

/// A private directory with HOME and the XDG directories for one engine, and the shell configuration that starts it.
fn stage(name: &str, more_args: &[&str]) -> Stage {
    let dir = std::env::temp_dir().join(format!("twsh-e-{}-{name}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&dir).expect("the stage directory must not exist yet");
    let socket = dir.join("e.sock").to_str().unwrap().to_owned();
    let mut engine_env = vec![];
    for (var, sub) in [("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_STATE_HOME", "state"), ("XDG_DATA_HOME", "data"), ("XDG_RUNTIME_DIR", "run")] {
        fs::DirBuilder::new().mode(0o700).create(dir.join(sub)).unwrap();
        engine_env.push((OsString::from(var), dir.join(sub).into_os_string()));
    }
    let mut engine_args: Vec<OsString> = vec!["--pipe".into(), socket.clone().into()];
    engine_args.extend(more_args.iter().map(OsString::from));
    let cfg = Config { socket: socket.clone(), install: Some((env("TWIME_TEST_ENGINE"), env("TWIME_TEST_ASSETS"))), engine_args, engine_env, ..Config::default() };
    Stage { dir, socket, cfg }
}

impl Stage {
    /// Process ids of the engines of this stage, zombies not counted (found by the socket path on the command line:
    /// also one that is still loading).
    fn engines(&self) -> Vec<i32> {
        #[cfg(not(target_os = "linux"))]
        return ps().into_iter().filter(|p| !p.state.starts_with('Z') && p.command.split(' ').any(|a| a == self.socket)).map(|p| p.pid).collect();
        #[cfg(target_os = "linux")]
        {
            let mut pids = vec![];
            for e in fs::read_dir("/proc").unwrap().flatten() {
                let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
                let cmdline = fs::read(e.path().join("cmdline")).unwrap_or_default();
                if cmdline.split(|b| *b == 0).any(|a| a == self.socket.as_bytes()) && !zombie(pid) {
                    pids.push(pid);
                }
            }
            pids
        }
    }

    /// The process id of the one engine of this stage that the shell is connected to. Linux: what the socket says.
    /// Elsewhere the socket cannot tell (0): the one process with this stage's socket on its command line. Never 0 or
    /// less: a signal sent to such an id would reach this whole process group.
    fn engine_pid(&self, sh: &Shell) -> i32 {
        let pid = if cfg!(target_os = "linux") {
            sh.engine_pid().unwrap() as i32
        } else {
            assert!(sh.engine_pid().is_some(), "no connection");
            let pids = self.engines();
            assert_eq!(pids.len(), 1, "engines of this stage: {pids:?}");
            pids[0]
        };
        assert!(pid > 0, "{pid}");
        pid
    }

    fn kill_engines(&self) {
        let until = Instant::now() + Duration::from_secs(10);
        let mut killed: Vec<i32> = vec![];
        loop {
            let pids = self.engines();
            // A dying process loses its command line before its sockets are closed: only gone (or a zombie) is ended.
            if pids.is_empty() && killed.iter().all(|p| ended(*p)) {
                return;
            }
            for p in pids {
                unsafe { libc::kill(p, libc::SIGKILL) };
                killed.push(p);
            }
            assert!(Instant::now() < until, "engine does not end");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// What the engine itself counts: (open sessions, sessions composing).
    fn status(&self) -> (u16, u16) {
        let c = Client::connect(&self.socket, None, 0).unwrap();
        let raw = c.call(&Request { session: 0, seq: 0, ack: None, event: Event::Status }.encode(), 2000).unwrap();
        match Wire::decode(&raw) {
            Some(Wire::Status { sessions, composing, .. }) => (sessions, composing),
            o => panic!("{o:?}"),
        }
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        self.kill_engines();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Gone, or a zombie none of whose other threads is left: a process ended by a signal can show its main thread as a
/// zombie while other threads are still on their way out, and its sockets are closed only when the last one is.
#[cfg(target_os = "linux")]
fn ended(pid: i32) -> bool {
    if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
        return true;
    }
    zombie(pid) && fs::read_dir(format!("/proc/{pid}/task")).map(|d| d.flatten().filter(|e| e.file_name().to_str() != Some(&pid.to_string())).count() == 0).unwrap_or(true)
}

/// Not Linux: gone, or a zombie, as `ps` shows it.
#[cfg(not(target_os = "linux"))]
fn ended(pid: i32) -> bool {
    ps().into_iter().find(|p| p.pid == pid).is_none_or(|p| p.state.starts_with('Z'))
}

/// One line of `ps` (not Linux, where /proc is read instead).
#[cfg(not(target_os = "linux"))]
struct Proc {
    pid: i32,
    parent: i32,
    /// First letter: Z zombie, T stopped.
    state: String,
    command: String,
}

#[cfg(not(target_os = "linux"))]
fn ps() -> Vec<Proc> {
    // ww: the command line is never cut short.
    let out = std::process::Command::new("ps").args(["-axww", "-o", "pid=,ppid=,stat=,command="]).output().expect("ps");
    assert!(out.status.success(), "ps failed");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (pid, parent, state) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?.to_owned());
            Some(Proc { pid, parent, state, command: f.collect::<Vec<_>>().join(" ") })
        })
        .collect()
}

/// True once the process is seen stopped (after SIGSTOP; the signal takes effect a moment later).
fn stopped(pid: i32) -> bool {
    #[cfg(target_os = "linux")]
    return fs::read_dir(format!("/proc/{pid}/task")).unwrap().flatten().all(|t| {
        fs::read_to_string(t.path().join("stat")).ok().and_then(|s| s.rsplit_once(") ").map(|(_, r)| r.starts_with('T'))).unwrap_or(false)
    });
    #[cfg(not(target_os = "linux"))]
    return ps().into_iter().any(|p| p.pid == pid && p.state.starts_with('T'));
}

/// No engine that this process started and that has ended is left unwaited-for.
fn assert_no_zombie_engine_child() {
    #[cfg(target_os = "linux")]
    {
        let me = std::process::id().to_string();
        for e in fs::read_dir("/proc").unwrap().flatten() {
            let Ok(stat) = fs::read_to_string(e.path().join("stat")) else { continue };
            let Some((head, rest)) = stat.rsplit_once(") ") else { continue };
            let mut f = rest.split(' ');
            let (state, parent) = (f.next().unwrap(), f.next().unwrap());
            assert!(!(state == "Z" && parent == me && head.ends_with("(twime-engined")), "zombie: {stat}");
        }
    }
    #[cfg(not(target_os = "linux"))]
    for p in ps() {
        assert!(!(p.state.starts_with('Z') && p.parent == std::process::id() as i32 && p.command.contains("twime-engined")), "zombie: {} {}", p.pid, p.command);
    }
}

#[cfg(target_os = "linux")]
fn zombie(pid: i32) -> bool {
    // "pid (comm) S ...": the state follows the last ')'.
    fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| s.rsplit_once(") ").map(|(_, r)| r.starts_with('Z'))).unwrap_or(false)
}

fn down(k: Key) -> Input {
    Input::KeyDown(KeyCode::Key(k), 0)
}

/// Standard keyboard: one key per bopomofo symbol or tone.
fn type_keys(c: &mut Context, keys: &str) {
    for k in keys.bytes() {
        let key = if k.is_ascii_digit() { Key::Digit(k - b'0') } else { Key::Letter(k) };
        let r = c.event(down(key));
        assert!(r.handled && !r.no_engine, "key {:?}", k as char);
        assert!(r.commit.is_empty());
    }
}

/// Presses a key the engine never gets until the shell has its connection; how long that took.
fn wait_engine(c: &mut Context) -> Duration {
    let t = Instant::now();
    while c.event(down(Key::Other)).no_engine {
        assert!(t.elapsed() < Duration::from_secs(20), "no engine");
        std::thread::sleep(Duration::from_millis(20));
    }
    t.elapsed()
}

fn started(st: &Stage) -> (Shell, Context) {
    let sh = Shell::new(st.cfg.clone());
    let mut c = sh.context();
    c.event(Input::FocusIn);
    wait_engine(&mut c);
    (sh, c)
}

fn text(c: &Context) -> String {
    c.reply().ui.comp.text()
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn types_a_sentence_and_commits_it() {
    let st = stage("sentence", &[]);
    let (_sh, mut c) = started(&st);
    type_keys(&mut c, "su3cl3");
    let r = c.reply();
    assert_eq!((r.ui.comp.text().as_str(), r.ui.comp.cursor), ("你好", 2));
    assert!(!r.ui.candidates_open);
    let r = c.event(down(Key::Enter));
    assert!(r.handled && r.ui_changed);
    assert_eq!(r.commit, "你好");
    assert!(r.ui.comp.segments.is_empty() && !r.ui.candidates_open);
    // Nothing composing: Enter belongs to the application.
    let r = c.event(down(Key::Enter));
    assert!(!r.handled && r.commit.is_empty() && !r.ui_changed);
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn candidates_open_and_select_by_digit_and_by_pick() {
    let st = stage("cands", &[]);
    let (_sh, mut c) = started(&st);
    type_keys(&mut c, "su3");
    let r = c.event(down(Key::Down));
    assert!(r.handled && r.ui_changed && r.ui.candidates_open);
    let page = r.ui.candidates.clone();
    assert!(page.items.len() >= 3 && page.items.len() <= usize::from(page.per_page) && usize::from(page.total) >= page.items.len(), "{page:?}");
    assert_eq!(page.items[usize::from(page.selected)], "你");
    // The digit keys count from 1.
    let r = c.event(down(Key::Digit(2)));
    assert!(r.handled && r.ui_changed && !r.ui.candidates_open && r.commit.is_empty());
    assert_eq!(r.ui.comp.text(), page.items[1]);
    // The same by a click on the third one.
    let r = c.event(down(Key::Down));
    assert!(r.ui.candidates_open);
    let page = r.ui.candidates.clone();
    let r = c.event(Input::Pick(2));
    assert!(r.ui_changed && !r.ui.candidates_open && r.commit.is_empty());
    assert_eq!(r.ui.comp.text(), page.items[2]);
    let r = c.event(down(Key::Enter));
    assert_eq!(r.commit, page.items[2]);
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn shift_tap_switches_the_mode() {
    let st = stage("shift", &[]);
    let (_sh, mut c) = started(&st);
    assert_eq!(c.reply().mode, Mode::Chinese);
    assert!(!c.event(Input::KeyDown(KeyCode::ShiftLeft, 0)).handled);
    let r = c.event(Input::KeyUp(KeyCode::ShiftLeft, Mods::SHIFT));
    assert!(!r.handled && r.mode_changed && r.mode == Mode::English);
    for k in *b"su3" {
        let r = c.event(down(if k.is_ascii_digit() { Key::Digit(k - b'0') } else { Key::Letter(k) }));
        assert!(!r.handled && !r.no_engine && !r.ui_changed);
    }
    // Held too long, and Shift used with a letter: no switch.
    c.event(Input::KeyDown(KeyCode::ShiftLeft, 0));
    std::thread::sleep(Duration::from_millis(600));
    assert!(!c.event(Input::KeyUp(KeyCode::ShiftLeft, Mods::SHIFT)).mode_changed);
    c.event(Input::KeyDown(KeyCode::ShiftLeft, 0));
    c.event(Input::KeyDown(KeyCode::Key(Key::Letter(b'a')), Mods::SHIFT));
    assert!(!c.event(Input::KeyUp(KeyCode::ShiftLeft, Mods::SHIFT)).mode_changed);
    assert!(!c.event(Input::KeyDown(KeyCode::ShiftRight, 0)).handled);
    let r = c.event(Input::KeyUp(KeyCode::ShiftRight, Mods::SHIFT));
    assert!(!r.handled && r.mode_changed && r.mode == Mode::Chinese);
    type_keys(&mut c, "su3");
    assert_eq!(text(&c), "你");
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn sensitive_field_passes_keys_also_across_an_engine_restart() {
    let st = stage("password", &[]);
    let (sh, mut c) = started(&st);
    type_keys(&mut c, "su3");
    assert_eq!(st.status(), (1, 1));
    // Ordinary -> sensitive without a focus change: the composition ends there and then.
    let r = c.event(Input::Sensitive(true));
    assert!(r.ui_changed && r.ui.comp.segments.is_empty());
    assert_eq!(st.status(), (1, 0));
    let pass = |c: &mut Context| {
        for k in *b"su3" {
            let r = c.event(down(if k.is_ascii_digit() { Key::Digit(k - b'0') } else { Key::Letter(k) }));
            assert!(!r.handled && !r.ui_changed && r.commit.is_empty());
        }
    };
    pass(&mut c);
    // The engine ends while the password field keeps the focus; the shell starts another one.
    let first = st.engine_pid(&sh);
    st.kill_engines();
    let t = Instant::now();
    pass(&mut c); // this key finds the connection dead
    while sh.engine_pid().is_none() {
        pass(&mut c);
        assert!(t.elapsed() < Duration::from_secs(10), "the engine did not come back");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_ne!(st.engine_pid(&sh), first);
    pass(&mut c);
    assert_eq!(st.status(), (1, 0), "a new session exists and nothing was composed in it");
    // Back to an ordinary field: the same session takes keys again.
    c.event(Input::Sensitive(false));
    type_keys(&mut c, "su3");
    assert_eq!(text(&c), "你");
    assert_eq!(st.status(), (1, 1));
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn focus_starts_the_engine_and_keys_typed_meanwhile_reach_it_in_order() {
    let st = stage("cold", &[]);
    let sh = Shell::new(st.cfg.clone());
    let mut c = sh.context();
    c.event(Input::Client { defer: true, focus_commit: true });
    let t = Instant::now();
    c.event(Input::FocusIn);
    assert!(t.elapsed() < Duration::from_millis(u64::from(CALL_TIMEOUT_MS)), "starting the engine must not hold the focus: {:?}", t.elapsed());
    assert_eq!(sh.spawn_attempts(), 1, "started at the focus");
    // Typed at once, long before the engine is loaded: every key is held, none is lost.
    for k in "su3cl3".bytes() {
        let r = c.event(down(Key::from_code(k).unwrap()));
        assert!(r.held && r.handled && r.holding);
    }
    let r = c.event(down(Key::Enter));
    assert!(r.held);
    let mut commit = String::new();
    let mut released = 0;
    while c.reply().holding {
        assert!(t.elapsed() < Duration::from_secs(3), "still holding after {:?}", t.elapsed());
        std::thread::sleep(Duration::from_millis(10));
        loop {
            let r = c.event(Input::Release);
            assert_eq!(r.release_before, 0, "the engine came: nothing goes back unchanged");
            if !r.released {
                break;
            }
            assert!(r.handled);
            commit.push_str(&r.commit);
            released += 1;
        }
    }
    eprintln!("cold start: the held keys reached the engine after {:?}", t.elapsed());
    assert_eq!((released, commit.as_str()), (7, "你好"));
    assert_eq!(sh.spawn_attempts(), 1);
    assert_eq!(st.engines().len(), 1);
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn without_defer_the_first_key_passes_while_the_engine_starts() {
    let st = stage("coldpass", &[]);
    let sh = Shell::new(st.cfg.clone());
    let mut c = sh.context();
    c.event(Input::FocusIn);
    let t = Instant::now();
    let r = c.event(down(Key::Letter(b's')));
    assert!(!r.handled && r.no_engine && !r.ui_changed && !r.held);
    assert!(t.elapsed() < Duration::from_millis(u64::from(CALL_TIMEOUT_MS)), "{:?}", t.elapsed());
    let took = wait_engine(&mut c);
    assert!(took < Duration::from_secs(3), "{took:?}");
    type_keys(&mut c, "su3cl3");
    assert_eq!(text(&c), "你好");
    assert_eq!(sh.spawn_attempts(), 1);
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn engine_killed_while_composing() {
    let st = stage("kill", &[]);
    let (sh, mut c) = started(&st);
    type_keys(&mut c, "su3");
    assert_eq!(text(&c), "你");
    st.kill_engines();
    let t = Instant::now();
    let r = c.event(down(Key::Letter(b'c')));
    assert!(!r.handled && r.no_engine, "the key passes");
    assert!(r.ui_changed && r.ui.comp.segments.is_empty() && !r.ui.candidates_open, "the composition on screen is cleared");
    assert!(r.commit.is_empty());
    let took = wait_engine(&mut c);
    assert!(t.elapsed() < Duration::from_secs(3), "{took:?}");
    eprintln!("after a kill: input possible after {took:?}");
    // The lost composition is gone for good: nothing of it comes back or is committed twice.
    type_keys(&mut c, "cl3");
    assert_eq!(text(&c), "好");
    assert_eq!(c.event(down(Key::Enter)).commit, "好");
    assert_eq!(sh.spawn_attempts(), 2);
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn missing_assets_start_the_engine_at_most_every_three_seconds() {
    let mut st = stage("noassets", &[]);
    st.cfg.install.as_mut().unwrap().1 = st.dir.join("no-such-assets");
    let sh = Shell::new(st.cfg.clone());
    let mut c = sh.context();
    c.event(Input::FocusIn);
    let t = Instant::now();
    let mut attempts = vec![];
    while t.elapsed() < Duration::from_millis(7500) {
        let r = c.event(down(Key::Letter(b's')));
        assert!(!r.handled && r.no_engine && !r.ui_changed);
        if sh.spawn_attempts() as usize > attempts.len() {
            attempts.push(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // At 0, 3 and 6 seconds.
    assert_eq!(attempts.len(), 3, "{attempts:?}");
    assert!(attempts.windows(2).all(|w| w[1] - w[0] >= Duration::from_secs(3)), "{attempts:?}");
    // The engines that gave up were waited for: none is left as a zombie child of this process.
    std::thread::sleep(Duration::from_millis(500));
    assert_no_zombie_engine_child();
    assert!(st.engines().is_empty());
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn slow_engine_passes_the_key_in_time() {
    // The engine's fault injection: a request of session 0xFFFF_xxxx is answered xxxx ms late. The shell's first session
    // gets such an id (400 ms); its later sessions are ordinary.
    let mut st = stage("slow", &["--test-faults"]);
    st.cfg.first_session = 0xFFFF_0000 | 400;
    let limit = Duration::from_millis(u64::from(CALL_TIMEOUT_MS));
    // Wait for the engine with a shell of its own: the one under test must meet the delay with its very first request.
    let other = Shell::new(Config { first_session: 7, ..st.cfg.clone() });
    wait_engine(&mut other.context());
    let sh = Shell::new(st.cfg.clone());
    let mut c = sh.context();
    c.event(Input::FocusIn);
    let t = Instant::now();
    let r = c.event(down(Key::Letter(b's')));
    let took = t.elapsed();
    assert!(!r.handled && r.no_engine && r.commit.is_empty());
    assert!(took >= limit && took < limit + Duration::from_millis(20), "{took:?}");
    eprintln!("delayed reply: the key returned after {took:?}");
    // The next key: new connection, new session, works.
    type_keys(&mut c, "su3");
    assert_eq!(text(&c), "你");
    // The same with an engine that is stopped in the middle of a composition.
    let pid = st.engine_pid(&sh);
    unsafe { libc::kill(pid, libc::SIGSTOP) };
    // The signal takes effect a moment later: wait until every thread of the engine is seen stopped.
    while !stopped(pid) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let t = Instant::now();
    let r = c.event(down(Key::Letter(b'c')));
    let took = t.elapsed();
    unsafe { libc::kill(pid, libc::SIGCONT) };
    assert!(!r.handled && r.no_engine && r.ui_changed && r.ui.comp.segments.is_empty() && r.commit.is_empty());
    assert!(took >= limit && took < limit + Duration::from_millis(20), "{took:?}");
    eprintln!("stopped engine: the key returned after {took:?}");
    type_keys(&mut c, "cl3");
    assert_eq!(text(&c), "好");
    assert_eq!(c.event(down(Key::Enter)).commit, "好");
}

#[test]
#[ignore = "needs TWIME_TEST_ENGINE and TWIME_TEST_ASSETS"]
fn two_contexts_do_not_mix_and_close_their_sessions() {
    let st = stage("two", &[]);
    let (sh, mut a) = started(&st);
    let mut b = sh.context();
    type_keys(&mut a, "su3");
    a.event(Input::FocusOut);
    b.event(Input::FocusIn);
    type_keys(&mut b, "cl3");
    assert_eq!(st.status().0, 2);
    assert_eq!(text(&b), "好");
    assert_eq!(b.event(down(Key::Enter)).commit, "好");
    b.event(Input::FocusOut);
    a.event(Input::FocusIn);
    type_keys(&mut a, "cl3");
    let r = a.event(down(Key::Enter));
    eprintln!("first context after coming back: commit {:?}", r.commit);
    assert!(r.commit.ends_with('好') && !r.commit.contains("好好"));
    drop(a);
    assert_eq!(st.status().0, 1);
    drop(b);
    assert_eq!(st.status().0, 0);
}
