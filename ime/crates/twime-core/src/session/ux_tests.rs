//! English input and punctuation.

use std::sync::Arc;

use twime_model::Assets;
use twime_proto::{Applied, CommitResult, Event, Key, KeyEvent, Mods, Mode, PeekView, peek};

use super::*;
use crate::keymap::key_of;

const DICT: &str = "\
ㄐㄧㄣ 今 -2
ㄐㄧㄣ 金 -2.5
ㄊㄧㄢ 天 -2
ㄊㄧㄢ 添 -3
ㄍㄨㄛˊ 國 -2
ㄍㄨㄛˊ 郭 -3
ㄑㄩㄢˊ 全 -3
ㄑㄩㄢˊ 權 -3.5
ㄌㄧˋ 力 -2.5
ㄌㄧˋ 利 -2.6
ㄌㄧˋ 立 -2.7
ㄑㄩㄢˊ-ㄌㄧˋ 權力 -4.13
ㄑㄩㄢˊ-ㄌㄧˋ 權利 -4.134
ㄑㄩㄢˊ-ㄌㄧˋ 全力 -4.52
";

fn engine(cfg: Config) -> Arc<Engine> {
    let assets = Assets::from_dictionary_text(DICT, "").unwrap();
    Engine::new(Arc::new(assets), Config { lambda: 0.0, ..cfg })
}

fn ux_with(cfg: Config) -> Session {
    let mut s = engine(cfg).new_session();
    s.set_ux(true);
    s
}

fn ux() -> Session {
    ux_with(Config::default())
}

const SHIFT: u8 = Mods::SHIFT;
const CAPS: u8 = Mods::CAPS;

fn ev(k: Key, mods: u8) -> KeyEvent {
    KeyEvent { key: k, mods: Mods(mods), down: true }
}

fn press(s: &mut Session, k: Key) -> Update {
    s.on_key(ev(k, 0))
}

fn sh(s: &mut Session, k: Key) -> Update {
    s.on_key(ev(k, SHIFT))
}

fn caps(s: &mut Session, k: Key) -> Update {
    s.on_key(ev(k, CAPS))
}

/// An event with the previous Update taken as applied.
fn step(s: &mut Session, e: Event) -> Update {
    let ack = s.pending.as_ref().map(|p| CommitResult { seq: p.seq, commit: Applied::Applied, comp: Applied::Applied });
    let seq = s.last_seq.wrapping_add(1);
    s.handle(seq, ack, e)
}

fn zhuyin(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for c in text.chars() {
        let k = if c == ' ' { Key::Space } else { key_of(c).unwrap_or_else(|| panic!("no key for {c}")) };
        last = press(s, k);
    }
    last
}

fn letters(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for c in text.chars() {
        last = if c.is_ascii_uppercase() {
            sh(s, Key::Letter(c.to_ascii_lowercase() as u8))
        } else {
            press(s, Key::Letter(c as u8))
        };
    }
    last
}

fn text(u: &Update) -> String {
    u.composition.as_ref().map(Comp::text).unwrap_or_default()
}

/// (composition text, context, mode, run, composing)
fn shape(s: &mut Session) -> (String, String, Mode, bool, bool) {
    let u = step(s, Event::Ping);
    (text(&u), s.context(), u.mode, u.run, u.composing)
}

#[test]
fn u1_u2_punctuation_table_without_composition() {
    let cases: Vec<(Key, u8, &str)> = vec![
        (Key::Comma, SHIFT, "，"),
        (Key::Period, SHIFT, "。"),
        (Key::Slash, SHIFT, "？"),
        (Key::Semicolon, SHIFT, "："),
        (Key::Digit(1), SHIFT, "！"),
        (Key::Quote, 0, "、"),
        (Key::Quote, SHIFT, "；"),
        (Key::LBracket, 0, "「"),
        (Key::RBracket, 0, "」"),
        (Key::LBracket, SHIFT, "『"),
        (Key::RBracket, SHIFT, "』"),
        (Key::Digit(9), SHIFT, "（"),
        (Key::Digit(0), SHIFT, "）"),
        (Key::Digit(6), SHIFT, "……"),
        (Key::Minus, SHIFT, "——"),
        // Marks without a Chinese counterpart are half-width.
        (Key::Digit(2), SHIFT, "@"),
        (Key::Digit(3), SHIFT, "#"),
        (Key::Digit(4), SHIFT, "$"),
        (Key::Digit(5), SHIFT, "%"),
        (Key::Digit(7), SHIFT, "&"),
        (Key::Digit(8), SHIFT, "*"),
        (Key::Equals, 0, "="),
        (Key::Equals, SHIFT, "+"),
        (Key::Backslash, 0, "\\"),
        (Key::Backslash, SHIFT, "|"),
        (Key::Backquote, SHIFT, "~"),
    ];
    for (k, m, want) in cases {
        let mut s = ux_with(Config { symbol_table: false, ..Config::default() });
        let u = s.on_key(ev(k, m));
        assert!(u.handled, "{k:?} {m}");
        assert_eq!(u.commit.as_deref(), Some(want), "{k:?} {m}");
        assert!(!u.composing && u.composition.is_none() && u.mode == Mode::Chinese);
        assert_eq!(s.context(), want);
    }
    // Without the symbol table the backquote types a half-width backquote.
    let mut s = ux_with(Config { symbol_table: false, ..Config::default() });
    assert_eq!(press(&mut s, Key::Backquote).commit.as_deref(), Some("`"));
}

#[test]
fn half_width_style_gives_half_width_marks() {
    let cases: Vec<(Key, u8, &str)> = vec![
        (Key::Comma, SHIFT, ","),
        (Key::Period, SHIFT, "."),
        (Key::Slash, SHIFT, "?"),
        (Key::Semicolon, SHIFT, ":"),
        (Key::Digit(1), SHIFT, "!"),
        (Key::Quote, 0, "'"),
        (Key::Quote, SHIFT, ";"),
        (Key::LBracket, 0, "["),
        (Key::RBracket, 0, "]"),
        (Key::LBracket, SHIFT, "{"),
        (Key::RBracket, SHIFT, "}"),
        (Key::Digit(9), SHIFT, "("),
        (Key::Digit(0), SHIFT, ")"),
        (Key::Digit(6), SHIFT, "^"),
        (Key::Minus, SHIFT, "_"),
    ];
    for (k, m, want) in cases {
        let mut s = ux_with(Config { punct_half: true, ..Config::default() });
        let u = s.on_key(ev(k, m));
        assert_eq!(u.commit.as_deref(), Some(want), "{k:?} {m}");
        assert!(u.punct_half, "the style is reported");
    }
    assert!(!step(&mut ux(), Event::Ping).punct_half);
}

#[test]
fn u3_u4_punctuation_commits_the_composition_as_shown() {
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    let u = sh(&mut s, Key::Period);
    assert_eq!(u.commit.as_deref(), Some("今天。"));
    assert!(!u.composing && s.context() == "今天。");
    // NEUTRAL: a position without a character is committed as typed together with the mark (it used to be swallowed).
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
    let u = sh(&mut s, Key::Period);
    assert!(u.handled && u.commit.as_deref() == Some("今ㄅㄧㄤ。") && !u.composing);
    // A half-typed syllable is not convertible either.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧ");
    let u = sh(&mut s, Key::Comma);
    assert!(u.handled && u.commit.is_none() && text(&u) == "ㄐㄧ");
    // The bracket pair and the other marks behave the same way.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    assert_eq!(press(&mut s, Key::LBracket).commit.as_deref(), Some("今「"));
}

#[test]
fn u5_candidate_window_selects_the_highlight_then_commits_once() {
    let mut s = ux();
    zhuyin(&mut s, "ㄑㄩㄢˊㄌㄧˋ");
    press(&mut s, Key::Down);
    let u = press(&mut s, Key::Space); // the second item is highlighted
    assert!(u.candidates_open);
    // Reference: the same selection followed by Enter.
    let mut r = ux();
    zhuyin(&mut r, "ㄑㄩㄢˊㄌㄧˋ");
    press(&mut r, Key::Down);
    press(&mut r, Key::Space);
    press(&mut r, Key::Enter);
    let want = r.on_key(ev(Key::Enter, 0)); // the selection only closes the window; a second Enter commits
    let committed = want.commit.clone().expect("committed");
    let u = sh(&mut s, Key::Comma);
    assert_eq!(u.commit.as_deref(), Some(format!("{committed}，").as_str()));
    assert!(!u.composing && !u.candidates_open);
    assert_eq!(s.context(), format!("{committed}，"));
}

#[test]
fn u6_uppercase_starts_an_english_run_that_ends_with_space_without_a_blank() {
    let mut s = ux();
    let u = sh(&mut s, Key::Letter(b'y'));
    assert!(u.handled && u.run && u.composing && text(&u) == "Y");
    assert!(u.composition.as_ref().unwrap().segments[0].run);
    assert!(u.committable);
    letters(&mut s, "ou");
    sh(&mut s, Key::Letter(b't'));
    let u = letters(&mut s, "ube");
    assert_eq!(text(&u), "YouTube");
    assert_eq!(u.composition.as_ref().unwrap().cursor, 7);
    let u = press(&mut s, Key::Space);
    assert!(u.handled);
    assert_eq!(u.commit.as_deref(), Some("YouTube"));
    assert!(!u.composing && !u.run && u.mode == Mode::Chinese);
    assert_eq!(s.context(), "YouTube");
    // The next bopomofo key composes again.
    assert_eq!(text(&zhuyin(&mut s, "ㄐㄧㄣ ")), "今");
}

#[test]
fn run_ends_with_enter_and_a_second_space_is_not_ours() {
    let mut s = ux();
    letters(&mut s, "Ab");
    let u = press(&mut s, Key::Enter);
    assert!(u.handled && u.commit.as_deref() == Some("Ab"));
    // Space without a composition goes to the host.
    assert!(!press(&mut s, Key::Space).handled);
    assert!(!peek(&s.peek_view(), ev(Key::Space, 0)));
}

#[test]
fn u7_full_width_mark_in_a_run_commits_both_and_half_width_style_continues_the_run() {
    let mut s = ux();
    letters(&mut s, "You");
    let u = sh(&mut s, Key::Comma);
    assert_eq!(u.commit.as_deref(), Some("You，"));
    assert!(!u.composing && s.context() == "You，");
    for (k, want) in [(Key::Period, "You。"), (Key::Slash, "You？"), (Key::Digit(1), "You！"), (Key::Digit(9), "You（"), (Key::Minus, "You——")] {
        let mut s = ux();
        letters(&mut s, "You");
        assert_eq!(sh(&mut s, k).commit.as_deref(), Some(want), "{k:?}");
    }
    let mut h = ux_with(Config { punct_half: true, ..Config::default() });
    letters(&mut h, "You");
    let u = sh(&mut h, Key::Comma);
    assert!(u.commit.is_none() && text(&u) == "You,");
    assert!(u.run);
}

#[test]
fn u8_run_keeps_programming_text_together() {
    for (typed, want) in [
        (vec![(Key::Letter(b'n'), SHIFT), (Key::Letter(b'o'), 0), (Key::Letter(b'd'), 0), (Key::Letter(b'e'), 0), (Key::Period, 0), (Key::Letter(b'j'), 0), (Key::Letter(b's'), 0)], "Node.js"),
        (vec![(Key::Letter(b'e'), SHIFT), (Key::Period, 0), (Key::Letter(b'g'), 0), (Key::Period, 0)], "E.g."),
        (vec![(Key::Letter(b'u'), SHIFT), (Key::Letter(b's'), 0), (Key::Digit(2), SHIFT), (Key::Letter(b'x'), 0)], "Us@x"),
        (vec![(Key::Letter(b'c'), SHIFT), (Key::Equals, SHIFT), (Key::Equals, SHIFT)], "C++"),
        (vec![(Key::Letter(b'd'), SHIFT), (Key::Quote, 0), (Key::Letter(b't'), 0)], "D't"),
        (vec![(Key::Letter(b'a'), SHIFT), (Key::Digit(4), 0), (Key::Letter(b'x'), 0), (Key::Minus, 0), (Key::Backslash, 0)], "A4x-\\"),
        (vec![(Key::Letter(b'a'), SHIFT), (Key::LBracket, 0), (Key::RBracket, 0), (Key::Semicolon, 0), (Key::Slash, 0), (Key::Comma, 0)], "A[];/,"),
        (vec![(Key::Letter(b'a'), SHIFT), (Key::NumpadDigit(7), 0), (Key::NumpadDecimal, 0), (Key::NumpadOp(0), 0)], "A7.+"),
        (vec![(Key::Letter(b'a'), SHIFT), (Key::Digit(3), SHIFT), (Key::Digit(8), SHIFT), (Key::Backquote, SHIFT)], "A#*~"),
    ] {
        let mut s = ux();
        let mut last = Update::default();
        for (k, m) in typed {
            last = s.on_key(ev(k, m));
            assert!(last.handled && last.run && last.commit.is_none(), "{k:?}");
        }
        assert_eq!(text(&last), want);
    }
}

#[test]
fn u9_run_editing_keys() {
    // Esc cancels, nothing committed.
    let mut s = ux();
    letters(&mut s, "Ab");
    let u = press(&mut s, Key::Esc);
    assert!(u.handled && u.commit.is_none() && !u.composing && s.context().is_empty());
    // Backspace removes one character; the last one ends the run.
    letters(&mut s, "Ab");
    assert_eq!(text(&press(&mut s, Key::Backspace)), "A");
    let u = press(&mut s, Key::Backspace);
    assert!(u.handled && !u.composing && !u.run && u.commit.is_none());
    // Arrows, Home, End, Tab, Delete: the run is committed and the key is not ours.
    for k in [Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End, Key::Tab, Key::Delete, Key::PageUp, Key::PageDown] {
        let mut s = ux();
        letters(&mut s, "Ab");
        let u = press(&mut s, k);
        assert!(!u.handled, "{k:?}");
        assert_eq!(u.commit.as_deref(), Some("Ab"), "{k:?}");
        assert!(!u.composing);
    }
    // 64 characters commit by themselves and the run ends.
    let mut s = ux();
    sh(&mut s, Key::Letter(b'a'));
    let mut u = Update::default();
    for _ in 0..62 {
        u = press(&mut s, Key::Letter(b'b'));
        assert!(u.commit.is_none());
    }
    assert_eq!(text(&u).chars().count(), 63);
    let u = press(&mut s, Key::Letter(b'c'));
    assert_eq!(u.commit.as_deref().map(|c| c.chars().count()), Some(64));
    assert!(!u.composing && !u.run);
    assert_eq!(s.context().chars().count(), 64);
    // The next lower-case letter is bopomofo again.
    assert_eq!(text(&zhuyin(&mut s, "ㄐ")), "ㄐ");
}

#[test]
fn u10_u11_number_pad_commits_half_width() {
    let mut s = ux();
    let mut out = String::new();
    for k in [Key::NumpadDigit(3), Key::NumpadDecimal, Key::NumpadDigit(5)] {
        let u = press(&mut s, k);
        assert!(u.handled && !u.composing);
        out.push_str(&u.commit.unwrap());
    }
    assert_eq!(out, "3.5");
    assert_eq!(s.context(), "3.5");
    for (k, c) in [(Key::NumpadOp(0), "+"), (Key::NumpadOp(1), "-"), (Key::NumpadOp(2), "*"), (Key::NumpadOp(3), "/")] {
        assert_eq!(press(&mut s, k).commit.as_deref(), Some(c));
    }
    // With a convertible composition the text and the digit are one commit.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    let u = press(&mut s, Key::NumpadDigit(3));
    assert_eq!(u.commit.as_deref(), Some("今天3"));
    // The main-row digit stays bopomofo.
    assert_eq!(text(&press(&mut ux(), Key::Digit(1))), "ㄅ");
}

#[test]
fn u12_u13_toggle_mode() {
    // Idle: toggles there and back.
    let mut s = ux();
    assert_eq!(s.mode(), Mode::Chinese);
    let u = step(&mut s, Event::ToggleMode);
    assert!(u.mode == Mode::English && u.commit.is_none() && !u.composing);
    assert_eq!(step(&mut s, Event::ToggleMode).mode, Mode::Chinese);
    // Convertible: committed first.
    zhuyin(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    let u = step(&mut s, Event::ToggleMode);
    assert_eq!(u.commit.as_deref(), Some("今天"));
    assert!(u.mode == Mode::English && !u.composing);
    assert_eq!(s.context(), "今天");
    // An English run is committed.
    step(&mut s, Event::ToggleMode);
    letters(&mut s, "Ab");
    let u = step(&mut s, Event::ToggleMode);
    assert_eq!(u.commit.as_deref(), Some("Ab"));
    assert_eq!(u.mode, Mode::English);
    // Not convertible, plainly typed: the keys become letters and the mode is English.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
    let u = step(&mut s, Event::ToggleMode);
    assert_eq!(u.commit.as_deref(), Some("rup 1u; "), "ㄐㄧㄣ=r u p, space, ㄅㄧㄤ=1 u ;, space");
    assert_eq!(u.mode, Mode::English);
    assert!(!u.composing);
    // Not convertible and no valid log (Backspace happened): nothing, no toggle.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
    press(&mut s, Key::Left);
    let u = step(&mut s, Event::ToggleMode);
    assert!(u.commit.is_none() && u.mode == Mode::Chinese && u.composing);
    // A pending, half-typed syllable that is plainly typed also counts.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧ");
    let u = step(&mut s, Event::ToggleMode);
    assert_eq!(u.commit.as_deref(), Some("ru"));
    assert_eq!(u.mode, Mode::English);
    // Version 1/2 sessions ignore it.
    let mut old = engine(Config::default()).new_session();
    assert_eq!(step(&mut old, Event::ToggleMode).mode, Mode::Chinese);
}

#[test]
fn u14_shift_enter_rescue_and_its_log() {
    // Two initials in a row: the log keeps both.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄍ");
    let u = s.on_key(ev(Key::Enter, SHIFT));
    assert!(u.handled);
    assert_eq!(u.commit.as_deref(), Some("re"));
    assert!(!u.composing && s.mode() == Mode::Chinese);
    // A finished word.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    assert_eq!(s.on_key(ev(Key::Enter, SHIFT)).commit.as_deref(), Some("rup wu0 "));
    assert_eq!(s.context(), "rup wu0 ");
    // A word without a legal character, rescued after a good one.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
    assert_eq!(s.on_key(ev(Key::Enter, SHIFT)).commit.as_deref(), Some("rup 1u; "));
    // With the candidate window open the rescue does nothing (and does not select).
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    press(&mut s, Key::Down);
    let u = s.on_key(ev(Key::Enter, SHIFT));
    assert!(u.handled && u.commit.is_none() && u.candidates_open && text(&u) == "今");
    // The log is invalid after Backspace, Delete, cursor movement, an opened window or a lock, and stays invalid
    // until the composition is empty again.
    type Setup = fn(&mut Session);
    let breaks: Vec<(&str, Setup)> = vec![
        ("backspace", |s| {
            press(s, Key::Backspace);
            zhuyin(s, "ㄅㄧㄤ ");
        }),
        ("delete", |s| {
            press(s, Key::Left);
            press(s, Key::Delete);
        }),
        ("left", |s| {
            press(s, Key::Left);
        }),
        ("right", |s| {
            press(s, Key::Left);
            press(s, Key::Right);
        }),
        ("home", |s| {
            press(s, Key::Home);
        }),
        ("end", |s| {
            press(s, Key::End);
        }),
    ];
    for (name, brk) in breaks {
        let mut s = ux();
        zhuyin(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
        brk(&mut s);
        let u = s.on_key(ev(Key::Enter, SHIFT));
        assert!(u.handled && u.commit.is_none() && u.composing, "{name}: the rescue is off");
        zhuyin(&mut s, "ㄅㄧㄤ ");
        let u = s.on_key(ev(Key::Enter, SHIFT));
        assert!(u.commit.is_none() && u.composing, "{name}: typing again does not bring the log back");
        // ToggleMode does not use the invalid log either.
        let u = step(&mut s, Event::ToggleMode);
        assert!(u.commit.is_none() && u.mode == Mode::Chinese, "{name}");
        // Esc empties the composition; a new composition has a new log.
        press(&mut s, Key::Esc);
        zhuyin(&mut s, "ㄅㄧㄤ ");
        assert_eq!(s.on_key(ev(Key::Enter, SHIFT)).commit.as_deref(), Some("1u; "), "{name}: fresh log after Esc");
    }
    // An opened window, and a lock made with it, also end the log.
    for lock in [false, true] {
        let mut s = ux();
        zhuyin(&mut s, "ㄐㄧㄣ ");
        press(&mut s, Key::Down);
        press(&mut s, if lock { Key::Digit(2) } else { Key::Esc });
        let u = s.on_key(ev(Key::Enter, SHIFT));
        assert!(u.handled && u.commit.is_none() && u.composing, "window (lock {lock}): the rescue is off");
    }
    // The 40th syllable (automatic commit) also invalidates it.
    let mut s = ux();
    for _ in 0..39 {
        zhuyin(&mut s, "ㄅㄧㄤ ");
    }
    let u = zhuyin(&mut s, "ㄅㄧㄤ ");
    assert!(u.commit.is_none(), "unconvertible syllables are not committed");
    let mut s = ux();
    for _ in 0..39 {
        zhuyin(&mut s, "ㄊㄧㄢ ");
    }
    let u = zhuyin(&mut s, "ㄐㄧㄣ ");
    assert!(u.commit.is_some());
    let u = s.on_key(ev(Key::Enter, SHIFT));
    assert!(u.commit.is_none(), "the log was invalidated by the automatic commit");
}

#[test]
fn u15_english_mode_and_caps_pass_everything_without_composition() {
    let mut s = ux();
    step(&mut s, Event::ToggleMode);
    let view = s.peek_view();
    assert_eq!(view.mode, Mode::English);
    let mut keys: Vec<Key> = (b'a'..=b'z').map(Key::Letter).collect();
    keys.extend((0..10).map(Key::Digit));
    keys.extend([
        Key::Minus, Key::Comma, Key::Period, Key::Slash, Key::Semicolon, Key::Space, Key::Enter, Key::Esc, Key::Backquote,
        Key::LBracket, Key::RBracket, Key::Quote, Key::Backslash, Key::Equals, Key::NumpadDigit(1), Key::NumpadDecimal, Key::NumpadOp(2),
    ]);
    for &k in &keys {
        for m in [0, SHIFT] {
            assert!(!peek(&view, ev(k, m)), "{k:?} {m}");
            let u = s.on_key(ev(k, m));
            assert!(!u.handled && u.commit.is_none() && !u.composing);
        }
    }
    // Caps Lock on: the same without a composition.
    let mut s = ux();
    for &k in &keys {
        assert!(!peek(&s.peek_view(), ev(k, CAPS)), "{k:?}");
        assert!(!peek(&s.peek_view(), ev(k, CAPS | SHIFT)), "{k:?}");
        assert!(!s.on_key(ev(k, CAPS)).handled);
    }
    // Without Caps Lock the Chinese rules are back.
    assert!(peek(&s.peek_view(), ev(Key::Letter(b'a'), 0)));
}

#[test]
fn u16_u23_caps_lock_overlay_with_a_composition() {
    // Letter with a convertible composition: committed, the key passes on.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    let u = caps(&mut s, Key::Letter(b'a'));
    assert!(!u.handled);
    assert_eq!(u.commit.as_deref(), Some("今天"));
    assert!(!u.composing);
    assert_eq!(s.context(), "今天");
    // Not convertible: cancelled (like Esc), nothing committed.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
    let u = caps(&mut s, Key::Letter(b'a'));
    assert!(!u.handled && u.commit.is_none() && !u.composing && s.context().is_empty());
    // An English run is committed.
    let mut s = ux();
    letters(&mut s, "Ab");
    let u = caps(&mut s, Key::Letter(b'c'));
    assert!(!u.handled && u.commit.as_deref() == Some("Ab") && !u.composing);
    // Text keys: digits, marks, number pad.
    for k in [Key::Digit(0), Key::Digit(5), Key::Minus, Key::Comma, Key::Quote, Key::Backquote, Key::NumpadDigit(4), Key::NumpadOp(1)] {
        let mut s = ux();
        zhuyin(&mut s, "ㄐㄧㄣ ");
        let u = caps(&mut s, k);
        assert!(!u.handled && u.commit.as_deref() == Some("今"), "{k:?}");
    }
    // Keys that are not text keys keep the ordinary composition rules.
    for k in [Key::Esc, Key::Backspace, Key::Enter, Key::Left, Key::Space] {
        let mut s = ux();
        zhuyin(&mut s, "ㄐㄧㄣ ");
        let u = caps(&mut s, k);
        assert!(u.handled, "{k:?}");
        if k == Key::Enter {
            assert_eq!(u.commit.as_deref(), Some("今"));
        }
        if k == Key::Space {
            assert!(u.candidates_open, "Space still opens the window under Caps Lock");
        }
    }
    // Shift+Enter keeps its meaning with Caps Lock on.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    let u = s.on_key(ev(Key::Enter, CAPS | SHIFT));
    assert_eq!(u.commit.as_deref(), Some("rup "));
    // Ctrl / Alt / Win combinations and Key::Other are never ours; the composition stays.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    for m in [Mods::CTRL, Mods::ALT, Mods::WIN, Mods::CTRL | CAPS] {
        let u = s.on_key(ev(Key::Letter(b'v'), m));
        assert!(!u.handled && u.composing, "{m}");
    }
    let u = caps(&mut s, Key::Other);
    assert!(!u.handled && u.composing);
    // With the candidate window open the plain digits still select; the other text keys follow the Caps rule.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    press(&mut s, Key::Down);
    let u = caps(&mut s, Key::Digit(2));
    assert!(u.handled && !u.candidates_open && text(&u) == "金", "digit 2 picks the second candidate");
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    press(&mut s, Key::Down);
    let u = s.on_key(ev(Key::Digit(2), CAPS | SHIFT));
    assert!(!u.handled && u.commit.as_deref() == Some("今") && !u.candidates_open);
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    press(&mut s, Key::Down);
    let u = caps(&mut s, Key::Letter(b'a'));
    assert!(!u.handled && u.commit.as_deref() == Some("今"));
    // The navigation keys of the window are not text keys.
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    press(&mut s, Key::Down);
    let u = caps(&mut s, Key::Space);
    assert!(u.handled && u.candidates_open);
}

#[test]
fn u17_mode_and_style_survive_text_resets() {
    for e in [Event::ExternalEdit, Event::Focus(true), Event::Focus(false), Event::Unsynced, Event::CompositionAborted, Event::Close, Event::Password(false)] {
        let mut s = ux_with(Config { punct_half: true, ..Config::default() });
        step(&mut s, Event::ToggleMode);
        assert_eq!(s.mode(), Mode::English);
        let u = step(&mut s, e.clone());
        assert_eq!(u.mode, Mode::English, "{e:?}");
        assert!(u.punct_half, "{e:?}");
        // From Chinese with a composition, and with a run.
        let mut s = ux();
        zhuyin(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
        let u = step(&mut s, e.clone());
        assert!(!u.composing, "{e:?}");
        assert_eq!(u.mode, Mode::Chinese);
        letters(&mut s, "Ab");
        let u = step(&mut s, e.clone());
        assert!(!u.composing && !u.run, "{e:?}");
        if !matches!(e, Event::Focus(false)) {
            assert!(u.commit.is_none(), "{e:?}");
        }
        assert!(s.context().is_empty());
    }
    // The initial mode comes from the configuration; a new session (engine restart) goes back to it.
    let s = ux_with(Config { default_english: true, ..Config::default() });
    assert_eq!(s.mode(), Mode::English);
    assert_eq!(ux().mode(), Mode::Chinese);
}

#[test]
fn u21_focus_leaving_commits_a_run_and_a_convertible_composition_only() {
    let mut s = ux();
    letters(&mut s, "Ab");
    let u = step(&mut s, Event::Focus(false));
    assert_eq!(u.commit.as_deref(), Some("Ab"));
    assert!(!u.composing && s.context().is_empty());
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    assert_eq!(step(&mut s, Event::Focus(false)).commit.as_deref(), Some("今天"));
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ㄅㄧㄤ ");
    assert!(step(&mut s, Event::Focus(false)).commit.is_none());
}

#[test]
fn u24_symbol_table() {
    let expect = ["<", ">", "\"", "_", "^", "《", "》", "〈", "〉", "／", "±", "％"];
    // a: the digits (page 1 = 1-9; page 2 = the last three, reached with Right/PageDown/Down)
    for (i, want) in expect.iter().enumerate() {
        let mut s = ux();
        let u = press(&mut s, Key::Backquote);
        assert!(u.handled && u.symbols && u.composing && u.candidates_open && u.commit.is_none());
        assert_eq!(text(&u), "`");
        let c = u.candidates.unwrap();
        assert_eq!((c.items.len(), c.total, c.per_page, c.selected, c.page), (9, 12, 9, 0, 0));
        assert_eq!(c.items.iter().map(String::as_str).collect::<Vec<_>>(), expect[..9]);
        assert!(!u.committable);
        let u = if i < 9 {
            press(&mut s, Key::Digit(i as u8 + 1))
        } else {
            let u = press(&mut s, Key::PageDown);
            assert!(u.handled && u.symbols && u.composing && u.commit.is_none());
            let c = u.candidates.unwrap();
            assert_eq!((c.items.len(), c.total, c.per_page, c.page), (3, 12, 9, 1));
            assert_eq!(c.items.iter().map(String::as_str).collect::<Vec<_>>(), expect[9..]);
            press(&mut s, Key::Digit(i as u8 - 8))
        };
        assert_eq!(u.commit.as_deref(), Some(*want));
        assert!(!u.composing && !u.symbols && !u.candidates_open);
        assert_eq!(s.context(), *want);
        // the next table opens on page 1 again
        let c = press(&mut s, Key::Backquote).candidates.unwrap();
        assert_eq!((c.page, c.items.len()), (0, 9));
    }
    // a: paging keys wrap; digits beyond page 2's three items are swallowed and keep the table
    for (fwd, back) in [(Key::Right, Key::Left), (Key::PageDown, Key::PageUp), (Key::Down, Key::Up)] {
        let mut s = ux();
        press(&mut s, Key::Backquote);
        assert_eq!(press(&mut s, fwd).candidates.unwrap().page, 1);
        let u = press(&mut s, Key::Digit(4));
        assert!(u.handled && u.symbols && u.commit.is_none() && u.candidates.unwrap().page == 1);
        assert_eq!(press(&mut s, fwd).candidates.unwrap().page, 0, "wraps forward");
        assert_eq!(press(&mut s, back).candidates.unwrap().page, 1, "wraps backward");
        assert_eq!(press(&mut s, back).candidates.unwrap().page, 0);
        let u = press(&mut s, Key::Digit(9));
        assert_eq!(u.commit.as_deref(), Some("〉"));
    }
    // a: the mouse (the index is within the shown page)
    for i in 0..12u16 {
        let mut s = ux();
        press(&mut s, Key::Backquote);
        if i >= 9 {
            press(&mut s, Key::PageDown);
        }
        let u = step(&mut s, Event::CandidatePick(i % 9));
        assert_eq!(u.commit.as_deref(), Some(expect[i as usize]));
        assert!(!u.composing && !u.symbols);
    }
    let mut s = ux();
    press(&mut s, Key::Backquote);
    press(&mut s, Key::PageDown);
    let u = step(&mut s, Event::CandidatePick(3));
    assert!(u.commit.is_none() && u.symbols, "no fourth item on page 2");
    // Esc on page 2 cancels as well and the page is forgotten
    assert!(press(&mut s, Key::Esc).handled);
    assert_eq!(press(&mut s, Key::Backquote).candidates.unwrap().page, 0);
    // b: Esc cancels and is swallowed; the others cancel and are then handled as without the table.
    let mut s = ux();
    press(&mut s, Key::Backquote);
    let u = press(&mut s, Key::Esc);
    assert!(u.handled && u.commit.is_none() && !u.composing && !u.symbols);
    let mut s = ux();
    press(&mut s, Key::Backquote);
    let u = sh(&mut s, Key::Period);
    assert_eq!(u.commit.as_deref(), Some("。"), "the anchor is never committed");
    assert!(!u.symbols && !u.composing);
    for k in [Key::Enter, Key::Space, Key::Tab, Key::Backspace, Key::Home, Key::End] {
        let mut s = ux();
        press(&mut s, Key::Backquote);
        let u = press(&mut s, k);
        assert!(!u.handled && u.commit.is_none() && !u.composing && !u.symbols, "{k:?}: the idle rules apply, the key goes to the host");
    }
    let mut s = ux();
    press(&mut s, Key::Backquote);
    let u = press(&mut s, Key::Letter(b'a'));
    assert!(u.handled && text(&u) == "ㄇ", "a bopomofo key starts a composition once the table is closed");
    let mut s = ux();
    press(&mut s, Key::Backquote);
    let u = step(&mut s, Event::ToggleMode);
    assert!(u.mode == Mode::English && u.commit.is_none() && !u.composing);
    // The backquote while the table is open closes it and (idle rules) opens it anew.
    let mut s = ux();
    press(&mut s, Key::Backquote);
    assert!(press(&mut s, Key::Backquote).symbols);
    // c: keys that never reach the core keep the table (peek is false for them).
    let mut s = ux();
    press(&mut s, Key::Backquote);
    let v = s.peek_view();
    assert!(v.symbols && v.composing);
    for e in [ev(Key::Other, 0), ev(Key::Letter(b'v'), Mods::CTRL), ev(Key::Letter(b'v'), Mods::ALT), ev(Key::Letter(b'a'), Mods::WIN)] {
        assert!(!peek(&v, e));
    }
    assert!(peek(&v, ev(Key::Digit(1), 0)));
    // d: failures and resets never commit and leave no anchor.
    for e in [Event::Focus(false), Event::ExternalEdit, Event::CompositionAborted, Event::Unsynced, Event::Focus(true), Event::Close] {
        let mut s = ux();
        press(&mut s, Key::Backquote);
        let u = step(&mut s, e.clone());
        assert!(u.commit.is_none() && !u.composing && !u.symbols, "{e:?}");
    }
    let mut s = ux();
    press(&mut s, Key::Backquote);
    s.on_key(ev(Key::Esc, 0));
    let seq = s.last_seq + 1;
    s.handle(seq, None, Event::Key(ev(Key::Backquote, 0)));
    let bad = CommitResult { seq, commit: Applied::NotApplicable, comp: Applied::Failed };
    let u = s.handle(seq + 1, Some(bad), Event::Ping);
    assert!(!u.composing && !u.symbols && u.commit.is_none());
    // A Caps Lock text key closes the table without a commit and passes on.
    let mut s = ux();
    press(&mut s, Key::Backquote);
    let u = caps(&mut s, Key::Letter(b'a'));
    assert!(!u.handled && u.commit.is_none() && !u.composing);
    // Old protocol versions: the backquote is not ours.
    let mut old = engine(Config::default()).new_session();
    assert!(!peek(&old.peek_view(), ev(Key::Backquote, 0)));
    assert!(!old.on_key(ev(Key::Backquote, 0)).handled);
    // Flag off: a plain backquote.
    let mut s = ux_with(Config { symbol_table: false, ..Config::default() });
    assert_eq!(press(&mut s, Key::Backquote).commit.as_deref(), Some("`"));
    // With a composition the backquote is swallowed (no table, no commit).
    let mut s = ux();
    zhuyin(&mut s, "ㄐㄧㄣ ");
    let u = press(&mut s, Key::Backquote);
    assert!(u.handled && u.commit.is_none() && text(&u) == "今" && !u.symbols);
}

#[test]
fn u24e_symbol_table_page_two_edges() {
    // Shift does not stop the paging keys; digits 4-9 on page 2 are swallowed and keep the table and the page.
    for k in [Key::Right, Key::PageDown, Key::Down] {
        let mut s = ux();
        press(&mut s, Key::Backquote);
        assert!(peek(&s.peek_view(), ev(k, SHIFT)));
        let u = sh(&mut s, k);
        assert!(u.handled && u.symbols && u.candidates.unwrap().page == 1, "{k:?}");
    }
    let mut s = ux();
    press(&mut s, Key::Backquote);
    press(&mut s, Key::PageDown);
    for d in 4..=9u8 {
        let u = press(&mut s, Key::Digit(d));
        assert!(u.handled && u.symbols && u.commit.is_none() && u.candidates.unwrap().page == 1, "digit {d}");
    }
    // A failed host write on page 2 goes back to the table as it was (same page, same items), then picks again.
    let mut s = ux();
    press(&mut s, Key::Backquote);
    press(&mut s, Key::PageDown);
    let seq = s.last_seq + 1;
    let u = s.handle(seq, None, Event::Key(ev(Key::Digit(2), 0)));
    assert_eq!(u.commit.as_deref(), Some("±"));
    let bad = CommitResult { seq, commit: Applied::Failed, comp: Applied::NotApplicable };
    let u = s.handle(seq + 1, Some(bad), Event::Ping);
    assert!(u.symbols && s.context().is_empty());
    let c = u.candidates.unwrap();
    assert_eq!((c.page, c.items.iter().map(String::as_str).collect::<Vec<_>>()), (1, vec!["／", "±", "％"]));
    assert_eq!(step(&mut s, Event::CandidatePick(2)).commit.as_deref(), Some("％"));
    // Every reset forgets the page.
    for e in [Event::Focus(false), Event::ExternalEdit, Event::Unsynced, Event::CompositionAborted] {
        let mut s = ux();
        press(&mut s, Key::Backquote);
        press(&mut s, Key::PageDown);
        step(&mut s, e.clone());
        assert_eq!(press(&mut s, Key::Backquote).candidates.unwrap().page, 0, "{e:?}");
    }
}

#[test]
fn u18_every_commit_entry_is_revertible() {
    type Setup = fn(&mut Session);
    type Act = Box<dyn Fn(&mut Session, u32) -> Update>;
    let key_act = |k: Key, m: u8| -> Act { Box::new(move |s, seq| s.handle(seq, None, Event::Key(ev(k, m)))) };
    let event_act = |e: Event| -> Act { Box::new(move |s, seq| s.handle(seq, None, e.clone())) };
    let comp2: Setup = |s| {
        zhuyin(s, "ㄐㄧㄣ ㄊㄧㄢ ");
    };
    let run2: Setup = |s| {
        letters(s, "Ab");
    };
    let bad2: Setup = |s| {
        zhuyin(s, "ㄐㄧㄣ ㄅㄧㄤ ");
    };
    let table: Setup = |s| {
        press(s, Key::Backquote);
    };
    let idle: Setup = |_| {};
    let cases: Vec<(&str, Setup, Act)> = vec![
        ("punctuation after composition", comp2, key_act(Key::Period, SHIFT)),
        ("bracket after composition", comp2, key_act(Key::LBracket, 0)),
        ("number pad after composition", comp2, key_act(Key::NumpadDigit(3), 0)),
        ("Shift+letter after composition", comp2, key_act(Key::Letter(b'y'), SHIFT)),
        ("Enter", comp2, key_act(Key::Enter, 0)),
        ("ToggleMode with composition", comp2, event_act(Event::ToggleMode)),
        ("rescue", bad2, key_act(Key::Enter, SHIFT)),
        ("ToggleMode rescue", bad2, event_act(Event::ToggleMode)),
        ("run Space", run2, key_act(Key::Space, 0)),
        ("run Enter", run2, key_act(Key::Enter, 0)),
        ("run Tab", run2, key_act(Key::Tab, 0)),
        ("run full-width mark", run2, key_act(Key::Comma, SHIFT)),
        ("run ToggleMode", run2, event_act(Event::ToggleMode)),
        ("caps with composition", comp2, key_act(Key::Letter(b'a'), CAPS)),
        ("caps with run", run2, key_act(Key::Letter(b'a'), CAPS)),
        ("focus leaving", comp2, event_act(Event::Focus(false))),
        ("symbol", table, key_act(Key::Digit(6), 0)),
        ("plain punctuation", idle, key_act(Key::Period, SHIFT)),
        ("number pad", idle, key_act(Key::NumpadDigit(7), 0)),
    ];
    for (name, setup, act) in cases {
        // Some context first ("X"), then the setup; a Ping confirms the setup and gives the state before the action.
        let prep = || {
            let mut s = ux();
            sh(&mut s, Key::Letter(b'x'));
            press(&mut s, Key::Enter);
            setup(&mut s);
            let u = step(&mut s, Event::Ping);
            let before = (text(&u), s.context(), u.mode, u.run, u.composing);
            (s, before)
        };
        let ok = |seq: u32, commit, comp| Some(CommitResult { seq, commit, comp });
        let (mut s, before) = prep();
        let seq = s.last_seq + 1;
        let u = act(&mut s, seq);
        assert!(u.commit.is_some(), "{name}: the action commits");
        // The state after a good write, as the reference for the other outcomes.
        let (mut r, _) = prep();
        let _ = act(&mut r, seq);
        let ur = r.handle(seq + 1, ok(seq, Applied::Applied, Applied::Applied), Event::Ping);
        let after = (text(&ur), r.context(), ur.mode);
        // commit Failed: everything is back to before the key, including the mode.
        let u2 = s.handle(seq + 1, ok(seq, Applied::Failed, Applied::NotApplicable), Event::Ping);
        let got = (text(&u2), s.context(), u2.mode, u2.run, u2.composing);
        assert_eq!(got, before, "{name}: commit Failed restores the state before the key");
        // commit Applied / comp Failed: the committed text stays in the context, composition and run are gone.
        let (mut t, _) = prep();
        let _ = act(&mut t, seq);
        let u3 = t.handle(seq + 1, ok(seq, Applied::Applied, Applied::Failed), Event::Ping);
        assert!(!u3.composing && !u3.run && !u3.symbols && text(&u3).is_empty(), "{name}: comp Failed clears the composition");
        assert_eq!(t.context(), after.1, "{name}: comp Failed keeps the committed context");
        assert_eq!(u3.mode, after.2, "{name}: the mode is that of the Update");
        // No ack: the context is forgotten (it may be wrong); the composition is as the Update left it.
        let (mut t, _) = prep();
        let _ = act(&mut t, seq);
        let u4 = t.handle(seq + 1, None, Event::Ping);
        assert!(t.context().is_empty(), "{name}: no ack clears the context");
        assert_eq!(text(&u4), after.0, "{name}: no ack keeps the composition");
        assert_eq!(u4.mode, after.2);
    }
}

#[test]
fn u19_old_connections_keep_the_old_behaviour() {
    let mut s = engine(Config::default()).new_session(); // ux off
    let idle = s.peek_view();
    assert!(!idle.ux);
    for (k, m) in [(Key::Letter(b'a'), SHIFT), (Key::Comma, SHIFT), (Key::LBracket, 0), (Key::NumpadDigit(1), 0)] {
        assert!(!peek(&idle, ev(k, m)));
        assert!(!s.on_key(ev(k, m)).handled);
    }
    zhuyin(&mut s, "ㄐㄧㄣ ");
    // Composing: every key is swallowed as before, Shift+letter included; nothing English happens.
    let u = sh(&mut s, Key::Letter(b'y'));
    assert!(u.handled && u.commit.is_none() && !u.run && text(&u) == "今");
    let u = s.on_key(ev(Key::Enter, SHIFT));
    assert!(u.handled && u.commit.is_none());
    // Caps cannot occur on a version 2 connection; if it did it would be ignored.
    let u = caps(&mut s, Key::Letter(b'a'));
    assert!(u.handled && u.composing);
    assert_eq!(step(&mut s, Event::ToggleMode).mode, Mode::Chinese);
    assert!(!step(&mut s, Event::Ping).punct_half);
}

#[test]
fn u20_replays_are_identical() {
    let run = || {
        let mut s = ux();
        let mut out = vec![];
        let script: Vec<(Key, u8)> = vec![
            (Key::Letter(b'y'), SHIFT), (Key::Letter(b'o'), 0), (Key::Comma, SHIFT), (Key::Letter(b'r'), 0), (Key::Letter(b'u'), 0),
            (Key::Letter(b'p'), 0), (Key::Space, 0), (Key::Enter, SHIFT), (Key::Backquote, 0), (Key::Digit(3), 0), (Key::NumpadDigit(4), 0),
            (Key::Letter(b'a'), CAPS),
        ];
        for (k, m) in script {
            out.push(s.on_key(ev(k, m)));
        }
        out.push(step(&mut s, Event::ToggleMode));
        out.push(step(&mut s, Event::Focus(false)));
        out
    };
    assert_eq!(run(), run());
}

fn all_keys() -> Vec<Key> {
    let mut keys: Vec<Key> = (b'a'..=b'z').map(Key::Letter).collect();
    keys.extend((0..10).map(Key::Digit));
    keys.extend((0..10).map(Key::NumpadDigit));
    keys.extend((0..4).map(Key::NumpadOp));
    keys.extend([
        Key::Minus, Key::Comma, Key::Period, Key::Slash, Key::Semicolon, Key::Space, Key::Enter, Key::Esc, Key::Backspace,
        Key::Delete, Key::Tab, Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End, Key::PageUp, Key::PageDown,
        Key::Other, Key::Backquote, Key::LBracket, Key::RBracket, Key::Quote, Key::Backslash, Key::Equals, Key::NumpadDecimal,
    ]);
    keys
}

fn setups() -> Vec<Box<dyn Fn(&mut Session)>> {
    vec![
        Box::new(|_| {}),
        Box::new(|s| {
            zhuyin(s, "ㄐㄧ");
        }),
        Box::new(|s| {
            zhuyin(s, "ㄐㄧㄣ ㄊㄧㄢ ");
        }),
        Box::new(|s| {
            zhuyin(s, "ㄅㄧㄤ ");
        }),
        Box::new(|s| {
            zhuyin(s, "ㄐㄧㄣ ");
            press(s, Key::Down);
        }),
        Box::new(|s| {
            letters(s, "Ab");
        }),
        Box::new(|s| {
            press(s, Key::Backquote);
        }),
        Box::new(|s| {
            step(s, Event::ToggleMode);
        }),
        Box::new(|s| {
            for _ in 0..39 {
                zhuyin(s, "ㄊㄧㄢ ");
            }
        }),
    ]
}

#[test]
fn peek_false_means_the_core_never_swallows() {
    for (si, setup) in setups().iter().enumerate() {
        for &k in &all_keys() {
            for m in 0..=Mods::ALL {
                for down in [true, false] {
                    let mut s = ux();
                    setup(&mut s);
                    let view: PeekView = s.peek_view();
                    let e = KeyEvent { key: k, mods: Mods(m), down };
                    let want = peek(&view, e);
                    assert_eq!(s.peek_view(), view);
                    let u = s.on_key(e);
                    if !want {
                        assert!(!u.handled, "setup {si} key {k:?} mods {m} down {down}: swallowed although peek said no");
                    }
                }
            }
        }
    }
}

#[test]
fn only_the_documented_keys_pass_although_peek_says_yes() {
    // Composition states: the keys that may pass are the Caps Lock text keys, and (English run) the keys that end the run.
    for si in [2usize, 5] {
        let mut passing: Vec<String> = vec![];
        for &k in &all_keys() {
            if k == Key::Other {
                continue;
            }
            for m in [0u8, SHIFT, CAPS, CAPS | SHIFT] {
                let mut t = ux();
                setups()[si](&mut t);
                let e = ev(k, m);
                if !peek(&t.peek_view(), e) {
                    continue;
                }
                if !t.on_key(e).handled {
                    passing.push(format!("{k:?}/{m}"));
                }
            }
        }
        for p in &passing {
            let caps_text = p.rsplit('/').next().is_some_and(|m| m.parse::<u8>().is_ok_and(|m| m & CAPS != 0));
            let run_end = si == 5 && ["Tab", "Delete", "Left", "Right", "Up", "Down", "Home", "End", "PageUp", "PageDown"].iter().any(|n| p.starts_with(&format!("{n}/")));
            assert!(caps_text || run_end, "unexpected pass-through {p} (setup {si})");
        }
        assert!(!passing.is_empty());
    }
}

#[test]
fn backquote_with_the_candidate_window_open_changes_nothing() {
    let mut s = ux();
    zhuyin(&mut s, "ㄑㄩㄢˊㄌㄧˋ");
    press(&mut s, Key::Down);
    let before = press(&mut s, Key::Space); // second item highlighted
    assert!(before.candidates_open);
    let u = press(&mut s, Key::Backquote);
    assert!(u.handled && u.commit.is_none() && u.candidates_open && !u.symbols);
    assert_eq!((text(&u), u.candidates.as_ref().map(|c| c.selected)), (text(&before), before.candidates.as_ref().map(|c| c.selected)));
    // Nor does it lock anything: choosing the highlighted item afterwards still works as before.
    let mut r = ux();
    zhuyin(&mut r, "ㄑㄩㄢˊㄌㄧˋ");
    press(&mut r, Key::Down);
    press(&mut r, Key::Space);
    assert_eq!(press(&mut s, Key::Enter).composition, press(&mut r, Key::Enter).composition);
}
