use twime_proto::*;

#[test]
fn v1_through_v6_snapshot_against_preconstruction_encoder() {
    for v in 1..=6 {
        for kind in 0..=3 {
            let options = vec!["原".into(), "新".into()];
            let hint = (kind > 0).then(|| LexuxHint { options: options.clone(), current: 1, auto: kind == 2, hide_first: kind == 3 });

            let u = Update { handled: true, composing: true, committable: true, candidates_open: true, mode: Mode::English,
                engine_us: 123, punct_half: true, composition: Some(Comp { cursor: 1, segments: vec![Segment { text: "隊".into(), locked: true, hint: 2, ..Default::default() }] }),
                candidates: Some(Cands { items: vec!["隊".into()], selected: 0, page: 2, total: 20, per_page: 9 }),
                commit: Some("甲".into()), notice: Some("乙".into()), hint,
                pop_gen: 42, completion: None, ..Default::default() };
            let bytes=Reply::Update { session:8,seq:9,update:u }.encode_for(v);
            let hex=bytes.iter().map(|b|format!("{b:02x}")).collect::<String>();
            assert_eq!(hex, include_str!("pop_legacy.hex").lines().nth((v as usize-1)*5+kind).unwrap());
        }
        let r = Request { session: 4, seq: 5, ack: Some(CommitResult { seq: 3, commit: Applied::Applied, comp: Applied::Failed }), event: Event::Key(KeyEvent::press(Key::Down)) };
        let hex=r.encode_for(v).iter().map(|b|format!("{b:02x}")).collect::<String>();
        assert_eq!(hex, include_str!("pop_legacy.hex").lines().nth((v as usize-1)*5+4).unwrap());
    }
}

#[test]
fn v7_wire_and_roundtrip() {
    for (event, tag) in [(Event::Idle(0x04030201), 18), (Event::DownCompletion(0x04030201), 19)] {
        let req = Request { session: 0, seq: 0, ack: None, event };
        assert_eq!(&req.encode_for(7)[10..], &[tag, 1, 2, 3, 4]);
        assert_eq!(Request::decode(&req.encode_for(7)), Some(req.clone()));
        assert!(req.encode_for(6).is_empty());
    }
    for q in 1..=9 {
        let h = CompletionHint { id: 42, options: (0..q).map(|_| "隊".into()).collect() };
        assert_eq!(h.labels()[0], "1.隊");
        let r = Reply::Update { session: 1, seq: 2, update: Update { pop_gen: 42, completion: Some(h), ..Default::default() } };
        let bytes = r.encode_for(7);
        assert_eq!(bytes[0], 11);
        assert_eq!(&bytes[bytes.len()-4..], &42u32.to_le_bytes());
        assert_eq!(Reply::decode(&bytes), Some(r));
        for end in 0..bytes.len() { assert!(Reply::decode(&bytes[..end]).is_none()); }
    }
}

#[test]
fn illegal_completion_wire_is_rejected() {
    let r = Reply::Update { session: 1, seq: 2, update: Update { completion: Some(CompletionHint { id: 1, options: vec!["a".into()] }), ..Default::default() } };
    let bytes = r.encode_for(7);
    // v6 fields: 9 header + 13 base + flags + toggle + notice = 25, then kind.
    let kind = bytes.len() - 13;
    assert_eq!(bytes[kind], 4);
    for count in [0, 10, 255] {
        let mut b = bytes.clone(); b[kind+5] = count;
        assert!(Reply::decode(&b).is_none());
    }
    let mut b = bytes.clone(); b[kind+6] = 0; b.remove(kind+8);
    assert!(Reply::decode(&b).is_none());
    let mut b = bytes.clone(); b[0] = 10;
    assert!(Reply::decode(&b).is_none());
    let mut b = bytes.clone(); b[kind+1..kind+5].fill(0);
    assert!(Reply::decode(&b).is_none());
}

#[test]
fn v7_ordinary_hints_keep_v6_payload_and_append_generation() {
    for kind in 0..=3 {
        let update=Update { pop_gen:123, hint:(kind!=0).then(||LexuxHint { options:vec!["原".into(),"新".into()],current:1,auto:kind==2,hide_first:kind==3 }),..Default::default() };
        let r=Reply::Update { session:3,seq:4,update };
        let old=r.encode_for(6);let new=r.encode_for(7);
        assert_eq!(&old[1..],&new[1..new.len()-4]);
        assert_eq!(&new[new.len()-4..],&123u32.to_le_bytes());
        assert_eq!(Reply::decode(&new),Some(r));
    }
}
