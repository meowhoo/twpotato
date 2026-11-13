//! FUZZY F2 functional matrix (FUZZY-F2-plan 3.4): word-level fuzzy suggestions. Synthetic dictionary only.
use super::super::*;
use super::place;
use crate::fuzzy::Index;
use crate::keymap::key_of;

const DICT: &str = "\
ㄉㄤˇ 黨 -2
ㄉㄤˇ 檔 -3
ㄉㄢˇ 膽 -2.5
ㄉㄢˋ 蛋 -2.5
ㄢˋ 案 -2
ㄢˋ 岸 -2.5
ㄢˋ 暗 -2.6
ㄢˋ 按 -2.7
ㄢ 安 -2
ㄉㄤˇ-ㄢˋ 檔案 -3
ㄉㄤˇ-ㄢˋ 黨案 -3.2
ㄉㄢˋ-ㄢˋ 蛋案 -3.5
ㄉㄢˇ-ㄢ 膽安 -3.6
ㄕˊ 時 -1
ㄕˊ 實 -1.5
ㄐㄧㄢ 間 -1
ㄐㄧㄢˋ 踐 -3
ㄕˊ-ㄐㄧㄢ 時間 -1
ㄕˊ-ㄐㄧㄢˋ 實踐 -3
ㄊㄞˊ 台 -1
ㄊㄞˊ 臺 -2
ㄊㄞ 胎 -2
ㄨㄢ 灣 -1
ㄊㄞˊ-ㄨㄢ 台灣 -1
ㄐㄩ 居 -1
ㄐㄩ-ㄉㄤˇ-ㄢˋ 居檔案 -3
";

fn assets() -> Assets {
    Assets::from_dictionary_text(DICT, "").unwrap()
}

fn engine_with(a: Assets, repair: Option<RepairMode>, fuzzy: bool, page_size: usize) -> Arc<Engine> {
    let index = Arc::new(Index::from_data_txt(&a, DICT));
    let repair = repair.map(|mode| Repair { mode, index: index.clone() });
    let fuzzy_words = fuzzy.then_some(index);
    Engine::new(Arc::new(a), Config { lambda: 0.0, candidates: true, page_size, repair, fuzzy_words, ..Config::default() })
}

fn sess(repair: Option<RepairMode>, fuzzy: bool) -> Session {
    engine_with(assets(), repair, fuzzy, 9).new_session()
}

const REPAIRS: [Option<RepairMode>; 3] = [None, Some(RepairMode::Auto), Some(RepairMode::Suggest)];

fn keys(text: &str) -> Vec<Key> {
    text.chars().map(|c| if c == ' ' { Key::Space } else { key_of(c).unwrap() }).collect()
}

fn type_str(s: &mut Session, text: &str) -> Update {
    let mut last = Update::default();
    for k in keys(text) {
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

/// (internal text, span, repairs, display) of the open list.
fn list(s: &Session) -> Vec<(String, usize, Vec<(usize, String)>, String)> {
    s.st.cand.as_ref().map_or(vec![], |c| c.items.iter().map(|i| (i.text.clone(), i.span, i.repairs.clone(), i.display())).collect())
}

fn shown(s: &Session) -> Vec<String> {
    list(s).into_iter().map(|x| x.3).collect()
}

fn pick(s: &mut Session, display: &str) -> Update {
    let i = s.st.cand.as_ref().unwrap().items.iter().position(|x| x.display() == display).unwrap_or_else(|| panic!("{display} not listed: {:?}", shown(s)));
    s.select(i, true);
    s.update(true, None)
}

#[test]
fn typed_dan_an_lists_dangan_and_the_choice_locks_and_commits_the_word() {
    for r in REPAIRS {
        let mut s = sess(r, true);
        let u = type_str(&mut s, "ㄉㄢˇㄢˋ");
        let before = text(&u);
        assert_ne!(before, "檔案");
        key(&mut s, Key::Down);
        let v = shown(&s);
        let i = v.iter().position(|x| x == "檔案（ㄉㄤˇ）").unwrap_or_else(|| panic!("{r:?} {v:?}"));
        assert!(i < 9 && i > 0, "first page, after the current item");
        let u = pick(&mut s, "檔案（ㄉㄤˇ）");
        assert_eq!(text(&u), "檔案", "{r:?}");
        assert!(u.committable);
        assert_eq!(s.st.repair, vec![Some("ㄉㄤˇ".to_string()), None]);
        assert_eq!(s.eff_syl(), vec!["ㄉㄤˇ".to_string(), "ㄢˋ".to_string()]);
        assert!(s.st.locks.iter().all(Option::is_some));
        // a new conversion (typing on, then Backspace) keeps the changed reading
        let u = type_str(&mut s, "ㄕˊ");
        assert!(text(&u).starts_with("檔案"), "{r:?}");
        let u = key(&mut s, Key::Backspace);
        assert_eq!(text(&u), "檔案");
        // the committed text has no reading marker
        assert_eq!(key(&mut s, Key::Enter).commit.as_deref(), Some("檔案"));
        assert!(s.st.repair.is_empty());
    }
}

#[test]
fn off_lists_nothing_extra_and_on_only_adds_marked_items() {
    for r in REPAIRS {
        for t in ["ㄉㄢˇㄢˋ", "ㄕˊㄐㄧㄢ ", "ㄊㄞ ㄨㄢ ", "ㄉㄤˇㄢˋ"] {
            let mut off = sess(r, false);
            let mut on = sess(r, true);
            type_str(&mut off, t);
            type_str(&mut on, t);
            key(&mut off, Key::Down);
            key(&mut on, Key::Down);
            let (a, b) = (list(&off), list(&on));
            assert!(a.iter().all(|x| x.2.is_empty() || r == Some(RepairMode::Auto)), "{t} off has no marker");
            // the off list is the on list without the added items (same order), and every added item is marked
            let mut rest = b.iter().filter(|x| !a.contains(x));
            assert!(rest.all(|x| !x.2.is_empty() && x.3.ends_with('）')), "{t}");
            let kept: Vec<_> = b.iter().filter(|x| a.contains(x)).cloned().collect();
            assert_eq!(kept, a, "{t}");
        }
    }
}

#[test]
fn strong_hits_end_the_first_page_and_others_follow_it() {
    let e = engine_with(assets(), None, true, 5);
    let mut s = e.new_session();
    type_str(&mut s, "ㄉㄢˇㄢˋ");
    key(&mut s, Key::Down);
    let v = shown(&s);
    // normal: the current item and three other ㄢˋ characters; strong (typed reading has no word): 檔案 黨案 蛋案;
    // the fourth hit 膽安 goes after the first page
    let p = |d: &str| v.iter().position(|x| x == d).unwrap_or_else(|| panic!("{d} {v:?}"));
    assert_eq!((p("檔案（ㄉㄤˇ）"), p("黨案（ㄉㄤˇ）"), p("蛋案（ㄉㄢˋ）")), (2, 3, 4), "{v:?}");
    assert_eq!(p("膽安（ㄢ）"), 5, "{v:?}");
    assert!(v[..2].iter().chain(&v[6..]).all(|x| !x.contains('（')));
}

#[test]
fn weak_hit_after_the_first_page() {
    // typed 時間 is frequent: 實踐 (tone at the second syllable) is not strong
    let mut s = sess(None, true);
    type_str(&mut s, "ㄕˊㄐㄧㄢ ");
    key(&mut s, Key::Down);
    let v = shown(&s);
    let n_off = v.iter().filter(|x| !x.contains('（')).count() - 1; // not the typed bopomofo (ZHUYIN-OUT)
    assert_eq!(v.iter().position(|x| x == "實踐（ㄐㄧㄢˋ）"), Some(n_off.min(9)), "{v:?}");
}

#[test]
fn place_caps_and_positions() {
    let it = |t: &str| CandItem { text: t.into(), span: 2, repairs: vec![], extra: vec![], raw: false, lit: false };
    let names = |v: &[CandItem]| v.iter().map(|x| x.text.clone()).collect::<Vec<_>>();
    let normal: Vec<CandItem> = (0..10).map(|i| it(&format!("n{i}"))).collect();
    // hits in order: s = strong, w = not strong
    let found = |spec: &str| spec.split(' ').map(|x| (it(x), x.starts_with('s'))).collect::<Vec<_>>();
    let mut v = normal.clone();
    place(&mut v, found("s0 w0 s1 s2 s3 w1 w2 w3 w4 w5 w6 w7"), 9);
    let n = names(&v);
    // A = s0 s1 s2 at the end of page 1; B = the rest in order (s3 included), 9 in all
    assert_eq!(&n[..15], ["n0", "n1", "n2", "n3", "n4", "n5", "s0", "s1", "s2", "w0", "s3", "w1", "w2", "w3", "w4"]);
    assert_eq!(n.iter().filter(|x| x.starts_with('w') || x.starts_with('s')).count(), 9, "cap 9 in all");
    assert_eq!(&n[15..], ["n6", "n7", "n8", "n9"]);
    // a short list: after the current item and the list
    let mut v = vec![it("cur")];
    place(&mut v, found("s0 s1 s2 w0 w1"), 9);
    assert_eq!(names(&v), ["cur", "s0", "s1", "s2", "w0", "w1"]);
    // small pages: A holds at most ps - 1, so the current item stays first and A stays on page 1
    let mut v = normal[..3].to_vec();
    place(&mut v, found("s0 s1 s2 w0"), 2);
    assert_eq!(names(&v), ["n0", "s0", "s1", "s2", "w0", "n1", "n2"]);
    let mut v = normal[..3].to_vec();
    place(&mut v, found("s0 s1"), 1);
    assert_eq!(names(&v), ["n0", "s0", "s1", "n1", "n2"]);
}

#[test]
fn user_words_come_first() {
    let a = assets();
    let w = a.user_word("黨暗", &["ㄉㄤˇ".to_string(), "ㄢˋ".to_string()], 30.0).unwrap();
    a.set_user(Some(Arc::new(a.build_user_lexicon([w]))));
    let mut s = engine_with(a, None, true, 9).new_session();
    type_str(&mut s, "ㄉㄢˇㄢˋ");
    key(&mut s, Key::Down);
    let v = shown(&s);
    let p = |d: &str| v.iter().position(|x| x == d).unwrap();
    assert!(p("黨暗（ㄉㄤˇ）") < p("檔案（ㄉㄤˇ）"), "{v:?}");
}

#[test]
fn word_candidates_off_and_short_runs_add_nothing() {
    let a = assets();
    let index = Arc::new(Index::from_data_txt(&a, DICT));
    let e = Engine::new(Arc::new(a), Config { lambda: 0.0, word_candidates: false, fuzzy_words: Some(index), ..Config::default() });
    let mut s = e.new_session();
    type_str(&mut s, "ㄉㄢˇㄢˋ");
    key(&mut s, Key::Down);
    assert!(shown(&s).iter().all(|x| !x.contains('（')));
    // one syllable before the cursor: no span of 2
    let mut s = sess(None, true);
    type_str(&mut s, "ㄉㄢˇㄢˋ");
    key(&mut s, Key::Left);
    key(&mut s, Key::Down);
    assert!(shown(&s).iter().all(|x| !x.contains('（')));
}

#[test]
fn changed_reading_moves_with_edits_and_survives_a_failed_commit() {
    for r in REPAIRS {
        let mut s = sess(r, true);
        type_str(&mut s, "ㄉㄢˇㄢˋ");
        key(&mut s, Key::Down);
        pick(&mut s, "檔案（ㄉㄤˇ）");
        key(&mut s, Key::Home);
        let u = type_str(&mut s, "ㄕˊ");
        assert_eq!(s.st.repair, vec![None, Some("ㄉㄤˇ".to_string()), None]);
        assert!(text(&u).ends_with("檔案"), "{r:?}");
        let u = key(&mut s, Key::Backspace);
        assert_eq!(text(&u), "檔案");
        assert_eq!(s.st.repair, vec![Some("ㄉㄤˇ".to_string()), None]);
        let seq = s.last_seq + 1;
        let u = s.handle(seq, None, Event::Key(KeyEvent::press(Key::Enter)));
        assert_eq!(u.commit.as_deref(), Some("檔案"));
        let u = s.handle(seq + 1, Some(CommitResult { seq, commit: Applied::Failed, comp: Applied::Failed }), Event::Ping);
        assert_eq!(text(&u), "檔案", "{r:?}");
        assert_eq!(s.st.repair, vec![Some("ㄉㄤˇ".to_string()), None]);
    }
}

#[test]
fn reopened_list_shows_the_reading_and_can_change_back() {
    let mut s = sess(None, true);
    type_str(&mut s, "ㄕˊㄐㄧㄢ ");
    key(&mut s, Key::Down);
    let u = pick(&mut s, "實踐（ㄐㄧㄢˋ）");
    assert_eq!(text(&u), "實踐");
    key(&mut s, Key::Down);
    let v = shown(&s);
    // items over the changed position show its reading; the typed reading comes back as a suggestion
    assert_eq!(v[0], "實踐（ㄐㄧㄢˋ）", "{v:?}");
    let u = pick(&mut s, "時間（ㄐㄧㄢ）");
    assert_eq!(text(&u), "時間");
    assert_eq!(s.eff_syl(), vec!["ㄕˊ".to_string(), "ㄐㄧㄢ".to_string()]);
}

#[test]
fn way_back_without_a_dictionary_word_of_the_typed_reading() {
    for r in REPAIRS {
        let mut s = sess(r, true);
        type_str(&mut s, "ㄉㄢˇㄢˋ");
        key(&mut s, Key::Down);
        pick(&mut s, "檔案（ㄉㄤˇ）");
        // 膽案 is no word: at the changed position the typed reading's characters are listed
        key(&mut s, Key::Left);
        key(&mut s, Key::Down);
        let v = shown(&s);
        assert!(v.contains(&"檔（ㄉㄤˇ）".to_string()), "{v:?}");
        // only the changed reading (ordinary items) and the way back: no F1 repair segment for a legal position
        assert!(v.iter().filter(|x| x.contains('（')).all(|x| x.ends_with("（ㄉㄤˇ）") || x.ends_with("（ㄉㄢˇ）")), "{r:?} {v:?}");
        let u = pick(&mut s, "膽（ㄉㄢˇ）");
        assert!(text(&u).starts_with('膽'), "{r:?}");
        assert_eq!(s.eff_syl(), vec!["ㄉㄢˇ".to_string(), "ㄢˋ".to_string()]);
        // back at the typed reading: no marker, no way-back items
        key(&mut s, Key::Down);
        assert!(shown(&s).iter().all(|x| !x.contains('（')), "{:?}", shown(&s));
    }
}

#[test]
fn norm_style_follows_the_changed_reading() {
    let mut a = assets();
    a.set_norm(twime_model::NormCfg::ALL);
    let e = engine_with(a, None, true, 9);
    let mut s = e.new_session();
    s.handle(1, None, Event::SetContext("臺北".into()));
    assert!(s.st.style_tai);
    type_str(&mut s, "ㄊㄞ ㄨㄢ ");
    key(&mut s, Key::Down);
    // 台 chosen where 臺 is legal for the changed reading ㄊㄞˊ (not for the typed ㄊㄞ): the style becomes 台
    let u = pick(&mut s, "台灣（ㄊㄞˊ）");
    assert_eq!(text(&u), "台灣");
    assert!(!s.st.style_tai);
}

#[test]
fn repaired_and_changed_positions_in_one_word() {
    // ㄐㄨ is illegal; auto repairs it to ㄐㄩ; 居檔案 changes ㄉㄢˇ to ㄉㄤˇ as well
    let mut s = sess(Some(RepairMode::Auto), true);
    type_str(&mut s, "ㄐㄨ ㄉㄢˇㄢˋ");
    key(&mut s, Key::Down);
    let u = pick(&mut s, "居檔案（ㄐㄩ、ㄉㄤˇ）");
    assert_eq!(text(&u), "居檔案");
    assert_eq!(s.st.repair, vec![Some("ㄐㄩ".to_string()), Some("ㄉㄤˇ".to_string()), None]);
    // suggest: the unchosen illegal position has no character, so no span reaches over it
    let mut s = sess(Some(RepairMode::Suggest), true);
    type_str(&mut s, "ㄐㄨ ㄉㄢˇㄢˋ");
    key(&mut s, Key::Down);
    assert!(shown(&s).iter().all(|x| !x.starts_with('居')));
}

#[test]
fn add_to_lexicon_uses_the_changed_reading() {
    let a = Arc::new(assets());
    let index = Arc::new(Index::from_data_txt(&a, DICT));
    let dir = std::env::temp_dir().join(format!("twime-core-f2-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let service = twime_userdata::Service::open(dir.clone(), a.clone(), None);
    let writer = service.spawn_writer();
    let cfg = Config { lambda: 0.0, fuzzy_words: Some(index), ..Config::default() };
    let e = Engine::with_user(a, cfg, Some(crate::UserLink { service, writer }), twime_proto::ToggleKey::None);
    let mut s = e.new_session();
    s.set_ux(true);
    type_str(&mut s, "ㄉㄢˇㄢˋ");
    key(&mut s, Key::Down);
    pick(&mut s, "檔案（ㄉㄤˇ）");
    // 檔 is not legal for the typed ㄉㄢˇ: the request is accepted only with the changed reading
    let u = s.handle(s.last_seq + 1, None, Event::AddToLexicon);
    assert_eq!(u.notice.as_deref(), Some("已送出加入詞庫：檔案"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Plan 3.4 compatibility (i): sequences that never open the list give the same Update in every step.
#[test]
fn sequences_without_a_list_are_identical() {
    let script: Vec<Event> = {
        let mut v = vec![Event::SetContext("臺灣".into())];
        let k = |t: &str| keys(t).into_iter().map(|k| Event::Key(KeyEvent::press(k))).collect::<Vec<_>>();
        v.extend(k("ㄉㄢˇㄢˋㄕˊㄐㄧㄢ "));
        v.extend([Key::Left, Key::Left, Key::Home, Key::End].map(|k| Event::Key(KeyEvent::press(k))));
        v.extend(k("ㄊㄞ ㄨㄢ "));
        v.extend([Key::Backspace, Key::Home, Key::Delete, Key::Enter].map(|k| Event::Key(KeyEvent::press(k))));
        v
    };
    for r in REPAIRS {
        let run = |f: bool| -> Vec<String> {
            let mut s = sess(r, f);
            script.iter().enumerate().map(|(i, ev)| format!("{:?}", s.handle(i as u32 + 1, None, ev.clone()))).collect()
        };
        assert_eq!(run(true), run(false), "{r:?}");
    }
}

/// Plan 3.4 compatibility (ii): choosing the same normal item (found by text, span and repairs) gives the same result.
#[test]
fn choosing_the_same_normal_item_gives_the_same_composition() {
    for r in REPAIRS {
        for (t, lefts) in [("ㄉㄢˇㄢˋ", 0), ("ㄕˊㄐㄧㄢ ㄉㄢˇㄢˋ", 1), ("ㄊㄞ ㄨㄢ ", 0)] {
            let mut off = sess(r, false);
            type_str(&mut off, t);
            for _ in 0..lefts {
                key(&mut off, Key::Left);
            }
            key(&mut off, Key::Down);
            let n = list(&off).len();
            for idx in 0..n {
                let mut a = sess(r, false);
                let mut b = sess(r, true);
                for s in [&mut a, &mut b] {
                    type_str(s, t);
                    for _ in 0..lefts {
                        key(s, Key::Left);
                    }
                    key(s, Key::Down);
                }
                let want = list(&a)[idx].clone();
                let j = list(&b).iter().position(|x| *x == want).expect("normal item in the on list");
                a.select(idx, true);
                b.select(j, true);
                let (ua, ub) = (a.update(true, None), b.update(true, None));
                assert_eq!((text(&ua), ua.committable), (text(&ub), ub.committable), "{t} {idx}");
                assert_eq!(key(&mut a, Key::Enter).commit, key(&mut b, Key::Enter).commit);
            }
        }
    }
}

/// Gate 4 (plan 3.3): the user's report row 21 (檔案 typed ㄉㄢˇ ㄢˋ) with its context.
#[test]
fn user_report_row_21() {
    let mut s = engine_with(assets(), Some(RepairMode::Suggest), true, 9).new_session();
    s.handle(1, None, Event::SetContext("他會產生匯出目錄和完整".into()));
    type_str(&mut s, "ㄉㄢˇㄢˋ");
    let u = key(&mut s, Key::Down);
    assert!(u.candidates.as_ref().unwrap().items.iter().any(|x| x == "檔案（ㄉㄤˇ）"));
    let u = pick(&mut s, "檔案（ㄉㄤˇ）");
    assert_eq!(text(&u), "檔案");
    assert!(u.committable);
}
