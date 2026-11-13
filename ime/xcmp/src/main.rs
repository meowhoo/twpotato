//! Cross-platform comparison client: starts one engine executable, types every case through the real wire protocol
//! (named pipe on Windows, Unix socket elsewhere) and writes what came back. The same source runs on both systems.
//!
//! xcmp ENGINE ASSETS_DIR CASES.tsv OUT_PREFIX SCRATCH_DIR [PROTOCOL_VERSION [USER_DIR]]
//!   CASES: id <TAB> set <TAB> expected text <TAB> zhuyin syllables separated by spaces
//!   SCRATCH_DIR must not exist (it is created; nothing that exists is ever removed). macOS: the socket is
//!   SCRATCH_DIR/x.sock, so the path must be short (a socket address holds 103 bytes)
//!   USER_DIR: a directory holding userlex.tsv; it is copied into the scratch directory and the engine runs with user
//!   data from the copy (without it the engine runs without user data)
//!   writes OUT_PREFIX.out.tsv: id, composition at the end, its segments, first candidates, every commit of the case
//!          joined, and one hash per reply of the case (handled, composing, list open, commit, composition, cursor;
//!          not the candidate lists); OUT_PREFIX.lat.tsv (one line per key: kind, round trip in us, engine time in us);
//!          OUT_PREFIX.summary.txt

use std::{fs, io::Write, path::PathBuf, process::{Command, Stdio}, time::{Duration, Instant}};
use twime_core::fuzzy;
use twime_proto::{Applied, CommitResult, Event, Key, KeyEvent, PROTOCOL_VERSION, Reply, Request, Update};
use twime_winpipe::Client;

struct Eng { c: Client, seq: u32, pend: Option<CommitResult>, last: Update, lat: Vec<(&'static str, u64, u32)>, commits: String, steps: Vec<u32> }
impl Eng {
    fn send(&mut self, kind: &'static str, ev: Event) -> Update {
        self.seq += 1;
        let r = Request { session: 1, seq: self.seq, ack: self.pend.take(), event: ev }.encode();
        let t = Instant::now();
        let b = self.c.call(&r, 20000).expect("call");
        let rt = t.elapsed().as_micros() as u64;
        let u = match Reply::decode(&b).expect("decode") { Reply::Update { update, .. } => update, o => panic!("{o:?}") };
        self.lat.push((kind, rt, u.engine_us));
        self.pend = Some(CommitResult { seq: self.seq, commit: if u.commit.is_some() { Applied::Applied } else { Applied::NotApplicable }, comp: if u.composition.is_some() { Applied::Applied } else { Applied::NotApplicable } });
        if let Some(c) = &u.commit { self.commits.push_str(c); }
        // Candidate lists are left out of the hash: near ties may order them differently across systems; the list at the
        // end of the case is compared in its own column.
        let step = format!("{}|{}|{}|{:?}|{:?}\n", u.handled, u.composing, u.candidates_open, u.commit,
            u.composition.as_ref().map(|c| (c.text(), c.cursor)));
        self.steps.push(step.bytes().fold(0x811c9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x01000193)));
        // XCMP_STEPS=1: every reply on stderr (to find where two systems part)
        if std::env::var_os("XCMP_STEPS").is_some() { eprint!("{}\t{kind}\t{step}", self.seq); }
        self.last = u.clone();
        u
    }
    fn comp(&self) -> String { self.last.composition.as_ref().map(|c| c.text()).unwrap_or_default() }
    fn segs(&self) -> String {
        self.last.composition.as_ref().map(|c| c.segments.iter().map(|s| s.text.clone()).collect::<Vec<_>>().join("|")).unwrap_or_default()
    }
}

fn pct(v: &[u64], p: f64) -> u64 {
    if v.is_empty() { return 0 }
    v[(((v.len() as f64) * p).ceil() as usize).clamp(1, v.len()) - 1]
}

/// Peak resident set in bytes (Linux: VmHWM); 0 where this tool has no source for it.
#[cfg(not(target_os = "macos"))]
fn peak_resident(pid: u32) -> u64 {
    fs::read_to_string(format!("/proc/{pid}/status")).ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("VmHWM:").and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())))
        .map_or(0, |kb| kb * 1024)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    // Absolute: the engine refuses a relative HOME, and its directories are derived from this one.
    let (engine, assets, cases, prefix, scratch) = (&a[1], &a[2], &a[3], &a[4], std::path::absolute(&a[5]).unwrap());
    let version: u16 = a.get(6).map_or(PROTOCOL_VERSION, |v| v.parse().expect("protocol version"));
    // Private to the user from the start: a copy of the user lexicon may go in here.
    let mut private = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut private, 0o700);
    private.create(&scratch).expect("the scratch directory must not exist yet");
    fs::create_dir(scratch.join("home")).unwrap();
    // macOS: the default socket directory follows HOME, which the engine below gets a different one of; both ends use
    // one explicit path instead (a name with `/` is taken as it is).
    let pipe = if cfg!(target_os = "macos") { scratch.join("x.sock").to_str().expect("scratch path").to_owned() } else { format!("xcmp-{}", std::process::id()) };
    let log = fs::File::create(scratch.join("engine.log")).unwrap();
    let home = scratch.join("home");
    let mut cmd = Command::new(engine);
    cmd.args(["--assets-dir", assets, "--candidates", "--pipe", &pipe, "--data-dir"]).arg(scratch.join("state"));
    if let Some(src) = a.get(7) {
        let user = scratch.join("user");
        fs::create_dir(&user).unwrap();
        fs::copy(PathBuf::from(src).join("userlex.tsv"), user.join("userlex.tsv")).expect("USER_DIR/userlex.tsv");
        cmd.arg("--user-dir").arg(user);
    }
    let mut child = cmd
        .env("LOCALAPPDATA", &home).env("HOME", &home).env_remove("XDG_CONFIG_HOME").env_remove("XDG_STATE_HOME")
        .stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log)).spawn().expect("engine");
    let until = Instant::now() + Duration::from_secs(120);
    let c = loop {
        if let Ok(c) = Client::connect(&pipe, None, 50) { break c }
        assert!(child.try_wait().unwrap().is_none(), "engine exited early (see engine.log in the scratch directory)");
        assert!(Instant::now() < until, "timeout");
        std::thread::sleep(Duration::from_millis(100));
    };
    let hello = c.call(&Request { session: 0, seq: 0, ack: None, event: Event::Hello(version) }.encode(), 5000).unwrap();
    match Reply::decode(&hello) {
        Some(Reply::Hello { version: v }) | Some(Reply::HelloCaps { version: v, .. }) => assert_eq!(v, version, "negotiated version"),
        o => panic!("unexpected answer to Hello: {o:?}"),
    }
    let mut e = Eng { c, seq: 0, pend: None, last: Update::default(), lat: vec![], commits: String::new(), steps: vec![] };
    let mut out = fs::File::create(format!("{prefix}.out.tsv")).unwrap();
    let text = fs::read_to_string(cases).unwrap();
    let mut n = 0;
    for l in text.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = l.split('\t').collect();
        let syl: Option<Vec<fuzzy::Slots>> = f[3].split(' ').filter(|x| !x.is_empty()).map(fuzzy::Slots::parse).collect();
        let syl = syl.unwrap_or_else(|| panic!("case {}: unreadable zhuyin", f[0]));
        (e.commits, e.steps) = (String::new(), vec![]);
        e.send("focus", Event::Focus(true));
        for s in &syl {
            let keys = fuzzy::keys_of(s);
            for (i, k) in keys.iter().enumerate() {
                e.send(if i + 1 == keys.len() { "convert" } else { "slot" }, Event::Key(KeyEvent::press(*k)));
            }
        }
        let (comp, segs) = (e.comp(), e.segs());
        let u = e.send("list", Event::Key(KeyEvent::press(Key::Down)));
        let items = u.candidates.as_ref().map(|c| c.items.iter().take(9).cloned().collect::<Vec<_>>().join("/")).unwrap_or_default();
        e.send("other", Event::Key(KeyEvent::press(Key::Esc)));
        e.send("commit", Event::Key(KeyEvent::press(Key::Enter)));
        writeln!(out, "{}\t{comp}\t{segs}\t{items}\t{}\t{}", f[0], e.commits, e.steps.iter().map(|h| format!("{h:08x}")).collect::<Vec<_>>().join(" ")).unwrap();
        n += 1;
    }
    #[cfg(not(target_os = "macos"))]
    let (ws, private, peak) = {
        let (ws, private) = twime_winpipe::memory_of_pid(child.id()).expect("engine memory");
        (ws, private, peak_resident(child.id()))
    };
    // macOS: resident now from ps (KiB); the peak from the resource usage of the ended engine (bytes there; it covers
    // loading too). Private memory is not measured: the field reads 0.
    #[cfg(target_os = "macos")]
    let (ws, private, peak) = {
        let ps = Command::new("ps").args(["-o", "rss=", "-p", &child.id().to_string()]).output();
        // The engine is ended and reaped before any failure of the measurement is reported: none is left running.
        let killed = child.kill();
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let mut status = 0;
        let reaped = unsafe { libc::wait4(child.id() as libc::pid_t, &mut status, 0, &mut usage) };
        killed.expect("kill the engine");
        assert_eq!(reaped, child.id() as libc::pid_t, "wait4");
        let ws = String::from_utf8_lossy(&ps.expect("ps").stdout).trim().parse::<u64>().expect("ps rss") * 1024;
        let peak = usage.ru_maxrss as u64;
        assert!(peak > 0, "engine peak memory reads as zero");
        (ws, 0u64, peak)
    };
    assert!(ws > 0, "engine memory reads as zero");
    let mut lat = fs::File::create(format!("{prefix}.lat.tsv")).unwrap();
    for (k, rt, us) in &e.lat { writeln!(lat, "{k}\t{rt}\t{us}").unwrap(); }
    let mut sum = String::new();
    for kind in ["convert", "slot", "list", "commit", "all"] {
        let mut rt: Vec<u64> = e.lat.iter().filter(|x| kind == "all" || x.0 == kind).map(|x| x.1).collect();
        let mut en: Vec<u64> = e.lat.iter().filter(|x| kind == "all" || x.0 == kind).map(|x| u64::from(x.2)).collect();
        rt.sort(); en.sort();
        sum += &format!("{kind}\tn={}\tround-trip us p50={} p95={} p99={} max={} over10ms={}\tengine us p50={} p99={} max={}\n",
            rt.len(), pct(&rt, 0.5), pct(&rt, 0.95), pct(&rt, 0.99), rt.last().copied().unwrap_or(0), rt.iter().filter(|x| **x > 10000).count(),
            pct(&en, 0.5), pct(&en, 0.99), en.last().copied().unwrap_or(0));
    }
    // End of run. Windows: working set / private bytes. Linux: VmRSS / RssAnon+VmSwap, and the peak resident set (VmHWM).
    // macOS: ps rss / not measured (0) / ru_maxrss.
    sum += &format!("cases={n}\tprotocol={version}\tresident_mb={:.1}\tprivate_mb={:.1}\tpeak_resident_mb={:.1}\n",
        ws as f64 / 1048576.0, private as f64 / 1048576.0, peak as f64 / 1048576.0);
    fs::write(format!("{prefix}.summary.txt"), &sum).unwrap();
    print!("{sum}");
    // macOS: the engine was ended and reaped above.
    #[cfg(not(target_os = "macos"))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}
