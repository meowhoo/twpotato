//! FUZZY F1 functional matrix (FUZZY-plan 3.8): illegal-syllable repair in `auto` and `suggest`, and the off
//! behaviour. Synthetic dictionary only.
use super::*;
use crate::fuzzy::Index;
use crate::keymap::key_of;

const DICT: &str = "\
ㄅㄛ 波 -1
ㄅㄛ 玻 -2
ㄌㄤˋ 浪 -1
ㄌㄤˋ 埌 -3
ㄅㄛ-ㄌㄤˋ 波浪 -1
ㄉㄚˋ 達 -0.5
ㄉㄚˋ 大 -1
ㄐㄩ 居 -1
ㄐㄩ 拘 -2
ㄓㄨ 豬 -1
ㄓㄨ 朱 -1.5
ㄓㄨ 居 -4
ㄐㄩ-ㄉㄚˋ 居大 -1
ㄊㄞˊ 台 -1
ㄊㄞˊ 臺 -2
ㄊㄞˊ 抬 -1.5
ㄊㄚ 她 -1
ㄊㄚ 他 -2
";

#[derive(Clone, Copy, Debug, PartialEq)]
enum M {
    Off,
    Auto,
    Suggest,
}
const MODES: [M; 3] = [M::Off, M::Auto, M::Suggest];

fn assets() -> Assets {
    Assets::from_dictionary_text(DICT, "").unwrap()
}

fn engine_from(a: Assets, m: M) -> Arc<Engine> {
    let index = Arc::new(Index::from_data_txt(&a, DICT));
    let repair = match m {
        M::Off => None,
        M::Auto => Some(Repair { mode: RepairMode::Auto, index }),
        M::Suggest => Some(Repair { mode: RepairMode::Suggest, index }),
    };
    Engine::new(Arc::new(a), Config { lambda: 0.0, candidates: true, page_size: 9, repair, ..Config::default() })
}

fn sess(m: M) -> Session {
    engine_from(assets(), m).new_session()
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

fn text(u: &Update) -> String {
    u.composition.as_ref().map(Comp::text).unwrap_or_default()
}

/// Internal text and display string of every item of the open list.
fn items(s: &Session) -> Vec<(String, String)> {
    s.st.cand.as_ref().map_or(vec![], |c| c.items.iter().map(|i| (i.text.clone(), i.display())).collect())
}

fn pick(s: &mut Session, display: &str) -> Update {
    let i = s.st.cand.as_ref().unwrap().items.iter().position(|x| x.display() == display).unwrap_or_else(|| panic!("{display} not listed"));
    s.select(i, true);
    s.update(true, None)
}

#[test]
fn off_keeps_the_old_behaviour() {
    let mut s = sess(M::Off);
    let u = type_str(&mut s, "ㄅㄨㄛ ㄌㄤˋ");
    assert_eq!(text(&u), "ㄅㄨㄛ浪");
    assert!(!u.committable);
    key(&mut s, Key::Left);
    let u = key(&mut s, Key::Down);
    // ZHUYIN-OUT: the list holds only the typed bopomofo; Esc closes it and keeps the composition
    assert!(u.candidates_open && s.candidate_items().unwrap().len() == 1);
    assert!(key(&mut s, Key::Esc).composing);
    // NEUTRAL: Enter commits what is shown (it used to be refused)
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("ㄅㄨㄛ浪"));
}

#[test]
fn auto_converts_first_middle_last_and_several() {
    for (typed, want) in [("ㄅㄨㄛ ㄌㄤˋ", "波浪"), ("ㄉㄚˋㄅㄨㄛ ㄌㄤˋ", "達波浪"), ("ㄉㄚˋㄐㄨ ", "達居"), ("ㄐㄨ ㄉㄚˋㄅㄨㄛ ", "居達波")] {
        let mut s = sess(M::Auto);
        let u = type_str(&mut s, typed);
        assert_eq!(text(&u), want, "{typed}");
        assert!(u.committable);
        assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some(want));
    }
}

#[test]
fn empty_repair_set_stays_unconverted() {
    let a = assets();
    let ix = Index::from_data_txt(&a, DICT);
    assert!(ix.repairs("ㄇㄧㄝˊ").is_empty());
    for m in MODES {
        let mut s = sess(m);
        let u = type_str(&mut s, "ㄉㄚˋㄇㄧㄝˊ");
        assert_eq!(text(&u), "達ㄇㄧㄝˊ", "{m:?}");
        assert!(!u.committable);
        // ZHUYIN-OUT: only the typed bopomofo is listed
        key(&mut s, Key::Down);
        assert_eq!(s.candidate_items().unwrap().iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["ㄇㄧㄝˊ"], "{m:?}");
    }
}

#[test]
fn auto_list_has_the_repair_segment_and_reselection_changes_the_reading() {
    let mut s = sess(M::Auto);
    assert_eq!(text(&type_str(&mut s, "ㄐㄨ ")), "居");
    let u = key(&mut s, Key::Down);
    assert!(u.candidates_open);
    let it = items(&s);
    // current list first (the repaired reading shown), then the other repair syllable's characters
    assert_eq!(it[0], ("居".into(), "居（ㄐㄩ）".into()));
    assert!(it.contains(&("豬".into(), "豬（ㄓㄨ）".into())));
    assert!(it.iter().position(|x| x.1 == "拘（ㄐㄩ）").unwrap() < it.iter().position(|x| x.1 == "豬（ㄓㄨ）").unwrap());
    // same character under two readings: two items
    assert!(it.contains(&("居".into(), "居（ㄓㄨ）".into())));
    // the shell sees the display strings
    assert_eq!(u.candidates.as_ref().unwrap().items[0], "居（ㄐㄩ）");
    let u = pick(&mut s, "豬（ㄓㄨ）");
    assert_eq!(text(&u), "豬");
    assert_eq!(s.eff_syl(), vec!["ㄓㄨ".to_string()]);
    assert_eq!(s.st.conv.as_ref().unwrap().syllables(), &["ㄓㄨ".to_string()]);
    // reopen: the current reading first, the other syllable after
    key(&mut s, Key::Down);
    let it = items(&s);
    assert_eq!(it[0].1, "豬（ㄓㄨ）");
    assert!(it.iter().any(|x| x.1 == "居（ㄐㄩ）"));
    assert_eq!(it.iter().filter(|x| x.1 == "居（ㄓㄨ）").count(), 1);
    // same character, other reading: the reading changes, the lock follows
    let u = pick(&mut s, "居（ㄐㄩ）");
    assert_eq!(text(&u), "居");
    assert_eq!(s.eff_syl(), vec!["ㄐㄩ".to_string()]);
    // commit text has no reading
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("居"));
}

#[test]
fn reselecting_a_reading_unlocks_the_old_group_first() {
    let mut s = sess(M::Auto);
    type_str(&mut s, "ㄐㄨ ㄉㄚˋ");
    // a word over the repaired position: one group locks both positions
    key(&mut s, Key::Down);
    let u = pick(&mut s, "居大（ㄐㄩ）");
    assert_eq!(text(&u), "居大");
    assert_eq!(s.st.locks, vec![Some('居'), Some('大')]);
    assert_eq!(s.st.groups[0], s.st.groups[1]);
    // another reading at the repaired position: the old group goes first (大 unlocked), then 豬 is locked alone
    key(&mut s, Key::Left);
    key(&mut s, Key::Down);
    let u = pick(&mut s, "豬（ㄓㄨ）");
    assert_eq!(s.st.locks, vec![Some('豬'), None]);
    assert_eq!(s.st.groups[1], 0);
    assert_eq!(text(&u), "豬達");
    // the same reading again (current list item) keeps the other locks
    key(&mut s, Key::End);
    key(&mut s, Key::Down);
    pick(&mut s, "大");
    key(&mut s, Key::Left);
    key(&mut s, Key::Down);
    pick(&mut s, "朱（ㄓㄨ）");
    assert_eq!(s.st.locks, vec![Some('朱'), Some('大')]);
    // backspace the repaired syllable: its repair entry goes with it
    key(&mut s, Key::End);
    key(&mut s, Key::Left);
    let u = key(&mut s, Key::Backspace);
    assert_eq!(text(&u), "大");
    assert_eq!(s.st.repair, vec![None]);
}

#[test]
fn edits_after_the_repaired_position_keep_it() {
    for m in [M::Auto, M::Suggest] {
        let mut s = sess(m);
        type_str(&mut s, "ㄐㄨ ");
        key(&mut s, Key::Down);
        pick(&mut s, "豬（ㄓㄨ）");
        let u = type_str(&mut s, "ㄉㄚˋㄉㄚˋ");
        assert_eq!(text(&u), "豬達達", "{m:?}");
        key(&mut s, Key::Backspace);
        key(&mut s, Key::Left);
        let u = key(&mut s, Key::Delete);
        assert_eq!(text(&u), "豬");
        assert_eq!(s.st.repair, vec![Some("ㄓㄨ".to_string())]);
    }
}

#[test]
fn repair_moves_with_insertions_and_deletions() {
    for m in [M::Auto, M::Suggest] {
        let mut s = sess(m);
        type_str(&mut s, "ㄐㄨ ");
        if m == M::Suggest {
            assert_eq!(text(&s.update(true, None)), "ㄐㄨ");
            key(&mut s, Key::Down);
        } else {
            key(&mut s, Key::Down);
        }
        pick(&mut s, "豬（ㄓㄨ）");
        key(&mut s, Key::Home);
        let u = type_str(&mut s, "ㄉㄚˋ");
        assert_eq!(text(&u), "達豬", "{m:?}");
        assert_eq!(s.st.repair, vec![None, Some("ㄓㄨ".to_string())]);
        key(&mut s, Key::Home);
        let u = key(&mut s, Key::Delete);
        assert_eq!(text(&u), "豬");
        assert_eq!(s.st.repair, vec![Some("ㄓㄨ".to_string())]);
    }
}

#[test]
fn suggest_opens_the_repair_list_and_can_choose_again() {
    let mut s = sess(M::Suggest);
    let u = type_str(&mut s, "ㄐㄨ ㄉㄚˋ");
    assert_eq!(text(&u), "ㄐㄨ達");
    assert!(!u.committable);
    // at the end (a legal position) the normal list
    key(&mut s, Key::Down);
    assert!(items(&s).iter().all(|x| x.0 == x.1));
    key(&mut s, Key::Esc);
    key(&mut s, Key::Left);
    let u = key(&mut s, Key::Down);
    assert!(u.candidates_open);
    let it = items(&s);
    // every repair syllable in R(t) order (S-b ㄐㄩ before K ㄓㄨ)
    assert_eq!(it.first().unwrap().1, "居（ㄐㄩ）");
    assert!(it.iter().position(|x| x.1 == "拘（ㄐㄩ）").unwrap() < it.iter().position(|x| x.1 == "豬（ㄓㄨ）").unwrap());
    let u = pick(&mut s, "居（ㄐㄩ）");
    assert_eq!(text(&u), "居達");
    assert!(u.committable);
    // chosen: reopen = current list + the other syllables
    key(&mut s, Key::Down);
    let it = items(&s);
    assert_eq!(it[0].1, "居（ㄐㄩ）");
    let u = pick(&mut s, "朱（ㄓㄨ）");
    assert_eq!(text(&u), "朱達");
    assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("朱達"));
}

#[test]
fn failed_commit_restores_the_repair() {
    for m in [M::Auto, M::Suggest] {
        let mut s = sess(m);
        type_str(&mut s, "ㄐㄨ ");
        key(&mut s, Key::Down);
        pick(&mut s, "豬（ㄓㄨ）");
        let seq = s.last_seq + 1;
        let u = s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)));
        assert_eq!(u.commit.as_deref(), Some("豬"));
        let u = s.handle(seq + 1, Some(CommitResult { seq, commit: Applied::Failed, comp: Applied::Failed }), Event::Ping);
        assert_eq!(text(&u), "豬", "{m:?}");
        assert_eq!(s.st.repair, vec![Some("ㄓㄨ".to_string())]);
        assert!(u.committable);
    }
}

#[test]
fn norm_style_follows_the_effective_syllable() {
    // ㄞˊ is illegal here; K (w -> ˇ is dropped, ㄞ ˊ stays) reaches it from ㄊㄞˊ
    let mut a = assets();
    a.set_norm(twime_model::NormCfg::ALL);
    let e = engine_from(a, M::Suggest);
    let mut s = e.new_session();
    s.handle(1, None, Event::SetContext("臺北".into()));
    assert!(s.st.style_tai);
    type_str(&mut s, "ㄞˊ");
    key(&mut s, Key::Down);
    let it = items(&s);
    // 臺 preferred by the style: first of the pair
    let p = |d: &str| it.iter().position(|x| x.1 == d).unwrap();
    assert!(p("臺（ㄊㄞˊ）") + 1 == p("台（ㄊㄞˊ）"));
    // choosing 台 where 臺 fits the effective syllable sets the style to 台
    let u = pick(&mut s, "台（ㄊㄞˊ）");
    assert_eq!(text(&u), "台");
    assert!(!s.st.style_tai);
}

#[test]
fn ta_default_acts_on_a_repaired_composition() {
    // ㄊㄧ is illegal here; K (ㄚ -> ㄧ) reaches it from ㄊㄚ
    let mut a = assets();
    a.set_ta_default(true);
    let mut s = engine_from(a, M::Auto).new_session();
    let u = type_str(&mut s, "ㄊㄧ ");
    assert_eq!(s.eff_syl(), vec!["ㄊㄚ".to_string()]);
    assert_eq!(text(&u), "他");
}

#[test]
fn syllable_limit_with_a_repair() {
    let mut s = sess(M::Auto);
    for _ in 0..19 {
        type_str(&mut s, "ㄐㄨ ㄉㄚˋ");
    }
    type_str(&mut s, "ㄐㄨ ");
    assert_eq!(s.st.syl.len(), MAX_SYLLABLES);
    // the next syllable commits the repaired composition first
    let u = type_str(&mut s, "ㄐㄨ ");
    assert_eq!(u.commit, Some("居達".repeat(19) + "居"));
    assert_eq!(text(&u), "居");
    assert_eq!(s.st.repair, vec![None]);
}

#[test]
fn user_words_cross_a_repaired_position_and_custom_locks_hold() {
    let mut a = assets();
    a.set_user_rules(true, twime_model::user::DEFAULT_BONUS).unwrap();
    let syl = |x: &str| x.split(' ').map(String::from).collect::<Vec<_>>();
    // forced 3-character word over the repaired position; 2-character word with a custom reading (豬 is not ㄐㄩ)
    let w = vec![a.user_word_opt("波浪大", &syl("ㄅㄛ ㄌㄤˋ ㄉㄚˋ"), None).unwrap(), a.user_word_opt("豬浪", &syl("ㄐㄩ ㄌㄤˋ"), None).unwrap()];
    let lex = a.build_user_lexicon(w);
    a.set_user(Some(Arc::new(lex)));
    let e = engine_from(a, M::Auto);
    let mut s = e.new_session();
    // forced word over the repaired position
    assert_eq!(text(&type_str(&mut s, "ㄅㄨㄛ ㄌㄤˋㄉㄚˋ")), "波浪大");
    key(&mut s, Key::Esc);
    // the custom-reading user word in a repaired composition: listed, chosen, its lock accepted
    type_str(&mut s, "ㄐㄨ ㄌㄤˋ");
    key(&mut s, Key::Down);
    let it = items(&s);
    assert!(it.contains(&("豬浪".into(), "豬浪（ㄐㄩ）".into())), "{it:?}");
    let u = pick(&mut s, "豬浪（ㄐㄩ）");
    assert_eq!(text(&u), "豬浪");
    assert!(u.committable);
    // the typed syllables of an illegal position are never covered by a user word (the vocabulary has legal syllables
    // only), the legal ones are when a whole reading matches
    assert_eq!(e.assets().user_cover(&syl("ㄐㄨ ㄌㄤˋ")), vec![false, false]);
    assert_eq!(e.assets().user_cover(&syl("ㄉㄚˋ ㄐㄩ ㄌㄤˋ")), vec![false, true, true]);
}

#[test]
fn add_to_lexicon_uses_the_effective_syllables() {
    let a = Arc::new(assets());
    let index = Arc::new(Index::from_data_txt(&a, DICT));
    let dir = std::env::temp_dir().join(format!("twime-core-fuzzy-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let service = twime_userdata::Service::open(dir.clone(), a.clone(), None);
    let writer = service.spawn_writer();
    let cfg = Config { lambda: 0.0, repair: Some(Repair { mode: RepairMode::Auto, index }), ..Config::default() };
    let e = Engine::with_user(a, cfg, Some(crate::UserLink { service, writer }), twime_proto::ToggleKey::None);
    let mut s = e.new_session();
    s.set_ux(true);
    type_str(&mut s, "ㄐㄨ ㄉㄚˋ");
    let u = s.handle(s.last_seq + 1, None, Event::AddToLexicon);
    assert_eq!(u.notice.as_deref(), Some("已送出加入詞庫：居達"));
    key(&mut s, Key::Esc);
    type_str(&mut s, "ㄉㄚˋㄓㄨ ");
    let u = s.handle(s.last_seq + 1, None, Event::AddToLexicon);
    assert_eq!(u.notice.as_deref(), Some("已送出加入詞庫：達豬"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_to_lexicon_after_a_suggest_choice() {
    let a = Arc::new(assets());
    let index = Arc::new(Index::from_data_txt(&a, DICT));
    let dir = std::env::temp_dir().join(format!("twime-core-fuzzy-s-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let service = twime_userdata::Service::open(dir.clone(), a.clone(), None);
    let writer = service.spawn_writer();
    let cfg = Config { lambda: 0.0, repair: Some(Repair { mode: RepairMode::Suggest, index }), ..Config::default() };
    let e = Engine::with_user(a, cfg, Some(crate::UserLink { service, writer }), twime_proto::ToggleKey::None);
    let mut s = e.new_session();
    s.set_ux(true);
    type_str(&mut s, "ㄐㄨ ㄉㄚˋ");
    // not chosen yet: not committable, refused
    let u = s.handle(s.last_seq + 1, None, Event::AddToLexicon);
    assert_eq!(u.notice.as_deref(), Some("組字未完成"));
    key(&mut s, Key::Left);
    key(&mut s, Key::Down);
    pick(&mut s, "豬（ㄓㄨ）");
    let u = s.handle(s.last_seq + 1, None, Event::AddToLexicon);
    assert_eq!(u.notice.as_deref(), Some("已送出加入詞庫：豬達"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A tiny random model (the model path, not the fallback) with the synthetic dictionary.
fn model_assets() -> Assets {
    let base = assets();
    let n = twime_model::vocab::Vocab::new(base.dictionary()).len();
    Assets::from_dictionary_model(DICT, "", twime_model::n1::N1::random(n, 32, 1, 7)).unwrap()
}

/// Address of the inference behind the current conversion (the same while an inference is reused).
fn inf_id(s: &Session) -> usize {
    s.st.conv.as_ref().unwrap().syllables().as_ptr() as usize
}

#[test]
fn inference_is_reused_only_while_the_effective_syllables_stay() {
    for m in MODES {
        let mut s = engine_from(model_assets(), m).new_session();
        type_str(&mut s, "ㄉㄚˋㄉㄚˋ");
        let a = inf_id(&s);
        assert!(!s.st.conv.as_ref().unwrap().fallback);
        // opening the list and choosing on clean input: no new inference
        key(&mut s, Key::Down);
        assert_eq!(inf_id(&s), a);
        key(&mut s, Key::Digit(2));
        assert_eq!(inf_id(&s), a, "{m:?}");
        // a completed syllable: a new inference
        type_str(&mut s, "ㄐㄨ ");
        let b = inf_id(&s);
        assert_ne!(a, b);
        if m == M::Off {
            continue;
        }
        if m == M::Suggest {
            assert!(!s.st.conv.as_ref().unwrap().complete());
        }
        key(&mut s, Key::Down);
        assert_eq!(inf_id(&s), b, "opening the list costs no inference");
        if m == M::Auto {
            // a current-reading item: the effective syllables stay, the inference is reused
            pick(&mut s, "拘（ㄐㄩ）");
            assert_eq!(inf_id(&s), b);
            key(&mut s, Key::Down);
        }
        // another reading: one new inference with the new effective syllables
        pick(&mut s, "豬（ㄓㄨ）");
        let c = inf_id(&s);
        assert_ne!(c, b);
        assert_eq!(s.st.conv.as_ref().unwrap().syllables().last().map(String::as_str), Some("ㄓㄨ"));
    }
}

#[test]
fn pn_word_crosses_a_repaired_position() {
    let mut a = model_assets();
    let base = std::env::temp_dir().join(format!("twime-core-fuzzy-pn-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&base).unwrap();
    let lexb = base.join("lex");
    let words = vec![twime_model::pn::BuildWord { word: "波浪大".into(), reading: "ㄅㄛ-ㄌㄤˋ-ㄉㄚˋ".into(), class: 0 }];
    twime_model::pn::build(&a, &words, &lexb).unwrap();
    let lex = twime_model::pn::PnLexicon::open(&lexb, &a).unwrap();
    a.set_pn(Some(Arc::new(lex)), twime_model::pn::PnCfg { b2: Some(30.0), b3: Some(30.0), h: None, cands: true });
    let e = engine_from(a, M::Auto);
    let mut s = e.new_session();
    // the same PN word on the typed legal syllables (the setup works), then over the repaired position
    assert_eq!(text(&type_str(&mut s, "ㄅㄛ ㄌㄤˋㄉㄚˋ")), "波浪大");
    key(&mut s, Key::Esc);
    assert_eq!(text(&type_str(&mut s, "ㄅㄨㄛ ㄌㄤˋㄉㄚˋ")), "波浪大");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn clean_event_sequences_are_identical_in_every_mode() {
    // FUZZY 3.7 1(d) within one build: no illegal syllable, every Update equal
    let script: Vec<Event> = {
        let mut v = vec![Event::SetContext("臺灣".into())];
        let keys = |t: &str| t.chars().map(|c| Event::Key(KeyEvent::press(if c == ' ' { Key::Space } else { key_of(c).unwrap() }))).collect::<Vec<_>>();
        v.extend(keys("ㄉㄚˋㄐㄩ ㄊㄞˊ"));
        v.extend([Key::Down, Key::Down, Key::Digit(2), Key::Left, Key::Down, Key::Right, Key::Esc, Key::Home].map(|k| Event::Key(KeyEvent::press(k))));
        v.extend(keys("ㄓㄨ ㄊㄚ "));
        v.extend([Key::Backspace, Key::End, Key::Space, Key::Digit(1), Key::Enter].map(|k| Event::Key(KeyEvent::press(k))));
        v
    };
    let run = |m: M| -> Vec<String> {
        let mut s = sess(m);
        script.iter().enumerate().map(|(i, ev)| format!("{:?}", s.handle(i as u32 + 1, None, ev.clone()))).collect()
    };
    let off = run(M::Off);
    assert_eq!(run(M::Auto), off);
    assert_eq!(run(M::Suggest), off);
}
