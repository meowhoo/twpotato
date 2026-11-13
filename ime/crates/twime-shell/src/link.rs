//! The connection to the engine: connect, negotiate the protocol version, start the engine when its socket is absent,
//! call with a deadline. Nothing here waits for the engine to come up: a key that finds no engine passes through.

use std::{
    ffi::OsString,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    time::{Duration, Instant},
};

use twime_proto::{Event, Reply as Wire, Request};
use twime_winpipe::{CallError, Client};

/// Time limit of one event (all requests it needs together), as in the Windows shell.
pub const CALL_TIMEOUT_MS: u32 = 30;
/// Least time between two attempts to start the engine while no engine has answered in between (a start that fails,
/// such as missing assets). Once a connection was made the next start may follow at once: an engine that is killed is
/// replaced without this delay.
pub const SPAWN_EVERY: Duration = Duration::from_secs(3);
/// The protocol version the Linux front ends speak (LINUX-plan 4.4). Any other answer: the engine is not used.
pub const VERSION: u16 = 5;

/// The defaults are the product. The other values exist for the tests of this crate; the C interface cannot set them.
#[derive(Clone, Debug)]
pub struct Config {
    /// Socket name as `twime_winpipe::qualify` takes it: `engine` is the user's engine; a path with `/` is used as it is.
    pub socket: String,
    /// (engine executable, assets directory). `None`: the install root, looked up at every start.
    pub install: Option<(PathBuf, PathBuf)>,
    /// More arguments for the engine.
    pub engine_args: Vec<OsString>,
    /// Environment variables set for the engine.
    pub engine_env: Vec<(OsString, OsString)>,
    pub timeout_ms: u32,
    /// Id of the first session.
    pub first_session: u32,
}

impl Default for Config {
    fn default() -> Config {
        Config { socket: "engine".into(), install: None, engine_args: vec![], engine_env: vec![], timeout_ms: CALL_TIMEOUT_MS, first_session: 1 }
    }
}

pub(crate) struct Link {
    cfg: Config,
    client: Option<Client>,
    /// Counts the connections made; a session belongs to one of them.
    generation: u32,
    /// Last attempt to start the engine.
    last_spawn: Option<Instant>,
    /// The engine process we started has not ended yet.
    child_running: Arc<AtomicBool>,
    /// The engine answered `Hello` with another version: not asked again before this.
    refused_until: Option<Instant>,
    spawn_attempts: u32,
    last_log: &'static str,
}

impl Link {
    pub fn new(cfg: Config) -> Link {
        Link { cfg, client: None, generation: 0, last_spawn: None, child_running: Arc::default(), refused_until: None, spawn_attempts: 0, last_log: "" }
    }

    /// A line on standard error when the state changes (never per key: the same line is not repeated).
    pub fn note(&mut self, what: &'static str) {
        self.log(what);
    }

    fn log(&mut self, what: &'static str) {
        if self.last_log != what {
            self.last_log = what;
            let _ = writeln!(std::io::stderr(), "twime-shell: {what}");
        }
    }

    pub fn deadline(&self) -> Instant {
        Instant::now() + Duration::from_millis(u64::from(self.cfg.timeout_ms))
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn spawn_attempts(&self) -> u32 {
        self.spawn_attempts
    }

    /// The thread of the last start attempt has not finished: the engine it started runs, or it is still trying.
    pub fn start_pending(&self) -> bool {
        self.child_running.load(Ordering::SeqCst)
    }

    pub fn server_pid(&self) -> Option<u32> {
        self.client.as_ref().map(|c| c.server_pid)
    }

    /// True while there is a connection whose other end has not gone away.
    pub fn up(&mut self) -> bool {
        if self.client.as_ref().is_some_and(|c| !c.alive()) {
            self.log("engine connection lost");
            self.client = None;
        }
        self.client.is_some()
    }

    /// For a key: connects when there is no connection (a missing socket fails at once) and says `Hello`; when the
    /// socket is absent the engine is started and this returns false without waiting for it.
    pub fn ensure(&mut self, deadline: Instant) -> bool {
        if self.up() {
            return true;
        }
        if self.refused_until.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        // Never the waiting connect: a stopped engine's queue of unaccepted connections fills up with the ones we gave up on.
        match Client::connect_nowait(&self.cfg.socket) {
            Ok(c) => {
                self.client = Some(c);
                self.generation = self.generation.wrapping_add(1).max(1);
                let hello = Request { session: 0, seq: 0, ack: None, event: Event::Hello(VERSION) };
                match self.call_bytes(&hello.encode(), deadline) {
                    Some(Wire::Hello { version: VERSION }) | Some(Wire::HelloCaps { version: VERSION, .. }) => {
                        // An engine is serving: the last start, if any, did not fail, so the next one need not wait.
                        self.last_spawn = None;
                        self.log("connected to the engine");
                        true
                    }
                    Some(_) => {
                        self.client = None;
                        self.refused_until = Some(Instant::now() + SPAWN_EVERY);
                        self.log("the engine does not speak protocol version 5: not used, keys pass through");
                        false
                    }
                    None => false,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.start_engine();
                false
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                self.log("the engine does not accept connections now");
                false
            }
            Err(_) => {
                self.log("cannot connect to the engine socket");
                false
            }
        }
    }

    /// Starts the engine and returns at once. At most one attempt every `SPAWN_EVERY` unless an engine answered since
    /// the last one, and none while the engine we started is still running (it is loading: its socket appears when it is ready).
    fn start_engine(&mut self) {
        if self.last_spawn.is_some_and(|t| t.elapsed() < SPAWN_EVERY) || self.child_running.load(Ordering::SeqCst) {
            return;
        }
        self.last_spawn = Some(Instant::now());
        #[cfg(not(target_os = "macos"))]
        let found = self.cfg.install.clone().or_else(|| {
            crate::paths::install_root(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"), |p| p.is_file())
        });
        // macOS: the engine and the assets are inside the application bundle of this process.
        #[cfg(target_os = "macos")]
        let found = self.cfg.install.clone().or_else(|| {
            let exe = std::env::current_exe().and_then(std::fs::canonicalize).ok()?;
            crate::paths::bundle_root(&exe, |p| p.is_file())
        });
        let Some((engine, assets)) = found else {
            self.log("no engine installed");
            return;
        };
        self.spawn_attempts += 1;
        let mut cmd = Command::new(&engine);
        cmd.arg("--assets-dir").arg(&assets).args(["--candidates", "--user-data"]).args(&self.cfg.engine_args);
        cmd.envs(self.cfg.engine_env.iter().map(|(k, v)| (k, v)));
        cmd.current_dir("/").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // The host process (fcitx5, the IBus shell) may hold descriptors that are not close-on-exec (display and bus
        // connections); the engine must not keep them open after the host is gone.
        #[cfg(target_os = "linux")]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::syscall(libc::SYS_close_range, 3 as libc::c_uint, libc::c_uint::MAX, libc::CLOSE_RANGE_CLOEXEC as libc::c_uint);
                Ok(())
            });
        }
        // The thread is made first and starts the engine itself, so that whoever started it is there to wait for it
        // (never a zombie, whenever it ends). No thread, no engine.
        let running = self.child_running.clone();
        running.store(true, Ordering::SeqCst);
        let waiter = std::thread::Builder::new().name("twime-engine-wait".into()).spawn(move || {
            match cmd.spawn() {
                Ok(mut child) => drop(child.wait()),
                Err(e) => drop(writeln!(std::io::stderr(), "twime-shell: cannot start the engine: {e}")),
            }
            running.store(false, Ordering::SeqCst);
        });
        match waiter {
            Ok(_) => self.log("starting the engine"),
            Err(_) => {
                self.child_running.store(false, Ordering::SeqCst);
                self.log("cannot start the engine: no thread");
            }
        }
    }

    pub fn drop_connection(&mut self) {
        self.client = None;
    }

    /// One request and its reply, finished before `deadline`. On any failure the connection is dropped.
    pub fn call(&mut self, req: &Request, deadline: Instant) -> Option<Wire> {
        self.call_bytes(&req.encode_for(VERSION), deadline)
    }

    fn call_bytes(&mut self, bytes: &[u8], deadline: Instant) -> Option<Wire> {
        let c = self.client.as_ref()?;
        let left = deadline.saturating_duration_since(Instant::now());
        let answer = if left.is_zero() { Err(CallError::Timeout) } else { c.call(bytes, (left.as_micros().div_ceil(1000) as u32).max(1)) };
        let why = match answer {
            Ok(b) => match Wire::decode(&b) {
                Some(r) => return Some(r),
                None => "the engine sent an unreadable reply",
            },
            Err(CallError::Timeout) => "engine call timed out",
            Err(CallError::Broken(_)) => "engine connection broken",
        };
        self.log(why);
        self.client = None;
        None
    }
}
