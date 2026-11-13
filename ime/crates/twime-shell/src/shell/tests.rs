//! The contract of LINUX-plan 4.2 against a scripted engine: what is sent, in which order, with which
//! session, sequence number and acknowledgement. (The real engine is in `tests/engine.rs`.)

use std::{
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    time::{Duration, Instant},
};

use twime_proto::{Applied, Cands, CommitResult, Comp, Event, Key, KeyEvent, Mode, Mods, Reply as Wire, Request, Segment, Update};
use twime_winpipe::{Client, PipeServer, ServerConn};

use super::*;
use crate::link::{CALL_TIMEOUT_MS, VERSION};

type Script = dyn Fn(&Request) -> Update + Send + Sync;

/// A stand-in engine on its own socket. It records every request; `Hello` is answered with `hello`, everything else
/// with the Update the script gives (session and sequence number echoed, shifted by `seq_shift`).
struct Fake {
    path: String,
    seen: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<Arc<ServerConn>>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn socket(name: &str) -> String {
    std::env::temp_dir().join(format!("twsh-{}-{name}.sock", std::process::id())).to_str().unwrap().to_owned()
}

impl Fake {
    fn start(path: &str, hello: u16, script: impl Fn(&Request) -> Update + Send + Sync + 'static) -> Fake {
        let server = PipeServer::with_name(Some(path.to_owned())).unwrap();
        let (seen, stop, conns) = (Arc::new(Mutex::new(vec![])), Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(vec![])));
        let script: Arc<Script> = Arc::new(script);
        let (seen2, stop2, conns2) = (seen.clone(), stop.clone(), conns.clone());
        let thread = std::thread::spawn(move || {
            loop {
                let Ok(conn) = server.accept() else { break };
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let conn = Arc::new(conn);
                conns2.lock().unwrap().push(conn.clone());
                let (seen, script) = (seen2.clone(), script.clone());
                std::thread::spawn(move || {
                    while let Ok(Some(bytes)) = conn.read() {
                        let req = Request::decode(&bytes).expect("request");
                        seen.lock().unwrap().push(req.clone());
                        let answer = match req.event {
                            Event::Hello(_) if hello >= 5 => Wire::HelloCaps { version: hello, caps: 0 }.encode(),
                            Event::Hello(_) => Wire::Hello { version: hello }.encode(),
                            _ => Wire::Update { session: req.session, seq: req.seq, update: script(&req) }.encode_for(VERSION),
                        };
                        if conn.write(&answer).is_err() {
                            break;
                        }
                    }
                });
            }
            // The listener goes away here: the socket file is removed and its lock released.
        });
        Fake { path: path.to_owned(), seen, stop, conns, thread: Some(thread) }
    }

    fn plain(path: &str) -> Fake {
        Fake::start(path, VERSION, |_| Update::default())
    }

    /// (session, event) of every request so far.
    fn events(&self) -> Vec<(u32, Event)> {
        self.seen.lock().unwrap().iter().map(|r| (r.session, r.event.clone())).collect()
    }

    fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Fake {
    /// The engine ends: every connection is cut and the socket disappears.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for c in self.conns.lock().unwrap().iter() {
            c.disconnect();
        }
        drop(Client::connect(&self.path, None, 0)); // wakes the accept
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(format!("{}.lock", self.path));
    }
}

/// A shell on the fake's socket that can start nothing.
fn shell(path: &str) -> Shell {
    shell_with(path, 2000)
}

fn shell_with(path: &str, timeout_ms: u32) -> Shell {
    Shell::new(Config { socket: path.to_owned(), install: Some(("/nonexistent/twime-engined".into(), "/nonexistent".into())), timeout_ms, ..Config::default() })
}

fn down(c: u8) -> Input {
    Input::KeyDown(KeyCode::Key(Key::Letter(c)), 0)
}

fn key(c: u8) -> Event {
    Event::Key(KeyEvent { key: Key::Letter(c), mods: Mods(0), down: true })
}

fn composing(text: &str) -> Update {
    Update {
        handled: true,
        composing: true,
        composition: Some(Comp { segments: vec![Segment { text: text.into(), ..Segment::default() }], cursor: text.chars().count() as u16 }),
        ..Update::default()
    }
}

#[test]
fn focus_connects_but_no_session_is_made_before_the_first_key() {
    let path = socket("idle");
    let fake = Fake::plain(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    for i in [Input::FocusIn, Input::Sensitive(true), Input::Sensitive(false), Input::Reset, Input::Pick(0), Input::FocusOut, Input::KeyUp(KeyCode::Key(Key::Letter(b'a')), 0)] {
        let r = c.event(i);
        assert!(!r.handled && !r.ui_changed && !r.no_engine && r.commit.is_empty(), "{i:?}");
    }
    drop(c);
    assert_eq!(fake.events(), vec![(0, Event::Hello(VERSION))], "the connection is made at the focus; no session, no Close");
}

#[test]
fn focus_starts_the_engine() {
    let path = socket("focusstart");
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::FocusIn);
    assert_eq!(sh.spawn_attempts(), 1, "started at the focus, not at the first key");
}

#[test]
fn first_key_opens_the_session_with_focus_then_the_key() {
    let path = socket("first");
    let fake = Fake::start(&path, VERSION, |r| if matches!(r.event, Event::Key(_)) { composing("ㄋ") } else { Update::default() });
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::FocusIn);
    let r = c.event(down(b's'));
    assert!(r.handled && r.ui_changed && !r.no_engine);
    assert_eq!((r.ui.comp.text().as_str(), r.ui.comp.cursor), ("ㄋ", 1));
    let reqs = fake.requests();
    assert_eq!(reqs.iter().map(|r| (r.session, r.seq, r.event.clone())).collect::<Vec<_>>(),
        vec![(0, 0, Event::Hello(VERSION)), (1, 1, Event::Focus(true)), (1, 2, key(b's'))]);
    assert_eq!(reqs[1].ack, None);
    assert_eq!(reqs[2].ack, Some(CommitResult { seq: 1, commit: Applied::NotApplicable, comp: Applied::NotApplicable }));
}

#[test]
fn a_key_without_focus_event_still_tells_focus_first() {
    let path = socket("nofocus");
    let fake = Fake::plain(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(down(b's'));
    assert_eq!(fake.events()[1..], [(1, Event::Focus(true)), (1, key(b's'))]);
}

#[test]
fn acknowledgement_says_applied_for_the_parts_the_update_had() {
    let path = socket("ack");
    let fake = Fake::start(&path, VERSION, |r| match r.event {
        Event::Key(k) if k.key == Key::Letter(b's') => Update { commit: Some("好".into()), ..composing("ㄋ") },
        _ => Update { handled: true, composing: true, ..Update::default() },
    });
    let sh = shell(&path);
    let mut c = sh.context();
    let r = c.event(down(b's'));
    assert_eq!(r.commit, "好");
    c.event(down(b'u'));
    c.event(down(b'u'));
    let reqs = fake.requests();
    assert_eq!(reqs[3].ack, Some(CommitResult { seq: 2, commit: Applied::Applied, comp: Applied::Applied }));
    assert_eq!(reqs[4].ack, Some(CommitResult { seq: 3, commit: Applied::NotApplicable, comp: Applied::NotApplicable }));
}

#[test]
fn update_without_a_part_keeps_it_while_composing_and_clears_it_after() {
    let path = socket("parts");
    let cands = Cands { items: vec!["你".into(), "妳".into()], selected: 1, page: 0, total: 12, per_page: 9 };
    let cands2 = cands.clone();
    let _fake = Fake::start(&path, VERSION, move |r| match r.event {
        Event::Key(k) if k.key == Key::Letter(b's') => composing("你"),
        Event::Key(k) if k.key == Key::Down => Update { handled: true, composing: true, candidates_open: true, candidates: Some(cands2.clone()), ..Update::default() },
        Event::Key(k) if k.key == Key::Letter(b'k') => Update { handled: true, composing: true, candidates_open: true, ..Update::default() }, // keeps both
        Event::Key(k) if k.key == Key::Esc => Update { handled: true, composing: true, ..Update::default() }, // list closed
        Event::Key(k) if k.key == Key::Enter => Update { handled: true, commit: Some("你".into()), ..Update::default() },
        _ => Update::default(),
    });
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(down(b's'));
    let r = c.event(Input::KeyDown(KeyCode::Key(Key::Down), 0));
    assert!(r.ui_changed && r.ui.candidates_open);
    assert_eq!(r.ui.candidates, cands);
    let r = c.event(down(b'k'));
    assert!(r.handled && !r.ui_changed, "nothing new to show");
    assert_eq!((r.ui.comp.text().as_str(), r.ui.candidates_open, r.ui.candidates.items.len()), ("你", true, 2));
    let r = c.event(Input::KeyDown(KeyCode::Key(Key::Esc), 0));
    assert!(r.ui_changed && !r.ui.candidates_open && r.ui.candidates.items.is_empty());
    assert_eq!(r.ui.comp.text(), "你");
    let r = c.event(Input::KeyDown(KeyCode::Key(Key::Enter), 0));
    assert!(r.ui_changed && r.ui.comp.segments.is_empty());
    assert_eq!(r.commit, "你");
}

#[test]
fn sensitive_field_hears_password_first_and_its_keys_stay_here() {
    let path = socket("pw");
    let fake = Fake::plain(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::FocusIn);
    c.event(Input::Sensitive(true));
    for k in [b's', b'u', b'3'] {
        let r = c.event(down(k));
        assert!(!r.handled && !r.no_engine);
    }
    assert_eq!(fake.events(), vec![(0, Event::Hello(VERSION)), (1, Event::Focus(true)), (1, Event::Password(true))], "no key reaches the engine");
    // The attribute changes without a focus change: the engine hears it at once, and keys are sent again.
    c.event(Input::Sensitive(false));
    c.event(down(b's'));
    assert_eq!(fake.events()[3..], [(1, Event::Password(false)), (1, key(b's'))]);
    // Telling the same state again sends nothing.
    c.event(Input::Sensitive(false));
    assert_eq!(fake.events().len(), 5);
}

#[test]
fn sensitive_change_ends_the_composition_as_the_engine_says() {
    let path = socket("pwcomp");
    let fake = Fake::start(&path, VERSION, |r| if matches!(r.event, Event::Key(_)) { composing("你") } else { Update::default() });
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(down(b's'));
    let r = c.event(Input::Sensitive(true));
    assert!(r.ui_changed && r.ui.comp.segments.is_empty());
    assert_eq!(fake.events().last(), Some(&(1, Event::Password(true))));
}

#[test]
fn engine_restart_under_a_focused_password_field() {
    let path = socket("pwrestart");
    let fake = Fake::plain(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::FocusIn);
    c.event(Input::Sensitive(true));
    c.event(down(b's'));
    assert_eq!(fake.events().len(), 3);
    drop(fake);
    let r = c.event(down(b'u'));
    assert!(!r.handled && r.no_engine);
    let fake = Fake::plain(&path);
    for k in [b's', b'u'] {
        let r = c.event(down(k));
        assert!(!r.handled && !r.no_engine);
    }
    assert_eq!(fake.events(), vec![(0, Event::Hello(VERSION)), (2, Event::Focus(true)), (2, Event::Password(true))],
        "the new session's first events restore focus and the sensitive state; still no key");
}

#[test]
fn engine_gone_clears_what_was_shown_and_the_next_session_is_new() {
    let path = socket("gone");
    let fake = Fake::start(&path, VERSION, |r| if matches!(r.event, Event::Key(_)) { composing("你") } else { Update::default() });
    let sh = shell(&path);
    let (mut a, mut b) = (sh.context(), sh.context());
    a.event(down(b's'));
    b.event(down(b's'));
    assert_eq!(fake.events().iter().map(|e| e.0).collect::<Vec<_>>(), [0, 1, 1, 2, 2], "one connection, two sessions");
    drop(fake);
    let r = a.event(down(b'u'));
    assert!(!r.handled && r.no_engine && r.ui_changed && r.ui.comp.segments.is_empty() && r.commit.is_empty());
    // The other context learns it at its next event, whatever that is.
    let r = b.event(Input::FocusOut);
    assert!(r.ui_changed && r.ui.comp.segments.is_empty());
    let fake = Fake::plain(&path);
    b.event(down(b's'));
    a.event(down(b's'));
    assert_eq!(fake.events(), vec![(0, Event::Hello(VERSION)), (3, Event::Focus(true)), (3, key(b's')), (4, Event::Focus(true)), (4, key(b's'))]);
}

#[test]
fn another_protocol_version_is_not_used() {
    let path = socket("version");
    let fake = Fake::start(&path, 4, |_| Update::default());
    let sh = shell(&path);
    let mut c = sh.context();
    for _ in 0..5 {
        let r = c.event(down(b's'));
        assert!(!r.handled && r.no_engine);
    }
    assert_eq!(fake.events(), vec![(0, Event::Hello(VERSION))], "asked once, no session request, not asked again at once");
}

#[test]
fn slow_engine_passes_the_key_within_the_limit_and_loses_the_session() {
    let path = socket("slow");
    let fake = Fake::start(&path, VERSION, |r| match r.event {
        Event::Key(k) if k.key == Key::Letter(b'u') => {
            std::thread::sleep(Duration::from_millis(300));
            Update::default()
        }
        Event::Key(_) => composing("ㄋ"),
        _ => Update::default(),
    });
    let sh = shell_with(&path, CALL_TIMEOUT_MS);
    let mut c = sh.context();
    assert!(c.event(down(b's')).handled);
    let t = Instant::now();
    let r = c.event(down(b'u'));
    let took = t.elapsed();
    assert!(!r.handled && r.no_engine && r.ui_changed && r.ui.comp.segments.is_empty() && r.commit.is_empty());
    assert!(took >= Duration::from_millis(u64::from(CALL_TIMEOUT_MS)) && took < Duration::from_millis(u64::from(CALL_TIMEOUT_MS) + 20), "{took:?}");
    // The next key: a new connection and a new session.
    assert!(c.event(down(b's')).handled);
    assert_eq!(fake.events()[4..], [(0, Event::Hello(VERSION)), (2, Event::Focus(true)), (2, key(b's'))]);
}

#[test]
fn one_time_limit_covers_all_requests_of_a_key() {
    // Focus and Password take 18 ms each: neither exceeds 30 ms alone, the two requests of this one key together do.
    let path = socket("budget");
    let _fake = Fake::start(&path, VERSION, |_| {
        std::thread::sleep(Duration::from_millis(18));
        Update::default()
    });
    let sh = shell_with(&path, CALL_TIMEOUT_MS);
    let mut c = sh.context();
    c.event(Input::Sensitive(true));
    let t = Instant::now();
    let r = c.event(down(b's'));
    let took = t.elapsed();
    assert!(!r.handled && r.no_engine);
    assert!(took < Duration::from_millis(u64::from(CALL_TIMEOUT_MS) + 20), "{took:?}");
}

#[test]
fn reply_for_another_request_is_not_trusted() {
    let path = socket("seq");
    let server = PipeServer::with_name(Some(path.clone())).unwrap();
    std::thread::spawn(move || {
        let conn = server.accept().unwrap();
        while let Ok(Some(bytes)) = conn.read() {
            let req = Request::decode(&bytes).unwrap();
            let answer = match req.event {
                Event::Hello(_) => Wire::HelloCaps { version: VERSION, caps: 0 }.encode(),
                _ => Wire::Update { session: req.session, seq: req.seq + 1, update: composing("你") }.encode_for(VERSION),
            };
            if conn.write(&answer).is_err() {
                break;
            }
        }
    });
    let sh = shell(&path);
    let mut c = sh.context();
    let r = c.event(down(b's'));
    assert!(!r.handled && r.no_engine && r.ui.comp.segments.is_empty());
    let _ = std::fs::remove_file(format!("{path}.lock"));
}

#[test]
fn shift_tap_toggles_and_shift_is_never_handled() {
    let path = socket("shift");
    let mode = Arc::new(AtomicBool::new(false));
    let mode2 = mode.clone();
    let fake = Fake::start(&path, VERSION, move |r| {
        if r.event == Event::ToggleMode {
            mode2.fetch_xor(true, Ordering::SeqCst);
        }
        // An engine that claims the toggle as handled: the Shift key still goes on to the application.
        Update { handled: true, mode: if mode2.load(Ordering::SeqCst) { Mode::English } else { Mode::Chinese }, ..Update::default() }
    });
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::FocusIn);
    let r = c.event(Input::KeyDown(KeyCode::ShiftLeft, 0));
    assert!(!r.handled && !r.mode_changed);
    let r = c.event(Input::KeyUp(KeyCode::ShiftLeft, Mods::SHIFT));
    assert!(!r.handled && r.mode_changed && r.mode == Mode::English);
    assert_eq!(fake.events(), vec![(0, Event::Hello(VERSION)), (1, Event::Focus(true)), (1, Event::ToggleMode)]);
    // English mode: letters are not even sent.
    assert!(!c.event(down(b'a')).handled);
    assert_eq!(fake.events().len(), 3);
    // Shift + letter is not a tap; neither is a tap with Caps Lock on, nor one with Ctrl held.
    c.event(Input::KeyDown(KeyCode::ShiftRight, 0));
    c.event(Input::KeyDown(KeyCode::Key(Key::Letter(b'a')), Mods::SHIFT));
    c.event(Input::KeyUp(KeyCode::Key(Key::Letter(b'a')), Mods::SHIFT));
    c.event(Input::KeyUp(KeyCode::ShiftRight, Mods::SHIFT));
    c.event(Input::KeyDown(KeyCode::ShiftLeft, Mods::CAPS));
    c.event(Input::KeyUp(KeyCode::ShiftLeft, Mods::CAPS | Mods::SHIFT));
    c.event(Input::KeyDown(KeyCode::Key(Key::Other), Mods::CTRL)); // the Ctrl key itself
    c.event(Input::KeyDown(KeyCode::ShiftLeft, Mods::CTRL));
    c.event(Input::KeyUp(KeyCode::ShiftLeft, Mods::CTRL | Mods::SHIFT));
    assert_eq!(fake.events().len(), 3);
    // A press that began before the focus moved here is not a tap.
    c.event(Input::KeyDown(KeyCode::ShiftLeft, 0));
    c.event(Input::FocusOut);
    c.event(Input::FocusIn);
    c.event(Input::KeyUp(KeyCode::ShiftLeft, Mods::SHIFT));
    assert_eq!(fake.events()[3..], [(1, Event::Focus(false)), (1, Event::Focus(true))]);
    // Back to Chinese with the right Shift.
    c.event(Input::KeyDown(KeyCode::ShiftRight, 0));
    let r = c.event(Input::KeyUp(KeyCode::ShiftRight, Mods::SHIFT));
    assert!(!r.handled && r.mode_changed && r.mode == Mode::Chinese);
}

#[test]
fn no_tap_in_a_sensitive_field() {
    let path = socket("shiftpw");
    let fake = Fake::plain(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::Sensitive(true));
    c.event(Input::KeyDown(KeyCode::ShiftLeft, 0));
    c.event(Input::KeyUp(KeyCode::ShiftLeft, Mods::SHIFT));
    assert_eq!(fake.events()[1..], [(1, Event::Focus(true)), (1, Event::Password(true))]);
}

#[test]
fn focus_reset_and_pick_reach_a_live_session() {
    let path = socket("events");
    let fake = Fake::start(&path, VERSION, |r| match r.event {
        Event::Key(k) if k.key == Key::Down => Update { candidates_open: true, candidates: Some(Cands { items: vec!["你".into(), "妳".into()], ..Cands::default() }), ..composing("你") },
        Event::Key(_) => composing("你"),
        Event::CandidatePick(_) => Update { handled: true, ..composing("妳") },
        _ => Update::default(),
    });
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::FocusIn);
    c.event(Input::Reset); // nothing composing, no session
    c.event(down(b's'));
    c.event(Input::Pick(1)); // no list open
    c.event(Input::KeyDown(KeyCode::Key(Key::Down), 0));
    let r = c.event(Input::Pick(1));
    assert!(r.ui_changed && !r.ui.candidates_open);
    assert_eq!(r.ui.comp.text(), "妳");
    let r = c.event(Input::Reset);
    assert!(r.ui_changed && r.ui.comp.segments.is_empty());
    c.event(Input::Reset); // nothing was sent since the last one: nothing to forget, not sent again
    c.event(Input::FocusOut);
    c.event(Input::Sensitive(true)); // changes while unfocused: told at once
    c.event(Input::FocusIn);
    c.event(Input::Reset); // no composition, but things happened since: told, the engine forgets the earlier text
    drop(c);
    assert_eq!(fake.events()[1..], [
        (1, Event::Focus(true)), (1, key(b's')),
        (1, Event::Key(KeyEvent { key: Key::Down, mods: Mods(0), down: true })), (1, Event::CandidatePick(1)),
        (1, Event::CompositionAborted), (1, Event::Focus(false)), (1, Event::Password(true)), (1, Event::Focus(true)),
        (1, Event::CompositionAborted), (1, Event::Close),
    ]);
    let seqs: Vec<u32> = fake.requests()[1..].iter().map(|r| r.seq).collect();
    assert_eq!(seqs, (1..=10).collect::<Vec<u32>>());
}

#[test]
fn keys_the_engine_never_takes_are_not_sent() {
    let path = socket("peek");
    let fake = Fake::plain(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(down(b's'));
    let n = fake.events().len();
    for i in [Input::KeyDown(KeyCode::Key(Key::Letter(b'c')), Mods::CTRL), Input::KeyDown(KeyCode::Key(Key::Other), 0),
        Input::KeyDown(KeyCode::Key(Key::Enter), 0), Input::KeyUp(KeyCode::Key(Key::Letter(b's')), 0)] {
        assert!(!c.event(i).handled);
    }
    assert_eq!(fake.events().len(), n);
}

#[test]
fn no_engine_and_nothing_to_start_passes_every_key() {
    let path = socket("none");
    let sh = shell(&path);
    let mut c = sh.context();
    for _ in 0..20 {
        let r = c.event(down(b's'));
        assert!(!r.handled && r.no_engine && !r.ui_changed);
    }
    assert_eq!(sh.spawn_attempts(), 1, "one attempt, the next not before three seconds");
}

#[test]
fn start_attempts_wait_three_seconds_only_while_no_engine_answers() {
    let path = socket("respawn");
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(down(b's'));
    c.event(down(b's'));
    assert_eq!(sh.spawn_attempts(), 1);
    // The start runs on its own thread; it fails (nothing to start). Wait until that attempt is over.
    let until = Instant::now() + Duration::from_secs(20);
    while sh.start_pending() {
        assert!(Instant::now() < until, "the start attempt never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
    // An engine comes up and serves; when it is gone the next key starts another one without the delay.
    let fake = Fake::plain(&path);
    assert!(!c.event(down(b's')).no_engine);
    drop(fake);
    c.event(down(b's'));
    c.event(down(b's'));
    assert_eq!(sh.spawn_attempts(), 2);
}

/// Linux only: there a connect to a full queue waits. Other systems answer a full queue differently.
#[cfg(target_os = "linux")]
#[test]
fn full_connection_queue_never_holds_a_key() {
    use std::os::unix::{io::AsRawFd, net::UnixListener};
    // An engine that is stopped: its socket is there and nobody accepts. The queue has room for one connection only,
    // so the connections given up after a timeout fill it at once (the real queue holds thousands; it takes longer).
    let path = socket("full");
    let listener = UnixListener::bind(&path).unwrap();
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let sh = shell_with(&path, CALL_TIMEOUT_MS);
    let mut c = sh.context();
    for i in 0..8 {
        let t = Instant::now();
        let r = c.event(down(b's'));
        let took = t.elapsed();
        assert!(!r.handled && r.no_engine, "key {i}");
        assert!(took < Duration::from_millis(u64::from(CALL_TIMEOUT_MS) + 20), "key {i}: {took:?}");
    }
    // The queue really is full: the connect that does not wait says so, the waiting one is still waiting.
    assert_eq!(Client::connect_nowait(&path).err().map(|e| e.kind()), Some(std::io::ErrorKind::WouldBlock));
    let path2 = path.clone();
    let waiting = std::thread::spawn(move || drop(Client::connect(&path2, None, 0)));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!waiting.is_finished(), "a waiting connect would have held the key");
    assert_eq!(sh.spawn_attempts(), 0, "the engine is there: nothing is started");
    drop(listener); // the waiting connect ends with it
    let _ = waiting.join();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn indicator_click_toggles_like_a_tap() {
    let path = socket("toggle");
    let fake = Fake::start(&path, VERSION, |r| Update { mode: if r.event == Event::ToggleMode { Mode::English } else { Mode::Chinese }, ..Update::default() });
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::ToggleMode); // no session yet: nothing
    assert_eq!(fake.events(), vec![]);
    c.event(down(b's'));
    let r = c.event(Input::ToggleMode);
    assert!(r.mode_changed && r.mode == Mode::English);
    assert_eq!(fake.events().last(), Some(&(1, Event::ToggleMode)));
    let n = fake.events().len();
    // Not with Caps Lock on (as the last key event said), not in a sensitive field.
    c.event(Input::KeyDown(KeyCode::Key(Key::Other), Mods::CAPS));
    c.event(Input::ToggleMode);
    c.event(Input::KeyUp(KeyCode::Key(Key::Other), 0));
    c.event(Input::Sensitive(true));
    c.event(Input::ToggleMode);
    assert_eq!(fake.events()[n..], [(1, Event::Password(true))]);
}

#[test]
fn committable_follows_the_engine_and_needs_a_composition() {
    let path = socket("committable");
    let _fake = Fake::start(&path, VERSION, |r| match r.event {
        Event::Key(k) if k.key == Key::Letter(b's') => composing("ㄋ"),
        Event::Key(k) if k.key == Key::Letter(b'u') => Update { committable: true, ..composing("你") },
        Event::Key(_) => Update { handled: true, committable: true, ..Update::default() }, // nothing composing
        _ => Update::default(),
    });
    let sh = shell(&path);
    let mut c = sh.context();
    assert!(!c.event(down(b's')).ui.committable);
    let r = c.event(down(b'u'));
    assert!(r.ui.committable && r.ui_changed);
    let r = c.event(Input::KeyDown(KeyCode::Key(Key::Enter), 0));
    assert!(!r.ui.committable && r.ui_changed && r.ui.comp.segments.is_empty());
}

// ---- holding keys while the engine starts ----

/// A shell whose "engine" is a script that only waits `secs` seconds and ends: while it runs the start is pending,
/// as a loading engine is. The test plays the engine that comes up (a Fake on the socket) when it wants.
fn shell_starting(path: &str, secs: &str) -> Shell {
    use std::os::unix::fs::PermissionsExt;
    let script = std::env::temp_dir().join(format!("twsh-{}-{}.sh", std::process::id(), path.rsplit('/').next().unwrap()));
    std::fs::write(&script, format!("#!/bin/sh\nexec sleep {secs}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    Shell::new(Config { socket: path.to_owned(), install: Some((script, "/nonexistent".into())), ..Config::default() })
}

fn deferring(sh: &Shell) -> Context {
    let mut c = sh.context();
    c.event(Input::Client { defer: true, focus_commit: true });
    c
}

/// Asks `Release` until nothing more is released; the replies of the released keys: (handled, commit).
fn drain(c: &mut Context) -> Vec<(bool, String)> {
    let mut out = vec![];
    loop {
        let r = c.event(Input::Release);
        if !r.released {
            return out;
        }
        out.push((r.handled, r.commit.clone()));
    }
}

#[test]
fn keys_before_the_engine_answers_are_held_and_go_to_it_in_order() {
    let path = socket("hold");
    let sh = shell_starting(&path, "5");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    assert!(sh.start_pending());
    let mut held = 0;
    for i in [down(b's'), Input::KeyUp(KeyCode::Key(Key::Letter(b's')), 0), Input::KeyDown(KeyCode::ShiftLeft, 0),
              Input::KeyUp(KeyCode::ShiftLeft, 0), down(b'u'), Input::KeyDown(KeyCode::Key(Key::Enter), 0)] {
        let r = c.event(i);
        assert!(r.held && r.handled && r.holding && r.release_before == 0, "{i:?}");
        held += 1;
    }
    // Still no engine: nothing is released yet.
    let r = c.event(Input::Release);
    assert!(!r.released && r.holding && r.release_before == 0);
    // The engine comes.
    let fake = Fake::start(&path, VERSION, |r| match r.event {
        Event::Key(k) if k.key == Key::Enter => Update { handled: true, commit: Some("你".into()), ..Update::default() },
        Event::Key(_) => composing("你"),
        _ => Update::default(),
    });
    let released = drain(&mut c);
    assert_eq!(released.len(), held);
    // The key presses were the engine's; the releases and the Shift keys go back to the application.
    assert_eq!(released.iter().map(|r| r.0).collect::<Vec<_>>(), [true, false, false, false, true, true]);
    assert_eq!(released.last().unwrap().1, "你");
    assert!(!c.reply().holding);
    let keys: Vec<Event> = fake.events().into_iter().map(|e| e.1).filter(|e| matches!(e, Event::Key(_) | Event::ToggleMode)).collect();
    // The Shift press and release came one after the other: a tap, judged by the times they came, not released.
    assert_eq!(keys, [key(b's'), Event::ToggleMode, key(b'u'), Event::Key(KeyEvent { key: Key::Enter, mods: Mods(0), down: true })]);
    // From now on keys go straight to the engine.
    assert!(!c.event(down(b's')).held);
}

#[test]
fn held_keys_go_back_unchanged_after_the_limit_and_the_next_keys_are_not_held() {
    let path = socket("holdlimit");
    let sh = shell_starting(&path, "10");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    assert!(c.event(down(b's')).held);
    assert!(c.event(down(b'u')).held);
    std::thread::sleep(HOLD_LIMIT + Duration::from_millis(50));
    let r = c.event(Input::Release);
    assert!(!r.released && !r.holding && r.release_before == 2);
    // The engine is still not there: keys pass at once instead of waiting the limit each time.
    let r = c.event(down(b's'));
    assert!(!r.held && !r.handled && r.no_engine);
}

#[test]
fn held_keys_go_back_at_once_when_no_engine_is_coming() {
    let path = socket("holdgone");
    let sh = shell_starting(&path, "0.3");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    assert!(c.event(down(b's')).held);
    let until = Instant::now() + Duration::from_secs(5);
    while sh.start_pending() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    let r = c.event(Input::Release);
    assert!(!r.released && !r.holding && r.release_before == 1, "the start ended without an engine: no waiting for the limit");
}

#[test]
fn another_event_sends_the_held_keys_on_first() {
    let path = socket("holdflush");
    let sh = shell_starting(&path, "5");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    c.event(down(b's'));
    c.event(down(b'u'));
    // Telling the same sensitive state again, or the same client, changes nothing: the keys stay held.
    assert_eq!(c.event(Input::Sensitive(false)).release_before, 0);
    assert_eq!(c.event(Input::Client { defer: true, focus_commit: true }).release_before, 0);
    let r = c.event(Input::FocusOut);
    assert!(r.release_before == 2 && !r.holding);
    assert!(!c.event(Input::Release).released);
}

#[test]
fn without_defer_nothing_is_held() {
    let path = socket("nodefer");
    let sh = shell_starting(&path, "5");
    let mut c = sh.context();
    c.event(Input::FocusIn);
    let r = c.event(down(b's'));
    assert!(!r.held && !r.handled && r.no_engine);
}

#[test]
fn keys_the_engine_would_not_take_are_not_held_and_a_sensitive_field_holds_nothing() {
    let path = socket("holdpeek");
    let sh = shell_starting(&path, "5");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    for i in [Input::KeyDown(KeyCode::Modifier, 0), Input::KeyDown(KeyCode::Key(Key::Other), 0),
              Input::KeyDown(KeyCode::Key(Key::Letter(b'c')), Mods::CTRL), Input::KeyDown(KeyCode::Key(Key::Enter), 0)] {
        let r = c.event(i);
        assert!(!r.held && !r.handled, "{i:?}");
    }
    c.event(Input::Sensitive(true));
    assert!(!c.event(down(b's')).held);
}

// ---- a client that cannot be trusted with a composition on a focus loss ----

fn composing_engine(path: &str) -> Fake {
    Fake::start(path, VERSION, |r| match r.event {
        Event::Key(k) if k.key == Key::Letter(b's') => Update { committable: true, ..composing("你") },
        Event::Focus(false) => Update { commit: Some("你".into()), ..Update::default() },
        _ => Update::default(),
    })
}

#[test]
fn untrusted_client_composition_is_finished_before_a_key_that_passes() {
    let path = socket("finish");
    let fake = composing_engine(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::Client { defer: false, focus_commit: false });
    let r = c.event(down(b's'));
    assert!(r.handled && !r.ui.committable, "the client is never told it may commit");
    // A modifier key alone does not finish it (Ctrl of Ctrl+C, Alt of Alt+Tab).
    let r = c.event(Input::KeyDown(KeyCode::Modifier, 0));
    assert!(!r.handled && r.commit.is_empty() && !r.ui.comp.segments.is_empty());
    let n = fake.events().len();
    // Tab with Alt held (Alt+Tab), F6, Ctrl+C: not the engine's; the composition is committed before the key goes on.
    let r = c.event(Input::KeyDown(KeyCode::Key(Key::Other), 0));
    assert!(!r.handled && r.commit == "你" && r.ui.comp.segments.is_empty() && r.ui_changed);
    assert_eq!(fake.events()[n..], [(1, Event::Focus(false)), (1, Event::Focus(true))]);
}

#[test]
fn untrusted_client_composition_is_dropped_on_a_focus_loss() {
    let path = socket("dropfocus");
    let fake = composing_engine(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    c.event(Input::Client { defer: false, focus_commit: false });
    c.event(down(b's'));
    let n = fake.events().len();
    let r = c.event(Input::FocusOut);
    assert!(r.commit.is_empty() && r.ui.comp.segments.is_empty());
    assert_eq!(fake.events()[n..], [(1, Event::CompositionAborted), (1, Event::Focus(false))]);
}

#[test]
fn trusted_client_keeps_the_old_rules() {
    let path = socket("trusted");
    let fake = composing_engine(&path);
    let sh = shell(&path);
    let mut c = sh.context();
    assert!(c.event(down(b's')).ui.committable);
    let n = fake.events().len();
    let r = c.event(Input::KeyDown(KeyCode::Key(Key::Other), 0));
    assert!(!r.handled && r.commit.is_empty() && !r.ui.comp.segments.is_empty());
    assert_eq!(fake.events().len(), n, "a key the engine does not take is not sent and changes nothing");
    let r = c.event(Input::FocusOut);
    assert_eq!(r.commit, "你");
}

#[test]
fn a_panel_key_is_never_held_and_sends_the_held_keys_on_first() {
    let path = socket("holdpanel");
    let sh = shell_starting(&path, "5");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    assert!(c.event(down(b's')).held);
    let r = c.event(Input::PanelKey(KeyCode::Key(Key::PageDown), 0));
    assert!(r.release_before == 1 && !r.held && !r.holding, "the front end has no event to keep for a panel key");
}

#[test]
fn a_shift_press_before_a_hold_does_not_spoil_the_next_tap() {
    let path = socket("holdshift");
    let sh = shell_starting(&path, "5");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    c.event(Input::KeyDown(KeyCode::ShiftRight, Mods::SHIFT)); // not held: nothing is held yet
    assert!(c.event(Input::KeyDown(KeyCode::Key(Key::Letter(b'a')), Mods::SHIFT)).held);
    assert!(c.event(Input::KeyUp(KeyCode::ShiftRight, 0)).held);
    assert_eq!(c.event(Input::Reset).release_before, 2);
    // The engine comes; the next right Shift tap toggles.
    let fake = Fake::start(&path, VERSION, |_| Update::default());
    c.event(down(b's'));
    c.event(Input::KeyDown(KeyCode::ShiftRight, 0));
    c.event(Input::KeyUp(KeyCode::ShiftRight, Mods::SHIFT));
    assert_eq!(fake.events().last().map(|e| e.1.clone()), Some(Event::ToggleMode));
}

#[test]
fn once_the_engine_answers_the_limit_no_longer_cuts_the_release_short() {
    let path = socket("holdcross");
    let sh = shell_starting(&path, "5");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    assert!(c.event(down(b's')).held);
    assert!(c.event(down(b'u')).held);
    let _fake = Fake::start(&path, VERSION, |_| composing("你"));
    assert!(c.event(Input::Release).released);
    std::thread::sleep(HOLD_LIMIT + Duration::from_millis(50));
    let r = c.event(Input::Release);
    assert!(r.released && r.handled && r.release_before == 0 && !r.holding, "the second key goes to the engine too");
}

#[test]
fn after_a_start_that_failed_keys_are_not_held_until_an_engine_answers() {
    let path = socket("holdoff");
    let sh = shell_starting(&path, "0.3");
    let mut c = deferring(&sh);
    c.event(Input::FocusIn);
    assert!(c.event(down(b's')).held);
    let until = Instant::now() + Duration::from_secs(5);
    while sh.start_pending() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(c.event(Input::Release).release_before, 1);
    // The next start attempt (after three seconds) is pending again, but its keys pass.
    std::thread::sleep(crate::SPAWN_EVERY);
    let r = c.event(down(b's'));
    assert!(sh.start_pending() && !r.held && r.no_engine);
    // An engine answers: holding is possible again.
    let _fake = Fake::plain(&path);
    assert!(!c.event(down(b's')).no_engine);
}
