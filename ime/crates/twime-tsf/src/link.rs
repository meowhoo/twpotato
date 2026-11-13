//! Connection to the engine process: connect, start the engine when absent, call with a hard timeout.
//!
//! The pipe name comes from the user and session (never from the environment). A dev-mode file in the install root
//! (`dev-mode`, writable only by whoever installed) allows overriding pipe name and skipping the image path check
//! for tests.

use std::{
    ffi::OsString,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use twime_proto::{Reply, Request, ids};
use twime_winpipe::{CallError, Client};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HMODULE, MAX_PATH},
        System::{
            LibraryLoader::GetModuleFileNameW,
            Threading::{CREATE_NO_WINDOW, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW},
        },
    },
    core::{PCWSTR, PWSTR},
};

use crate::trace;

/// Key-path time limit of one request.
pub const CALL_TIMEOUT_MS: u32 = 30;
const RECONNECT_EVERY: Duration = Duration::from_millis(500);
const SPAWN_EVERY: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostClass { Normal, Elevated, Service, Low }

fn classify_host(rid: u32, elevated: bool, sid: &str, session: u32, query_failed: bool) -> HostClass {
    if query_failed || session == 0 || matches!(sid, "S-1-5-18" | "S-1-5-19" | "S-1-5-20") {
        HostClass::Service
    } else if rid >= 0x3000 || elevated {
        HostClass::Elevated
    } else if rid < 0x2000 {
        HostClass::Low
    } else {
        HostClass::Normal
    }
}

pub struct Link {
    host: HostClass,
    client: Option<Client>,
    last_try: Option<Instant>,
    last_spawn: Option<Instant>,
    root: PathBuf,
    pipe: String,
    engine_exe: PathBuf,
    dev: bool,
    pub spawned: u32,
    /// Wire version of the current connection: 1 until the engine confirmed 2 with `Hello`.
    version: u16,
    caps: u8,
    /// A new connection has not said `Hello` yet; it is sent by the first `call` (never from `ensure`, which the
    /// key-test callback runs).
    needs_hello: bool,
    /// Time limit of one request (tests raise it on a loaded machine).
    timeout_ms: u32,
    /// Counts the connections made so far; a context is synchronised for one generation only (UX1 "ready").
    generation: u32,
    /// Optional user settings (`ux.cfg` in the install root).
    settings: Settings,
}

/// The few UX settings (no settings window yet: a small text file `ux.cfg` next to the DLL).
/// Lines are `key=value`; unknown lines are ignored. `shift_tap=off`, `default_mode=english`, `punct=half`,
/// `symbol_table=off`.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub shift_tap: bool,
    pub default_english: bool,
    pub punct_half: bool,
    pub symbol_table: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { shift_tap: true, default_english: false, punct_half: false, symbol_table: true }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
            match k.as_str() {
                "shift_tap" => s.shift_tap = !matches!(v.as_str(), "off" | "0" | "false"),
                "default_mode" => s.default_english = v == "english",
                "punct" => s.punct_half = v == "half",
                "symbol_table" => s.symbol_table = !matches!(v.as_str(), "off" | "0" | "false"),
                _ => {}
            }
        }
        s
    }

    /// Engine command line flags for the settings that are the engine's business.
    pub fn engine_flags(&self) -> String {
        let mut f = String::new();
        if self.default_english {
            f.push_str(" --default-english");
        }
        if self.punct_half {
            f.push_str(" --punct-half");
        }
        if !self.symbol_table {
            f.push_str(" --no-symbol-table");
        }
        f
    }
}

pub(crate) fn dll_dir() -> PathBuf {
    let mut buf = [0u16; MAX_PATH as usize * 2];
    let n = unsafe { GetModuleFileNameW(Some(HMODULE(crate::MODULE.load(std::sync::atomic::Ordering::SeqCst) as _)), &mut buf) } as usize;
    let p = PathBuf::from(OsString::from_wide_lossy(&buf[..n]));
    p.parent().map(Path::to_path_buf).unwrap_or_default()
}

trait FromWide {
    fn from_wide_lossy(w: &[u16]) -> OsString;
}
impl FromWide for OsString {
    fn from_wide_lossy(w: &[u16]) -> OsString {
        std::os::windows::ffi::OsStringExt::from_wide(w)
    }
}

impl Link {
    pub fn new() -> Link {
        let root = dll_dir();
        let dev = root.join("dev-mode").exists();
        let (host, default_pipe) = match twime_winpipe::current_token_security() {
            Ok(t) => (classify_host(t.integrity_rid, t.elevated, &t.user, t.session_id, false),
                twime_winpipe::pipe_name(&twime_winpipe::Identity { user: t.user, logon: String::new() })),
            Err(_) => (classify_host(0, false, "", 0, true), String::new()),
        };
        let root_for_settings = root.clone();
        let pipe = if dev { std::env::var("TWIME_PIPE").ok().map(|p| twime_winpipe::qualify(&p)).unwrap_or(default_pipe) } else { default_pipe };
        Link {
            host,
            client: None,
            last_try: None,
            last_spawn: None,
            engine_exe: root.join(ids::ENGINE_NAME),
            root,
            pipe,
            dev,
            spawned: 0,
            version: 1,
            caps: 0,
            needs_hello: false,
            timeout_ms: CALL_TIMEOUT_MS,
            generation: 0,
            settings: std::fs::read_to_string(root_for_settings.join("ux.cfg")).map(|t| Settings::parse(&t)).unwrap_or_default(),
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Number of the current connection (0 = none yet). Changes whenever a connection is made.
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Wire version of the connection (1 when the engine is old or has not been asked yet).
    pub fn version(&self) -> u16 {
        self.version
    }

    pub fn doc_text_allowed(&self) -> bool {
        self.version == 4 || (self.version >= 5 && self.caps & twime_proto::CAP_DOC_TEXT != 0)
    }

    /// A disposable control connection: a timed-out refresh must not discard the input sessions.
    pub fn user_settings(&self) -> Result<twime_proto::UserReply, ()> {
        if self.host == HostClass::Service { return Err(()); }
        let expect = if self.dev { None } else { Some(self.engine_exe.to_string_lossy().into_owned()) };
        let c = Client::connect(&self.pipe, expect.as_deref(), 0).map_err(|_| ())?;
        let hello = Request { session: 0, seq: 0, ack: None, event: twime_proto::Event::Hello(twime_proto::PROTOCOL_VERSION) };
        let raw = c.call(&hello.encode(), self.timeout_ms).map_err(|_| ())?;
        if !matches!(Reply::decode(&raw), Some(Reply::HelloCaps { version: 5.., .. }) | Some(Reply::Hello { version: 5.. })) {
            return Err(());
        }
        let req = Request { session: 0, seq: 0, ack: None, event: twime_proto::Event::User(twime_proto::UserReq::GetSettings) };
        let raw = c.call(&req.encode_for(self.version), self.timeout_ms).map_err(|_| ())?;
        match Reply::decode(&raw) {
            Some(Reply::User(reply)) => Ok(reply),
            _ => Err(()),
        }
    }

    /// Dev mode (private test stage): test hooks are honoured.
    pub fn is_dev(&self) -> bool {
        self.dev
    }

    pub fn is_up(&self) -> bool {
        self.client.is_some()
    }

    pub fn marker_present(&self) -> bool {
        self.uninstalling()
    }

    fn uninstalling(&self) -> bool {
        self.root.join(ids::UNINSTALL_MARKER).exists()
    }

    /// Tries to connect (at most every 500 ms); starts the engine if the pipe does not exist (at most every 10 s,
    /// never while an uninstall is in progress). Never blocks beyond the OS calls.
    pub fn ensure(&mut self) -> bool {
        if self.host == HostClass::Service { return false; }
        if self.client.as_ref().is_some_and(|c| !c.alive()) {
            trace::log("link: server gone");
            self.client = None;
            self.last_try = None;
        }
        if self.client.is_some() {
            return true;
        }
        if self.last_try.is_some_and(|t| t.elapsed() < RECONNECT_EVERY) {
            return false;
        }
        self.last_try = Some(Instant::now());
        if self.uninstalling() {
            return false;
        }
        let expect = if self.dev { None } else { Some(self.engine_exe.to_string_lossy().into_owned()) };
        match Client::connect(&self.pipe, expect.as_deref(), 0) {
            Ok(c) => {
                trace::log("link: connected");
                trace::note_connected();
                self.client = Some(c);
                self.version = 1;
                self.caps = 0;
                self.needs_hello = true;
                self.generation = self.generation.wrapping_add(1).max(1);
                true
            }
            Err(e) => {
                if self.host == HostClass::Normal && e.kind() == std::io::ErrorKind::NotFound && self.last_spawn.is_none_or(|t| t.elapsed() >= SPAWN_EVERY) {
                    self.last_spawn = Some(Instant::now());
                    self.spawn_engine();
                }
                false
            }
        }
    }

    /// Starts the engine from the install root with the fixed path (never from PATH).
    pub fn spawn_engine(&mut self) {
        if self.host != HostClass::Normal { return; }
        if !self.engine_exe.exists() || self.uninstalling() {
            return;
        }
        let mut cmd = format!("\"{}\" --assets-dir \"{}\" --candidates", self.engine_exe.display(), self.root.join("assets").display());
        cmd.push_str(&self.settings.engine_flags());
        if self.dev
            && let Ok(extra) = std::env::var("TWIME_ENGINE_ARGS")
        {
            cmd.push(' ');
            cmd.push_str(&extra);
        }
        let mut w: Vec<u16> = std::ffi::OsStr::new(&cmd).encode_wide().chain(Some(0)).collect();
        let exe: Vec<u16> = self.engine_exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let dir: Vec<u16> = self.root.as_os_str().encode_wide().chain(Some(0)).collect();
        let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        let r = unsafe {
            CreateProcessW(PCWSTR(exe.as_ptr()), Some(PWSTR(w.as_mut_ptr())), None, None, false, CREATE_NO_WINDOW | DETACHED_PROCESS, None, PCWSTR(dir.as_ptr()), &si, &mut pi)
        };
        if r.is_ok() {
            self.spawned += 1;
            unsafe {
                let _ = CloseHandle(pi.hProcess);
                let _ = CloseHandle(pi.hThread);
            }
            trace::note_spawn();
            trace::log("link: engine spawned");
        } else {
            trace::log("link: engine spawn failed");
        }
    }

    pub fn drop_connection(&mut self) {
        self.client = None;
    }

    /// Version negotiation, before the first request of a new connection. An engine that does
    /// not know `Hello` closes the connection (or does not answer): then the connection is made again without `Hello`
    /// and treated as version 1. Not retried on that second connection.
    fn handshake(&mut self) -> bool {
        self.needs_hello = false;
        trace::log("handshake");
        let hello = Request { session: 0, seq: 0, ack: None, event: twime_proto::Event::Hello(twime_proto::PROTOCOL_VERSION) };
        let Some(c) = &self.client else { return false };
        let raw = c.call(&hello.encode(), self.timeout_ms);
        let answer = raw.ok().and_then(|b| Reply::decode(&b));
        match answer {
            Some(Reply::Hello { version }) => {
                self.version = version.clamp(1, twime_proto::PROTOCOL_VERSION);
                self.caps = 0;
                return true;
            }
            Some(Reply::HelloCaps { version, caps }) => {
                self.version = version.clamp(1, twime_proto::PROTOCOL_VERSION);
                self.caps = caps;
                return true;
            }
            _ => {}
        }
        trace::log("link: handshake failed, reconnecting as version 1");
        self.client = None;
        let expect = if self.dev { None } else { Some(self.engine_exe.to_string_lossy().into_owned()) };
        match Client::connect(&self.pipe, expect.as_deref(), 0) {
            Ok(c) => {
                self.client = Some(c);
                self.version = 1;
                self.caps = 0;
                self.generation = self.generation.wrapping_add(1).max(1);
                true
            }
            Err(_) => false,
        }
    }

    /// Version negotiation for a new connection, to run before its first session request. Called by the shell's
    /// `exchange` (the key-processing path) only: `ensure` (the key-test callback) never sends anything, and a direct
    /// `call` such as the final `Close` of an ending composition goes out as it is (version 1 bytes are valid on any
    /// engine). False if the connection is lost.
    pub fn negotiate(&mut self) -> bool {
        if self.needs_hello { self.handshake() } else { self.client.is_some() }
    }

    /// One request/reply with the hard time limit. On any failure the connection is dropped.
    pub fn call(&mut self, req: &Request) -> Result<Reply, ()> {
        let Some(c) = &self.client else { return Err(()) };
        match c.call(&req.encode_for(self.version), self.timeout_ms) {
            Ok(bytes) => match Reply::decode(&bytes) {
                Some(r) => Ok(r),
                None => {
                    self.client = None;
                    Err(())
                }
            },
            Err(e) => {
                trace::log(match e {
                    CallError::Timeout => "link: call timeout",
                    CallError::Broken(_) => "link: call broken",
                });
                self.client = None;
                Err(())
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use twime_proto::{Cands, Event, Update};
    use twime_winpipe::PipeServer;

    pub(crate) fn link_to(pipe: &str) -> Link {
        Link {
            host: HostClass::Normal,
            client: None,
            last_try: None,
            last_spawn: None,
            root: PathBuf::new(),
            pipe: twime_winpipe::qualify(pipe),
            engine_exe: PathBuf::new(),
            dev: true,
            spawned: 0,
            version: 1,
            caps: 0,
            needs_hello: false,
            timeout_ms: 3000,
            generation: 0,
            settings: Settings::default(),
        }
    }

    fn key_request() -> Request {
        Request { session: 1, seq: 1, ack: None, event: Event::Ping }
    }

    fn update_with_candidates() -> Reply {
        let c = Cands { items: vec!["權利".into()], selected: 0, page: 0, total: 12, per_page: 9 };
        Reply::Update { session: 1, seq: 1, update: Update { candidates: Some(c), candidates_open: true, ..Default::default() } }
    }

    /// An engine of version 1 closes the connection on an event it does not know (the original decoder fails).
    #[test]
    fn old_engine_that_drops_hello_is_used_as_version_1_on_a_second_connection() {
        let name = format!("twime-link-old-{}", std::process::id());
        let server = PipeServer::with_name(Some(name.clone())).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Option<Event>>::new()));
        let s2 = seen.clone();
        std::thread::spawn(move || {
            while let Ok(conn) = server.accept() {
                let s3 = s2.clone();
                std::thread::spawn(move || {
                    while let Ok(Some(p)) = conn.read() {
                        match Request::decode(&p) {
                            Some(r) if !matches!(r.event, Event::Hello(_)) => {
                                s3.lock().unwrap().push(Some(r.event));
                                if conn.write(&update_with_candidates().encode()).is_err() {
                                    break;
                                }
                            }
                            _ => {
                                s3.lock().unwrap().push(None);
                                conn.disconnect(); // what a version 1 engine does with an unknown event
                                break;
                            }
                        }
                    }
                });
            }
        });
        let mut l = link_to(&name);
        assert!(l.ensure());
        assert!(l.needs_hello);
        assert!(l.negotiate());
        let r = l.call(&key_request()).expect("served after the downgrade");
        assert_eq!(l.version(), 1);
        let Reply::Update { update, .. } = r else { panic!() };
        // The version 1 bytes carry no totals.
        assert_eq!(update.candidates.unwrap().total, 0);
        // The failed Hello, then only real requests: no second Hello on the second connection.
        let seen = seen.lock().unwrap();
        assert_eq!(&*seen, &[None, Some(Event::Ping)]);
    }

    #[test]
    fn engine_that_confirms_version_2_gets_totals() {
        let name = format!("twime-link-new-{}", std::process::id());
        let server = PipeServer::with_name(Some(name.clone())).unwrap();
        std::thread::spawn(move || {
            while let Ok(conn) = server.accept() {
                std::thread::spawn(move || {
                    let mut version = 1;
                    while let Ok(Some(p)) = conn.read() {
                        let Some(r) = Request::decode(&p) else { break };
                        let reply = match r.event {
                            Event::Hello(v) => {
                                version = v.min(2);
                                Reply::Hello { version }
                            }
                            _ => update_with_candidates(),
                        };
                        if conn.write(&reply.encode_for(version)).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let mut l = link_to(&name);
        assert!(l.ensure() && l.version() == 1, "not negotiated before the first call");
        assert!(l.negotiate());
        let Reply::Update { update, .. } = l.call(&key_request()).unwrap() else { panic!() };
        assert_eq!(l.version(), 2);
        assert_eq!((update.candidates.as_ref().unwrap().total, update.candidates.unwrap().per_page), (12, 9));
        // A broken connection starts over: the next connection negotiates again.
        l.drop_connection();
        l.last_try = None;
        assert!(l.ensure() && l.needs_hello && l.version() == 1);
    }

    #[test]
    fn version_3_is_negotiated_and_every_connection_has_its_own_generation() {
        let name = format!("twime-link-v3-{}", std::process::id());
        let server = PipeServer::with_name(Some(name.clone())).unwrap();
        std::thread::spawn(move || {
            while let Ok(conn) = server.accept() {
                std::thread::spawn(move || {
                    while let Ok(Some(p)) = conn.read() {
                        let Some(r) = Request::decode(&p) else { break };
                        let reply = match r.event {
                            Event::Hello(v) => Reply::Hello { version: v.min(3) },
                            _ => update_with_candidates(),
                        };
                        if conn.write(&reply.encode_for(3)).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let mut l = link_to(&name);
        assert_eq!(l.generation(), 0);
        assert!(l.ensure() && l.negotiate());
        assert_eq!((l.version(), l.generation()), (3, 1));
        let Reply::Update { update, .. } = l.call(&key_request()).unwrap() else { panic!() };
        assert_eq!(update.candidates.unwrap().total, 12);
        // A new connection is a new generation (every context has to synchronise again).
        l.drop_connection();
        l.last_try = None;
        assert!(l.ensure());
        assert_eq!(l.generation(), 2);
        // Staying connected keeps the generation.
        assert!(l.ensure());
        assert_eq!(l.generation(), 2);
    }

    #[test]
    fn hello_caps_and_legacy_doc_text_permissions() {
        for (tag, reply, version, allowed) in [
            ("caps-doc", Reply::HelloCaps { version: 5, caps: twime_proto::CAP_DOC_TEXT }, 5, true),
            ("caps-no-doc", Reply::HelloCaps { version: 5, caps: 0 }, 5, false),
            ("hello4", Reply::Hello { version: 4 }, 4, true),
            ("hello3", Reply::Hello { version: 3 }, 3, false),
        ] {
            let name = format!("twime-link-{tag}-{}", std::process::id());
            let server = PipeServer::with_name(Some(name.clone())).unwrap();
            let worker = std::thread::spawn(move || {
                let conn = server.accept().unwrap();
                let raw = conn.read().unwrap().unwrap();
                let req = Request::decode(&raw).unwrap();
                assert_eq!(req.event, Event::Hello(twime_proto::PROTOCOL_VERSION));
                assert_eq!((req.session, req.seq, req.ack), (0, 0, None));
                conn.write(&reply.encode_for(version)).unwrap();
                let raw = conn.read().unwrap().unwrap();
                assert_eq!(Request::decode(&raw).unwrap().event, Event::Ping);
                conn.write(&update_with_candidates().encode_for(version)).unwrap();
                let _ = conn.read(); // Keep the reply buffered until the client has read it and closed.
            });
            let mut l = link_to(&name);
            assert!(l.ensure() && l.negotiate());
            assert_eq!(l.version(), version);
            assert_eq!(l.doc_text_allowed(), allowed);
            assert_eq!(l.generation(), 1, "no version-1 reconnect");
            assert!(l.call(&key_request()).is_ok());
            l.drop_connection();
            worker.join().unwrap();
        }
    }

    #[test]
    fn settings_control_has_no_session_or_ack_and_preserves_the_input_connection() {
        use twime_proto::{LexError, UserReply, UserReq, UserSettings};
        for (tag, answer) in [
            ("settings", UserReply::Settings { settings: UserSettings { toggle_key: twime_proto::ToggleKey::RightShift, ..Default::default() }, file_error: false, last_error: None }),
            ("disabled", UserReply::Error(LexError::Disabled)),
        ] {
            let name = format!("twime-link-control-{tag}-{}", std::process::id());
            let server = PipeServer::with_name(Some(name.clone())).unwrap();
            let expected = answer.clone();
            let worker = std::thread::spawn(move || {
                let input = server.accept().unwrap();
                let raw = input.read().unwrap().unwrap();
                assert_eq!(Request::decode(&raw).unwrap().event, Event::Hello(twime_proto::PROTOCOL_VERSION));
                let hello = Reply::HelloCaps { version: 5, caps: twime_proto::CAP_DOC_TEXT };
                input.write(&hello.encode()).unwrap();
                let control = server.accept().unwrap();
                let raw = control.read().unwrap().unwrap();
                assert_eq!(Request::decode(&raw).unwrap().event, Event::Hello(twime_proto::PROTOCOL_VERSION));
                control.write(&hello.encode()).unwrap();
                let raw = control.read().unwrap().unwrap();
                let req = Request::decode(&raw).unwrap();
                assert_eq!((req.session, req.seq, req.ack), (0, 0, None));
                assert_eq!(req.event, Event::User(UserReq::GetSettings));
                control.write(&Reply::User(answer).encode_for(5)).unwrap();
                let raw = input.read().unwrap().unwrap();
                assert_eq!(Request::decode(&raw).unwrap().event, Event::Ping);
                input.write(&update_with_candidates().encode_for(5)).unwrap();
                let _ = input.read();
            });
            let mut l = link_to(&name);
            assert!(l.ensure() && l.negotiate());
            assert_eq!(l.user_settings(), Ok(expected));
            assert_eq!((l.version(), l.generation(), l.doc_text_allowed()), (5, 1, true));
            assert!(l.call(&key_request()).is_ok());
            l.drop_connection();
            worker.join().unwrap();
        }
    }

    #[test]
    fn failed_settings_control_preserves_input_and_reconnect_state() {
        for timeout in [false, true] {
            let name = format!("twime-link-control-fail-{timeout}-{}", std::process::id());
            let server = PipeServer::with_name(Some(name.clone())).unwrap();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let input = server.accept().unwrap();
                let _ = input.read().unwrap().unwrap();
                let hello = Reply::HelloCaps { version: 5, caps: twime_proto::CAP_DOC_TEXT };
                input.write(&hello.encode()).unwrap();
                let control = server.accept().unwrap();
                let _ = control.read().unwrap().unwrap();
                control.write(&hello.encode()).unwrap();
                let raw = control.read().unwrap().unwrap();
                assert_eq!(Request::decode(&raw).unwrap().event, Event::User(twime_proto::UserReq::GetSettings));
                if timeout {
                    // Hold the control connection open until its client has timed out.
                    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                } else {
                    control.disconnect();
                }
                let raw = input.read().unwrap().unwrap();
                assert_eq!(Request::decode(&raw).unwrap().event, Event::Ping);
                input.write(&update_with_candidates().encode_for(5)).unwrap();
                let _ = input.read();
            });
            let mut l = link_to(&name);
            assert!(l.ensure() && l.negotiate());
            let reconnect_state = (l.last_try, l.last_spawn, l.spawned);
            l.timeout_ms = 100;
            assert_eq!(l.user_settings(), Err(()));
            if timeout { done_tx.send(()).unwrap(); }
            assert!(l.is_up());
            assert_eq!((l.last_try, l.last_spawn, l.spawned), reconnect_state);
            assert_eq!((l.version(), l.generation(), l.doc_text_allowed(), l.needs_hello), (5, 1, true, false));
            l.timeout_ms = 3000;
            assert!(l.call(&key_request()).is_ok());
            l.drop_connection();
            worker.join().unwrap();
        }
    }

    #[test]
    fn settings_file_parsing() {
        assert_eq!(Settings::parse(""), Settings::default());
        let s = Settings::parse("shift_tap=off
default_mode = English
punct=half
symbol_table=off
nonsense
x=y
");
        assert_eq!(s, Settings { shift_tap: false, default_english: true, punct_half: true, symbol_table: false });
        assert_eq!(s.engine_flags(), " --default-english --punct-half --no-symbol-table");
        assert_eq!(Settings::default().engine_flags(), "");
        assert!(Settings::parse("shift_tap=on").shift_tap);
    }
}

#[cfg(test)]
mod host_policy_tests {
    use super::*;
    #[test]
    fn classifies_integrity_elevation_services_and_query_failure() {
        for rid in [0x2000, 0x2100, 0x2fff] {
            assert_eq!(classify_host(rid, false, "S-1-5-21-123", 1, false), HostClass::Normal);
        }
        for rid in [0x3000, 0x4000, u32::MAX] {
            assert_eq!(classify_host(rid, false, "S-1-5-21-123", 1, false), HostClass::Elevated);
        }
        for rid in [0, 0x1000, 0x1fff, 0x2000] {
            assert_eq!(classify_host(rid, true, "S-1-5-21-123", 1, false), HostClass::Elevated);
        }
        for rid in [0, 0x1000, 0x1fff] {
            assert_eq!(classify_host(rid, false, "S-1-5-21-123", 1, false), HostClass::Low);
        }
        for rid in [0, 0x2000, 0x3000, 0x4000] {
            for elevated in [false, true] {
                for sid in ["S-1-5-18", "S-1-5-19", "S-1-5-20"] {
                    assert_eq!(classify_host(rid, elevated, sid, 1, false), HostClass::Service);
                }
                assert_eq!(classify_host(rid, elevated, "S-1-5-21-123", 0, false), HostClass::Service);
                assert_eq!(classify_host(rid, elevated, "S-1-5-21-123", 1, true), HostClass::Service);
            }
        }
    }
    #[test]
    fn restricted_hosts_do_not_spawn_and_service_does_not_connect() {
        for host in [HostClass::Service, HostClass::Elevated, HostClass::Low] {
            let mut link = super::tests::link_to("private-p2-unused");
            link.host = host;
            if host == HostClass::Service {
                assert!(!link.ensure());
                assert!(link.last_try.is_none());
                assert!(link.user_settings().is_err());
            }
            link.spawn_engine();
            assert_eq!(link.spawned, 0);
            assert!(link.last_spawn.is_none());
        }
    }

    #[test]
    fn only_normal_attempts_start_when_pipe_is_missing() {
        let pipe = format!("twpotato-p2-missing-{}", std::process::id());
        for host in [HostClass::Normal, HostClass::Elevated, HostClass::Low] {
            let mut link = super::tests::link_to(&pipe);
            link.host = host;
            assert!(!link.ensure());
            assert_eq!(link.last_spawn.is_some(), host == HostClass::Normal);
        }
    }

    #[test]
    fn elevated_and_low_can_connect_to_existing_engine() {
        for (index, host) in [HostClass::Elevated, HostClass::Low].into_iter().enumerate() {
            let pipe = format!("twpotato-p2-host-{}-{index}", std::process::id());
            let _server = twime_winpipe::PipeServer::with_name(Some(pipe.clone())).unwrap();
            let mut link = super::tests::link_to(&pipe);
            link.host = host;
            assert!(link.ensure());
            assert!(link.last_spawn.is_none());
        }
    }
}

/// Manual measurement, not part of the normal run: `TWIME_COLD_ROOT=<dir with twime-engined.exe and assets\>` and
/// `cargo test -p twime-tsf cold_start_window -- --ignored --nocapture`. Starts a private engine (own pipe name and
/// state directory, never the installed one) through the real `Link::ensure` the way a key would, and prints how long
/// the shell stays without an engine: spawn, pipe visible, first successful connect, first handshake and `Focus`.
#[cfg(test)]
mod cold_start {
    use super::*;
    use twime_proto::Event;

    #[test]
    #[ignore]
    fn cold_start_window() {
        let root = PathBuf::from(std::env::var("TWIME_COLD_ROOT").expect("TWIME_COLD_ROOT"));
        let pipe = format!("twdiag-cold-{}", std::process::id());
        let full = twime_winpipe::qualify(&pipe);
        unsafe { std::env::set_var("TWIME_ENGINE_ARGS", format!("--pipe {pipe} --data-dir \"{}\"", root.join("state").display())) };
        std::fs::create_dir_all(root.join("state")).unwrap();
        let mut l = super::tests::link_to(&pipe);
        l.engine_exe = root.join("twime-engined.exe");
        l.root = root;
        let _stop = StopEngine(twime_winpipe::qualify(&pipe));
        let t0 = Instant::now();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let s2 = seen.clone();
        std::thread::spawn(move || {
            while !Path::new(&full).exists() { std::thread::sleep(Duration::from_millis(2)); }
            s2.store(t0.elapsed().as_millis() as u64, std::sync::atomic::Ordering::SeqCst);
        });
        let mut passthrough_keys = 0u32;
        // One key every 60 ms, each key-test calls `ensure` (which throttles itself to one connect per 500 ms).
        while !l.ensure() {
            passthrough_keys += 1;
            assert!(t0.elapsed() < Duration::from_secs(60), "engine never came up");
            std::thread::sleep(Duration::from_millis(60));
        }
        let connected = t0.elapsed().as_millis();
        assert!(l.negotiate());
        let handshake = t0.elapsed().as_millis();
        let req = Request { session: 1, seq: 1, ack: None, event: Event::Focus(true) };
        l.timeout_ms = 3000;
        l.call(&req).expect("Focus");
        let focus = t0.elapsed().as_millis();
        // The real shell is "ready" only after its sync timer fires: add up to the retry delay (1000 ms for the first five tries).
        println!("COLD spawned={} pipe_visible_ms={} connected_ms={connected} handshake_ms={handshake} first_focus_ms={focus} passthrough_keys_before_connect={passthrough_keys}",
            l.spawned, seen.load(std::sync::atomic::Ordering::SeqCst));
    }
}

/// Ends the private measurement engine (found through its pipe's server pid, also when the wait timed out) when the measurement is over or panics.
/// If the engine never created its pipe (load stuck past the 60 s limit) there is no pid to find: stop the
/// `twime-engined.exe` that runs from the scratch directory by hand.
#[cfg(test)]
struct StopEngine(String);
#[cfg(test)]
impl Drop for StopEngine {
    fn drop(&mut self) {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
        let pid = Client::connect(&self.0, None, 0).map_or(0, |c| c.server_pid);
        if pid != 0 && let Ok(h) = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) } {
            unsafe { let _ = TerminateProcess(h, 0); let _ = CloseHandle(h); }
        }
    }
}
