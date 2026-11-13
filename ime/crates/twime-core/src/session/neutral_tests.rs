//! NEUTRAL (R9-targets, NEUTRAL section): an illegal neutral-tone syllable converts with a toned reading of its
//! body, and a syllable without any character never blocks the composition. Synthetic dictionary only.
use super::*;
use crate::fuzzy::Index;
use crate::keymap::key_of;
use twime_proto::Mods;

// 也 (ㄧㄝˇ) is the most frequent single character of the body ㄧㄝ; 涼 (ㄌㄧㄤˊ) that of ㄌㄧㄤ. No ㄧㄝ˙ / ㄌㄧㄤ˙ / ㄦ˙.
const DICT: &str = "\
ㄧㄝˊ 爺 -3
ㄧㄝˇ 也 -1
ㄧㄝˋ 夜 -2
ㄧㄝˊ-ㄧㄝˊ 爺爺 -3
ㄆㄧㄠˋ 漂 -2
ㄆㄧㄠˋ 票 -3
ㄌㄧㄤˊ 涼 -1
ㄌㄧㄤˋ 亮 -2
ㄆㄧㄠˋ-ㄌㄧㄤˋ 漂亮 -2
ㄦˊ 兒 -2
ㄦˋ 二 -1
ㄉㄜ˙ 的 -1
ㄉㄜˊ 得 -2
ㄐㄧㄣ 今 -2
ㄊㄧㄢ 天 -2
";

fn sess() -> Session {
    sess_with(None, false)
}

fn sess_with(repair: Option<RepairMode>, ux: bool) -> Session {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let repair = repair.map(|mode| Repair { mode, index: Arc::new(Index::from_data_txt(&a, DICT)) });
    let e = Engine::new(Arc::new(a), Config { lambda: 0.0, candidates: true, page_size: 9, repair, ..Config::default() });
    let mut s = e.new_session();
    s.set_ux(ux);
    s
}

/// Types bopomofo symbols; ' ' is Space.
fn type_str(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for c in text.chars() {
        let k = if c == ' ' { Key::Space } else { key_of(c).unwrap() };
        last = s.on_key(KeyEvent::press(k));
        s.assert_parallel();
    }
    last
}

fn key(s: &mut Session, k: Key) -> Update {
    let u = s.on_key(KeyEvent::press(k));
    s.assert_parallel();
    u
}

fn shift(s: &mut Session, k: Key) -> Update {
    s.on_key(KeyEvent { key: k, mods: Mods(Mods::SHIFT), down: true })
}

fn text(u: &Update) -> String {
    u.composition.as_ref().map(Comp::text).unwrap_or_default()
}

fn items(s: &Session) -> Vec<String> {
    s.st.cand.as_ref().map_or(vec![], |c| c.items.iter().map(CandItem::display).collect())
}

#[test]
fn a_neutral_syllable_without_a_character_takes_the_reading_of_the_word_before_it() {
    let mut s = sess();
    // 爺爺: the word ㄧㄝˊ-ㄧㄝˊ wins over the more frequent single character 也 (ㄧㄝˇ)
    let u = type_str(&mut s, "ㄧㄝˊㄧㄝ˙");
    assert_eq!(text(&u), "爺爺");
    assert!(u.committable && u.composition.as_ref().unwrap().segments.iter().all(|g| !g.unconverted));
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("爺爺"));
    // 漂亮: ㄌㄧㄤˋ from the word, not 涼 (ㄌㄧㄤˊ, the most frequent single character)
    let mut s = sess();
    assert_eq!(text(&type_str(&mut s, "ㄆㄧㄠˋㄌㄧㄤ˙")), "漂亮");
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("漂亮"));
    // alone: the most frequent single character of the body's readings
    let mut s = sess();
    assert_eq!(text(&type_str(&mut s, "ㄧㄝ˙")), "也");
    let mut s = sess();
    assert_eq!(text(&type_str(&mut s, "ㄦ˙")), "二");
    // neutral tone typed first is the same syllable
    let mut s = sess();
    assert_eq!(text(&type_str(&mut s, "ㄧㄝˊ˙ㄧㄝ ")), "爺爺");
}

#[test]
fn a_legal_neutral_syllable_is_untouched() {
    let mut s = sess();
    assert_eq!(text(&type_str(&mut s, "ㄧㄝˇㄉㄜ˙")), "也的");
    assert_eq!(s.repair_kinds(), vec![RepairKind::Typed, RepairKind::Typed]);
}

#[test]
fn the_list_of_a_neutral_position_has_its_reading_the_other_readings_and_the_typed_bopomofo() {
    let mut s = sess();
    type_str(&mut s, "ㄧㄝˊㄧㄝ˙");
    key(&mut s, Key::Down);
    let it = items(&s);
    // the current reading first (with the word), then the other toned readings, the typed bopomofo last
    assert_eq!(it.first().map(String::as_str), Some("爺爺（ㄧㄝˊ）"));
    assert!(it.contains(&"也（ㄧㄝˇ）".to_string()) && it.contains(&"夜（ㄧㄝˋ）".to_string()), "{it:?}");
    assert_eq!(it.last().map(String::as_str), Some("ㄧㄝ˙"));
    // choosing another reading keeps it
    let i = it.iter().position(|x| x == "夜（ㄧㄝˋ）").unwrap();
    s.select(i, true);
    let u = s.update(true, None);
    assert_eq!(text(&u), "爺夜");
    assert_eq!(s.repair_kinds()[1], RepairKind::Chosen("ㄧㄝˋ".into()));
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("爺夜"));
    // choosing the typed bopomofo makes it literal
    let mut s = sess();
    type_str(&mut s, "ㄧㄝˊㄧㄝ˙");
    key(&mut s, Key::Down);
    let n = items(&s).len();
    s.select(n - 1, true);
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("爺ㄧㄝ˙"));
}

#[test]
fn a_syllable_without_any_reading_never_blocks_the_composition() {
    // ㄅㄧㄤ˙: no toned reading of the body either
    let mut s = sess();
    let u = type_str(&mut s, "ㄐㄧㄣ ㄅㄧㄤ˙ㄊㄧㄢ ");
    assert_eq!(text(&u), "今ㄅㄧㄤ˙天");
    assert!(!u.committable && u.composition.as_ref().unwrap().segments[1].unconverted);
    // Backspace / Left edit it like any syllable
    key(&mut s, Key::Left);
    let u = key(&mut s, Key::Backspace);
    assert_eq!(text(&u), "今天");
    type_str(&mut s, "ㄅㄧㄤ˙");
    key(&mut s, Key::End);
    // Enter commits what is shown and ends the composition
    let u = key(&mut s, Key::Enter);
    assert_eq!(u.commit.as_deref(), Some("今ㄅㄧㄤ˙天"));
    assert!(!u.composing);
    // what follows is a new composition, converted and committed as usual
    let u = type_str(&mut s, "ㄐㄧㄣ ");
    assert_eq!(text(&u), "今");
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("今"));
    // pending symbols still wait (Enter ignores them)
    let mut s = sess();
    type_str(&mut s, "ㄅㄧㄤ˙ㄐㄧ");
    let u = key(&mut s, Key::Enter);
    assert!(u.commit.is_none() && u.composing);
}

#[test]
fn version_3_punctuation_and_shift_letters_commit_as_shown() {
    let mut s = sess_with(None, true);
    type_str(&mut s, "ㄐㄧㄣ ㄅㄧㄤ˙");
    let u = shift(&mut s, Key::Comma);
    assert_eq!(u.commit.as_deref(), Some("今ㄅㄧㄤ˙，"));
    let mut s = sess_with(None, true);
    type_str(&mut s, "ㄅㄧㄤ˙");
    let u = shift(&mut s, Key::Letter(b'a'));
    assert_eq!(u.commit.as_deref(), Some("ㄅㄧㄤ˙"));
    assert!(u.run);
    // the neutral fallback converts there as well
    let mut s = sess_with(None, true);
    type_str(&mut s, "ㄧㄝˊㄧㄝ˙");
    let u = shift(&mut s, Key::Period);
    assert_eq!(u.commit.as_deref(), Some("爺爺。"));
}

#[test]
fn strict_paths_keep_their_rule() {
    // Shift+Enter rescue still gives the keys
    let mut s = sess_with(None, true);
    type_str(&mut s, "ㄅㄧㄤ˙");
    let u = shift(&mut s, Key::Enter);
    assert_eq!(u.commit.as_deref(), Some("1u;7"));
    // leaving the field drops a composition with a position without a character (plans 4.1 c)
    let mut s = sess_with(None, true);
    type_str(&mut s, "ㄅㄧㄤ˙");
    let u = s.handle(s.last_seq + 1, None, Event::Focus(false));
    assert!(u.commit.is_none() && !u.composing);
    // ... and commits a converted neutral fallback
    let mut s = sess_with(None, true);
    type_str(&mut s, "ㄧㄝˊㄧㄝ˙");
    let u = s.handle(s.last_seq + 1, None, Event::Focus(false));
    assert_eq!(u.commit.as_deref(), Some("爺爺"));
}

#[test]
fn with_f1_on_f1_decides() {
    for mode in [RepairMode::Auto, RepairMode::Suggest] {
        let mut s = sess_with(Some(mode), false);
        type_str(&mut s, "ㄧㄝˊㄧㄝ˙");
        let k = s.repair_kinds();
        assert!(!matches!(k[1], RepairKind::Typed), "{mode:?}");
        // the neutral extra items are F1-off only
        assert!(s.neutral_items(1, "ㄧㄝˊ").is_empty());
    }
}

#[test]
fn a_chosen_reading_survives_edits_before_it() {
    let mut s = sess();
    type_str(&mut s, "ㄧㄝˊㄧㄝ˙");
    key(&mut s, Key::Down);
    let it = items(&s);
    let i = it.iter().position(|x| x == "夜（ㄧㄝˋ）").unwrap();
    s.select(i, true);
    // insert a syllable before both, then delete it again
    key(&mut s, Key::Home);
    assert_eq!(text(&type_str(&mut s, "ㄐㄧㄣ ")), "今爺夜");
    assert_eq!(s.repair_kinds()[2], RepairKind::Chosen("ㄧㄝˋ".into()));
    key(&mut s, Key::Backspace);
    key(&mut s, Key::End);
    let u = key(&mut s, Key::Enter);
    assert_eq!(u.commit.as_deref(), Some("爺夜"));
}

#[test]
fn literal_and_unconverted_positions_commit_what_is_shown_and_teach_nothing() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let cache = Some(twime_model::cache::CacheCfg { gamma: 4.0, window: 64, cap: None, source: twime_model::cache::CacheSource::Confirmed });
    let e = Engine::new(Arc::new(a), Config { lambda: 0.0, candidates: true, page_size: 9, cache, ..Config::default() });
    let mut s = e.new_session();
    // ㄅ + Space stays literal (ZHUYIN-OUT 3.5); ㄅㄧㄤ˙ has no reading at all; 今 and the neutral fallback convert
    let u = type_str(&mut s, "ㄐㄧㄣ ㄅ ㄅㄧㄤ˙ㄧㄝˊㄧㄝ˙");
    let shown = text(&u);
    assert_eq!(shown, "今ㄅㄅㄧㄤ˙爺爺");
    let before = s.confirmed_pairs();
    let u = key(&mut s, Key::Enter);
    assert_eq!(u.commit.as_deref(), Some(shown.as_str()));
    assert!(!u.commit.unwrap().contains('\u{E000}'));
    assert_eq!(s.confirmed_pairs(), before);
}

#[test]
fn an_unconverted_position_blocks_learning_of_explicit_choices() {
    let a = Arc::new(Assets::from_dictionary_text(DICT, "").unwrap());
    let cache = Some(twime_model::cache::CacheCfg { gamma: 4.0, window: 64, cap: None, source: twime_model::cache::CacheSource::Confirmed });
    let e = Engine::new(a, Config { lambda: 0.0, candidates: true, page_size: 9, cache, ..Config::default() });
    let pick = |s: &mut Session, want: &str| {
        key(s, Key::Down);
        let i = items(s).iter().position(|x| x == want).unwrap_or_else(|| panic!("{want} not listed"));
        s.select(i, true);
    };
    for tail in ["", "ㄅㄧㄤ˙"] {
        let mut s = e.new_session();
        type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ");
        key(&mut s, Key::Left);
        pick(&mut s, "今");
        key(&mut s, Key::End);
        pick(&mut s, "天");
        type_str(&mut s, tail);
        let before = s.confirmed_pairs();
        let u = key(&mut s, Key::Enter);
        assert_eq!(u.commit.as_deref(), Some(format!("今天{tail}").as_str()));
        if tail.is_empty() {
            assert!(s.confirmed_pairs() > before, "control: two explicit choices are learnt");
        } else {
            assert_eq!(s.confirmed_pairs(), before, "with an unconverted position nothing is learnt");
        }
    }
}
