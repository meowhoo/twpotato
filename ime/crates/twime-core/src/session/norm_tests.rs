//! 台/臺 style following and the 佈 word rules in the session. No model: the fallback
//! (best single character of the dictionary) picks 臺 / 佈 on purpose, the output layer must turn them into the default.

use std::sync::Arc;

use twime_model::{Assets, NormCfg};
use twime_proto::{Applied, CommitResult, Event, Key, KeyEvent};

use super::*;
use crate::keymap::key_of;

const DICT: &str = "\
ㄊㄞˊ 臺 -1.5
ㄊㄞˊ 台 -2
ㄨㄢ 灣 -2
ㄒㄩㄢ 宣 -2
ㄅㄨˋ 佈 -1.5
ㄅㄨˋ 布 -2
ㄊㄞˊ-ㄨㄢ 臺灣 -3
ㄊㄞˊ-ㄨㄢ 台灣 -3.5
";

/// `tai_first`: the dictionary prefers 台 over 臺 (so the fallback picks 台 and the style has to turn it into 臺).
fn session(norm: NormCfg, tai_first: bool) -> Session {
    let dict = if tai_first { DICT.replace("臺 -1.5", "臺 -2.5") } else { DICT.to_string() };
    let mut assets = Assets::from_dictionary_text(&dict, "").unwrap();
    assets.set_norm(norm);
    Engine::new(Arc::new(assets), Config { lambda: 0.0, page_size: 9, ..Config::default() }).new_session()
}

fn type_str(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for c in text.chars() {
        let key = if c == ' ' { Key::Space } else { key_of(c).unwrap() };
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

/// Types the syllables, opens the candidate window, picks the item whose text is `want`, commits; returns the commit.
fn pick_and_commit(s: &mut Session, syl: &str, want: &str) -> String {
    type_str(s, syl);
    let u = key(s, Key::Down);
    let items = u.candidates.unwrap().items;
    let n = items.iter().position(|x| x == want).unwrap_or_else(|| panic!("{want} not in {items:?}"));
    key(s, Key::Digit(n as u8 + 1));
    key(s, Key::Enter).commit.unwrap()
}

fn commit_of(s: &mut Session, syl: &str) -> String {
    type_str(s, syl);
    key(s, Key::Enter).commit.unwrap()
}

#[test]
fn default_is_tai_and_buu_words_fold() {
    let mut s = session(NormCfg::ALL, false);
    assert_eq!(text(&type_str(&mut s, "ㄊㄞˊ")), "台");
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("台"));
    assert_eq!(commit_of(&mut s, "ㄒㄩㄢ ㄅㄨˋ"), "宣布");
}

#[test]
fn rules_off_keep_the_old_output() {
    let mut s = session(NormCfg::default(), false);
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "臺");
    assert_eq!(commit_of(&mut s, "ㄒㄩㄢ ㄅㄨˋ"), "宣佈");
}

#[test]
fn choosing_tai_trad_is_followed_and_choosing_tai_goes_back() {
    let mut s = session(NormCfg::ALL, true);
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "台");
    // The user selects 臺灣: it stays, and later unlocked 台/臺 characters follow it (also across commits).
    assert_eq!(pick_and_commit(&mut s, "ㄊㄞˊㄨㄢ ", "臺灣"), "臺灣");
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "臺");
    assert_eq!(commit_of(&mut s, "ㄊㄞˊㄨㄢ "), "臺灣");
    // 佈 words are not affected by the style.
    assert_eq!(commit_of(&mut s, "ㄒㄩㄢ ㄅㄨˋ"), "宣布");
    // Choosing 台灣 explicitly goes back to the default.
    assert_eq!(pick_and_commit(&mut s, "ㄊㄞˊㄨㄢ ", "台灣"), "台灣");
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "台");
}

#[test]
fn set_context_decides_the_style_and_focus_resets_it() {
    let mut s = session(NormCfg::ALL, true);
    s.on_key(KeyEvent::press(Key::Esc));
    s.handle(1, None, Event::SetContext("今天去臺北".into()));
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "臺");
    // The last of 台/臺 in the original context wins.
    s.handle(2, None, Event::SetContext("臺北和台中".into()));
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "台");
    s.handle(3, None, Event::SetContext("臺北".into()));
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "臺");
    // A context without either character is a new context: default.
    s.handle(4, None, Event::SetContext("今天".into()));
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "台");
    s.handle(5, None, Event::SetContext("臺北".into()));
    s.handle(6, None, Event::Focus(false));
    assert_eq!(commit_of(&mut s, "ㄊㄞˊ"), "台");
}

#[test]
fn committed_text_and_unlocking_do_not_change_the_style() {
    let mut s = session(NormCfg::ALL, true);
    assert_eq!(pick_and_commit(&mut s, "ㄊㄞˊㄨㄢ ", "臺灣"), "臺灣");
    // The context now ends with 臺; typing 台 text by hand does not matter (commit of English / symbols too).
    // Select 臺灣 again, unlock it with Backspace of the group: the style stays.
    type_str(&mut s, "ㄊㄞˊㄨㄢ ");
    let u = key(&mut s, Key::Down);
    let n = u.candidates.unwrap().items.iter().position(|x| x == "臺灣").unwrap();
    key(&mut s, Key::Digit(n as u8 + 1));
    key(&mut s, Key::Backspace);
    assert_eq!(text(&type_str(&mut s, "ㄨㄢ ")), "臺灣");
}

#[test]
fn failed_commit_ack_keeps_the_style() {
    let mut s = session(NormCfg::ALL, true);
    pick_and_commit(&mut s, "ㄊㄞˊㄨㄢ ", "臺灣");
    type_str(&mut s, "ㄊㄞˊ");
    let seq = 100;
    let u = s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)));
    assert_eq!(u.commit.as_deref(), Some("臺"));
    // The host did not take the text: the composition is back, the style is kept.
    s.handle(seq + 1, Some(CommitResult { seq, commit: Applied::Failed, comp: Applied::Applied }), Event::Ping);
    assert_eq!(text(&key(&mut s, Key::Right)), "臺");
}

#[test]
fn candidate_pairs_follow_the_preferred_glyph() {
    let mut s = session(NormCfg::ALL, false);
    type_str(&mut s, "ㄊㄞˊㄨㄢ ");
    let items = key(&mut s, Key::Down).candidates.unwrap().items;
    // Current item 台灣 first, its partner 臺灣 directly after it.
    assert_eq!(&items[..2], &["台灣", "臺灣"]);
    key(&mut s, Key::Esc);
    key(&mut s, Key::Enter);
    s.handle(9, None, Event::SetContext("臺".into()));
    type_str(&mut s, "ㄊㄞˊㄨㄢ ");
    let items = key(&mut s, Key::Down).candidates.unwrap().items;
    assert_eq!(&items[..2], &["臺灣", "台灣"]);
}

#[test]
fn rollback_after_a_style_change_rewrites_the_composition_in_the_kept_style() {
    let mut s = session(NormCfg::ALL, true);
    type_str(&mut s, "ㄊㄞˊ");
    assert_eq!(text(&key(&mut s, Key::Right)), "台");
    // As if a selection in the same event had set the style, and the commit then failed.
    let before = s.st.clone();
    s.st.style_tai = true;
    s.pending = Some(Pending { seq: 5, before: Some(before), commit: true });
    s.handle(6, Some(CommitResult { seq: 5, commit: Applied::Failed, comp: Applied::Applied }), Event::Ping);
    assert!(s.st.style_tai && s.st.cand.is_none());
    let u = key(&mut s, Key::Right);
    assert_eq!(text(&u), "臺");
}

#[test]
fn composition_failure_delete_and_insert_keep_the_style() {
    let mut s = session(NormCfg::ALL, true);
    pick_and_commit(&mut s, "ㄊㄞˊㄨㄢ ", "臺灣");
    type_str(&mut s, "ㄊㄞˊ ㄨㄢ ");
    key(&mut s, Key::Left);
    key(&mut s, Key::Delete);
    key(&mut s, Key::Home);
    type_str(&mut s, "ㄊㄞˊ");
    assert!(s.st.style_tai);
    let seq = 40;
    s.handle(seq, None, Event::Key(KeyEvent::press(Key::Esc)));
    s.handle(seq + 1, Some(CommitResult { seq, commit: Applied::Applied, comp: Applied::Failed }), Event::Ping);
    assert!(s.st.style_tai);
}
