//! P3/P4: full pipe round trips, validated outside timing against an offline expected file.
use super::*;
use std::sync::mpsc;

// Receive time is captured immediately after the pipe call, before reply decoding.
fn idle_written(conn: &mut Conn, generation: u32, tx: mpsc::SyncSender<Instant>) -> (Result<Update, CallError>, Instant) {
    conn.seq += 1;
    let request = Request { session: 13, seq: conn.seq, ack: None, event: Event::Idle(generation) };
    let bytes = conn.c.call_with_written(&request.encode(), 5000, |written| tx.send(written).unwrap());
    let received = Instant::now();
    let response = bytes.and_then(|bytes| match Reply::decode(&bytes) {
        Some(Reply::Update { update, .. }) => Ok(update),
        _ => Err(CallError::Broken(std::io::Error::other("unexpected reply"))),
    });
    (response, received)
}

fn report_offsets(kind: &str, delay: u64, offsets: Vec<f64>) {
    let late = offsets.iter().filter(|&&offset| offset > delay as f64 + 2.0).count();
    println!("pop_{kind}_offset_late_{delay}={late}");
    line(&format!("pop_{kind}_offset_{delay}"), offsets);
}

fn prepare(conn: &mut Conn, u: &serde_json::Value, s: u32, times: &mut Vec<f64>) -> Update {
    conn.call(s, Event::SetContext(u["context"].as_str().unwrap().into())).expect("context");
    let mut last = Update::default();
    for x in u["syllables"].as_array().unwrap() {
        for k in syllable_keys(x.as_str().unwrap()).unwrap() {
            let t=Instant::now(); last=conn.call(s,Event::Key(KeyEvent::press(k))).expect("key"); times.push(t.elapsed().as_secs_f64()*1000.);
        }
    }
    last
}
pub(super) fn run(a: &[String]) {
    std::panic::set_hook(Box::new(|_| eprintln!("acpop latency error (text withheld)")));
    if let Some(m)=arg(a,"--cpu-mask") { unsafe { windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(),usize::from_str_radix(&m,16).unwrap()).expect("affinity"); } }
    let man: serde_json::Value=serde_json::from_slice(&std::fs::read(&a[3]).unwrap()).unwrap();
    let expected: Vec<Vec<String>>=serde_json::from_slice(&std::fs::read(arg(a,"--expected").expect("--expected")).unwrap()).unwrap();
    let units=man["units"].as_array().unwrap(); assert_eq!(units.len(),expected.len());
    let typing_only=a.iter().any(|s| s=="--typing-only");
    let on=arg(a,"--tag").as_deref()!=Some("off");
    let mut conn=Conn { c:Client::connect(&a[2],None,200).expect("connect"),seq:0 }; assert!(conn.hello().0>=7);
    let pid=Some(conn.c.server_pid);
    let (mut idle,mut typing)=(vec![],vec![]);
    let mut classes=[0usize;5]; // valid, suppressed/ineligible, invalid, error, timeout
    for (i,u) in units.iter().enumerate().cycle().take(100+units.len()).enumerate() {
        let (idx,u)=u;
        let mut warm_times=vec![];
        let last=prepare(&mut conn,u,13,if i>=100 {&mut typing} else {&mut warm_times});
        if typing_only { continue; }
        let t=Instant::now(); let result=conn.call(13,Event::Idle(last.pop_gen)); let elapsed=t.elapsed().as_secs_f64()*1000.;
        if i==0 { println!("pop_cold_idle_ms={elapsed:.4}"); }
        if i>=100 {
            match result {
                Ok(ref update) if update.completion.as_ref().is_some_and(|h| h.id==last.pop_gen && h.options==expected[idx] && !expected[idx].is_empty()) => { idle.push(elapsed); classes[0]+=1; }
                Ok(ref update) if update.completion.is_none() && (last.pop_gen==0 || expected[idx].is_empty()) => classes[1]+=1,
                Ok(_) => classes[2]+=1,
                Err(CallError::Timeout) => classes[4]+=1,
                Err(_) => classes[3]+=1,
            }
        }
    }
    line("pop_idle",idle); line("pop_typing",typing);
    if typing_only { conn.call(13,Event::Close).expect("close"); return; }
    println!("pop_valid={} suppressed_or_ineligible={} invalid={} errors={} timeouts={}",classes[0],classes[1],classes[2],classes[3],classes[4]);
    if on && (classes[0]<500 || classes[2]!=0 || classes[3]!=0 || classes[4]!=0) { std::process::exit(3); }
    if a.iter().any(|s| s=="--idle-only") { conn.call(13,Event::Close).expect("close"); return; }
    // Shell contract: key_start is key submission to the client; on the same
    // connection its pipe request waits for Idle's reply. That wait is timed.
    // Overlap is only the client interval [Idle write completion, reply receipt),
    // reported as evidence, not as a gate or a claim of engine computation overlap.
    for delay in [0u64,2,5,8] {
        let mut same=vec![]; let mut cross=vec![]; let mut baseline=vec![]; let mut overlapping=0usize;
        let mut same_overlapping=0usize;
        let mut same_offsets=vec![]; let mut cross_offsets=vec![];
        let mut other=Conn { c:Client::connect(&a[2],None,200).expect("B"),seq:0 }; other.hello();
        for u in units {
            let last=prepare(&mut conn,u,13,&mut vec![]);
            let (tx,rx)=mpsc::sync_channel(1);
            let handle=std::thread::spawn(move || {
                let (response,received)=idle_written(&mut conn,last.pop_gen,tx);
                (conn,response,received)
            });
            let start=rx.recv().unwrap();
            let due=start+std::time::Duration::from_millis(delay);
            if let Some(left)=due.checked_duration_since(Instant::now()) { std::thread::sleep(left); }
            let key_start=Instant::now();
            let (c,response,end)=handle.join().unwrap(); conn=c; response.expect("idle");
            conn.call(13,Event::Key(KeyEvent::press(Key::Left))).expect("waiting key");
            same.push(key_start.elapsed().as_secs_f64()*1000.);
            same_offsets.push(key_start.duration_since(start).as_secs_f64()*1000.);
            // Count after timing, just as for the cross-connection observations below.
            same_overlapping+=usize::from(start<=key_start && key_start<end);
            let last=prepare(&mut conn,u,13,&mut vec![]);
            prepare(&mut other,u,14,&mut vec![]);
            let t=Instant::now(); other.call(14,Event::Key(KeyEvent::press(Key::Left))).expect("B baseline"); baseline.push(t.elapsed().as_secs_f64()*1000.);
            other.call(14,Event::Key(KeyEvent::press(Key::Right))).expect("B reset");
            let (tx,rx)=mpsc::sync_channel(1);
            let handle=std::thread::spawn(move || {
                let (response,received)=idle_written(&mut conn,last.pop_gen,tx);
                (conn,response,received)
            });
            let start=rx.recv().unwrap(); let due=start+std::time::Duration::from_millis(delay);
            if let Some(left)=due.checked_duration_since(Instant::now()) { std::thread::sleep(left); }
            let key_start=Instant::now();
            other.call(14,Event::Key(KeyEvent::press(Key::Left))).expect("B key");
            cross.push(key_start.elapsed().as_secs_f64()*1000.);
            cross_offsets.push(key_start.duration_since(start).as_secs_f64()*1000.);
            let (c,response,end)=handle.join().unwrap(); conn=c; response.expect("idle");
            overlapping+=usize::from(start<=key_start && key_start<end);
        }
        line(&format!("pop_wait_{delay}"),same); line(&format!("pop_cross_base_{delay}"),baseline); line(&format!("pop_cross_{delay}"),cross);
        report_offsets("wait",delay,same_offsets); report_offsets("cross",delay,cross_offsets);
        println!("pop_overlap_{delay}={overlapping}");
        println!("pop_wait_overlap_{delay}={same_overlapping}");
        other.call(14,Event::Close).expect("close B");
    }
    mem(pid,"pop_memory"); conn.call(13,Event::Close).expect("close");
}
