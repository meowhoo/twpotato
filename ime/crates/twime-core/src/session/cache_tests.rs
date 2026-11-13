//! CACHE: the document snapshot serves one composition; every commit, `clear_composition` and
//! `fresh` drops it; a failed commit restores it with the state before the key.

use super::*;
use crate::keymap::key_of;

const DICT: &str = "\
ㄐㄧㄣ 今 -2
ㄐㄧㄣ 金 -2.5
ㄊㄧㄢ 天 -2
ㄊㄧㄢ 添 -3
ㄍㄨㄛˊ 國 -2
";

fn engine(cache: bool) -> Arc<Engine> {
    let assets = Assets::from_dictionary_text(DICT, "").unwrap();
    let cache = cache.then_some(twime_model::cache::CacheCfg { gamma: 4.0, window: 64, cap: None, source: Default::default() });
    Engine::new(Arc::new(assets), Config { lambda: 1.0, candidates: true, page_size: 2, cache, ..Config::default() })
}

fn type_str(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for c in text.chars() {
        let key = if c == ' ' { Key::Space } else { key_of(c).unwrap() };
        last = s.on_key(KeyEvent::press(key));
    }
    last
}

fn ev(s: &mut Session, e: Event) -> Update {
    let ack = s.pending.as_ref().map(|p| CommitResult { seq: p.seq, commit: Applied::Applied, comp: Applied::Applied });
    let seq = s.last_seq + 1;
    s.handle(seq, ack, e)
}

const DOC: &str = "今天金天";

#[test]
fn snapshot_serves_the_next_composition_only() {
    let mut s = engine(true).new_session();
    ev(&mut s, Event::DocText(DOC.into()));
    assert_eq!(s.doc_pairs(), (Some(3), None));
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    assert_eq!(s.doc_pairs(), (Some(3), Some(3)));
    // ignored while composing
    ev(&mut s, Event::DocText(String::new()));
    assert_eq!(s.doc_pairs(), (Some(3), Some(3)));
    // a commit drops it; the next composition without a new snapshot has no cache
    let u = s.on_key(KeyEvent::press(Key::Enter));
    assert!(u.commit.is_some());
    assert_eq!(s.doc_pairs(), (None, None));
    type_str(&mut s, "ㄐㄧㄣ ");
    assert_eq!(s.doc_pairs(), (None, None));
    // Esc (clear_composition) drops it
    s.on_key(KeyEvent::press(Key::Esc));
    ev(&mut s, Event::DocText(DOC.into()));
    type_str(&mut s, "ㄐㄧㄣ ");
    s.on_key(KeyEvent::press(Key::Esc));
    assert_eq!(s.doc_pairs(), (None, None));
    // deleting the composition to empty drops it
    ev(&mut s, Event::DocText(DOC.into()));
    type_str(&mut s, "ㄐㄧㄣ ");
    s.on_key(KeyEvent::press(Key::Backspace));
    assert_eq!(s.doc_pairs(), (None, None));
    ev(&mut s, Event::DocText(DOC.into()));
    type_str(&mut s, "ㄐ");
    s.on_key(KeyEvent::press(Key::Backspace));
    assert_eq!(s.doc_pairs(), (None, None));
    // an empty snapshot is no snapshot
    ev(&mut s, Event::DocText("今".into()));
    assert_eq!(s.doc_pairs(), (None, None));
}

#[test]
fn fresh_states_drop_it() {
    for e in [Event::Focus(true), Event::Focus(false), Event::ExternalEdit, Event::CompositionAborted, Event::Unsynced, Event::SetContext("今天".into()), Event::Password(false)] {
        let mut s = engine(true).new_session();
        ev(&mut s, Event::DocText(DOC.into()));
        type_str(&mut s, "ㄐㄧㄣ");
        ev(&mut s, e.clone());
        assert_eq!(s.doc_pairs(), (None, None), "{e:?}");
    }
}

#[test]
fn no_cache_settings_or_password_ignore_it() {
    let mut s = engine(false).new_session();
    ev(&mut s, Event::DocText(DOC.into()));
    assert_eq!(s.doc_pairs(), (None, None));
    let mut s = engine(true).new_session();
    ev(&mut s, Event::Password(true));
    ev(&mut s, Event::DocText(DOC.into()));
    assert_eq!(s.doc_pairs(), (None, None));
}

#[test]
fn the_40th_syllable_commit_drops_it_before_the_new_conversion() {
    let mut s = engine(true).new_session();
    ev(&mut s, Event::DocText(DOC.into()));
    for _ in 0..39 {
        type_str(&mut s, "ㄊㄧㄢ ");
    }
    assert_eq!(s.doc_pairs().1, Some(3));
    let u = type_str(&mut s, "ㄐㄧㄣ ");
    assert!(u.commit.is_some() && u.composing);
    assert_eq!(s.doc_pairs(), (None, None));
}

#[test]
fn a_failed_commit_restores_the_snapshot_and_unsynced_drops_it() {
    let mut s = engine(true).new_session();
    ev(&mut s, Event::DocText(DOC.into()));
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    let seq = s.last_seq + 1;
    let ack = s.pending.as_ref().map(|p| CommitResult { seq: p.seq, commit: Applied::Applied, comp: Applied::Applied });
    let u = s.handle(seq, ack, Event::Key(KeyEvent::press(Key::Enter)));
    assert!(u.commit.is_some());
    assert_eq!(s.doc_pairs(), (None, None));
    let fail = CommitResult { seq, commit: Applied::Failed, comp: Applied::NotApplicable };
    let u = s.handle(seq + 1, Some(fail), Event::Ping);
    assert!(u.composing);
    assert_eq!(s.doc_pairs(), (Some(3), Some(3)));
    ev(&mut s, Event::Unsynced);
    assert_eq!(s.doc_pairs(), (None, None));
}

#[test]
fn without_doc_text_the_cache_engine_equals_the_plain_engine() {
    let (mut a, mut b) = (engine(true).new_session(), engine(false).new_session());
    for t in ["ㄐㄧㄣ ㄊㄧㄢ ", "ㄍㄨㄛˊ"] {
        assert_eq!(type_str(&mut a, t), type_str(&mut b, t));
    }
    assert_eq!(a.on_key(KeyEvent::press(Key::Enter)), b.on_key(KeyEvent::press(Key::Enter)));
}

// ---- CACHE-c: only runs of characters explicitly chosen in the candidate window.

fn engine_confirmed() -> Arc<Engine> {
    let assets = Assets::from_dictionary_text(DICT, "").unwrap();
    let cache = Some(twime_model::cache::CacheCfg { gamma: 4.0, window: 64, cap: None, source: twime_model::cache::CacheSource::Confirmed });
    Engine::new(Arc::new(assets), Config { lambda: 1.0, candidates: true, page_size: 2, cache, ..Config::default() })
}

/// Opens the list after position `j` and takes `item` explicitly (`CandidatePick`), or by a number key with `digit`.
fn choose(s: &mut Session, j: usize, item: &str, digit: bool) {
    s.on_key(KeyEvent::press(Key::Home));
    for _ in 0..=j {
        s.on_key(KeyEvent::press(Key::Right));
    }
    let u = s.on_key(KeyEvent::press(Key::Down));
    let i = u.candidates.expect("candidate list").items.iter().position(|x| x == item).expect("item on the first page");
    if digit {
        s.on_key(KeyEvent::press(Key::Digit(i as u8 + 1)));
    } else {
        ev(s, Event::CandidatePick(i as u16));
    }
    s.on_key(KeyEvent::press(Key::End));
}

/// Types 今天 (ㄐㄧㄣ ㄊㄧㄢ), chooses 金 and 添 explicitly, commits.
fn type_choose_commit(s: &mut Session) -> String {
    type_str(s, "ㄐㄧㄣ ㄊㄧㄢ ");
    choose(s, 0, "金", true);
    choose(s, 1, "添", false);
    s.on_key(KeyEvent::press(Key::Enter)).commit.expect("commit")
}

#[test]
fn repeated_explicit_choices_stay_bounded_and_preserve_confirmed_runs() {
    for digit in [true, false] {
        let mut s = engine_confirmed().new_session();
        type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
        assert_eq!(s.st.syl.len(), 2);
        // Keep an adjacent explicit choice while replacing the first position's group.
        choose(&mut s, 1, "添", digit);
        for i in 0..1_000 {
            choose(&mut s, 0, if i % 2 == 0 { "金" } else { "今" }, digit);
            assert_eq!(s.st.syl.len(), 2);
            assert!(
                s.st.explicit.len() <= s.st.syl.len(),
                "choice {}, digit={digit}: {} explicit groups for {} syllables",
                i + 1,
                s.st.explicit.len(),
                s.st.syl.len()
            );
        }
        assert_eq!(s.st.explicit.len(), 2);
        assert_eq!(s.on_key(KeyEvent::press(Key::Enter)).commit.as_deref(), Some("今添"));
        assert_eq!(s.confirmed_pairs(), 1);
        ev(&mut s, Event::DocText("今天今添".into()));
        assert_eq!(s.doc_pairs(), (Some(1), None));
        type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
        assert_eq!(s.doc_pairs(), (Some(1), Some(1)));
    }
}

#[test]
fn confirmed_records_only_explicit_runs() {
    let mut s = engine_confirmed().new_session();
    // an untouched conversion records nothing
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    let t = s.on_key(KeyEvent::press(Key::Enter)).commit.unwrap();
    assert_eq!(s.confirmed_pairs(), 0);
    ev(&mut s, Event::DocText(t));
    assert_eq!(s.doc_pairs(), (None, None));
    // one chosen character next to an unchosen one: no run
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    choose(&mut s, 1, "添", true);
    assert_eq!(s.on_key(KeyEvent::press(Key::Enter)).commit.as_deref(), Some("今添"));
    assert_eq!(s.confirmed_pairs(), 0);
    // two adjacent chosen characters: a run; the cache is its pair where the whole run is in the document
    assert_eq!(type_choose_commit(&mut s), "金添");
    assert_eq!(s.confirmed_pairs(), 1);
    ev(&mut s, Event::DocText("今天金添".into()));
    assert_eq!(s.doc_pairs(), (Some(1), None));
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    assert_eq!(s.doc_pairs(), (Some(1), Some(1)));
    s.on_key(KeyEvent::press(Key::Esc));
    // the run is no longer in the text before the cursor (deleted, another document): no cache, and it is forgotten
    ev(&mut s, Event::DocText("今天金天".into()));
    assert_eq!(s.doc_pairs(), (None, None));
    assert_eq!(s.confirmed_pairs(), 0);
    ev(&mut s, Event::DocText("金添".into()));
    assert_eq!(s.doc_pairs(), (None, None));
}

#[test]
fn confirmed_ignores_implicit_and_undone_choices() {
    let mut s = engine_confirmed().new_session();
    s.set_ux(true);
    // the highlighted item taken by a full-width mark (version 3 text key) is not a choice
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    choose(&mut s, 0, "金", true);
    s.on_key(KeyEvent::press(Key::Down));
    s.on_key(KeyEvent::press(Key::Down)); // highlight moves to the second item
    let u = s.on_key(KeyEvent { key: Key::Comma, mods: twime_proto::Mods(twime_proto::Mods::SHIFT), down: true });
    assert!(u.commit.as_deref().is_some_and(|t| t.starts_with("金")));
    assert_eq!(s.confirmed_pairs(), 0);
    // a choice undone by Backspace and retyped without a choice
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    choose(&mut s, 0, "金", true);
    choose(&mut s, 1, "添", true);
    s.on_key(KeyEvent::press(Key::Backspace));
    type_str(&mut s, "ㄊㄧㄢ ");
    s.on_key(KeyEvent::press(Key::Enter)).commit.expect("commit");
    assert_eq!(s.confirmed_pairs(), 0);
}

#[test]
fn confirmed_lifetime() {
    // kept by host edits and failed or unconfirmed commits; cleared by focus, close, password and a new context
    for (e, kept) in [
        (Event::ExternalEdit, true),
        (Event::Unsynced, true),
        (Event::CompositionAborted, true),
        (Event::Focus(true), false),
        (Event::Focus(false), false),
        (Event::Close, false),
        (Event::Password(false), false),
        (Event::SetContext(String::new()), false),
    ] {
        let mut s = engine_confirmed().new_session();
        type_choose_commit(&mut s);
        ev(&mut s, e.clone());
        assert_eq!(s.confirmed_pairs(), usize::from(kept), "{e:?}");
    }
    // a failed commit: the record stays, harmless because the run is not in the document
    let mut s = engine_confirmed().new_session();
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
    choose(&mut s, 0, "金", true);
    choose(&mut s, 1, "添", true);
    let seq = s.last_seq + 1;
    let ack = s.pending.as_ref().map(|p| CommitResult { seq: p.seq, commit: Applied::Applied, comp: Applied::Applied });
    assert!(s.handle(seq, ack, Event::Key(KeyEvent::press(Key::Enter))).commit.is_some());
    let fail = CommitResult { seq, commit: Applied::Failed, comp: Applied::NotApplicable };
    s.handle(seq + 1, Some(fail), Event::Ping);
    assert_eq!(s.confirmed_pairs(), 1);
    ev(&mut s, Event::Unsynced);
    ev(&mut s, Event::DocText("今天".into()));
    assert_eq!((s.doc_pairs(), s.confirmed_pairs()), ((None, None), 0));
    // the plain document cache and an engine without a cache never record
    for eng in [engine(true), engine(false)] {
        let mut s = eng.new_session();
        type_choose_commit(&mut s);
        assert_eq!(s.confirmed_pairs(), 0);
    }
}

#[test]
fn confirmed_without_choices_equals_the_plain_engine() {
    let (mut a, mut b) = (engine_confirmed().new_session(), engine(false).new_session());
    for _ in 0..3 {
        ev(&mut a, Event::DocText(DOC.into()));
        assert_eq!(type_str(&mut a, "ㄐㄧㄣ ㄊㄧㄢ "), type_str(&mut b, "ㄐㄧㄣ ㄊㄧㄢ "));
        assert_eq!(a.on_key(KeyEvent::press(Key::Enter)), b.on_key(KeyEvent::press(Key::Enter)));
    }
}

#[test]
fn confirmed_another_document_sharing_a_run_without_focus_events() {
    // plan 8th edition 2.2 (Codex r8): a document that contains an old run but not the session's anchor (its last commit of
    // >= 4 characters) gets nothing, and the session forgets everything
    let mut s = engine_confirmed().new_session();
    type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ㄐㄧㄣ ㄊㄧㄢ ");
    choose(&mut s, 0, "金", true);
    choose(&mut s, 1, "添", true);
    assert_eq!(s.on_key(KeyEvent::press(Key::Enter)).commit.as_deref(), Some("金添今天"));
    assert_eq!(s.confirmed_pairs(), 1);
    ev(&mut s, Event::DocText("天天金添今天".into()));
    assert_eq!(s.doc_pairs(), (Some(1), None), "same document: the anchor and the run are there");
    ev(&mut s, Event::DocText("今天金添".into()));
    assert_eq!((s.doc_pairs(), s.confirmed_pairs()), ((None, None), 0), "another document with the short run only");
    ev(&mut s, Event::DocText("天天金添今天".into()));
    assert_eq!(s.doc_pairs(), (None, None), "forgotten for good");
}
