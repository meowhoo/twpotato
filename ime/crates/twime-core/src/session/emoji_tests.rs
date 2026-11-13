//! EMOJI (EMOJI-plan 4 V1): emoji at the end of the candidate list and their choice. Synthetic dictionary and table.
use super::*;
use crate::keymap::key_of;
use twime_model::emoji::EmojiTable;
use twime_proto::Mods;

const DICT: &str = "\
ㄞˋ 愛 -2
ㄞˋ 礙 -3
ㄒㄧㄣ 心 -2
ㄞˋ-ㄒㄧㄣ 愛心 -2
ㄇㄠ 貓 -2
ㄇㄠ 錨 -2.1
ㄇㄠ 蝥 -2.2
ㄇㄠ 氂 -2.3
ㄇㄠ 犛 -2.4
ㄇㄠ 髦 -2.5
ㄨㄛˇ 我 -1
ㄅㄧㄥ 冰 -2
";

const HEART: &str = "\u{2764}\u{fe0f}";

fn table_text() -> String {
    format!(
        "#emoji v1 test\n\
         ㄞˋ-ㄒㄧㄣ\t{HEART} 🤎 🫶\tcldr\n\
         ㄇㄠ\t🐈 🐱 😼\tcldr\n\
         ㄒㄧㄣ\t🆕\tcldr\n\
         ㄅㄚˋ-ㄒㄧㄣ\t🧪\tcldr\n\
         ㄞˋ-ㄞˋ-ㄞˋ-ㄞˋ-ㄞˋ-ㄞˋ\t🔢\tcldr\n\
         ㄞˋ-ㄞˋ-ㄞˋ-ㄞˋ-ㄞˋ-ㄞˋ-ㄞˋ\t🔣\tcldr\n\
         ㄅㄧㄥ\t冰 🧊\tcldr\n"
    )
}

fn sess_with(table: Option<&str>, page_size: usize) -> Session {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let emoji = table.map(|t| Arc::new(EmojiTable::parse(t).unwrap()));
    let mut s = Engine::new(Arc::new(a), Config { lambda: 0.0, candidates: true, page_size, emoji, ..Config::default() }).new_session();
    s.set_ux(true);
    s
}

fn sess(on: bool, page_size: usize) -> Session {
    sess_with(on.then(table_text).as_deref(), page_size)
}

/// Types bopomofo symbols; ' ' is Space.
fn type_str(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for c in text.chars() {
        let k = if c == ' ' { Key::Space } else { key_of(c).unwrap() };
        last = s.on_key(KeyEvent::press(k));
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

fn texts(s: &Session) -> Vec<String> {
    s.candidate_items().unwrap_or_default().into_iter().map(|x| x.0).collect()
}

/// The emoji items of the open list.
fn emoji_of(s: &Session) -> Vec<String> {
    texts(s).into_iter().zip(s.candidate_raw_flags()).filter(|(_, r)| *r).map(|(t, _)| t).collect()
}

fn index_of(s: &Session, emoji: &str) -> usize {
    texts(s).iter().position(|t| t == emoji).unwrap_or_else(|| panic!("{emoji} not listed"))
}

/// How many items `add_emoji` would add to a one-item list at the current state.
fn added(s: &Session) -> usize {
    let mut items = vec![CandItem { text: "x".into(), span: 1, repairs: vec![], extra: vec![], raw: false, lit: false }];
    s.add_emoji(&s.repair_kinds(), &mut items);
    items.len() - 1
}

const LOVE: &str = "ㄞˋㄒㄧㄣ ";

#[test]
fn off_and_missing_table_change_nothing() {
    for key_syl in [LOVE, "ㄇㄠ ", "ㄨㄛˇ"] {
        let (mut off, mut on) = (sess(false, 9), sess(true, 9));
        type_str(&mut off, key_syl);
        type_str(&mut on, key_syl);
        let (uo, un) = (key(&mut off, Key::Down), key(&mut on, Key::Down));
        let strip: Vec<_> = on.candidate_identities().into_iter().zip(on.candidate_raw_flags()).filter(|(_, r)| !*r).map(|(i, _)| i).collect();
        // E3: the on list without its emoji is the off list
        assert_eq!(strip, off.candidate_identities(), "{key_syl}");
        assert!(off.candidate_raw_flags().iter().all(|r| !*r));
        assert_eq!(uo.composition, un.composition);
        assert_eq!(uo.candidates.as_ref().unwrap().items.first(), un.candidates.as_ref().unwrap().items.first());
    }
    // an engine whose table is missing is the off engine (Config.emoji = None)
    let mut s = sess_with(None, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    assert!(emoji_of(&s).is_empty());
}

#[test]
fn hit_puts_emoji_at_the_very_end() {
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    let u = key(&mut s, Key::Down);
    let t = texts(&s);
    assert_eq!(t.first().map(String::as_str), Some("愛心"));
    assert_eq!(&t[t.len() - 4..t.len() - 1], [HEART.to_string(), "🤎".to_string(), "🫶".to_string()]);
    assert_eq!(emoji_of(&s).len(), 3);
    assert_eq!(u.candidates.as_ref().unwrap().total as usize, t.len());
    // the shown string is the emoji itself
    key(&mut s, Key::End);
    let u = key(&mut s, Key::Home);
    assert_eq!(u.candidates.as_ref().unwrap().selected, 0);
    // a one-syllable key: at most 3, a dictionary word with the same text is not repeated
    let mut s = sess(true, 9);
    type_str(&mut s, "ㄅㄧㄥ ");
    key(&mut s, Key::Down);
    assert_eq!(emoji_of(&s), ["🧊"]);
    assert_eq!(texts(&s).iter().filter(|t| *t == "冰").count(), 1);
}

#[test]
fn number_key_and_enter_commit_the_emoji() {
    for by_enter in [false, true] {
        let mut s = sess(true, 9);
        type_str(&mut s, LOVE);
        key(&mut s, Key::Down);
        let i = index_of(&s, HEART);
        let seq = s.last_seq + 1;
        let u = if by_enter {
            for _ in 0..i {
                key(&mut s, Key::Down);
            }
            s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)))
        } else {
            assert!(i < 9);
            s.handle(seq, None, Event::Key(KeyEvent::press(Key::Digit(i as u8 + 1))))
        };
        assert_eq!(u.commit.as_deref(), Some(HEART), "enter={by_enter}");
        assert!(u.composition.is_none() && u.candidates.is_none() && !u.committable && !u.candidates_open);
        assert!(s.st.syl.is_empty() && !s.st.composing());
        // the commit is remembered as context like any committed text (scalars of the emoji)
        assert_eq!(s.st.prev.iter().collect::<String>(), HEART);
        // the next word is typed as usual
        let u = type_str(&mut s, "ㄨㄛˇ");
        assert_eq!(text(&u), "我");
        assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("我"));
    }
}

#[test]
fn a_failed_host_write_restores_the_composition() {
    for digit in [true, false] {
        let mut s = sess(true, 9);
        type_str(&mut s, LOVE);
        key(&mut s, Key::Down);
        let i = index_of(&s, "🤎");
        let seq = s.last_seq + 1;
        let u = if digit {
            s.handle(seq, None, Event::Key(KeyEvent::press(Key::Digit(i as u8 + 1))))
        } else {
            for _ in 0..i {
                key(&mut s, Key::Down);
            }
            let seq = s.last_seq + 1;
            s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)))
        };
        assert_eq!(u.commit.as_deref(), Some("🤎"));
        let seq = s.last_seq;
        let u = s.handle(seq + 1, Some(CommitResult { seq, commit: Applied::Failed, comp: Applied::Failed }), Event::Ping);
        assert_eq!(text(&u), "愛心", "digit={digit}");
        assert!(s.st.prev.is_empty());
        assert_eq!(s.st.syl.len(), 2);
    }
    // a mouse pick too
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    let i = index_of(&s, "🫶");
    let seq = s.last_seq + 1;
    let u = s.handle(seq, None, Event::CandidatePick(i as u16));
    assert_eq!(u.commit.as_deref(), Some("🫶"));
    let u = s.handle(seq + 1, Some(CommitResult { seq, commit: Applied::Failed, comp: Applied::Failed }), Event::Ping);
    assert_eq!(text(&u), "愛心");
    assert!(s.st.prev.is_empty());
}

#[test]
fn mouse_pick_commits_the_emoji() {
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    let i = index_of(&s, "🤎");
    let u = s.handle(s.last_seq + 1, None, Event::CandidatePick(i as u16));
    assert_eq!(u.commit.as_deref(), Some("🤎"));
    assert!(u.composition.is_none() && u.candidates.is_none());
}

#[test]
fn trigger_conditions() {
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    assert_eq!(added(&s), 3);
    // the cursor is not at the end
    s.st.cursor = 1;
    assert_eq!(added(&s), 0);
    s.st.cursor = 2;
    assert_eq!(added(&s), 3);
    // a syllable is half typed
    type_str(&mut s, "ㄅ");
    assert_eq!(added(&s), 0);
    // no password field
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    s.password = true;
    assert_eq!(added(&s), 0);
    // an illegal syllable (not in the dictionary) never gets emoji, though the table has its reading
    let mut s = sess(true, 9);
    type_str(&mut s, "ㄅㄚˋㄒㄧㄣ ");
    assert_eq!(s.st.syl, ["ㄅㄚˋ", "ㄒㄧㄣ"]);
    assert_eq!(added(&s), 0);
    // 6 syllables are looked up, 7 are not, 0 never
    let mut s = sess(true, 9);
    type_str(&mut s, &"ㄞˋ".repeat(6));
    assert_eq!(added(&s), 1);
    type_str(&mut s, "ㄞˋ");
    assert_eq!(s.st.syl.len(), 7);
    assert_eq!(added(&s), 0);
    assert_eq!(added(&sess(true, 9)), 0);
    // an empty list gets nothing
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    let mut none = vec![];
    s.add_emoji(&s.repair_kinds(), &mut none);
    assert!(none.is_empty());
    // the POP / completion list path never adds
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    s.pop_opening = true;
    s.open_candidates();
    s.pop_opening = false;
    assert!(s.st.cand.as_ref().unwrap().items.iter().all(|i| !i.raw));
}

#[test]
fn repaired_or_suggested_positions_get_no_emoji() {
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    let typed = vec![RepairKind::Typed, RepairKind::Typed];
    let mut items = vec![CandItem { text: "x".into(), span: 1, repairs: vec![], extra: vec![], raw: false, lit: false }];
    s.add_emoji(&typed, &mut items);
    assert_eq!(items.len(), 4);
    for other in [RepairKind::Chosen("ㄞ".into()), RepairKind::Auto("ㄞ".into()), RepairKind::Unchosen] {
        for at in 0..2 {
            let mut kinds = typed.clone();
            kinds[at] = other.clone();
            let mut items = vec![CandItem { text: "x".into(), span: 1, repairs: vec![], extra: vec![], raw: false, lit: false }];
            s.add_emoji(&kinds, &mut items);
            assert_eq!(items.len(), 1, "{other:?} at {at}");
        }
    }
    // a suggestion of FUZZY F2 that the user chose (repair set at a legal position)
    s.st.repair[0] = Some("ㄞ".into());
    assert_eq!(added(&s), 0);
    s.st.repair[0] = None;
    assert_eq!(added(&s), 3);
}

#[test]
fn a_user_lock_does_not_matter() {
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    key(&mut s, Key::Digit(1)); // choose the word: its positions are locked
    assert!(s.st.locks.iter().any(Option::is_some));
    key(&mut s, Key::Down);
    assert_eq!(emoji_of(&s).len(), 3);
}

#[test]
fn text_that_equals_an_ordinary_item_is_skipped() {
    let mut s = sess_with(Some("#emoji v1\nㄇㄠ\t貓 🐈\tcldr\n"), 9);
    type_str(&mut s, "ㄇㄠ ");
    key(&mut s, Key::Down);
    assert_eq!(emoji_of(&s), ["🐈"]);
    assert_eq!(texts(&s).iter().filter(|t| *t == "貓").count(), 1);
}

#[test]
fn page_sizes_and_short_lists() {
    for ps in [1usize, 4, 9] {
        for want in ["🐈", "🐱", "😼"] {
            let mut s = sess(true, ps);
            type_str(&mut s, "ㄇㄠ ");
            key(&mut s, Key::Down);
            let i = index_of(&s, want);
            assert_eq!(s.candidate_items().unwrap().len(), 10); // with the typed bopomofo (ZHUYIN-OUT) last
            // page forward to the page of the emoji, then its number key
            for _ in 0..i / ps {
                key(&mut s, Key::Right);
            }
            let u = key(&mut s, Key::Digit((i % ps) as u8 + 1));
            assert_eq!(u.commit.as_deref(), Some(want), "ps {ps}");
            assert!(s.st.syl.is_empty());
        }
    }
    // a short list: the emoji sits on the first page
    let mut s = sess(true, 9);
    type_str(&mut s, "ㄒㄧㄣ ");
    key(&mut s, Key::Down);
    assert_eq!(texts(&s), ["心", "🆕", "ㄒㄧㄣ"]);
    assert_eq!(key(&mut s, Key::Digit(2)).commit.as_deref(), Some("🆕"));
    // the number key of an item that is not there does nothing, as before
    let mut s = sess(true, 9);
    type_str(&mut s, "ㄒㄧㄣ ");
    key(&mut s, Key::Down);
    let u = key(&mut s, Key::Digit(5));
    assert!(u.commit.is_none() && s.st.cand.is_some());
}

#[test]
fn highlighted_emoji_then_punctuation_or_shift_letter() {
    // punctuation: the emoji and the mark are committed together, both enter the context
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    for _ in 0..index_of(&s, HEART) {
        key(&mut s, Key::Down);
    }
    let seq = s.last_seq + 1;
    let u = s.handle(seq, None, Event::Key(KeyEvent { key: Key::Comma, mods: Mods(Mods::SHIFT), down: true }));
    assert_eq!(u.commit.as_deref(), Some(format!("{HEART}，").as_str()));
    assert_eq!(s.st.prev.iter().collect::<String>(), format!("{HEART}，"));
    assert!(u.composition.is_none());
    // a failed write restores the whole composition
    let u = s.handle(seq + 1, Some(CommitResult { seq, commit: Applied::Failed, comp: Applied::Failed }), Event::Ping);
    assert_eq!(text(&u), "愛心");
    assert!(s.st.prev.is_empty());
    // Shift+letter: the emoji is committed and an English run begins
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    for _ in 0..index_of(&s, HEART) {
        key(&mut s, Key::Down);
    }
    let u = shift(&mut s, Key::Letter(b'a'));
    assert_eq!(u.commit.as_deref(), Some(HEART));
    assert_eq!(s.st.run, "A");
    assert_eq!(s.st.prev.iter().collect::<String>(), HEART);
    // an ordinary highlighted item: unchanged (the head is the committed sentence)
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    let u = shift(&mut s, Key::Comma);
    assert_eq!(u.commit.as_deref(), Some("愛心，"));
}

#[test]
fn emoji_context_is_unknown_characters_for_the_next_conversion() {
    // `prev` holds the scalars; committing an emoji then typing converts without trouble and like committing other unknown text
    let mut a = sess(true, 9);
    type_str(&mut a, LOVE);
    key(&mut a, Key::Down);
    let i = index_of(&a, "🤎");
    key(&mut a, Key::Digit(i as u8 + 1));
    let ua = type_str(&mut a, "ㄨㄛˇ");
    let mut b = sess(false, 9);
    b.st.prev = "🤎".chars().collect();
    let ub = type_str(&mut b, "ㄨㄛˇ");
    assert_eq!(text(&ua), text(&ub));
    // a two-scalar emoji (with the variation selector) is two context characters, the context keeps its cap
    let mut s = sess(true, 9);
    s.st.prev = vec!['x'; CONTEXT - 1];
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    let i = index_of(&s, HEART);
    key(&mut s, Key::Digit(i as u8 + 1));
    assert_eq!(s.st.prev.len(), CONTEXT);
    assert!(s.st.prev.ends_with(&['\u{2764}', '\u{fe0f}']));
}

#[test]
fn select_refuses_an_emoji_item() {
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    let i = index_of(&s, HEART);
    s.select(i, true);
    // nothing was locked or committed
    assert!(s.st.locks.iter().all(Option::is_none));
    assert_eq!(s.st.syl.len(), 2);
}

/// Emoji and U+FE0F are not in the model's vocabulary: both map to UNK, the id of every unknown character.
#[test]
fn emoji_characters_are_unknown_to_the_model_vocabulary() {
    let (dict, _) = twime_model::lexicon::Dictionary::parse(DICT);
    let v = twime_model::vocab::Vocab::new(&dict);
    for c in ['\u{1F600}', '\u{2764}', '\u{FE0F}', '\u{1F441}', 'α'] {
        assert_eq!(v.char_id(c), twime_model::vocab::UNK, "{c:?}");
    }
    assert_ne!(v.char_id('貓'), twime_model::vocab::UNK);
}

/// With a real (zero-layer) model in the loop: committing an emoji is, for the next conversion, the same as having committed
/// as many other unknown characters as the emoji has scalars (`😀` one, `❤️` two), and a conversion never fails after it.
#[test]
fn next_conversion_after_an_emoji_equals_that_after_unknown_characters() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("twime-emoji-model-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("data.txt"), DICT).unwrap();
    std::fs::write(dir.join("aliases.txt"), "").unwrap();
    let (dict, _) = twime_model::lexicon::Dictionary::parse(DICT);
    let n = twime_model::vocab::Vocab::new(&dict).len();
    let mut b = b"TWG1".to_vec();
    for v in [n as u32, 32, 0, 32] {
        b.extend(v.to_le_bytes());
    }
    b.extend((0..n * 32).flat_map(|i| (((i % 7) as f32 - 3.0) * 0.1).to_le_bytes()));
    b.extend((0..32).flat_map(|_| 1f32.to_le_bytes()));
    std::fs::write(dir.join("model.bin"), b).unwrap();
    let paths = twime_model::ModelPaths { data_txt: dir.join("data.txt"), aliases: dir.join("aliases.txt"), model: dir.join("model.bin"), prior: None };
    let make = |table: Option<String>| {
        let assets = Assets::load(&paths).unwrap();
        let emoji = table.map(|t| Arc::new(EmojiTable::parse(&t).unwrap()));
        let mut s = Engine::new(Arc::new(assets), Config { lambda: 0.0, candidates: true, emoji, ..Config::default() }).new_session();
        s.set_ux(true);
        s
    };
    for (emoji, unknown) in [("\u{1F600}", "α"), ("\u{2764}\u{FE0F}", "αβ"), ("\u{1F441}\u{FE0F}", "γδ")] {
        let table = format!("#emoji v1\nㄞˋ-ㄒㄧㄣ\t{emoji}\tcldr\n");
        let mut a = make(Some(table));
        type_str(&mut a, LOVE);
        key(&mut a, Key::Down);
        let i = index_of(&a, emoji);
        assert_eq!(key(&mut a, Key::Digit(i as u8 + 1)).commit.as_deref(), Some(emoji));
        assert_eq!(a.st.prev.len(), emoji.chars().count());
        let ua = type_str(&mut a, "ㄨㄛˇ");
        let mut b = make(None);
        b.st.prev = unknown.chars().collect();
        b.st.recent_output = true;
        let ub = type_str(&mut b, "ㄨㄛˇ");
        assert_eq!(text(&ua), text(&ub), "{emoji:?}");
        assert_eq!(ua.composition, ub.composition, "{emoji:?}");
        assert_eq!(key(&mut a, Key::Enter).commit, key(&mut b, Key::Enter).commit);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The model input of the next conversion: the context characters become token ids by `n1::encode`; an emoji (and U+FE0F) must give
/// UNK, one per scalar, exactly like other unknown characters, while a known character does not (so the comparison can tell).
#[test]
fn context_tokens_after_an_emoji_are_unk_per_scalar() {
    use twime_model::{n1, vocab::{UNK, Vocab}};
    let (dict, _) = twime_model::lexicon::Dictionary::parse(DICT);
    let v = Vocab::new(&dict);
    let syl = vec!["ㄨㄛˇ".to_string()];
    let (base, first) = n1::encode(&v, "", &syl);
    assert_eq!(first, 1);
    for (emoji, unknown) in [("\u{1F600}", "α"), ("\u{2764}\u{FE0F}", "αβ"), ("\u{1F441}\u{FE0F}", "γδ")] {
        let (a, fa) = n1::encode(&v, emoji, &syl);
        let (b, fb) = n1::encode(&v, unknown, &syl);
        assert_eq!((&a, fa), (&b, fb), "{emoji:?}");
        assert_eq!(fa, emoji.chars().count() + 1);
        assert!(a[..fa - 1].iter().all(|&t| t == UNK) && a[fa..] == base[first..]);
    }
    // control: a known character is not UNK, so the equalities above are not vacuous
    assert_ne!(n1::encode(&v, "貓", &syl).0[0], UNK);
    // and the session hands the committed emoji to that encoder as the context it keeps (the scalars of the emoji)
    let mut s = sess(true, 9);
    type_str(&mut s, LOVE);
    key(&mut s, Key::Down);
    let i = index_of(&s, HEART);
    key(&mut s, Key::Digit(i as u8 + 1));
    let ctx: String = s.st.prev.iter().collect();
    assert_eq!(ctx, HEART);
    assert_eq!(n1::encode(&v, &ctx, &syl).0, [UNK, UNK, n1::encode(&v, "", &syl).0[0], n1::encode(&v, "", &syl).0[1]]);
}
