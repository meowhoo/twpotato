//! ZHUYIN-OUT (ZHUYIN-OUT-plan 3, 6.1): bopomofo typed as it is, like the Microsoft New Phonetic IME. Synthetic
//! dictionary only.
use super::*;
use crate::fuzzy::Index;
use crate::keymap::key_of;
use twime_model::emoji::EmojiTable;
use twime_proto::Mods;

const DICT: &str = "\
ㄅㄚ 八 -2
ㄅㄚˊ 拔 -2
ㄅㄚˇ 把 -2
ㄅㄚˋ 爸 -2
ㄅㄚˋ 罷 -3
ㄅㄨˋ 不 -1
ㄧㄠˋ 要 -1
ㄉㄜ˙ 的 -1
ㄨㄛˇ 我 -1
ㄊㄧㄢ 天 -2
ㄊㄧㄢ 添 -3
ㄓ 之 -2
ㄚ 啊 -3
ㄅㄛ 波 -1
ㄅㄛ 玻 -2
ㄌㄤˋ 浪 -1
ㄅㄛ-ㄌㄤˋ 波浪 -1
ㄇㄠ 貓 -2
ㄊㄚ 他 -2
ㄊㄚ 她 -3
";

fn engine_with(page_size: usize, f1: bool, emoji: bool) -> Arc<Engine> {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let repair = f1.then(|| Repair { mode: RepairMode::Suggest, index: Arc::new(Index::from_data_txt(&a, DICT)) });
    let emoji = emoji.then(|| Arc::new(EmojiTable::parse("#emoji v1 test\nㄇㄠ\t🐈 🐱\tcldr\n").unwrap()));
    Engine::new(Arc::new(a), Config { lambda: 0.0, candidates: true, page_size, repair, emoji, ..Config::default() })
}

fn sess_with(page_size: usize, f1: bool, emoji: bool) -> Session {
    let mut s = engine_with(page_size, f1, emoji).new_session();
    s.set_ux(true);
    s
}

fn sess() -> Session {
    sess_with(9, false, false)
}

/// F1 suggest, F2 words, AUTOCOMP-LIST and EMOJI together (the product's list features).
fn sess_full(page_size: usize) -> Session {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let index = Arc::new(Index::from_data_txt(&a, DICT));
    let emoji = Some(Arc::new(EmojiTable::parse("#emoji v1 test
ㄇㄠ	🐈 🐱	cldr
").unwrap()));
    let cfg = Config {
        lambda: 0.0,
        candidates: true,
        page_size,
        repair: Some(Repair { mode: RepairMode::Suggest, index: index.clone() }),
        fuzzy_words: Some(index),
        autocomp_list: true,
        emoji,
        ..Config::default()
    };
    let mut s = Engine::new(Arc::new(a), cfg).new_session();
    s.set_ux(true);
    s
}

/// Types bopomofo symbols; ' ' is Space.
fn type_str(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for c in text.chars() {
        let k = if c == ' ' { Key::Space } else { key_of(c).unwrap_or_else(|| panic!("no key for {c}")) };
        last = key(s, k);
    }
    last
}

fn key(s: &mut Session, k: Key) -> Update {
    let u = s.on_key(KeyEvent::press(k));
    s.assert_parallel();
    u
}

fn text(u: &Update) -> String {
    u.composition.as_ref().map(Comp::text).unwrap_or_default()
}

fn texts(s: &Session) -> Vec<String> {
    s.candidate_items().unwrap_or_default().into_iter().map(|x| x.0).collect()
}

/// `typed`, then Down, Up (to the last item), Enter (choose the typed bopomofo), Enter (commit): the committed text.
fn via_list(typed: &str) -> (String, String) {
    let mut s = sess();
    type_str(&mut s, typed);
    key(&mut s, Key::Down);
    assert_eq!(s.candidate_lit_flags().last(), Some(&true), "{typed}");
    key(&mut s, Key::Up);
    let u = key(&mut s, Key::Enter);
    assert!(u.commit.is_none() && u.composing && u.committable, "{typed}");
    let shown = text(&u);
    let u = key(&mut s, Key::Enter);
    (shown, u.commit.unwrap())
}

#[test]
fn format_tone_after_the_symbols_first_tone_unmarked() {
    for (typed, want) in [
        ("ㄅㄚˋ", "ㄅㄚˋ"),
        ("ㄅㄚˊ", "ㄅㄚˊ"),
        ("ㄅㄚˇ", "ㄅㄚˇ"),
        ("ㄅㄚ ", "ㄅㄚ"),
        ("ㄉㄜ˙", "ㄉㄜ˙"),
        ("˙ㄉㄜ ", "ㄉㄜ˙"), // neutral tone typed first is the same syllable
    ] {
        let (shown, commit) = via_list(typed);
        assert_eq!((shown.as_str(), commit.as_str()), (want, want), "{typed}");
        assert!(!commit.contains('\u{02C9}') && !commit.contains('\u{E000}'));
        // every character is one UTF-16 unit
        assert!(commit.chars().all(|c| c.len_utf16() == 1));
    }
}

#[test]
fn several_syllables_and_mixed_with_characters() {
    let mut s = sess();
    type_str(&mut s, "ㄅㄨˋ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    key(&mut s, Key::Enter);
    type_str(&mut s, "ㄧㄠˋ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    let u = key(&mut s, Key::Enter);
    assert_eq!(text(&u), "ㄅㄨˋㄧㄠˋ");
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("ㄅㄨˋㄧㄠˋ"));
    // a literal and a converted syllable
    let mut s = sess();
    type_str(&mut s, "ㄅㄚ ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    key(&mut s, Key::Enter);
    let u = type_str(&mut s, "ㄅㄚ ");
    assert_eq!(text(&u), "ㄅㄚ八");
    assert_eq!(s.st.conv.as_ref().unwrap().chars.iter().filter(|c| c.is_some()).count(), 1);
    let c = u.composition.as_ref().unwrap();
    assert!(!c.segments[0].unconverted && !c.segments[0].locked && c.segments[0].hint == 0);
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("ㄅㄚ八"));
}

#[test]
fn a_literal_in_the_middle_keeps_the_later_text_converting() {
    // the same as a syllable without a legal character in that place (an illegal one)
    let mut a = sess();
    type_str(&mut a, "ㄨㄛˇ");
    key(&mut a, Key::Down);
    key(&mut a, Key::End);
    key(&mut a, Key::Enter);
    let ua = type_str(&mut a, "ㄊㄧㄢ ");
    assert_eq!(text(&ua), "ㄨㄛˇ天");
    let mut b = sess();
    type_str(&mut b, "ㄨㄛˇㄅㄧㄤ ");
    let ub = type_str(&mut b, "ㄊㄧㄢ ");
    assert_eq!(text(&ub), "我ㄅㄧㄤ天");
    // the tail is converted in both
    assert_eq!(a.st.conv.as_ref().unwrap().chars.last(), b.st.conv.as_ref().unwrap().chars.last());
    assert_eq!(key(&mut a, Key::Enter).commit.as_deref(), Some("ㄨㄛˇ天"));
}

#[test]
fn a_syllable_without_a_character_lists_only_the_typed_bopomofo() {
    let mut s = sess();
    type_str(&mut s, "ㄅㄧㄤ ");
    let u = key(&mut s, Key::Down);
    assert!(u.candidates_open && texts(&s) == ["ㄅㄧㄤ"]);
    assert_eq!(key(&mut s, Key::Enter).commit, None);
    let u = key(&mut s, Key::Enter);
    assert_eq!(u.commit.as_deref(), Some("ㄅㄧㄤ"));
    // F1 suggest: the repair characters, then the typed bopomofo
    let mut s = sess_with(9, true, false);
    type_str(&mut s, "ㄅㄨㄛ ");
    key(&mut s, Key::Down);
    let t = texts(&s);
    assert!(t.len() >= 2 && t.last().map(String::as_str) == Some("ㄅㄨㄛ"), "{t:?}");
    assert_eq!(s.candidate_lit_flags().iter().filter(|f| **f).count(), 1);
    assert!(s.st.raw.is_none());
    key(&mut s, Key::End);
    key(&mut s, Key::Enter);
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("ㄅㄨㄛ"));
}

#[test]
fn the_typed_bopomofo_is_the_last_item_on_every_page_size() {
    for ps in [1usize, 4, 9] {
        let mut s = sess_with(ps, false, true);
        type_str(&mut s, "ㄇㄠ ");
        key(&mut s, Key::Down);
        let n = texts(&s).len();
        assert_eq!(texts(&s), ["貓", "🐈", "🐱", "ㄇㄠ"], "ps {ps}");
        assert_eq!(s.candidate_lit_flags(), [false, false, false, true]);
        assert_eq!(s.candidate_raw_flags(), [false, true, true, false]);
        // Up from the first item wraps to it; End goes there
        let u = key(&mut s, Key::Up);
        let c = u.candidates.unwrap();
        assert_eq!(c.page as usize * ps + c.selected as usize, n - 1, "ps {ps}");
        key(&mut s, Key::Home);
        let u = key(&mut s, Key::End);
        let c = u.candidates.unwrap();
        assert_eq!(c.page as usize * ps + c.selected as usize, n - 1);
        // a number key on its page
        let mut s = sess_with(ps, false, true);
        type_str(&mut s, "ㄇㄠ ");
        key(&mut s, Key::Down);
        for _ in 0..(n - 1) / ps {
            key(&mut s, Key::Right);
        }
        let u = key(&mut s, Key::Digit(((n - 1) % ps) as u8 + 1));
        assert!(u.commit.is_none() && u.composing, "ps {ps}");
        assert_eq!(text(&u), "ㄇㄠ");
    }
    // a CandidatePick on a page other than the first
    let mut s = sess_with(2, false, true);
    type_str(&mut s, "ㄇㄠ ");
    key(&mut s, Key::Down);
    key(&mut s, Key::Right); // page 1: 🐱 and ㄇㄠ
    let u = s.handle(s.last_seq + 1, None, Event::CandidatePick(1));
    s.assert_parallel();
    assert_eq!(text(&u), "ㄇㄠ");
    assert!(u.commit.is_none() && u.candidates.is_none());
}

#[test]
fn state_stays_consistent_and_what_is_shown_is_what_is_committed() {
    let mut s = sess();
    type_str(&mut s, "ㄅㄚ ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    let u = key(&mut s, Key::Enter);
    assert!(u.committable && s.st.lit == [true]);
    type_str(&mut s, "ㄊㄧㄢ ");
    // before the literal: insert a syllable; Left, Down on a literal does nothing
    key(&mut s, Key::Home);
    type_str(&mut s, "ㄨㄛˇ");
    assert_eq!(s.st.lit, [false, true, false]);
    key(&mut s, Key::Right);
    let u = key(&mut s, Key::Down);
    assert!(!u.candidates_open && u.composing, "a literal position does not open the list");
    key(&mut s, Key::Space);
    assert_eq!(s.comp().text(), "我ㄅㄚ天");
    // Delete / Backspace keep the vectors in step
    key(&mut s, Key::Delete);
    assert_eq!(s.st.lit, [false, true]);
    key(&mut s, Key::Backspace);
    assert_eq!(s.st.lit, [false]);
    assert!(!s.has_lit());
    // the shown text is the committed text
    type_str(&mut s, "ㄅㄚ ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    let u = key(&mut s, Key::Enter);
    let shown = text(&u);
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some(shown.as_str()));
    assert_eq!(s.context(), shown);
}

#[test]
fn nothing_is_learned_and_the_rescue_works_as_before() {
    let mut s = sess();
    type_str(&mut s, "ㄅㄚ ");
    // before the list: the raw-key rescue works
    let mut r = sess();
    type_str(&mut r, "ㄅㄚ ");
    let u = r.on_key(KeyEvent { key: Key::Enter, mods: Mods(Mods::SHIFT), down: true });
    assert_eq!(u.commit.as_deref(), Some("18 "));
    // choosing the typed bopomofo ends it, like any choice
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    key(&mut s, Key::Enter);
    let u = s.on_key(KeyEvent { key: Key::Enter, mods: Mods(Mods::SHIFT), down: true });
    assert!(u.commit.is_none() && u.composing);
    // Esc closes the list and keeps the composition
    let mut s = sess();
    type_str(&mut s, "ㄅㄚ ");
    key(&mut s, Key::Down);
    let u = key(&mut s, Key::Esc);
    assert!(u.composing && !u.candidates_open);
    key(&mut s, Key::Down);
    let u = key(&mut s, Key::Backspace);
    assert!(u.composing && !u.candidates_open);
    // a text key takes the highlighted typed bopomofo and commits everything
    let mut s = sess();
    type_str(&mut s, "ㄅㄚ ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    let u = s.on_key(KeyEvent { key: Key::Period, mods: Mods(Mods::SHIFT), down: true });
    assert_eq!(u.commit.as_deref(), Some("ㄅㄚ。"));
    // a context is kept as for any commit; no confirmed run is made
    assert_eq!(s.confirmed_pairs(), 0);
}

#[test]
fn a_lone_symbol_and_space_stays_as_typed() {
    // ㄅ Space Enter -> ㄅ
    let mut s = sess();
    let u = type_str(&mut s, "ㄅ ");
    assert!(u.committable && text(&u) == "ㄅ");
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("ㄅ"));
    // ㄏ Space ㄏ Space Enter -> ㄏㄏ
    let mut s = sess();
    type_str(&mut s, "ㄏ ㄏ ");
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("ㄏㄏ"));
    // 我 ㄉ 天
    let mut s = sess();
    type_str(&mut s, "ㄨㄛˇㄉ ㄊㄧㄢ ");
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("我ㄉ天"));
    // ㄓ and ㄚ read as characters, so they are converted
    let mut s = sess();
    assert_eq!(text(&type_str(&mut s, "ㄓ ")), "之");
    assert_eq!(text(&type_str(&mut s, "ㄚ ")), "之啊");
    // a tone typed after it is an ordinary syllable (here without a character: refused)
    let mut s = sess();
    let u = type_str(&mut s, "ㄅˋ");
    assert!(!u.committable);
    // raw policy: the keys still rescue; the typed bopomofo is not a choice
    let mut s = sess();
    type_str(&mut s, "ㄅ ");
    let u = s.on_key(KeyEvent { key: Key::Enter, mods: Mods(Mods::SHIFT), down: true });
    assert_eq!(u.commit.as_deref(), Some("1 "));
    // Down does not open a list on it; Esc empties the composition and nothing is committed
    let mut s = sess();
    type_str(&mut s, "ㄅ ");
    let u = key(&mut s, Key::Down);
    assert!(!u.candidates_open);
    let u = key(&mut s, Key::Esc);
    assert!(u.commit.is_none() && !u.composing);
    // a wrongly typed one can be removed
    let mut s = sess();
    type_str(&mut s, "ㄅ ");
    let u = key(&mut s, Key::Backspace);
    assert!(!u.composing);
    // F1 with a repair for it: left to F1 (stays unconverted, not typed-as-is)
    let mut s = sess_with(9, true, false);
    assert!(s.engine.cfg.repair.as_ref().unwrap().index.repairs("ㄅㄨㄛ").len() > 0);
    type_str(&mut s, "ㄅㄨㄛ ");
    assert!(!s.has_lit());
}

#[test]
fn a_swap_is_dissolved_by_a_literal_inside_it_and_hidden_by_one_outside() {
    use super::lexux_tests::pair;
    // a literal outside the swap: kept, hidden
    let mut s = pair();
    key(&mut s, Key::Backslash);
    assert_eq!(s.st.swaps.len(), 1);
    type_str(&mut s, "ㄅ ");
    assert!(s.has_lit() && s.lexux_spans().is_empty() && s.lexux_next().is_none() && !s.switchable());
    assert_eq!(s.st.swaps.len(), 1, "the swap is kept");
    assert!(s.lexux_hint().is_none());
    key(&mut s, Key::Backspace);
    assert!(!s.has_lit() && !s.lexux_spans().is_empty(), "removing the literal brings the hints back");
    // a literal inside the swap
    let mut s = pair();
    key(&mut s, Key::Backslash);
    key(&mut s, Key::Left);
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    key(&mut s, Key::Enter);
    assert!(s.st.swaps.is_empty() && s.st.lit == [true, false] && s.st.groups.iter().all(|&g| g == 0));
    // the vectors stay in step through an appended syllable of a completion switch
    let mut s = pair();
    type_str(&mut s, "ㄅ ");
    s.assert_parallel();
}

#[test]
fn with_a_literal_no_completion_and_no_lexicon_entry() {
    let mut s = sess();
    type_str(&mut s, "ㄅㄚ ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    key(&mut s, Key::Enter);
    assert!(!s.pop_ready());
    let mut items = vec![CandItem { text: "x".into(), span: 1, repairs: vec![], extra: vec![], raw: false, lit: false }];
    s.add_autocomp_list(&mut items);
    assert_eq!(items.len(), 1);
    // the emoji of a reading are not offered for a composition holding a literal
    let mut s = sess_with(9, false, true);
    type_str(&mut s, "ㄇㄠ ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    key(&mut s, Key::Enter);
    let mut items = vec![CandItem { text: "x".into(), span: 1, repairs: vec![], extra: vec![], raw: false, lit: false }];
    s.add_emoji(&s.repair_kinds(), &mut items);
    assert_eq!(items.len(), 1);
}

#[test]
fn a_failed_host_write_restores_the_literal() {
    let mut s = sess();
    type_str(&mut s, "ㄅㄚ ");
    key(&mut s, Key::Down);
    key(&mut s, Key::End);
    key(&mut s, Key::Enter);
    let u = key(&mut s, Key::Enter);
    assert_eq!(u.commit.as_deref(), Some("ㄅㄚ"));
    let seq = s.last_seq;
    let ack = CommitResult { seq, commit: Applied::Failed, comp: Applied::Applied };
    let u = s.handle(seq + 1, Some(ack), Event::Ping);
    s.assert_parallel();
    assert_eq!(text(&u), "ㄅㄚ");
    assert_eq!(s.st.lit, [true]);
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("ㄅㄚ"));
}

#[test]
fn every_commit_path_carries_the_text() {
    let prep = |ps: usize| {
        let mut s = sess_with(ps, false, false);
        type_str(&mut s, "ㄅㄚ ");
        key(&mut s, Key::Down);
        key(&mut s, Key::End);
        key(&mut s, Key::Enter);
        s
    };
    let mut s = prep(9);
    assert_eq!(s.handle(50, None, Event::Focus(false)).commit.as_deref(), Some("ㄅㄚ"));
    let mut s = prep(9);
    let u = s.on_key(KeyEvent { key: Key::Letter(b'a'), mods: Mods(Mods::CAPS), down: true });
    assert_eq!(u.commit.as_deref(), Some("ㄅㄚ"));
    let mut s = prep(9);
    assert_eq!(s.handle(50, None, Event::ToggleMode).commit.as_deref(), Some("ㄅㄚ"));
    // the 40th syllable: the 39 before it (a literal first) are committed
    let mut s = prep(9);
    for _ in 0..38 {
        assert!(type_str(&mut s, "ㄊㄧㄢ ").commit.is_none());
    }
    assert_eq!(s.st.syl.len(), 39);
    let u = type_str(&mut s, "ㄊㄧㄢ ");
    let t = u.commit.as_deref().unwrap();
    assert!(t.chars().count() == 40 && t.starts_with("ㄅㄚ天"));
    assert_eq!(text(&u), "天");
}

/// A fixed-seed key generator over typing, editing and list keys: the vectors stay parallel, nothing internal leaks, and in a
/// committable state without a list Enter commits what is shown.
#[test]
fn random_keys_keep_the_state_sound() {
    let alphabet: Vec<Key> = {
        let mut v: Vec<Key> = "ㄅㄚㄨㄛㄉㄜㄊㄧㄢㄓㄇㄠˊˇˋ˙".chars().map(|c| key_of(c).unwrap()).collect();
        for _ in 0..6 {
            v.push(Key::Space);
        }
        for _ in 0..3 {
            v.push(Key::Down);
        }
        v.extend([Key::Up, Key::End, Key::Enter, Key::Enter, Key::Backspace, Key::Backspace, Key::Delete, Key::Left, Key::Right, Key::Esc, Key::Home]);
        v.extend((1..=9).map(Key::Digit));
        v
    };
    let internal = |t: &str| t.contains('\u{E000}') || t.contains('\u{02C9}');
    let (mut checked, mut lit_seen) = (0usize, 0usize);
    for seed in 1..=300u64 {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let n = 20 + (next() % 40) as usize;
        let keys: Vec<Key> = (0..n).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
        let ps = [1usize, 4, 9][(seed % 3) as usize];
        let mk = || if seed % 3 == 0 { sess_full(ps) } else { sess_with(ps, seed % 2 == 0, seed % 5 == 0) };
        let mut s = mk();
        for (i, &k) in keys.iter().enumerate() {
            // the state before the key, to be replayed for the Enter check
            let u = key(&mut s, k);
            lit_seen += usize::from(s.has_lit());
            let mut strings: Vec<String> = vec![];
            strings.extend(u.commit.clone());
            strings.extend(u.composition.iter().map(Comp::text));
            strings.extend(u.candidates.iter().flat_map(|c| c.items.clone()));
            assert!(strings.iter().all(|t| !internal(t)), "seed {seed} key {i}: {strings:?}");
            if u.committable && !u.candidates_open && u.commit.is_none() && i % 7 == 0 {
                // a fresh session replays the same keys; Enter then commits the shown composition
                let mut r = mk();
                for &kk in &keys[..=i] {
                    key(&mut r, kk);
                }
                let shown = r.comp().text();
                let e = key(&mut r, Key::Enter);
                assert_eq!(e.commit.as_deref(), Some(shown.as_str()), "seed {seed} key {i}");
                checked += 1;
            }
            if u.candidates_open {
                // choosing and then committing: the committed text is what the composition showed after the choice
                let mut r = mk();
                for &kk in &keys[..=i] {
                    key(&mut r, kk);
                }
                let e = key(&mut r, Key::Enter);
                if e.commit.is_none() && e.committable && !e.candidates_open {
                    let shown = text(&e);
                    assert_eq!(key(&mut r, Key::Enter).commit.as_deref(), Some(shown.as_str()), "seed {seed} key {i} (list)");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 100 && lit_seen > 500, "{checked} {lit_seen}");
}

/// The same over the LEXUX fixture (hints, the backslash cycle, swaps, completions that append syllables): literals stay in
/// step and nothing internal leaks.
#[test]
fn random_keys_with_lexux_keep_the_state_sound() {
    use super::lexux_tests::assets;
    let alphabet: Vec<Key> = {
        let mut v: Vec<Key> = "ㄚㄚㄚㄚㄧㄅ".chars().map(|c| key_of(c).unwrap()).collect();
        v.extend([Key::Space, Key::Space, Key::Space, Key::Space, Key::Space, Key::Space, Key::Space, Key::Down, Key::Down, Key::Up, Key::End, Key::Enter, Key::Backslash, Key::Backslash, Key::Backslash, Key::Backslash, Key::Backspace, Key::Delete, Key::Left, Key::Right, Key::Esc, Key::Digit(1), Key::Digit(2)]);
        v
    };
    let mk = |auto: bool, list: bool| {
        let cfg = Config {
            lambda: 0.,
            lexux: Some(twime_model::lexux::Config { a: 100., b: auto.then_some(100.), w: false }),
            autocomp_list: list,
            autocomp: Some(twime_model::autocomp::AutoParams::DEPLOYED),
            ..Config::default()
        };
        let mut s = Engine::new(Arc::new(assets()), cfg).new_session();
        s.set_ux(true);
        s.set_lexux_peer(true);
        s
    };
    let (mut lit_seen, mut swaps_seen, mut both) = (0usize, 0usize, 0usize);
    for seed in 1..=150u64 {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let n = 20 + (next() % 30) as usize;
        let keys: Vec<Key> = (0..n).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
        let mut s = mk(seed % 2 == 0, seed % 3 == 0);
        for (i, &k) in keys.iter().enumerate() {
            let u = key(&mut s, k);
            lit_seen += usize::from(s.has_lit());
            swaps_seen += usize::from(!s.st.swaps.is_empty());
            both += usize::from(s.has_lit() && !s.st.swaps.is_empty());
            let mut strings: Vec<String> = vec![];
            strings.extend(u.commit.clone());
            strings.extend(u.composition.iter().map(Comp::text));
            strings.extend(u.candidates.iter().flat_map(|c| c.items.clone()));
            strings.extend(u.hint.iter().flat_map(|h| h.options.clone()));
            assert!(strings.iter().all(|t| !t.contains('\u{E000}') && !t.contains('\u{02C9}')), "seed {seed} key {i}: {strings:?}");
            // a literal hides every hint
            assert!(!s.has_lit() || (s.lexux_spans().is_empty() && u.hint.is_none()), "seed {seed} key {i}");
            if u.committable && !u.candidates_open && u.commit.is_none() && i % 5 == 0 {
                let mut r = mk(seed % 2 == 0, seed % 3 == 0);
                for &kk in &keys[..=i] {
                    key(&mut r, kk);
                }
                let shown = r.comp().text();
                assert_eq!(key(&mut r, Key::Enter).commit.as_deref(), Some(shown.as_str()), "seed {seed} key {i}");
            }
        }
    }
    assert!(lit_seen > 300 && swaps_seen > 10, "{lit_seen} {swaps_seen} {both}");
}
