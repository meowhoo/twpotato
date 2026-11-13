use twime_proto::*;
fn entry() -> LexEntry {
    LexEntry {
        word: "志明".into(),
        reading: "ㄓˋ ㄇㄧㄥˊ".into(),
        weight: Some(12.0),
    }
}
fn reqs() -> Vec<UserReq> {
    vec![
        UserReq::GetSettings,
        UserReq::SetSettings(UserSettings {
            toggle_key: ToggleKey::RightShift,
            default_mode: Mode::English,
            punct_half: true,
            symbol_table: false,
            cache: true,
        }),
        UserReq::List {
            query: "志".into(),
            offset: 3,
            limit: 80,
        },
        UserReq::Add(vec![entry()]),
        UserReq::Remove(vec![("志明".into(), "ㄓˋ ㄇㄧㄥˊ".into())]),
        UserReq::ReadingOptions("志明".into()),
    ]
}
fn errors() -> Vec<LexError> {
    vec![
        LexError::Length,
        LexError::UnknownChar(2),
        LexError::IllegalSyllable(1),
        LexError::IllegalChar(0),
        LexError::Weight,
        LexError::Full,
        LexError::Io,
        LexError::Busy,
        LexError::Disabled,
        LexError::BadReading,
    ]
}

#[test]
fn reading_capacity_rejection_keeps_batch_index_and_full_code() {
    let reply = Reply::User(UserReply::Added {
        added: 32, updated: 1, rejected: vec![(32, LexError::Full)],
    });
    assert_eq!(Reply::decode(&reply.encode_for(5)), Some(reply));
}
fn replies() -> Vec<Reply> {
    let mut v = vec![
        Reply::HelloCaps {
            version: 5,
            caps: CAP_DOC_TEXT,
        },
        Reply::Update {
            session: 1,
            seq: 2,
            update: Update {
                toggle_key: ToggleKey::LeftShift,
                notice: Some("已送出".into()),
                ..Default::default()
            },
        },
    ];
    let mut user = vec![
        UserReply::Settings {
            settings: UserSettings::default(),
            file_error: true,
            last_error: Some("讀取失敗".into()),
        },
        UserReply::Settings {
            settings: UserSettings::default(),
            file_error: false,
            last_error: None,
        },
        UserReply::Page {
            total: 10,
            offset: 3,
            entries: vec![
                entry(),
                LexEntry {
                    weight: None,
                    ..entry()
                },
            ],
            invalid_lines: 4,
        },
        UserReply::Added {
            added: 1,
            updated: 2,
            rejected: errors()
                .into_iter()
                .enumerate()
                .map(|(i, e)| (i as u32, e))
                .collect(),
        },
        UserReply::Removed(4),
        UserReply::Readings {
            default: Some("ㄓˋ ㄇㄧㄥˊ".into()),
            per_char: vec![vec!["ㄓˋ".into()], vec![]],
        },
        UserReply::Readings {
            default: None,
            per_char: vec![],
        },
    ];
    user.extend(errors().into_iter().map(UserReply::Error));
    v.extend(user.into_iter().map(Reply::User));
    v
}
#[test]
fn v5_roundtrips_and_all_truncations() {
    for event in reqs()
        .into_iter()
        .map(Event::User)
        .chain([Event::AddToLexicon])
    {
        let req = Request {
            session: 0,
            seq: 9,
            ack: None,
            event,
        };
        let b = req.encode();
        assert_eq!(Request::decode(&b), Some(req));
        for n in 0..b.len() {
            assert_eq!(Request::decode(&b[..n]), None, "request truncated at {n}");
        }
        let mut extra = b;
        extra.push(0);
        assert_eq!(Request::decode(&extra), None);
    }
    for rep in replies() {
        let b = rep.encode_for(5);
        assert_eq!(Reply::decode(&b), Some(rep));
        for n in 0..b.len() {
            assert_eq!(Reply::decode(&b[..n]), None, "reply truncated at {n}");
        }
        let mut extra = b;
        extra.push(0);
        assert_eq!(Reply::decode(&extra), None);
    }
}
#[test]
fn old_updates_drop_v5_fields_byte_for_byte() {
    for version in 1..=4 {
        let base = Update {
            handled: true,
            commit: Some("明".into()),
            punct_half: true,
            ..Default::default()
        };
        let new = Update {
            toggle_key: ToggleKey::None,
            notice: Some("notice".into()),
            ..base.clone()
        };
        let wrap = |update| Reply::Update {
            session: 11,
            seq: 13,
            update,
        };
        assert_eq!(
            wrap(base).encode_for(version),
            wrap(new).encode_for(version)
        );
    }
}
#[test]
fn encoded_entry_length_matches_wire() {
    for weight in [None, Some(10.0)] {
        let e = LexEntry { weight, ..entry() };
        let rep = Reply::User(UserReply::Page {
            total: 1,
            offset: 0,
            entries: vec![e.clone()],
            invalid_lines: 0,
        });
        assert_eq!(rep.encode_for(5).len(), 18 + encoded_len_entry(&e));
    }
}
#[test]
fn illegal_enums_and_deterministic_fuzz() {
    assert!(Reply::decode(&[9, 5, 10]).is_none());
    let mut b = Reply::User(UserReply::Settings {
        settings: UserSettings::default(),
        file_error: false,
        last_error: None,
    })
    .encode_for(5);
    for offset in 2..=7 {
        let old = b[offset];
        b[offset] = 255;
        assert!(Reply::decode(&b).is_none());
        b[offset] = old;
    }
    let mut state = 0x3141592653589793u64;
    for _ in 0..30_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let mut b: Vec<_> = (0..(state as usize % 160))
            .map(|i| (state >> ((i % 8) * 8)) as u8)
            .collect();
        if !b.is_empty() {
            b[0] = [1, 7, 8, 9][state as usize % 4];
        }
        let _ = Reply::decode(&b);
        let _ = Request::decode(&b);
    }
}
