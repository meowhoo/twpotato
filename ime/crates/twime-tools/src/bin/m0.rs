//! M0 measurements of the engine transport (Windows named pipe to the real engine process).
//!
//!   m0 rtt    <pipe> <n>                       ping round trip; reply sizes 25 B / 256 B / 2 KB (engine needs --test-faults)
//!   m0 e2e    <pipe> <units> <docs> [--clients N] [--cpu-mask HEX] [--warmup 200] [--longest-reps 500] [--engine-pid P]
//!   m0 cold   <engine.exe> <pipe> [engine args...]  start-up, warm connect and kill-restart times
//!   m0 faults <pipe>                           timeout / late reply / crash behavior (engine needs --test-faults)
//!
//! Only counts and times are printed, never sentence text.

use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use twime_core::keymap::key_of;
use twime_proto::{Event, Key, KeyEvent, Reply, Request, Update};
use twime_tools::cases::{Case, load_cases};
use twime_winpipe::{CallError, Client};

const LONGEST_ID: &str = "ey-be4cabb8e5df-038";
const TIMEOUT_MS: u32 = 30;

struct Conn {
    c: Client,
    seq: u32,
}

impl Conn {
    fn open(pipe: &str) -> std::io::Result<Conn> {
        Ok(Conn { c: Client::connect(pipe, None, 200)?, seq: 0 })
    }
    fn call(&mut self, session: u32, event: Event, timeout: u32) -> Result<Update, CallError> {
        self.seq += 1;
        let r = Request { session, seq: self.seq, ack: None, event };
        let bytes = self.c.call(&r.encode(), timeout)?;
        match Reply::decode(&bytes) {
            Some(Reply::Update { update, .. }) => Ok(update),
            _ => Err(CallError::Broken(std::io::Error::other("unexpected reply"))),
        }
    }
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * p) as usize).min(v.len() - 1)]
}

fn line(name: &str, mut ms: Vec<f64>) {
    if ms.is_empty() {
        println!("{name}\tn=0");
        return;
    }
    println!(
        "{name}\tn={}\tp50={:.3}\tp95={:.3}\tp99={:.3}\tmax={:.3}\t(ms)",
        ms.len(),
        pct(&mut ms, 0.50),
        pct(&mut ms, 0.95),
        pct(&mut ms, 0.99),
        ms.last().copied().unwrap_or(0.0)
    );
}

/// Keys that type one syllable in the standard order; the last one completes it.
fn syllable_keys(s: &str) -> Option<Vec<Key>> {
    let mut keys = vec![];
    let mut tone = false;
    for c in s.chars() {
        keys.push(key_of(c)?);
        tone = matches!(c, 'ˊ' | 'ˇ' | 'ˋ' | '˙');
    }
    if !tone {
        keys.push(Key::Space);
    }
    Some(keys)
}

#[derive(Default)]
struct Stats {
    trigger: Vec<f64>,
    trigger_full: Vec<f64>,
    other: Vec<f64>,
    engine: Vec<f64>,
    overhead: Vec<f64>,
    errors: usize,
}

impl Stats {
    fn merge(&mut self, o: Stats) {
        self.trigger.extend(o.trigger);
        self.trigger_full.extend(o.trigger_full);
        self.other.extend(o.other);
        self.engine.extend(o.engine);
        self.overhead.extend(o.overhead);
        self.errors += o.errors;
    }
    fn print(&self, tag: &str) {
        line(&format!("{tag}_trigger_all_prefix"), self.trigger.clone());
        line(&format!("{tag}_trigger_full_length"), self.trigger_full.clone());
        line(&format!("{tag}_other_keys"), self.other.clone());
        line(&format!("{tag}_engine_self_reported_trigger"), self.engine.clone());
        line(&format!("{tag}_pipe_overhead_trigger"), self.overhead.clone());
        println!("{tag}_errors\t{}", self.errors);
    }
}

/// Types one unit and records the round trip of every key. `record` false = warm-up.
fn run_unit(conn: &mut Conn, session: u32, c: &Case, record: bool, st: &mut Stats) -> bool {
    let seqs: Option<Vec<Vec<Key>>> = c.symbols.iter().map(|s| syllable_keys(s)).collect();
    let Some(seqs) = seqs else { return false };
    if conn.call(session, Event::SetContext(c.context.clone()), 5000).is_err() {
        st.errors += 1;
        return false;
    }
    let n = seqs.len();
    for (i, keys) in seqs.iter().enumerate() {
        for (j, k) in keys.iter().enumerate() {
            let last = j + 1 == keys.len();
            let t = Instant::now();
            let r = conn.call(session, Event::Key(KeyEvent::press(*k)), 5000);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok(u) => {
                    if record {
                        if last {
                            st.trigger.push(ms);
                            if i + 1 == n {
                                st.trigger_full.push(ms);
                            }
                            st.engine.push(f64::from(u.engine_us) / 1000.0);
                            st.overhead.push(ms - f64::from(u.engine_us) / 1000.0);
                        } else {
                            st.other.push(ms);
                        }
                    }
                }
                Err(_) => {
                    st.errors += 1;
                    return false;
                }
            }
        }
    }
    let _ = conn.call(session, Event::Key(KeyEvent::press(Key::Esc)), 5000);
    true
}

fn set_affinity(mask: usize) {
    unsafe {
        let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), mask);
    }
}

fn arg_value(a: &[String], flag: &str) -> Option<String> {
    a.iter().position(|x| x == flag).and_then(|i| a.get(i + 1)).cloned()
}

fn mem_report(pid: Option<u32>, tag: &str) {
    if let Some(p) = pid {
        match twime_winpipe::memory_of_pid(p) {
            Ok((ws, private)) => println!("{tag}\tpid={p}\tworking_set_mb={:.1}\tprivate_mb={:.1}", ws as f64 / 1048576.0, private as f64 / 1048576.0),
            Err(e) => println!("{tag}\tpid={p}\tmemory query failed: {e}"),
        }
    }
}

fn e2e(a: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let pipe = &a[2];
    let cases = load_cases(&a[3], &a[4])?;
    let clients: usize = arg_value(a, "--clients").map_or(1, |v| v.parse().unwrap_or(1));
    let warmup: usize = arg_value(a, "--warmup").map_or(200, |v| v.parse().unwrap_or(200));
    let reps: usize = arg_value(a, "--longest-reps").map_or(500, |v| v.parse().unwrap_or(500));
    let engine_pid: Option<u32> = arg_value(a, "--engine-pid").and_then(|v| v.parse().ok());
    if let Some(m) = arg_value(a, "--cpu-mask") {
        set_affinity(usize::from_str_radix(&m, 16)?);
    }
    let longest = cases.iter().find(|c| c.id == LONGEST_ID).ok_or("longest sample missing")?;
    println!("units\t{}\tlongest_syllables\t{}\tclients\t{clients}", cases.len(), longest.symbols.len());
    mem_report(engine_pid, "engine_memory_before");
    let st_dev = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..clients)
            .map(|ci| {
                let cases = &cases;
                sc.spawn(move || {
                    let mut conn = Conn::open(pipe).expect("connect");
                    let mut st = Stats::default();
                    // Warm-up on this client's first units (not recorded).
                    for c in cases.iter().skip(ci).step_by(clients).take(warmup.div_ceil(clients).max(1)) {
                        run_unit(&mut conn, 1, c, false, &mut st);
                    }
                    for c in cases.iter().skip(ci).step_by(clients) {
                        run_unit(&mut conn, 1, c, true, &mut st);
                    }
                    st
                })
            })
            .collect();
        let mut all = Stats::default();
        for h in hs {
            all.merge(h.join().expect("client thread"));
        }
        all
    });
    st_dev.print("dev");
    mem_report(engine_pid, "engine_memory_after_dev");
    if clients == 1 {
        let mut conn = Conn::open(pipe)?;
        let mut st = Stats::default();
        for _ in 0..reps {
            run_unit(&mut conn, 2, longest, true, &mut st);
        }
        st.print("longest");
        mem_report(engine_pid, "engine_memory_after_longest");
    }
    Ok(())
}

fn rtt(a: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let pipe = &a[2];
    let n: usize = a[3].parse()?;
    let mut conn = Conn::open(pipe)?;
    for (name, session) in [("ping_reply_25B", 1u32), ("reply_256B", 0xFFFD_0100), ("reply_2KB", 0xFFFD_0800)] {
        for _ in 0..1000 {
            conn.call(session, Event::Ping, 5000).map_err(|e| format!("{e:?}"))?;
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            conn.call(session, Event::Ping, 5000).map_err(|e| format!("{e:?}"))?;
            v.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        line(&format!("rtt_{name}"), v);
    }
    Ok(())
}

fn wait_ready(pipe: &str, child: &mut Child, limit: Duration) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < limit {
        if let Ok(Some(_)) = child.try_wait() {
            return None;
        }
        if let Ok(mut c) = Conn::open(pipe)
            && c.call(1, Event::Ping, 2000).is_ok()
        {
            return Some(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    None
}

fn cold(a: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (exe, pipe) = (&a[2], &a[3]);
    let args = &a[4..];
    let spawn = || Command::new(exe).args(args).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let mut v = vec![];
    for i in 0..5 {
        let mut child = spawn()?;
        let d = wait_ready(pipe, &mut child, Duration::from_secs(30)).ok_or("engine did not become ready")?;
        println!("start_to_first_reply_ms\trun={i}\t{:.0}", d.as_secs_f64() * 1000.0);
        v.push(d.as_secs_f64() * 1000.0);
        if i == 4 {
            // Warm path with the engine running: a new connection plus one request.
            let mut w = vec![];
            for _ in 0..1000 {
                let t = Instant::now();
                let mut c = Conn::open(pipe)?;
                c.call(1, Event::Ping, 2000).map_err(|e| format!("{e:?}"))?;
                w.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            line("warm_connect_plus_first_request", w);
            // Kill and restart.
            let pid = child.id();
            let t = Instant::now();
            child.kill()?;
            child.wait()?;
            println!("kill_ms\tpid={pid}\t{:.1}", t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let mut child2 = spawn()?;
            let d = wait_ready(pipe, &mut child2, Duration::from_secs(30)).ok_or("restart failed")?;
            println!("restart_after_kill_ms\t{:.0}\t(total incl. spawn {:.0})", d.as_secs_f64() * 1000.0, t.elapsed().as_secs_f64() * 1000.0);
            child2.kill()?;
            child2.wait()?;
        } else {
            child.kill()?;
            child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    line("cold_start_ms", v);
    Ok(())
}

fn faults(a: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let pipe = &a[2];
    // (a) engine never answers
    let mut c = Conn::open(pipe)?;
    let t = Instant::now();
    let r = c.call(0xFFFF_FFFF, Event::Ping, TIMEOUT_MS);
    println!("stall_forever\tresult={}\telapsed_ms={:.1}", fmt_err(&r), t.elapsed().as_secs_f64() * 1000.0);
    drop(c);
    let mut c2 = Conn::open(pipe)?;
    let t = Instant::now();
    let ok = c2.call(1, Event::Ping, TIMEOUT_MS).is_ok();
    println!("after_stall_new_connection\tok={ok}\telapsed_ms={:.2}", t.elapsed().as_secs_f64() * 1000.0);
    // (b) late reply (100 ms) with a 30 ms limit; the connection is dropped, the stale reply never reaches a later call
    let mut c = Conn::open(pipe)?;
    let t = Instant::now();
    let r = c.call(0xFFFF_0064, Event::Ping, TIMEOUT_MS);
    println!("late_reply_100ms\tresult={}\telapsed_ms={:.1}", fmt_err(&r), t.elapsed().as_secs_f64() * 1000.0);
    drop(c);
    std::thread::sleep(Duration::from_millis(150));
    let mut c = Conn::open(pipe)?;
    let ok = c.call(1, Event::Ping, TIMEOUT_MS).is_ok();
    println!("after_late_reply_new_connection\tok={ok}");
    // (c) engine dies during the call
    let mut c = Conn::open(pipe)?;
    let t = Instant::now();
    let r = c.call(0xFFFE_0000, Event::Ping, TIMEOUT_MS);
    println!("engine_exit_during_call\tresult={}\telapsed_ms={:.1}", fmt_err(&r), t.elapsed().as_secs_f64() * 1000.0);
    // (d) no engine
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    let r = Conn::open(pipe);
    println!("connect_without_engine\tresult={}\telapsed_ms={:.2}", r.as_ref().map_or_else(|e| format!("{:?}", e.kind()), |_| "connected".into()), t.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}

fn fmt_err(r: &Result<Update, CallError>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(CallError::Timeout) => "Timeout".into(),
        Err(CallError::Broken(e)) => format!("Broken({:?})", e.kind()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("rtt") if a.len() >= 4 => rtt(&a),
        Some("e2e") if a.len() >= 5 => e2e(&a),
        Some("cold") if a.len() >= 4 => cold(&a),
        Some("faults") if a.len() >= 3 => faults(&a),
        Some("session") if a.len() >= 4 => {
            // m5 fault injection: open an input session with one pending key, then end the connection in the given way.
            let mut c = Conn::open(&a[2])?;
            let r = c.call(1, Event::Key(KeyEvent::press(Key::Digit(1))), 2000);
            println!("first key: {}", if r.is_ok() { "answered" } else { "failed (engine refused or gone)" });
            match a[3].as_str() {
                "close" => {
                    let _ = c.call(1, Event::Close, 2000);
                }
                "hold" => std::thread::sleep(Duration::from_secs(a.get(4).and_then(|s| s.parse().ok()).unwrap_or(8))),
                _ => {} // "drop": just end the process
            }
            Ok(())
        }
        _ => {
            eprintln!("usage: see the comment at the top of m0.rs");
            std::process::exit(2);
        }
    }
}
