//! Protocol version 3: golden bytes of the new encodings, and proof that the
//! version 1 / 2 encodings are unchanged when the new fields are present in memory (they are dropped, not encoded).

use twime_proto::*;

fn v3_requests() -> Vec<Request> {
    let k = |key, mods| Event::Key(KeyEvent { key, mods: Mods(mods), down: true });
    vec![
        Request { session: 1, seq: 2, ack: None, event: k(Key::Backquote, 0) },
        Request { session: 1, seq: 3, ack: None, event: k(Key::LBracket, Mods::SHIFT) },
        Request { session: 1, seq: 4, ack: None, event: k(Key::RBracket, 0) },
        Request { session: 1, seq: 5, ack: None, event: k(Key::Quote, Mods::SHIFT) },
        Request { session: 1, seq: 6, ack: None, event: k(Key::Backslash, 0) },
        Request { session: 1, seq: 7, ack: None, event: k(Key::Equals, Mods::SHIFT) },
        Request { session: 1, seq: 8, ack: None, event: k(Key::NumpadDigit(9), 0) },
        Request { session: 1, seq: 9, ack: None, event: k(Key::NumpadDecimal, 0) },
        Request { session: 1, seq: 10, ack: None, event: k(Key::NumpadOp(3), 0) },
        Request { session: 1, seq: 11, ack: None, event: k(Key::Letter(b'a'), Mods::CAPS | Mods::SHIFT) },
        Request { session: 1, seq: 12, ack: None, event: Event::ToggleMode },
        Request { session: 0, seq: 0, ack: None, event: Event::Hello(3) },
    ]
}

fn update_v3() -> Update {
    Update {
        handled: true,
        commit: Some("You，".into()),
        composition: Some(Comp { segments: vec![Segment { text: "Ab".into(), run: true, ..Default::default() }], cursor: 2 }),
        candidates: Some(Cands { items: vec!["<".into(), ">".into()], selected: 0, page: 0, total: 9, per_page: 9 }),
        composing: true,
        candidates_open: true,
        committable: true,
        mode: Mode::English,
        engine_us: 77,
        punct_half: true,
        run: true,
        symbols: true,
        ..Default::default()
    }
}

fn v3_replies() -> Vec<Reply> {
    vec![
        Reply::Update { session: 1, seq: 2, update: update_v3() },
        Reply::Update { session: 1, seq: 3, update: Update::default() },
        Reply::Update { session: 1, seq: 4, update: Update { run: true, composing: true, ..Default::default() } },
    ]
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

const REQUESTS: &[&str] = &[
    "0101000000020000000001140001",
    "0101000000030000000001150101",
    "0101000000040000000001160001",
    "0101000000050000000001170101",
    "0101000000060000000001180001",
    "0101000000070000000001190101",
    "0101000000080000000001270001",
    "01010000000900000000011a0001",
    "01010000000a00000000012b0001",
    "01010000000b0000000001611101",
    "01010000000c000000000e",
    "010000000000000000000d0300",
];
const REPLIES: &[&str] = &[
    "06010000000200000001010101014d000000010600596f75efbc8c010200010008020041620200000000020001003c01003e09000907",
    "06010000000300000000000000000000000000000000",
    "06010000000400000000010000000000000000000002",
];

#[test]
fn golden_v3_request_bytes() {
    for (i, r) in v3_requests().iter().enumerate() {
        if REQUESTS.is_empty() {
            println!("REQ {}", hex(&r.encode()));
            continue;
        }
        let want = REQUESTS[i];
        assert_eq!(hex(&r.encode()), want, "request {i} bytes changed");
        assert_eq!(Request::decode(&unhex(want)).as_ref(), Some(r), "request {i} decode");
    }
}

#[test]
fn golden_v3_reply_bytes() {
    for (i, r) in v3_replies().iter().enumerate() {
        if REPLIES.is_empty() {
            println!("REP {}", hex(&r.encode_for(3)));
            continue;
        }
        assert_eq!(hex(&r.encode_for(3)), REPLIES[i], "reply {i} bytes changed");
        assert_eq!(Reply::decode(&unhex(REPLIES[i])).as_ref(), Some(r), "reply {i} decode");
    }
}

/// A version 1 or 2 connection gets exactly the bytes the encoder gave before: the new fields are dropped.
#[test]
fn older_versions_encode_without_the_new_fields() {
    let full = update_v3();
    let mut old = full.clone();
    old.punct_half = false;
    old.run = false;
    old.symbols = false;
    if let Some(c) = old.composition.as_mut() {
        for s in &mut c.segments {
            s.run = false;
        }
    }
    for v in [1u16, 2] {
        let a = Reply::Update { session: 1, seq: 2, update: full.clone() }.encode_for(v);
        let b = Reply::Update { session: 1, seq: 2, update: old.clone() }.encode_for(v);
        assert_eq!(a, b, "version {v}");
        assert_eq!(a[0], 2, "version {v} keeps the message tag");
    }
    assert_eq!(Reply::Update { session: 1, seq: 2, update: full }.encode_for(3)[0], 6);
    // A version 1 / 2 decoder (tag 2) refuses a run flag in a segment; version 3 data never arrives there.
    let v3 = Reply::Update { session: 1, seq: 2, update: update_v3() }.encode_for(3);
    let mut as_old = v3.clone();
    as_old[0] = 2;
    assert_eq!(Reply::decode(&as_old), None);
}

#[test]
fn v3_round_trip_and_truncation() {
    for r in v3_replies() {
        assert_eq!(Reply::decode(&r.encode_for(3)), Some(r.clone()));
        let b = r.encode_for(3);
        for n in 0..b.len() {
            assert_eq!(Reply::decode(&b[..n]), None, "truncated to {n}");
        }
        let mut extra = b.clone();
        extra.push(0);
        assert_eq!(Reply::decode(&extra), None);
    }
    for r in v3_requests() {
        assert_eq!(Request::decode(&r.encode()), Some(r.clone()));
        let b = r.encode();
        for n in 0..b.len() {
            assert_eq!(Request::decode(&b[..n]), None, "truncated to {n}");
        }
    }
}

#[test]
fn bad_v3_values_are_refused() {
    // Mods beyond the five defined bits, an unknown key code, a segment flag above 15, trailing flags above 7.
    let base = Request { session: 1, seq: 2, ack: None, event: Event::Key(KeyEvent::press(Key::Backquote)) }.encode();
    let mut b = base.clone();
    let n = b.len();
    b[n - 2] = 32;
    assert_eq!(Request::decode(&b), None);
    let mut b = base.clone();
    b[n - 3] = 27;
    assert_eq!(Request::decode(&b), None);
    let mut b = base;
    b[n - 3] = 44;
    assert_eq!(Request::decode(&b), None);
    let r = Reply::Update { session: 1, seq: 2, update: update_v3() }.encode_for(3);
    let mut b = r.clone();
    let last = b.len() - 1;
    b[last] = 8;
    assert_eq!(Reply::decode(&b), None);
}

#[test]
fn key_codes_of_version_3_are_distinct_and_round_trip() {
    let mut seen = std::collections::HashSet::new();
    let mut keys = vec![Key::Backquote, Key::LBracket, Key::RBracket, Key::Quote, Key::Backslash, Key::Equals, Key::NumpadDecimal];
    keys.extend((0..10).map(Key::NumpadDigit));
    keys.extend((0..4).map(Key::NumpadOp));
    for k in keys {
        assert!(seen.insert(k.code()), "{k:?}");
        assert_eq!(Key::from_code(k.code()), Some(k));
        assert!(k.code() >= 20 && !(48..=57).contains(&k.code()) && !(97..=122).contains(&k.code()));
    }
    // The version 1/2 codes are untouched (0..=19, '0'..='9', 'a'..='z').
    assert_eq!(Key::Other.code(), 0);
    assert_eq!(Key::PageDown.code(), 19);
}
