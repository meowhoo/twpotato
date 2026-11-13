//! Word-level candidates in the session.

use std::sync::Arc;

use twime_model::Assets;
use twime_proto::{Applied, CommitResult, Event, Key, KeyEvent, Mods};

use super::*;
use crate::keymap::key_of;

const DICT: &str = "\
ㄑㄩㄢˊ 全 -3
ㄑㄩㄢˊ 權 -3.5
ㄑㄩㄢˊ 泉 -4
ㄌㄧˋ 力 -2.5
ㄌㄧˋ 利 -2.6
ㄌㄧˋ 立 -2.7
ㄌㄧˋ 歷 -3
ㄧˇ 以 -2
ㄈㄨˋ 赴 -3
ㄈㄨˋ 副 -3.1
ㄑㄩㄢˊ-ㄌㄧˋ 權力 -4.13
ㄑㄩㄢˊ-ㄌㄧˋ 權利 -4.134
ㄑㄩㄢˊ-ㄌㄧˋ 全力 -4.52
ㄑㄩㄢˊ-ㄌㄧˋ-ㄧˇ-ㄈㄨˋ 全力以赴 -6
";

fn session(page_size: usize) -> Session {
    let assets = Assets::from_dictionary_text(DICT, "").unwrap();
    Engine::new(Arc::new(assets), Config { lambda: 0.0, page_size, ..Config::default() }).new_session()
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

fn shift(s: &mut Session, k: Key) -> Update {
    s.on_key(KeyEvent { key: k, mods: Mods(Mods::SHIFT), down: true })
}

fn text(u: &Update) -> String {
    u.composition.as_ref().map(Comp::text).unwrap_or_default()
}

fn locked(u: &Update) -> Vec<bool> {
    u.composition.as_ref().unwrap().segments.iter().map(|g| g.locked).collect()
}

/// All items over all pages, by walking with Down.
fn all_items(s: &mut Session) -> Vec<String> {
    let first = key(s, Key::Down);
    let c = first.candidates.unwrap();
    let total = c.total as usize;
    let mut out = c.items.clone();
    // Page forward with PageDown until the list wraps.
    let mut page = c.page;
    loop {
        let u = key(s, Key::PageDown);
        let c = u.candidates.unwrap();
        if c.page <= page {
            break;
        }
        page = c.page;
        out.extend(c.items);
    }
    assert_eq!(out.len(), total);
    out
}

#[test]
fn words_are_listed_selected_and_locked_as_a_group() {
    let mut s = session(3);
    let u = type_str(&mut s, "ㄑㄩㄢˊㄌㄧˋ");
    assert_eq!(text(&u), "全力");
    let u = key(&mut s, Key::Down);
    let c = u.candidates.unwrap();
    // Current word first; total and page size are reported for the page indicator.
    assert_eq!(c.items[0], "全力");
    assert_eq!((c.per_page, c.page, c.selected), (3, 0, 0));
    let items = all_items(&mut s);
    assert_eq!(items.len() as u16, c.total);
    assert_eq!(&items[..3], &["全力", "權力", "權利"]);
    // Every legal character of the last syllable is reachable: as a single, or as the end of a word that keeps 全.
    for ch in ["力", "利", "立", "歷"] {
        assert!(items.iter().any(|x| x == ch) || items.iter().any(|x| x.starts_with('全') && x.ends_with(ch)), "{ch}: {items:?}");
    }
    // Digit 2 picks 權力 and locks both characters as one group.
    let u = key(&mut s, Key::Digit(2));
    assert!(!u.candidates_open);
    assert_eq!(text(&u), "權力");
    assert_eq!(locked(&u), vec![true, true]);
    // The lock survives a later syllable.
    let u = type_str(&mut s, "ㄧˇ");
    assert_eq!(text(&u), "權力以");
    assert_eq!(locked(&u), vec![true, true, false]);
    // Backspace of the last locked character unlocks the whole group.
    key(&mut s, Key::Left); // before 以? cursor is at the end; move left once so the caret is after 力
    let u = key(&mut s, Key::Backspace);
    assert_eq!(text(&u), "全以");
    assert_eq!(locked(&u), vec![false, false]);
}

#[test]
fn paging_keys_tab_home_end() {
    let mut s = session(2);
    type_str(&mut s, "ㄑㄩㄢˊㄌㄧˋ");
    let total = key(&mut s, Key::Down).candidates.unwrap().total as usize;
    assert_eq!(total, 7); // six candidates and the typed bopomofo (ZHUYIN-OUT)
    let sel = |u: Update| {
        let c = u.candidates.unwrap();
        (c.page, c.selected)
    };
    assert_eq!(sel(key(&mut s, Key::Tab)), (1, 0));
    assert_eq!(sel(shift(&mut s, Key::Tab)), (0, 0));
    // Shift+Tab on the first page wraps to the last page.
    let last_page = ((total - 1) / 2) as u16;
    assert_eq!(sel(shift(&mut s, Key::Tab)).0, last_page);
    assert_eq!(sel(key(&mut s, Key::Home)), (0, 0));
    let (p, i) = sel(key(&mut s, Key::End));
    assert_eq!(p as usize * 2 + i as usize, total - 1);
    // Tab on the last page wraps to the first.
    assert_eq!(sel(key(&mut s, Key::Tab)), (0, 0));
    // PageDown / PageUp / Right / Left behave as before.
    assert_eq!(sel(key(&mut s, Key::PageDown)), (1, 0));
    assert_eq!(sel(key(&mut s, Key::PageUp)), (0, 0));
    assert_eq!(sel(key(&mut s, Key::Right)), (1, 0));
    assert_eq!(sel(key(&mut s, Key::Left)), (0, 0));
}

#[test]
fn selecting_the_current_word_regroups_and_unlock_follows_the_new_group() {
    let mut s = session(9);
    type_str(&mut s, "ㄑㄩㄢˊㄌㄧˋㄧˇㄈㄨˋ");
    // The four-syllable word 全力以赴: the current item at the end is the shortest multi-character word equal to the
    // sentence, i.e. the span of length 4 (no shorter word equals the sentence's tail).
    let u = key(&mut s, Key::Down);
    let c = u.candidates.unwrap();
    assert_eq!(c.items[0], "全力以赴");
    let u = key(&mut s, Key::Enter);
    assert_eq!(locked(&u), vec![true; 4]);
    // Select the current two-character word 全力 again from the second syllable: a new group over [0, 2); the old group
    // keeps only its remainder [2, 4).
    key(&mut s, Key::Left);
    key(&mut s, Key::Left);
    let u = key(&mut s, Key::Down);
    let c = u.candidates.unwrap();
    assert_eq!(c.items[0], "全力");
    let u = key(&mut s, Key::Enter);
    assert_eq!(locked(&u), vec![true; 4]);
    // Deleting the last syllable unlocks only the old group (its remaining member 以); 全力 stays locked.
    key(&mut s, Key::End);
    let u = key(&mut s, Key::Backspace);
    assert_eq!(locked(&u), vec![true, true, false]);
}

#[test]
fn inserting_inside_a_group_dissolves_it_and_a_failed_commit_restores_the_locks() {
    let mut s = session(9);
    type_str(&mut s, "ㄑㄩㄢˊㄌㄧˋ");
    key(&mut s, Key::Down);
    key(&mut s, Key::Digit(2)); // 權力, locked
    key(&mut s, Key::Left); // caret between 權 and 力
    let u = type_str(&mut s, "ㄧˇ");
    assert_eq!(text(&u), "全以力");
    assert_eq!(locked(&u), vec![false, false, false]);
    // A commit the host rejects returns the session to the state before the key, locks included.
    let mut s = session(9);
    type_str(&mut s, "ㄑㄩㄢˊㄌㄧˋ");
    key(&mut s, Key::Down);
    key(&mut s, Key::Digit(2));
    let seq = 100;
    let u = s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)));
    assert_eq!(u.commit.as_deref(), Some("權力"));
    let fail = CommitResult { seq, commit: Applied::Failed, comp: Applied::NotApplicable };
    let u = s.handle(seq + 1, Some(fail), Event::Ping);
    assert_eq!(text(&u), "權力");
    assert_eq!(locked(&u), vec![true, true]);
}

#[test]
fn word_candidates_off_gives_single_characters_as_before() {
    let assets = Assets::from_dictionary_text(DICT, "").unwrap();
    let mut s = Engine::new(Arc::new(assets), Config { lambda: 0.0, word_candidates: false, ..Config::default() }).new_session();
    type_str(&mut s, "ㄑㄩㄢˊㄌㄧˋ");
    let c = key(&mut s, Key::Down).candidates.unwrap();
    assert!(c.items[..c.items.len() - 1].iter().all(|x| x.chars().count() == 1)); // the last item is the typed bopomofo (ZHUYIN-OUT)
    assert_eq!(c.items[0], "力");
}

#[test]
fn digits_and_picks_beyond_the_page_do_nothing() {
    let mut s = session(2);
    type_str(&mut s, "ㄑㄩㄢˊㄌㄧˋ");
    let u = key(&mut s, Key::Down);
    assert!(u.candidates_open);
    // Page size 2: digit 3 would be the first item of the next page; it must not select.
    let u = key(&mut s, Key::Digit(3));
    assert!(u.handled && u.candidates_open, "the list stays open");
    assert_eq!(locked(&u), vec![false, false]);
    let u = s.handle(50, None, Event::CandidatePick(2));
    assert!(u.candidates_open);
    assert_eq!(locked(&u), vec![false, false]);
    // A pick inside the page still works.
    let u = s.handle(51, None, Event::CandidatePick(1));
    assert!(!u.candidates_open);
    assert_eq!(locked(&u), vec![true, true]);
}
