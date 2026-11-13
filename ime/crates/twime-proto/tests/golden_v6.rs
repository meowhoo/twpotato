use twime_proto::*;

fn sample() -> Reply {
    Reply::Update {
        session: 1,
        seq: 2,
        update: Update {
            handled: true,
            composing: true,
            committable: true,
            composition: Some(Comp {
                cursor: 3,
                segments: vec![
                    Segment {
                        text: "a".into(),
                        hint: 0,
                        ..Default::default()
                    },
                    Segment {
                        text: "b".into(),
                        hint: 1,
                        ..Default::default()
                    },
                    Segment {
                        text: "c".into(),
                        hint: 2,
                        locked: true,
                        ..Default::default()
                    },
                ],
            }),
            hint: Some(LexuxHint { options: vec!["base".into(), "next".into()], current: 1, auto: false, hide_first: false }),
            ..Default::default()
        },
    }
}
fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn version_six_update_golden() {
    let bytes = unhex(concat!(
        "0a0100000002000000", // tag, session, sequence
        "010100010000000000", // handled, composing, candidates, committable, mode, engine time
        "00",                 // no commit
        "0103000300",         // composition, cursor, segments
        "0000010061",
        "0001010062",
        "0102010063",     // flags, hint, UTF-8 string
        "00000000",       // candidates, UX flags, toggle, notice
        "010200010004006261736504006e657874" // presence, count, current, frozen options
    ));
    assert!(sample().encode_for(6) == bytes);
    assert!(Reply::decode(&bytes) == Some(sample()));
    assert!(PROTOCOL_VERSION >= 6);
}

#[test]
fn older_versions_drop_hints_without_changing_a_byte() {
    let new = sample();
    let mut old = new.clone();
    if let Reply::Update { update, .. } = &mut old {
        update.hint = None;
        for s in &mut update.composition.as_mut().unwrap().segments {
            s.hint = 0;
        }
    }
    for version in 1..=5 {
        let b = new.encode_for(version);
        assert!(b == old.encode_for(version));
        assert!(Reply::decode(&b) == Some(old.clone()));
    }
}

#[test]
fn malformed_version_six_is_rejected() {
    let bytes = sample().encode_for(6);
    for i in 0..bytes.len() {
        assert!(Reply::decode(&bytes[..i]).is_none());
    }
    let mut b = bytes.clone();
    b.push(0);
    assert!(Reply::decode(&b).is_none());
    let mut b = bytes.clone();
    b[25] = 3;
    assert!(Reply::decode(&b).is_none());
    let mut b = bytes;
    b[43] = 4;
    assert!(Reply::decode(&b).is_none());
}

#[test]
fn hidden_first_option_round_trips_as_kind_three() {
    let mut r = sample();
    if let Reply::Update { update, .. } = &mut r {
        update.hint = Some(LexuxHint { options: vec!["a".into(), "ab".into(), "abc".into()], current: 0, auto: false, hide_first: true });
    }
    let bytes = r.encode_for(6);
    assert!(bytes[43] == 3);
    let Some(Reply::Update { update, .. }) = Reply::decode(&bytes) else { panic!("decode") };
    let h = update.hint.unwrap();
    assert!(h.hide_first && !h.auto && h.options.len() == 3);
    assert!(h.labels().len() == 2 && h.labels().iter().all(|l| !l.contains("（原）")));
    // auto with a hidden first option is not a valid hint
    assert!(!LexuxHint { options: vec!["a".into(), "b".into()], current: 0, auto: true, hide_first: true }.valid());
}

#[test]
fn empty_version_six_golden() {
    let r = Reply::Update {
        session: 0,
        seq: 0,
        update: Update::default(),
    };
    let b = unhex("0a000000000000000000000000000000000000000000000000");
    assert!(r.encode_for(6) == b);
    assert!(Reply::decode(&b) == Some(r));
}

#[test]
fn invalid_cycle_count_index_and_empty_option_are_rejected() {
    let base = sample().encode_for(6);
    let start = base.len() - 17;
    for (offset, value) in [(1, 1), (3, 2), (5, 0)] {
        let mut b = base.clone(); b[start + offset] = value;
        assert!(Reply::decode(&b).is_none());
    }
}

#[test]
fn automatic_hint_wire_tag_preserves_frozen_provenance_and_older_bytes() {
    let ordinary = sample();
    let mut automatic = ordinary.clone();
    if let Reply::Update { update, .. } = &mut automatic {
        update.hint.as_mut().unwrap().auto = true;
    }
    let baseline = ordinary.encode_for(6);
    let bytes = automatic.encode_for(6);
    assert!(bytes.len() == baseline.len());
    assert!(bytes.iter().zip(&baseline).filter(|(a, b)| a != b).count() == 1);
    assert!(Reply::decode(&bytes) == Some(automatic.clone()));
    for i in 0..bytes.len() {
        assert!(Reply::decode(&bytes[..i]).is_none());
    }
    for version in 1..=5 {
        assert!(ordinary.encode_for(version) == automatic.encode_for(version));
    }
}

fn check_labels(auto: bool, count: usize) {
    let mut hint = LexuxHint {
        options: (1..=count).map(|n| n.to_string()).collect(), current: 0, auto, hide_first: false,
    };
    for current in (0..count).cycle().take(count * 3) {
        hint.current = current;
        let labels = hint.labels();
        for (i, label) in labels.iter().enumerate() {
            assert!(label.contains("（原）") == (i == usize::from(auto)));
            assert!(label.contains("（目前）") == (i == current));
            assert!(label.starts_with(&format!("\\ {}", hint.options[i])));
        }
        assert!(hint.text() == format!("按反斜線切換：{}", labels.join("　")));
    }
}

#[test]
fn ordinary_original_marker_stays_at_zero_while_current_cycles() {
    check_labels(false, 3);
}

#[test]
fn automatic_original_marker_stays_at_one_while_current_cycles() {
    check_labels(true, 2);
}
