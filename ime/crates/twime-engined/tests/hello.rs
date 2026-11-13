#![cfg(windows)]
//! Version negotiation: a connection that never sends `Hello` gets the version 1 bytes, a
//! connection that negotiates version 2 gets the candidate totals.

use std::sync::Arc;

use twime_core::{Config, Engine, keymap::key_of};
use twime_engined::{Options, runlock::RunLock, serve};
use twime_model::Assets;
use twime_proto::{Cands, Event, Key, KeyEvent, PROTOCOL_VERSION, Reply, Request};
use twime_winpipe::{Client, PipeServer};

const DICT: &str = "\
ㄑㄩㄢˊ 全 -3
ㄑㄩㄢˊ 權 -3.5
ㄌㄧˋ 力 -2.5
ㄌㄧˋ 利 -2.6
ㄌㄧˋ 立 -2.7
ㄑㄩㄢˊ-ㄌㄧˋ 權力 -4.13
ㄑㄩㄢˊ-ㄌㄧˋ 權利 -4.134
";

fn start(tag: &str) -> String {
    start_with(tag, Options::default())
}

fn start_with(tag: &str, opts: Options) -> String {
    let name = format!("twime-hello-{tag}-{}", std::process::id());
    let assets = Assets::from_dictionary_text(DICT, "").unwrap();
    let engine = Engine::new(Arc::new(assets), Config { lambda: 0.0, candidates: true, ..Config::default() });
    let server = PipeServer::with_name(Some(name.clone())).unwrap();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("hello-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lock = RunLock::acquire(&dir, "test-logon".into()).unwrap();
    std::thread::spawn(move || {
        let _ = serve(engine, server, opts, lock);
    });
    name
}

fn call(c: &Client, session: u32, seq: u32, event: Event) -> Reply {
    let bytes = c.call(&Request { session, seq, ack: None, event }.encode(), 2000).unwrap();
    Reply::decode(&bytes).expect("a reply the current decoder accepts")
}

fn press(c: &Client, seq: &mut u32, key: Key) -> Reply {
    *seq += 1;
    call(c, 1, *seq, Event::Key(KeyEvent::press(key)))
}

/// Types ㄑㄩㄢˊㄌㄧˋ and opens the candidates; returns the candidate part of the last reply.
fn open_candidates(c: &Client) -> Cands {
    let mut seq = 0;
    for ch in "ㄑㄩㄢˊㄌㄧˋ".chars() {
        press(c, &mut seq, key_of(ch).unwrap());
    }
    match press(c, &mut seq, Key::Down) {
        Reply::Update { update, .. } => update.candidates.expect("candidates open"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_connection_without_hello_gets_version_1_bytes() {
    let name = start("v1");
    let c = Client::connect(&name, None, 500).unwrap();
    let cands = open_candidates(&c);
    assert_eq!((cands.total, cands.per_page), (0, 0));
    assert!(cands.items.iter().any(|x| x == "權力"));
    assert!(cands.items.iter().any(|x| x == "權利"));
}

#[test]
fn hello_2_enables_totals_and_hello_1_does_not() {
    let name = start("v2");
    let c = Client::connect(&name, None, 500).unwrap();
    assert_eq!(call(&c, 0, 0, Event::Hello(2)), Reply::Hello { version: 2 });
    let cands = open_candidates(&c);
    assert!(cands.total as usize >= cands.items.len() && cands.per_page == 9, "{cands:?}");
    // A newer shell gets version 5 with document access disabled.
    let c2 = Client::connect(&name, None, 500).unwrap();
    assert_eq!(call(&c2, 0, 0, Event::Hello(9)), Reply::HelloCaps { version: PROTOCOL_VERSION, caps: 0 });
    let c3 = Client::connect(&name, None, 500).unwrap();
    assert_eq!(call(&c3, 0, 0, Event::Hello(1)), Reply::Hello { version: 1 });
    assert_eq!(open_candidates(&c3).total, 0);
}

#[test]
fn the_version_is_fixed_by_the_first_session_event() {
    let name = start("fixed");
    let c = Client::connect(&name, None, 500).unwrap();
    assert_eq!(call(&c, 0, 0, Event::Hello(1)), Reply::Hello { version: 1 });
    let mut seq = 0;
    press(&c, &mut seq, Key::Letter(b'q'));
    // Even after the session is closed a later Hello cannot raise the version.
    call(&c, 1, 99, Event::Close);
    assert_eq!(call(&c, 0, 0, Event::Hello(2)), Reply::Hello { version: 1 });
    assert_eq!(open_candidates(&c).total, 0);
}

fn update_of(r: Reply) -> twime_proto::Update {
    match r {
        Reply::Update { update, .. } => update,
        other => panic!("{other:?}"),
    }
}

fn key_with(c: &Client, seq: &mut u32, key: Key, mods: u8) -> twime_proto::Update {
    *seq += 1;
    update_of(call(c, 1, *seq, Event::Key(KeyEvent { key, mods: twime_proto::Mods(mods), down: true })))
}

#[test]
fn hello_3_turns_on_english_runs_punctuation_and_mode_toggle() {
    let name = start("v3");
    let c = Client::connect(&name, None, 500).unwrap();
    assert_eq!(call(&c, 0, 0, Event::Hello(3)), Reply::Hello { version: 3 });
    let mut seq = 0;
    let u = key_with(&c, &mut seq, Key::Letter(b'y'), twime_proto::Mods::SHIFT);
    assert!(u.handled && u.run && u.composing, "{u:?}");
    key_with(&c, &mut seq, Key::Letter(b'o'), 0);
    let u = key_with(&c, &mut seq, Key::Comma, twime_proto::Mods::SHIFT);
    assert_eq!(u.commit.as_deref(), Some("Yo，"));
    seq += 1;
    let u = update_of(call(&c, 1, seq, Event::ToggleMode));
    assert_eq!(u.mode, twime_proto::Mode::English);
    // Version 3 numbers on the number pad.
    let u = key_with(&c, &mut seq, Key::NumpadDigit(3), 0);
    assert!(!u.handled, "English mode: nothing is handled");
}

#[test]
fn older_connections_never_see_the_new_behaviour() {
    for (i, hello) in [None, Some(1u16), Some(2)].into_iter().enumerate() {
        let name = start(&format!("old{i}"));
        let c = Client::connect(&name, None, 500).unwrap();
        if let Some(v) = hello {
            assert_eq!(call(&c, 0, 0, Event::Hello(v)), Reply::Hello { version: v });
        }
        let mut seq = 0;
        let u = key_with(&c, &mut seq, Key::Letter(b'y'), twime_proto::Mods::SHIFT);
        assert!(!u.handled && !u.run && !u.composing, "{hello:?}: {u:?}");
        seq += 1;
        let u = update_of(call(&c, 1, seq, Event::ToggleMode));
        assert_eq!(u.mode, twime_proto::Mode::Chinese);
        let u = key_with(&c, &mut seq, Key::Backquote, 0);
        assert!(!u.handled);
        // Bytes are the ones of that version: a tag 2 update, which the current decoder reads with the new fields off.
        let bytes = Request { session: 1, seq: 99, ack: None, event: Event::Key(KeyEvent::press(Key::Letter(b'q'))) }.encode();
        let raw = c.call(&bytes, 2000).unwrap();
        assert_eq!(raw[0], 2);
    }
}

#[test]
fn the_mode_belongs_to_the_session_not_to_the_connection() {
    let name = start("modes");
    let c = Client::connect(&name, None, 500).unwrap();
    assert_eq!(call(&c, 0, 0, Event::Hello(3)), Reply::Hello { version: 3 });
    // Session 1 goes English; session 2 on the same connection is untouched, and so is a later key in each.
    let u = update_of(call(&c, 1, 1, Event::ToggleMode));
    assert_eq!(u.mode, twime_proto::Mode::English);
    let u = update_of(call(&c, 2, 1, Event::Ping));
    assert_eq!(u.mode, twime_proto::Mode::Chinese);
    let u = update_of(call(&c, 2, 2, Event::Key(KeyEvent::press(Key::Letter(b'q')))));
    assert!(u.handled && u.composing, "session 2 is still in Chinese mode");
    let u = update_of(call(&c, 1, 2, Event::Key(KeyEvent::press(Key::Letter(b'q')))));
    assert!(!u.handled && !u.composing, "session 1 is in English mode");
    // A focus round trip and an external edit leave the mode alone; closing the session forgets it.
    for e in [Event::Focus(false), Event::Focus(true), Event::ExternalEdit] {
        assert_eq!(update_of(call(&c, 1, 3, e)).mode, twime_proto::Mode::English);
    }
    call(&c, 1, 4, Event::Close);
    assert_eq!(update_of(call(&c, 1, 5, Event::Ping)).mode, twime_proto::Mode::Chinese, "a new session starts at the default");
}

/// Version 4 only when the assets use the document cache; `DocText` on a lower version is a
/// protocol error (the connection is dropped), on version 4 it is an ordinary event.
#[test]
fn version_4_only_with_the_document_cache() {
    let name = start_with("v4", Options { doc_text: true, ..Options::default() });
    let c = Client::connect(&name, None, 500).unwrap();
    assert_eq!(call(&c, 0, 0, Event::Hello(4)), Reply::Hello { version: 4 });
    assert!(matches!(call(&c, 1, 1, Event::DocText("全力".into())), Reply::Update { .. }));
    let c5 = Client::connect(&name, None, 500).unwrap();
    assert_eq!(call(&c5, 0, 0, Event::Hello(PROTOCOL_VERSION)), Reply::HelloCaps { version: PROTOCOL_VERSION, caps: twime_proto::CAP_DOC_TEXT });
    assert!(matches!(call(&c5, 1, 1, Event::DocText("全力".into())), Reply::Update { .. }));
    let name3 = start("v3doc");
    let c3 = Client::connect(&name3, None, 500).unwrap();
    assert_eq!(call(&c3, 0, 0, Event::Hello(4)), Reply::Hello { version: 3 });
    let r = c3.call(&Request { session: 1, seq: 1, ack: None, event: Event::DocText("全力".into()) }.encode(), 2000);
    assert!(r.is_err(), "DocText on a version 3 connection must drop it");
    for event in [Event::User(twime_proto::UserReq::GetSettings), Event::AddToLexicon] {
        let c4 = Client::connect(&name, None, 500).unwrap();
        assert_eq!(call(&c4, 0, 0, Event::Hello(4)), Reply::Hello { version: 4 });
        assert!(c4.call(&Request { session: 0, seq: 1, ack: None, event }.encode(), 2000).is_err());
    }
}

#[test]
fn v7_idle_missing_session_and_unnegotiated_events_never_create_or_change_sessions() {
    let name = start("pop");
    for version in [1, 2, 3, 5, 6, 7] {
        let c = Client::connect(&name, None, 500).unwrap();
        call(&c, 0, 0, Event::Hello(version));
        let before = call(&c, 0, 1, Event::Status);
        let u = update_of(call(&c, 42, 1, Event::Idle(1)));
        assert!(!u.handled && u.completion.is_none() && !u.composing);
        assert_eq!(before, call(&c, 0, 2, Event::Status));
        if version < 7 {
            let u = update_of(call(&c, 42, 2, Event::DownCompletion(1)));
            assert!(!u.handled && !u.composing);
            assert_eq!(before, call(&c, 0, 3, Event::Status));
            call(&c, 1, 3, Event::Key(KeyEvent::press(Key::Letter(b'q'))));
            let prior = update_of(call(&c, 1, 4, Event::Ping));
            call(&c, 1, 5, Event::Idle(123));
            call(&c, 1, 6, Event::DownCompletion(123));
            let after = update_of(call(&c, 1, 7, Event::Ping));
            assert_eq!(prior.composition, after.composition);
            assert_eq!(prior.candidates, after.candidates);
            call(&c, 1, 8, Event::Close);
        }
    }
    let c=Client::connect(&name,None,500).unwrap();
    call(&c,0,0,Event::Hello(7));
    assert!(update_of(call(&c,1,1,Event::Key(KeyEvent::press(Key::Letter(b'q'))))).composing);
    let req=Request { session:1,seq:2,ack:Some(twime_proto::CommitResult { seq:1,commit:twime_proto::Applied::NotApplicable,comp:twime_proto::Applied::Failed }),event:Event::Idle(1) };
    let reply=Reply::decode(&c.call(&req.encode_for(7),2000).unwrap()).unwrap();
    assert!(!update_of(reply).composing);
    assert!(matches!(call(&c,0,3,Event::Status),Reply::Status { composing:0,.. }));
    call(&c,1,4,Event::Close);
}
