//! Golden bytes of protocol version 1, captured from the encoder before the N3b changes.
//! A v1 peer (old shell or old engine) must keep working: the current decoder must read these bytes, and a connection
//! that has not negotiated version 2 must produce exactly these bytes.

use twime_proto::*;

fn requests() -> Vec<Request> {
    let k = |key, mods| Event::Key(KeyEvent { key, mods: Mods(mods), down: true });
    let ack = Some(CommitResult { seq: 6, commit: Applied::Applied, comp: Applied::NotApplicable });
    vec![
        Request { session: 1, seq: 2, ack: None, event: k(Key::Letter(b'a'), 0) },
        Request { session: 7, seq: 9, ack, event: k(Key::Down, 1) },
        Request { session: 3, seq: 4, ack: None, event: Event::CandidatePick(5) },
        Request { session: 3, seq: 5, ack: None, event: Event::Focus(true) },
        Request { session: 3, seq: 6, ack: None, event: Event::Close },
        Request { session: 0, seq: 0, ack: None, event: Event::Status },
        // The other events: their branches of the encoder are unchanged by version 2, bytes taken from it after the change.
        Request { session: 3, seq: 7, ack: None, event: Event::Focus(false) },
        Request { session: 3, seq: 7, ack: None, event: Event::ExternalEdit },
        Request { session: 3, seq: 7, ack: None, event: Event::CompositionAborted },
        Request { session: 3, seq: 7, ack: None, event: Event::Unsynced },
        Request { session: 3, seq: 7, ack: None, event: Event::Password(true) },
        Request { session: 3, seq: 7, ack: None, event: Event::Ping },
        Request { session: 3, seq: 7, ack: None, event: Event::SetContext("今天".into()) },
        Request { session: 3, seq: 7, ack: None, event: Event::Shutdown },
    ]
}

fn replies() -> Vec<Reply> {
    let comp = Comp {
        segments: vec![
            Segment { text: "權".into(), locked: true, ..Default::default() },
            Segment { text: "ㄌㄧ".into(), pending: true, ..Default::default() },
        ],
        cursor: 1,
    };
    let full = Update {
        handled: true,
        commit: Some("今天".into()),
        composition: Some(comp),
        candidates: Some(Cands { items: vec!["權利".into(), "權力".into(), "力".into()], selected: 1, page: 0, ..Default::default() }),
        composing: true,
        candidates_open: true,
        committable: false,
        mode: Mode::Chinese,
        engine_us: 1234,
        ..Default::default()
    };
    vec![
        Reply::Update { session: 1, seq: 2, update: full },
        Reply::Update { session: 1, seq: 3, update: Update::default() },
        Reply::Disabled { session: 3, seq: 4 },
        Reply::Hello { version: 1 },
        Reply::Status { sessions: 3, composing: 1, abnormal: 2 },
    ]
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

const REQUESTS: &[&str] = &[
    "0101000000020000000001610001",
    "01070000000900000001060000000100010f0101",
    "01030000000400000000060500",
    "010300000005000000000201",
    "0103000000060000000008",
    "010000000000000000000b",
    "010300000007000000000200",
    "0103000000070000000003",
    "0103000000070000000004",
    "0103000000070000000005",
    "010300000007000000000701",
    "0103000000070000000009",
    "010300000007000000000a0600e4bb8ae5a4a9",
    "010300000007000000000c",
];
const REPLIES: &[&str] = &[
    "0201000000020000000101010000d2040000010600e4bb8ae5a4a90101000200010300e6ac8a020600e3848ce384a7010100000003000600e6ac8ae588a90600e6ac8ae58a9b0300e58a9b",
    "020100000003000000000000000000000000000000",
    "030300000004000000",
    "040100",
    "05030001000200",
];

#[test]
fn golden_request_bytes() {
    let rs = requests();
    for (i, r) in rs.iter().enumerate() {
        if REQUESTS.is_empty() {
            println!("REQ {}", hex(&r.encode()));
            continue;
        }
        assert_eq!(hex(&r.encode()), REQUESTS[i], "request {i} bytes changed");
        assert_eq!(Request::decode(&unhex(REQUESTS[i])).as_ref(), Some(r), "request {i} decode");
    }
}

#[test]
fn golden_reply_bytes() {
    let rs = replies();
    for (i, r) in rs.iter().enumerate() {
        if REPLIES.is_empty() {
            println!("REP {}", hex(&r.encode()));
            continue;
        }
        assert_eq!(hex(&r.encode()), REPLIES[i], "reply {i} bytes changed");
        assert_eq!(Reply::decode(&unhex(REPLIES[i])).as_ref(), Some(r), "reply {i} decode");
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}
