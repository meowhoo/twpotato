use super::*;
use crate::keymap::key_of;

const DICT: &str = "\
ㄐㄧㄣ 今 -2
ㄐㄧㄣ 金 -2.5
ㄊㄧㄢ 天 -2
ㄊㄧㄢ 添 -3
ㄍㄨㄛˊ 國 -2
ㄍㄨㄛˊ 郭 -3
ㄇㄧㄣˊ 民 -2
ㄉㄜ˙ 的 -1
ㄅㄚ 八 -2
";

fn engine(candidates: bool) -> Arc<Engine> {
    let assets = Assets::from_dictionary_text(DICT, "").unwrap();
    Engine::new(Arc::new(assets), Config { lambda: 1.0, candidates, page_size: 2, ..Config::default() })
}

fn sess() -> Session {
    engine(true).new_session()
}

/// Types a string of bopomofo symbols; ' ' is Space.
fn type_str(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for c in text.chars() {
        let key = if c == ' ' { Key::Space } else { key_of(c).unwrap_or_else(|| panic!("no key for {c}")) };
        last = s.on_key(KeyEvent::press(key));
    }
    last
}

fn key(s: &mut Session, k: Key) -> Update {
    s.on_key(KeyEvent::press(k))
}

fn text(u: &Update) -> String {
    u.composition.as_ref().map(Comp::text).unwrap_or_default()
}

#[test]
fn types_converts_and_commits() {
    let mut s = sess();
    let u = type_str(&mut s, "ㄐㄧㄣ ");
    assert!(u.handled && u.composing && text(&u) == "今");
    let u = type_str(&mut s, "ㄊㄧㄢ ");
    assert_eq!(text(&u), "今天");
    assert!(u.committable);
    let u = key(&mut s, Key::Enter);
    assert!(u.handled);
    assert_eq!(u.commit.as_deref(), Some("今天"));
    assert!(!u.composing && u.composition.is_none());
    assert_eq!(s.context(), "今天");
}

#[test]
fn tone_marks_complete_and_neutral_tone() {
    let mut s = sess();
    assert_eq!(text(&type_str(&mut s, "ㄍㄨㄛˊ")), "國");
    assert_eq!(text(&type_str(&mut s, "ㄉㄜ˙")), "國的");
    // Neutral tone typed first, then the body, completed by Space.
    assert_eq!(text(&type_str(&mut s, "˙ㄉㄜ ")), "國的的");
    // Order within a syllable is free: final before initial.
    assert_eq!(text(&type_str(&mut s, "ㄚㄅ ")), "國的的八");
}

#[test]
fn pending_bopomofo_is_shown_and_backspace_removes_last_slot() {
    let mut s = sess();
    let u = type_str(&mut s, "ㄐㄧㄣ");
    assert_eq!(text(&u), "ㄐㄧㄣ");
    assert!(u.composition.as_ref().unwrap().segments[0].pending);
    assert!(!u.committable);
    assert_eq!(text(&key(&mut s, Key::Backspace)), "ㄐㄧ");
    assert_eq!(text(&key(&mut s, Key::Backspace)), "ㄐ");
    let u = key(&mut s, Key::Backspace);
    assert!(!u.composing && u.handled);
    // Same slot replaces: initial typed twice.
    assert_eq!(text(&type_str(&mut s, "ㄐㄊ")), "ㄊ");
}

#[test]
fn illegal_syllable_is_kept_and_enter_commits_it_as_typed() {
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ");
    let u = type_str(&mut s, "ㄅㄧㄤ "); // not a syllable (a lone symbol and Space stays as typed, ZHUYIN-OUT 3.5)
    let c = u.composition.as_ref().unwrap();
    assert!(c.segments[1].unconverted && c.segments[1].text == "ㄅㄧㄤ");
    assert!(!u.committable);
    // NEUTRAL: Enter commits what is shown, the syllable as its typed bopomofo (it used to block the composition)
    let u = key(&mut s, Key::Enter);
    assert!(u.handled && u.commit.as_deref() == Some("今ㄅㄧㄤ") && !u.composing);
    assert_eq!(s.context(), "今ㄅㄧㄤ");
    // Backspace still removes it; then the rest commits.
    type_str(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
    key(&mut s, Key::Backspace);
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("今"));
}

#[test]
fn esc_cancels_everything_and_enter_ignores_pending_symbols() {
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ㄊ");
    let u = key(&mut s, Key::Enter);
    assert!(u.handled && u.commit.is_none() && u.composing);
    let u = key(&mut s, Key::Esc);
    assert!(u.handled && !u.composing && u.commit.is_none());
    assert_eq!(s.context(), "");
}

#[test]
fn forty_syllable_rule() {
    let mut s = sess();
    for _ in 0..39 {
        type_str(&mut s, "ㄊㄧㄢ ");
    }
    let u = type_str(&mut s, "ㄐㄧㄣ"); // pending 40th symbols: nothing committed yet
    assert!(u.commit.is_none() && text(&u).chars().count() == 39 + 3);
    let u = key(&mut s, Key::Space);
    assert_eq!(u.commit.as_deref(), Some("天".repeat(39).as_str()));
    assert_eq!(text(&u), "今");
    // Unconvertible syllable among 39 while the raw-key rescue still has the whole log: the 40th is refused, nothing is
    // committed (Shift+Enter can still turn every key into a letter).
    let mut s = sess();
    type_str(&mut s, "ㄅㄧㄤ ");
    for _ in 0..38 {
        type_str(&mut s, "ㄊㄧㄢ ");
    }
    let u = type_str(&mut s, "ㄐㄧㄣ ");
    assert!(u.commit.is_none());
    assert_eq!(u.composition.as_ref().unwrap().segments.len(), 39);
    // NEUTRAL: once the log is gone (an editing key), the 40th commits the 39 as shown and is kept.
    key(&mut s, Key::Left);
    key(&mut s, Key::End);
    let u = type_str(&mut s, "ㄐㄧㄣ ");
    assert_eq!(u.commit.as_deref(), Some(format!("ㄅㄧㄤ{}", "天".repeat(38)).as_str()));
    assert_eq!(text(&u), "今");
}

#[test]
fn cursor_insert_delete() {
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    key(&mut s, Key::Left);
    let u = type_str(&mut s, "ㄍㄨㄛˊ");
    assert_eq!(text(&u), "今國天");
    assert_eq!(u.composition.as_ref().unwrap().cursor, 2);
    let u = key(&mut s, Key::Home);
    assert_eq!(u.composition.as_ref().unwrap().cursor, 0);
    let u = key(&mut s, Key::Delete);
    assert_eq!(text(&u), "國天");
    let u = key(&mut s, Key::End);
    assert_eq!(u.composition.as_ref().unwrap().cursor, 2);
    assert_eq!(text(&key(&mut s, Key::Backspace)), "國");
    // Typing in the middle shows the pending symbols at the caret.
    key(&mut s, Key::Left);
    let u = type_str(&mut s, "ㄊ");
    assert_eq!(u.composition.as_ref().unwrap().segments.len(), 2);
    assert_eq!(u.composition.as_ref().unwrap().cursor, 1);
}

#[test]
fn candidates_lock_and_unlock() {
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    let u = key(&mut s, Key::Space); // typing empty: opens candidates for the syllable before the caret
    assert!(u.handled && u.candidates_open);
    let c = u.candidates.unwrap();
    assert_eq!((c.items.clone(), c.selected, c.page), (vec!["天".to_string(), "添".to_string()], 0, 0));
    // Space moves the highlight, digit picks by number on the page.
    let u = key(&mut s, Key::Space);
    assert_eq!(u.candidates.unwrap().selected, 1);
    let u = key(&mut s, Key::Digit(2));
    assert!(!u.candidates_open && text(&u) == "今添");
    assert!(u.composition.unwrap().segments[1].locked);
    // The lock survives conversion of a later syllable and dies with its syllable.
    let u = type_str(&mut s, "ㄍㄨㄛˊ");
    assert_eq!(text(&u), "今添國");
    key(&mut s, Key::Backspace);
    key(&mut s, Key::Backspace);
    let u = type_str(&mut s, "ㄊㄧㄢ ");
    assert_eq!(text(&u), "今天");
    assert!(!u.composition.unwrap().segments[1].locked);
    // Esc closes the list first, a second Esc cancels the composition.
    key(&mut s, Key::Down);
    assert!(!key(&mut s, Key::Esc).candidates_open);
    assert!(key(&mut s, Key::Esc).composition.is_none());
}

#[test]
fn candidate_paging_and_typing_through() {
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ");
    let u = key(&mut s, Key::Down);
    assert_eq!(u.candidates.as_ref().unwrap().items, vec!["今".to_string(), "金".to_string()]);
    // A bopomofo key closes the list and starts the next syllable.
    let u = type_str(&mut s, "ㄊ");
    assert!(!u.candidates_open && text(&u) == "今ㄊ");
    // Disabled candidates: Space with empty typing is swallowed, no list.
    let mut s = engine(false).new_session();
    type_str(&mut s, "ㄐㄧㄣ ");
    let u = key(&mut s, Key::Space);
    assert!(u.handled && !u.candidates_open && u.candidates.is_none());
}

#[test]
fn focus_and_external_events() {
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ");
    key(&mut s, Key::Enter);
    type_str(&mut s, "ㄊㄧㄢ ");
    let u = s.handle(100, None, Event::Focus(false));
    assert_eq!(u.commit.as_deref(), Some("天"));
    assert!(!u.composing && !u.handled);
    assert_eq!(s.context(), "");
    // Unconvertible composition: not committed.
    type_str(&mut s, "ㄅㄧㄤ ");
    let u = s.handle(101, None, Event::Focus(false));
    assert!(u.commit.is_none() && !u.composing);
    // External edit / abort / unsynced clear composition and context.
    for ev in [Event::ExternalEdit, Event::CompositionAborted, Event::Unsynced, Event::Focus(true), Event::Close] {
        let mut s = sess();
        type_str(&mut s, "ㄐㄧㄣ ");
        key(&mut s, Key::Enter);
        type_str(&mut s, "ㄊㄧㄢ ");
        let u = s.handle(7, None, ev);
        assert!(!u.composing && u.commit.is_none() && s.context().is_empty());
        // The next key starts from the empty state.
        assert_eq!(text(&type_str(&mut s, "ㄐㄧㄣ ")), "今");
    }
}

#[test]
fn password_field_passes_everything() {
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ");
    let u = s.handle(5, None, Event::Password(true));
    assert!(!u.composing);
    for k in [Key::Letter(b'a'), Key::Digit(1), Key::Space, Key::Enter] {
        let u = key(&mut s, k);
        assert!(!u.handled && !u.composing && s.context().is_empty());
    }
    s.handle(6, None, Event::Password(false));
    assert!(key(&mut s, Key::Digit(1)).handled);
}

#[test]
fn commit_contract_ack_table() {
    let ok = |seq| Some(CommitResult { seq, commit: Applied::Applied, comp: Applied::Applied });
    // commit Failed: state returns to before the key (composition and context).
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    let seq = s.last_seq + 1;
    let u = s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)));
    assert_eq!(u.commit.as_deref(), Some("今天"));
    let fail = CommitResult { seq, commit: Applied::Failed, comp: Applied::NotApplicable };
    let u = s.handle(seq + 1, Some(fail), Event::Ping);
    assert!(u.composing && text(&u) == "今天" && s.context().is_empty());
    // comp Failed after a good commit: composition dropped, commit kept in the context.
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ");
    key(&mut s, Key::Enter);
    type_str(&mut s, "ㄊㄧㄢ ");
    let seq = s.last_seq + 1;
    s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)));
    let bad = CommitResult { seq, commit: Applied::Applied, comp: Applied::Failed };
    let u = s.handle(seq + 1, Some(bad), Event::Ping);
    assert!(!u.composing);
    assert_eq!(s.context(), "今天");
    // comp Failed on a key that typed a syllable (no commit): composition emptied.
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ");
    let seq = s.last_seq + 1;
    s.handle(seq, None, Event::Key(KeyEvent::press(Key::Space)));
    let bad = CommitResult { seq, commit: Applied::NotApplicable, comp: Applied::Failed };
    assert!(!s.handle(seq + 1, Some(bad), Event::Ping).composing);
    // A stale ack (wrong seq) or none after a commit forgets the context; a good ack keeps it.
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ");
    let seq = s.last_seq + 1;
    s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)));
    s.handle(seq + 1, ok(seq), Event::Ping);
    assert_eq!(s.context(), "今");
    let mut s = sess();
    type_str(&mut s, "ㄐㄧㄣ ");
    let seq = s.last_seq + 1;
    s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)));
    s.handle(seq + 1, ok(seq + 5), Event::Ping);
    assert_eq!(s.context(), "");
}

#[test]
fn peek_agrees_with_handling_in_every_state() {
    // States: idle, typing, composed, converted-with-illegal, candidates open, 39 syllables.
    let setups: Vec<Box<dyn Fn(&mut Session)>> = vec![
        Box::new(|_| {}),
        Box::new(|s| {
            type_str(s, "ㄐㄧ");
        }),
        Box::new(|s| {
            type_str(s, "ㄐㄧㄣ ㄊㄧㄢ ");
        }),
        Box::new(|s| {
            type_str(s, "ㄅㄧㄤ ");
        }),
        Box::new(|s| {
            type_str(s, "ㄐㄧㄣ ");
            key(s, Key::Down);
        }),
        Box::new(|s| {
            for _ in 0..39 {
                type_str(s, "ㄊㄧㄢ ");
            }
        }),
    ];
    let mut keys: Vec<Key> = (b'a'..=b'z').map(Key::Letter).collect();
    keys.extend((0..10).map(Key::Digit));
    keys.extend([
        Key::Minus, Key::Comma, Key::Period, Key::Slash, Key::Semicolon, Key::Space, Key::Enter, Key::Esc,
        Key::Backspace, Key::Delete, Key::Tab, Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End,
        Key::PageUp, Key::PageDown, Key::Other,
    ]);
    for (si, setup) in setups.iter().enumerate() {
        for &k in &keys {
            for m in 0..16u8 {
                for down in [true, false] {
                    let mut s = sess();
                    setup(&mut s);
                    let view = s.peek_view();
                    let ev = KeyEvent { key: k, mods: twime_proto::Mods(m), down };
                    assert_eq!(s.peek_view(), view); // peek_view has no side effects
                    let want = peek(&view, ev);
                    let u = s.on_key(ev);
                    assert_eq!(u.handled, want, "setup {si} key {k:?} mods {m} down {down}");
                }
            }
        }
    }
}

#[test]
fn same_keys_same_updates() {
    let run = || {
        let mut s = sess();
        let mut out = vec![];
        for c in "ㄐㄧㄣ ㄊㄧㄢ ㄍㄨㄛˊ ㄇㄧㄣˊ".chars() {
            let k = if c == ' ' { Key::Space } else { key_of(c).unwrap() };
            out.push(s.on_key(KeyEvent::press(k)));
        }
        out.push(key(&mut s, Key::Down));
        out.push(key(&mut s, Key::Digit(1)));
        out.push(key(&mut s, Key::Enter));
        out
    };
    assert_eq!(run(), run());
}
