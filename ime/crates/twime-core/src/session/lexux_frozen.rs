//! Numeric wire snapshots, replayable against the pre-LEXUX session source.
use super::*;

pub fn snapshots() -> (Vec<u8>, usize) {
    let data = "ㄚ 一 -1\nㄚ 乙 -2\nㄧ 丁 -1\nㄗ 子 -1\n";
    let mut bytes = vec![];
    let mut frames = 0;
    let cases: Vec<Vec<KeyEvent>> = vec![
        vec![
            Key::Letter(b'y'),
            Key::Space,
            Key::Letter(b'y'),
            Key::Space,
            Key::Backslash,
        ],
        vec![
            Key::Letter(b'y'),
            Key::Space,
            Key::Down,
            Key::Esc,
            Key::Backslash,
        ],
        vec![
            Key::Letter(b'y'),
            Key::Space,
            Key::Down,
            Key::Digit(2),
            Key::Enter,
        ],
        vec![
            Key::Letter(b'y'),
            Key::Space,
            Key::Home,
            Key::Letter(b'u'),
            Key::Space,
            Key::Delete,
            Key::End,
            Key::Backspace,
        ],
        vec![
            Key::Letter(b'y'),
            Key::Space,
            Key::Letter(b'b'),
            Key::Enter,
            Key::Backslash,
            Key::Esc,
        ],
        vec![Key::Backquote, Key::Right, Key::Esc, Key::Backslash],
    ]
    .into_iter()
    .map(|v| v.into_iter().map(KeyEvent::press).collect())
    .chain([
        vec![
            KeyEvent::press(Key::Letter(b'y')),
            KeyEvent::press(Key::Space),
            KeyEvent {
                key: Key::Backslash,
                mods: twime_proto::Mods(twime_proto::Mods::SHIFT),
                down: true,
            },
        ],
        vec![
            KeyEvent::press(Key::Letter(b'y')),
            KeyEvent::press(Key::Space),
            KeyEvent {
                key: Key::Backslash,
                mods: twime_proto::Mods(twime_proto::Mods::CAPS),
                down: true,
            },
        ],
        vec![
            KeyEvent {
                key: Key::Letter(b'a'),
                mods: twime_proto::Mods(twime_proto::Mods::SHIFT),
                down: true,
            },
            KeyEvent::press(Key::Backslash),
            KeyEvent::press(Key::Enter),
        ],
    ])
    .collect();
    for ux in [false, true] {
        for events in &cases {
            let a = Arc::new(Assets::from_dictionary_text(data, "").unwrap());
            let mut s = Engine::new(a, Config::default()).new_session();
            s.set_ux(ux);
            #[cfg(test)]
            s.set_lexux_peer(true);
            for ev in events {
                let u = s.on_key(*ev);
                for version in 1..=5 {
                    let b = twime_proto::Reply::Update {
                        session: 1,
                        seq: 1,
                        update: u.clone(),
                    }
                    .encode_for(version);
                    bytes.extend((b.len() as u32).to_le_bytes());
                    bytes.extend(b);
                    frames += 1;
                }
            }
        }
    }
    (bytes, frames)
}

#[test]
fn missing_keys_match_pre_lexux_session_wire_bytes() {
    let (bytes, frames) = snapshots();
    assert!(frames == 420);
    assert!(bytes.as_slice() == include_bytes!("../../tests/fixtures/lexux-session-pre.bin"));
}
