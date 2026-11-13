//! AUTOCOMP-LIST (AUTOCOMP-LIST-plan 5, L5): completions inside the candidate list. Synthetic dictionary only.
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use twime_model::{ModelPaths, lexicon::Dictionary, vocab::Vocab};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const DICT: &str = "\
ㄓㄨㄥ 中 -2
ㄏㄨㄚˊ 華 -2
ㄇㄧㄣˊ 民 -2
ㄍㄨㄛˊ 國 -2
ㄒㄧㄢˋ 憲 -3
ㄈㄚˇ 法 -2
ㄉㄨㄟˋ 隊 -2
ㄉㄜ˙ 的 -1
ㄓㄨㄥ-ㄏㄨㄚˊ 中華 -3
ㄓㄨㄥ-ㄏㄨㄚˊ-ㄇㄧㄣˊ-ㄍㄨㄛˊ 中華民國 -3
ㄓㄨㄥ-ㄏㄨㄚˊ-ㄇㄧㄣˊ-ㄍㄨㄛˊ-ㄒㄧㄢˋ-ㄈㄚˇ 中華民國憲法 -3.3
";

fn assets() -> Assets {
    assets_from(DICT)
}

fn assets_from(dict_text: &str) -> Assets {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/autocomp/aclist-tests")
        .join(format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("data.txt"), dict_text).unwrap();
    std::fs::write(p.join("aliases.txt"), "").unwrap();
    let (dict, _) = Dictionary::parse(dict_text);
    let n = Vocab::new(&dict).len();
    let mut b = b"TWG1".to_vec();
    for v in [n as u32, 32, 0, 32] {
        b.extend(v.to_le_bytes());
    }
    b.extend(vec![0u8; n * 32 * 4]);
    for _ in 0..32 {
        b.extend(1f32.to_le_bytes());
    }
    std::fs::write(p.join("model.bin"), b).unwrap();
    Assets::load(&ModelPaths { data_txt: p.join("data.txt"), aliases: p.join("aliases.txt"), model: p.join("model.bin"), prior: None }).unwrap()
}

fn session(on: bool, page_size: usize) -> Session {
    let cfg = Config { lambda: 0., candidates: true, page_size, autocomp_list: on, ..Config::default() };
    Engine::new(Arc::new(assets()), cfg).new_session()
}

fn typed(s: &mut Session, syllables: &[&str]) {
    for syl in syllables {
        for c in syl.chars() {
            s.on_key(KeyEvent::press(keymap::key_of(c).unwrap()));
        }
        if !syl.ends_with(['ˊ', 'ˇ', 'ˋ', '˙']) {
            s.on_key(KeyEvent::press(Key::Space));
        }
    }
}

fn texts(s: &Session) -> Vec<String> {
    s.candidate_items().unwrap_or_default().into_iter().map(|x| x.0).collect()
}

const ZH: [&str; 2] = ["ㄓㄨㄥ", "ㄏㄨㄚˊ"];

mod pop_tests {
    use super::*;
    fn pop_session(ps: usize) -> Session {
        let mut a = assets();
        let dir = std::env::temp_dir().join(format!("twime-pop-lm-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let tokens = ["<PAD>", "<BOS>", "<UNK>", "中", "華", "民", "國", "隊"];
        std::fs::write(dir.join("lm-vocab.json"), serde_json_for_tokens(&tokens)).unwrap();
        let mut bytes = b"TWLMF32\0".to_vec();
        for x in [8u32, 4, 0, 1, 4, 35] { bytes.extend(x.to_le_bytes()); }
        for _ in 0..(8*4+35*4) { bytes.extend(0f32.to_le_bytes()); }
        for _ in 0..4 { bytes.extend(1f32.to_le_bytes()); }
        for _ in 0..4 { bytes.extend(0f32.to_le_bytes()); }
        std::fs::write(dir.join("lm-f32.bin"), bytes).unwrap();
        a.completion_lm = twime_model::pop::CompletionLm::load_optional(&dir).unwrap();
        let mut s = Engine::new(Arc::new(a), Config { lambda: 0., page_size: ps, autocomp_list: true, autocomp_pop: true, ..Config::default() }).new_session();
        s.set_pop_peer(true);
        s.set_lexux_peer(true);
        s.set_ux(true);
        s
    }
    #[test]
    fn pop_cap_40_does_not_change_list_cap_20_or_off_behavior() {
        let mut dict = DICT.to_string();
        for i in 0..45 {
            let c = char::from_u32(0x4e00+i).unwrap();
            dict.push_str(&format!("ㄉㄨㄟˋ {c} -1\nㄓㄨㄥ-ㄏㄨㄚˊ-ㄉㄨㄟˋ 中華{c} {}\n", -3.0-i as f64/100.));
        }
        let mut s = pop_session(9);
        let mut a = assets_from(&dict);
        a.completion_lm = s.engine.assets.completion_lm.clone();
        s.engine = Engine::new(Arc::new(a), s.engine.cfg.clone());
        typed(&mut s, &ZH);
        let full = s.autocomp_lists_now(usize::MAX);
        assert_eq!(full[0].1.len(), 46);
        let expected20: Vec<_> = full[0].1.iter().take(20).cloned().collect();
        let expected40: Vec<_> = full[0].1.iter().take(40).cloned().collect();
        assert_eq!(s.completion_raw_pool(), expected40);
        assert_eq!(s.completion_ui_pool(), expected40);
        let conv = s.st.conv.as_ref().unwrap();
        assert_eq!(s.engine.assets.autocomp_list_merged(conv, &s.eff_syl(), &[false, false]), expected20);
        // No Idle: POP on/off both open exactly the accepted LIST candidates.
        let mut off = Engine::new(s.engine.assets.clone(), Config { autocomp_pop: false, ..s.engine.cfg.clone() }).new_session();
        off.set_pop_peer(true);
        typed(&mut off, &ZH);
        s.on_key(KeyEvent::press(Key::Down));
        off.on_key(KeyEvent::press(Key::Down));
        assert_eq!(s.candidate_identities(), off.candidate_identities());
        let completions: Vec<_> = s.st.cand.as_ref().unwrap().items.iter().filter(|i| !i.extra.is_empty()).collect();
        assert_eq!(completions.len(), 9);
        assert!(completions.iter().all(|i| expected20.iter().any(|c| c.word==i.text && c.k==i.span && c.extra==i.extra)));
    }
    fn serde_json_for_tokens(tokens: &[&str]) -> String { format!("[{}]", tokens.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(",")) }
    #[test]
    fn deduped_chemical_hint_and_down_list_share_order_and_identities() {
        const D: &str = "ㄦˋ 二 -1\nㄧㄤˇ 氧 -1\nㄏㄨㄚˋ 化 -1\nㄊㄢˋ 碳 -1\nㄑㄧˋ 氣 -1\nㄌㄧㄡˊ 硫 -1\nㄉㄢˋ 氮 -1\nㄦˋ-ㄧㄤˇ 二氧 -2\nㄦˋ-ㄧㄤˇ-ㄏㄨㄚˋ 二氧化 -2\nㄦˋ-ㄧㄤˇ-ㄏㄨㄚˋ-ㄊㄢˋ 二氧化碳 -3\nㄦˋ-ㄧㄤˇ-ㄏㄨㄚˋ-ㄊㄢˋ-ㄑㄧˋ 二氧化碳氣 -4\nㄦˋ-ㄧㄤˇ-ㄏㄨㄚˋ-ㄌㄧㄡˊ 二氧化硫 -5\nㄦˋ-ㄧㄤˇ-ㄏㄨㄚˋ-ㄉㄢˋ 二氧化氮 -6\n";
        for ps in [1, 3, 9] {
            let mut s = pop_session(ps);
            let mut a = assets_from(D);
            a.completion_lm = s.engine.assets.completion_lm.clone();
            s.engine = Engine::new(Arc::new(a), s.engine.cfg.clone());
            typed(&mut s, &["ㄦˋ", "ㄧㄤˇ"]);
            let pool = s.completion_ui_pool();
            let ranked = s.engine.assets.completion_lm.as_ref().unwrap().rank("二氧", pool).unwrap();
            assert_eq!(ranked.iter().map(|c| c.extra_chars.as_str()).collect::<Vec<_>>(),
                ["化", "化碳", "化硫", "化氮", "化碳氣"]);
            let expected: Vec<_> = ranked.into_iter().filter(|c| c.extra_chars != "化碳氣").take(ps).collect();
            let h = idle(&mut s).completion.unwrap();
            assert_eq!(h.options, expected.iter().map(|c| c.extra_chars.clone()).collect::<Vec<_>>());
            s.handle(s.last_seq+1, None, Event::DownCompletion(h.id));
            let items = &s.st.cand.as_ref().unwrap().items;
            for (item, completion) in items.iter().zip(&expected) {
                assert_eq!((&item.text, item.span, &item.extra), (&completion.word, completion.k, &completion.extra));
            }
            assert_eq!(items.iter().take(h.options.len()).map(|i| i.text.chars().skip(i.span).collect::<String>()).collect::<Vec<_>>(), h.options);
        }
    }
    fn idle(s: &mut Session) -> Update {
        let g = s.pop_gen;
        let seq = s.last_seq + 1;
        let ack = Some(CommitResult { seq: s.last_seq, commit: Applied::NotApplicable, comp: Applied::Applied });
        s.handle(seq, ack, Event::Idle(g))
    }
    #[test]
    fn idle_preserves_text_candidates_mode_and_has_stable_numbered_down_list() {
        for ps in [1, 4, 9] {
            let mut s = pop_session(ps);
            typed(&mut s, &ZH);
            let before = s.update(false, None);
            assert_ne!(before.pop_gen, 0);
            let u = idle(&mut s);
            assert_eq!(u.composition, before.composition);
            assert_eq!(u.candidates, before.candidates);
            assert_eq!(u.mode, before.mode);
            let h = u.completion.unwrap();
            assert_eq!(h.options, vec!["民國"]);
            let down = s.handle(s.last_seq+1, None, Event::DownCompletion(h.id));
            assert!(down.candidates_open);
            assert!(s.st.cand.as_ref().unwrap().items[0].text == "中華民國");
            s.on_key(KeyEvent::press(Key::Digit(1)));
            assert_eq!(s.st.syl.len(), 4);
            assert!(s.st.groups.iter().all(|g| *g != 0 && *g == s.st.groups[0]));
            assert_eq!(s.st.cursor, 4);
            assert_eq!(s.comp().text(), "中華民國");
        }
    }
    #[test]
    fn typing_arms_empty_pool_and_idle_leaves_visible_state_unchanged() {
        let mut s = pop_session(9);
        typed(&mut s, &["ㄉㄜ˙", "ㄉㄜ˙"]);
        let before = s.update(false, None);
        assert_ne!(before.pop_gen, 0, "typing eligibility does not require a nonempty pool");
        assert!(s.pop_result.is_empty());
        let after = idle(&mut s);
        assert!(after.completion.is_none());
        assert_eq!(after.pop_gen, 0);
        assert_eq!(after.composition, before.composition);
        assert_eq!(after.candidates, before.candidates);
        assert_eq!(after.mode, before.mode);
    }
    #[test]
    fn typing_requires_two_legal_trailing_syllables_and_a_completed_action() {
        let mut s = pop_session(9);
        typed(&mut s, &ZH[..1]);
        assert_eq!(s.update(false, None).pop_gen, 0);
        typed(&mut s, &ZH[1..]);
        assert_ne!(s.update(false, None).pop_gen, 0);
        s.st.syl[0] = "invalid".into();
        assert!(!s.pop_ready());
        s.st.syl[0] = ZH[0].into();
        s.on_key(KeyEvent::press(keymap::key_of('ㄉ').unwrap()));
        assert_eq!(s.update(false, None).pop_gen, 0);
    }
    #[test]
    fn every_invalidating_event_rejects_old_idle_and_old_down_id() {
        let events = [Event::ExternalEdit, Event::CompositionAborted, Event::Unsynced, Event::Focus(false), Event::Focus(true), Event::Password(true), Event::Close, Event::ToggleMode, Event::CandidatePick(0),
            Event::Key(KeyEvent::press(Key::Esc)), Event::Key(KeyEvent::press(Key::Enter)), Event::Key(KeyEvent::press(Key::Other)), Event::Key(KeyEvent::press(Key::Left))];
        for ev in events {
            let mut s = pop_session(9);
            typed(&mut s, &ZH);
            let g = s.pop_gen;
            assert!(idle(&mut s).completion.is_some());
            s.handle(s.last_seq+1, None, ev);
            assert!(s.handle(s.last_seq+1, None, Event::Idle(g)).completion.is_none());
            assert!(s.pop_result.is_empty());
        }
    }
    #[test]
    fn stale_generation_and_keyup_and_ack_failure() {
        let mut s = pop_session(9);
        typed(&mut s, &ZH);
        let g = s.pop_gen;
        let snapshot = s.prepare_idle(s.last_seq+1, None, g).unwrap();
        s.on_key(KeyEvent::press(Key::Left));
        assert!(s.finish_idle(g, snapshot.compute()).completion.is_none());
        let mut s = pop_session(9);
        typed(&mut s, &ZH);
        let g = s.pop_gen;
        s.on_key(KeyEvent { down: false, ..KeyEvent::press(Key::Other) });
        assert_eq!(s.pop_gen, g);
        assert!(idle(&mut s).completion.is_some());
        let mut s = pop_session(9);
        typed(&mut s, &ZH);
        let ack = Some(CommitResult { seq: s.last_seq, commit: Applied::NotApplicable, comp: Applied::Failed });
        assert!(s.handle(s.last_seq+1, ack, Event::Idle(s.pop_gen)).completion.is_none());
        assert!(!s.st.composing());
    }
    #[test]
    fn mismatched_id_opens_ordinary_aclist_and_missing_assets_disable() {
        let mut s = pop_session(9);
        typed(&mut s, &ZH);
        idle(&mut s);
        s.handle(s.last_seq+1, None, Event::DownCompletion(s.pop_gen+1));
        assert_eq!(s.st.cand.as_ref().unwrap().items[0].text, "中華");
        let mut s = session(true, 9);
        s.set_pop_peer(true);
        typed(&mut s, &ZH);
        assert_eq!(s.update(false, None).pop_gen, 0);
        assert!(idle(&mut s).completion.is_none());
    }
    #[test]
    fn backslash_hint_suppresses_entire_generation_and_new_syllable_rearms() {
        let mut s = pop_session(9);
        let cfg = Config { lexux: Some(twime_model::lexux::Config { a: 100., b: None, w: false }),
            autocomp: Some(twime_model::autocomp::AutoParams { theta: 0.6, theta2: 0.3 }), ..s.engine.cfg.clone() };
        s.engine = Engine::new(s.engine.assets.clone(), cfg);
        typed(&mut s, &ZH);
        assert!(s.lexux_hint().is_some());
        assert!(!s.pop_eligible);
        assert!(idle(&mut s).completion.is_none());
        s.hint_mods = twime_proto::Mods(twime_proto::Mods::SHIFT);
        assert!(s.lexux_hint().is_none());
        assert!(idle(&mut s).completion.is_none());
        typed(&mut s, &["ㄉㄜ˙"]);
        assert_eq!(s.update(false, None).pop_gen, s.pop_gen, "a new completed syllable rearms the generation");
        assert!(idle(&mut s).completion.is_none(), "an empty UI pool is resolved only at Idle");
    }
    #[test]
    fn full_pages_match_hint_numbers_preserve_ordinary_order_and_filter_length_before_cap() {
        for ps in [1,4,9] {
            let mut s=pop_session(ps);
            let mut data=DICT.to_string();
            for (r1,c1) in [("ㄉㄨㄟˋ","隊"),("ㄇㄧㄣˊ","民"),("ㄍㄨㄛˊ","國"),("ㄈㄚˇ","法")] {
                for (r2,c2) in [("ㄉㄨㄟˋ","隊"),("ㄇㄧㄣˊ","民"),("ㄍㄨㄛˊ","國")] {
                    data+=&format!("ㄓㄨㄥ-ㄏㄨㄚˊ-{r1}-{r2} 中華{c1}{c2} -4\n");
                }
                data+=&format!("ㄓㄨㄥ-ㄏㄨㄚˊ-{r1} 中華{c1} -4\n");
            }
            let mut a=assets_from(&data);
            a.completion_lm=s.engine.assets.completion_lm.clone();
            s.engine=Engine::new(Arc::new(a),s.engine.cfg.clone());
            typed(&mut s,&ZH);
            s.pop_opening=true;s.open_candidates();s.pop_opening=false;
            let ordinary=s.st.cand.take().unwrap().items;
            let h=idle(&mut s).completion.unwrap();
            assert_eq!(h.options.len(),ps);
            s.handle(s.last_seq+1,None,Event::DownCompletion(h.id));
            let items=&s.st.cand.as_ref().unwrap().items;
            for (item,suffix) in items.iter().zip(&h.options) {
                assert_eq!(item.text.chars().skip(item.span).collect::<String>(),*suffix);
                assert!(!item.extra.is_empty());
            }
            assert!(items[ps..].iter().zip(&ordinary).all(|(a,b)| a.text==b.text && a.span==b.span && a.repairs==b.repairs));
            s.on_key(KeyEvent::press(Key::PageDown));
            assert_eq!(s.st.cand.as_ref().unwrap().selected / ps, 1);
            assert_eq!(s.st.cand.as_ref().unwrap().items[ps].text, ordinary[0].text);
            s.on_key(KeyEvent::press(Key::Esc));
            s.fresh(false);
            let mut input=vec!["ㄉㄜ˙";36];input.extend(ZH);
            typed(&mut s,&input);
            let h=idle(&mut s).completion.unwrap();
            assert!(h.options.iter().all(|s|s.chars().count()==1));
        }
    }
    #[test]
    fn every_required_feature_gate_disables_pop() {
        for gate in 0..5 {
            let mut s=pop_session(9);
            let mut cfg=s.engine.cfg.clone();
            match gate {
                0 => cfg.autocomp_pop=false,
                1 => cfg.autocomp_list=false,
                2 => cfg.candidates=false,
                3 => s.set_pop_peer(false),
                _ => s.password=true,
            }
            s.engine=Engine::new(s.engine.assets.clone(),cfg);
            typed(&mut s,&ZH);
            assert_eq!(s.update(false,None).pop_gen,0);
            assert!(idle(&mut s).completion.is_none());
        }
    }
    #[test]
    fn completion_down_preserves_fuzzy_repair_identities_and_relative_order() {
        const D:&str="ㄉㄤˇ 黨 -2\nㄉㄢˇ 膽 -2.5\nㄢˋ 案 -2\nㄕˊ 時 -1\nㄉㄤˇ-ㄢˋ 檔案 -3\nㄉㄢˇ-ㄢˋ-ㄕˊ 膽案時 -3\n";
        let mut s=pop_session(9);
        let mut a=assets_from(D);
        a.completion_lm=s.engine.assets.completion_lm.clone();
        let index=Arc::new(crate::fuzzy::Index::from_data_txt(&a,D));
        let cfg=Config { fuzzy_words:Some(index.clone()),repair:Some(Repair { mode:RepairMode::Suggest,index }),..s.engine.cfg.clone() };
        s.engine=Engine::new(Arc::new(a),cfg);
        typed(&mut s,&["ㄉㄢˇ","ㄢˋ"]);
        s.pop_opening=true;s.open_candidates();s.pop_opening=false;
        let ordinary=s.candidate_identities();s.st.cand=None;
        assert!(ordinary.iter().any(|i|!i.2.is_empty()));
        let h=idle(&mut s).completion.unwrap();
        s.handle(s.last_seq+1,None,Event::DownCompletion(h.id));
        let got=s.candidate_identities();
        assert_eq!(&got[h.options.len()..],ordinary.as_slice());
    }
}

#[test]
fn completions_take_the_end_of_the_first_page() {
    let mut s = session(true, 9);
    typed(&mut s, &ZH);
    s.on_key(KeyEvent::press(Key::Down));
    let t = texts(&s);
    let mut off = session(false, 9);
    typed(&mut off, &ZH);
    off.on_key(KeyEvent::press(Key::Down));
    let base = texts(&off);
    assert!(t[0] == base[0], "the current output stays first");
    let added: Vec<&String> = t.iter().filter(|x| !base.contains(x)).collect();
    // 中華民國憲法 needs 4 more syllables after 中華 (more than 3): only 中華民國 is offered
    assert!(added.len() == 1 && added[0] == "中華民國");
    // the ordinary items keep their order
    let rest: Vec<&String> = t.iter().filter(|x| base.contains(x)).collect();
    assert!(rest.iter().map(|x| x.as_str()).eq(base.iter().map(String::as_str)));
    let i = t.iter().position(|x| x == "中華民國").unwrap();
    assert!(i < 9);
}

#[test]
fn choosing_a_completion_appends_and_locks() {
    let mut s = session(true, 9);
    typed(&mut s, &ZH);
    s.on_key(KeyEvent::press(Key::Down));
    let i = texts(&s).iter().position(|x| x == "中華民國").unwrap();
    s.on_key(KeyEvent::press(Key::Digit((i + 1) as u8)));
    assert!(s.comp().text() == "中華民國");
    assert!(s.st.syl.len() == 4 && s.st.cursor == 4);
    assert!(s.st.locks.iter().all(Option::is_some));
    let g = s.st.groups[0];
    assert!(g != 0 && s.st.groups.iter().all(|&x| x == g));
}

#[test]
fn page_size_one_puts_completions_after_the_first_page() {
    let mut s = session(true, 1);
    typed(&mut s, &ZH);
    s.on_key(KeyEvent::press(Key::Down));
    let t = texts(&s);
    assert!(!t[0].starts_with("中華民國"));
    assert!(t.iter().any(|x| x == "中華民國"));
}

#[test]
fn not_at_the_end_or_off_gives_no_completion() {
    let mut s = session(true, 9);
    typed(&mut s, &ZH);
    s.on_key(KeyEvent::press(Key::Left));
    s.on_key(KeyEvent::press(Key::Down));
    assert!(texts(&s).iter().all(|x| !x.starts_with("中華民國")));
    let mut off = session(false, 9);
    typed(&mut off, &ZH);
    off.on_key(KeyEvent::press(Key::Down));
    assert!(texts(&off).iter().all(|x| !x.starts_with("中華民國")));
}

#[test]
fn off_and_on_type_the_same_without_the_list() {
    let mut on = session(true, 9);
    let mut off = session(false, 9);
    for syl in ["ㄓㄨㄥ", "ㄏㄨㄚˊ", "ㄇㄧㄣˊ"] {
        typed(&mut on, &[syl]);
        typed(&mut off, &[syl]);
        assert!(on.comp().text() == off.comp().text());
    }
    assert!(on.on_key(KeyEvent::press(Key::Enter)).commit == off.on_key(KeyEvent::press(Key::Enter)).commit);
}

fn texts_of(a: Assets, page_size: usize, fuzzy_dict: Option<&str>, typed_syl: &[&str]) -> (Vec<String>, Vec<String>) {
    let a = Arc::new(a);
    let mut out = vec![];
    for on in [false, true] {
        let fuzzy_words = fuzzy_dict.map(|d| Arc::new(crate::fuzzy::Index::from_data_txt(&a, d)));
        let cfg = Config { lambda: 0., candidates: true, page_size, autocomp_list: on, fuzzy_words, ..Config::default() };
        let mut s = Engine::new(a.clone(), cfg).new_session();
        typed(&mut s, typed_syl);
        s.on_key(KeyEvent::press(Key::Down));
        out.push(s.candidate_items().unwrap_or_default().into_iter().map(|x| x.2).collect());
    }
    let on = out.pop().unwrap();
    (out.pop().unwrap(), on)
}

/// The ordinary items keep their relative order (`on` restricted to the `off` items equals `off`).
fn keeps_order(off: &[String], on: &[String]) -> bool {
    on.iter().filter(|x| off.contains(x)).eq(off.iter())
}

#[test]
fn page_size_four_reserves_the_last_slot_only() {
    let (off, on) = texts_of(assets(), 4, None, &ZH);
    assert!(keeps_order(&off, &on));
    // one completion, a = min(1, 4, 3) = 1, i0 = max(1, min(|N|, 3)) with |N| = 1
    assert_eq!(on.iter().position(|x| x == "中華民國"), Some(1.min(off.len()).max(1)), "{on:?}");
    assert_eq!(on[0], off[0]);
}

#[test]
fn the_same_word_from_two_k_is_listed_once() {
    const D: &str = "ㄏㄚ 哈 -1\nㄏㄚ-ㄏㄚ-ㄏㄚ-ㄏㄚ 哈哈哈哈 -3\n";
    let a = assets_from(D);
    let (off, on) = texts_of(a, 9, None, &["ㄏㄚ", "ㄏㄚ", "ㄏㄚ"]);
    // k = 3 (+1 syllable) and k = 2 (+2 syllables) both give 哈哈哈哈
    assert_eq!(on.iter().filter(|x| *x == "哈哈哈哈").count(), 1, "{on:?}");
    assert!(keeps_order(&off, &on));
}

#[test]
fn no_completion_past_the_syllable_cap() {
    let mut syl = vec!["ㄓㄨㄥ"; MAX_SYLLABLES - 2];
    syl.push("ㄏㄨㄚˊ");
    // MAX - 1 syllables typed: 中華民國 needs 2 more
    let (off, on) = texts_of(assets(), 9, None, &syl);
    assert_eq!(off, on);
    let mut syl = vec!["ㄓㄨㄥ"; MAX_SYLLABLES - 3];
    syl.push("ㄏㄨㄚˊ");
    let (_, on) = texts_of(assets(), 9, None, &syl);
    assert!(on.iter().any(|x| x == "中華民國"), "exactly at the cap is allowed");
}

#[test]
fn user_words_decide_the_k_they_complete() {
    let a = assets();
    let w = a.user_word("中華隊", &["ㄓㄨㄥ".into(), "ㄏㄨㄚˊ".into(), "ㄉㄨㄟˋ".into()], 30.0).unwrap();
    a.set_user(Some(Arc::new(a.build_user_lexicon([w]))));
    let (off, on) = texts_of(a, 9, None, &ZH);
    let added: Vec<&String> = on.iter().filter(|x| !off.contains(x)).collect();
    assert_eq!(added, ["中華隊"], "{on:?}");
}

#[test]
fn with_fuzzy_words_the_full_list_comes_first() {
    const D: &str = "\
ㄉㄤˇ 黨 -2
ㄉㄢˇ 膽 -2.5
ㄢˋ 案 -2
ㄕˊ 時 -1
ㄉㄤˇ-ㄢˋ 檔案 -3
ㄉㄢˇ-ㄢˋ-ㄕˊ 膽案時 -3
";
    let a = assets_from(D);
    let (off, on) = texts_of(a, 9, Some(D), &["ㄉㄢˇ", "ㄢˋ"]);
    assert!(off.iter().any(|x| x.contains('（')), "F2 lists a suggestion: {off:?}");
    assert!(keeps_order(&off, &on), "{off:?} {on:?}");
    let i = on.iter().position(|x| x == "膽案時").unwrap_or_else(|| panic!("{on:?}"));
    assert!(i >= 1 && i < 9);
}
