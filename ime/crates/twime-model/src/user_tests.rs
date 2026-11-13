use super::*;
use crate::{CandParams, Conversion, NormCfg, pn, prior};
use std::sync::atomic::{AtomicUsize, Ordering};

const DICT: &str = "ㄑㄩㄢˊ 全 -1\nㄑㄩㄢˊ 權 -2\nㄑㄩㄢˊ 泉 -3\nㄌㄧˋ 力 -1\nㄌㄧˋ 利 -2\nㄌㄧˋ 立 -3\nㄑㄩㄢˊ-ㄌㄧˋ 全力 -1\nㄑㄩㄢˊ-ㄌㄧˋ 權利 -9\nㄊㄞˊ 台 -1\nㄊㄞˊ 臺 -2\nㄨㄢ 灣 -1\nㄐㄧ 積 -1\nㄉㄧㄢˋ 電 -1\nㄊㄚ 他 -1\nㄊㄚ 她 -2\n";

fn syl(s: &str) -> Vec<String> { s.split_whitespace().map(str::to_owned).collect() }

fn tiny(model: bool) -> Assets {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    if model { a.model = Some(crate::n1::N1::random(a.vocab.len(), 32, 1, 11)); }
    a
}

fn set(a: &Assets, words: &[(&str, &str)]) {
    a.set_user(Some(Arc::new(a.build_user_lexicon(words.iter().map(|(w, s)| a.user_word(w, &syl(s), 30.0).unwrap())))));
}

fn temp() -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("twime-user-1b-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lex(a: &Assets, words: &[(&str, &str)]) -> Arc<pn::PnLexicon> {
    let base = temp().join("lex");
    let words: Vec<_> = words.iter().map(|(w, s)| pn::BuildWord { word: w.to_string(), reading: s.replace(' ', "-"), class: 0 }).collect();
    pn::build(a, &words, &base).unwrap();
    Arc::new(pn::PnLexicon::open(&base, a).unwrap())
}

fn cfg() -> pn::PnCfg { pn::PnCfg { b2: Some(1.0), b3: Some(1.0), h: Some(1), cands: true } }

// A tiny uniform unigram prior, with empty higher-order tables, in the existing pack format.
fn pack(a: &Assets) -> prior::Pack {
    let n = a.vocab.len() as u32;
    let mut b = b"TWPR1\0\0\0".to_vec();
    b.extend(1u32.to_le_bytes()); b.extend(n.to_le_bytes());
    b.extend((n as u64).to_le_bytes()); b.extend(n.to_le_bytes());
    b.extend(0.4f64.to_le_bytes()); b.extend(1u32.to_le_bytes());
    for id in 0..n { b.extend((a.vocab.char_of(id).unwrap_or('\0') as u32).to_le_bytes()); }
    for _ in 0..n { b.extend(1u32.to_le_bytes()); }
    for _ in 0..3 {
        b.extend(0u64.to_le_bytes()); b.extend(1u32.to_le_bytes()); b.extend(0u32.to_le_bytes());
        for _ in 0..3 { b.extend(0u32.to_le_bytes()); }
    }
    let path = temp().join("prior.pack");
    std::fs::write(&path, b).unwrap();
    prior::Pack::load(&path).unwrap()
}

// Reuse real model inference, then give the tiny test an explicit, reproducible score ordering.
fn base(a: &Assets, s: &[String], preferred: &str) -> Conversion {
    let mut c = a.convert("", s, &[], 0.0);
    let inf = Arc::get_mut(&mut c.inf).unwrap();
    for ((ids, scores), ch) in inf.sets.iter().zip(&mut inf.scores).zip(preferred.chars()) {
        for (&id, score) in ids.iter().zip(scores) { *score = if id == a.vocab_id(ch) { 1.0 } else { 0.0 }; }
    }
    a.rebeam(&c, &[], 0.0)
}

fn candidate_bits(c: &[crate::Candidate]) -> Vec<(String, usize, u64, bool)> {
    c.iter().map(|x| (x.text.clone(), x.span, x.score.to_bits(), x.current)).collect()
}

fn original_states(a: &Assets, c: &Conversion, locks: &[Option<char>], lambda: f64) -> Vec<(u64, Vec<u32>)> {
    let inf = &c.inf;
    let locked: Vec<_> = (0..inf.sets.len()).map(|i| locks.get(i).copied().flatten().map(|c| a.vocab_id(c))).collect();
    let lp = pn::log_softmax(&inf.scores);
    let (cs, ls): (Vec<_>, Vec<_>) = inf.scores.iter().zip(&inf.sets).zip(&locked).map(|((sc, ids), lk)| match lk {
        Some(id) => (vec![*id], vec![0.0]), None => prior::top_k(sc, ids),
    }).unzip();
    let jumps = a.pn.as_ref().map(|(lex, cfg)| {
        let hits: Vec<_> = lex.hits(&a.syl_codes(&inf.syllables)).into_iter().map(|(s, _, ids)| (s, ids)).collect();
        pn::usable_jumps(&hits, &inf.sets, &lp, &locked, cfg, None)
    }).unwrap_or_default();
    let pr = |h: &[u32], id| a.prior.as_ref().map_or(0.0, |pk| pk.logp(h, id));
    pn::beam_states(&cs, &ls, &inf.sets, &lp, &locked, &jumps, lambda, &pr, None).unwrap().into_iter().map(|(s, ids)| (s.to_bits(), ids)).collect()
}

#[test]
fn no_usable_user_is_bit_identical_pn_and_lambda_matrix() {
    for pn_on in [false, true] {
        let mut a = tiny(true);
        a.prior = Some(pack(&a));
        if pn_on { a.set_pn(Some(lex(&a, &[("權利", "ㄑㄩㄢˊ ㄌㄧˋ")])), cfg()); }
        let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
        let c = base(&a, &s, "全力");
        for lambda in [0.0, 0.5] {
            for case in 0..4 {
                let locks = if case == 3 { vec![Some('全'), None] } else { vec![] };
                a.set_user(None);
                let before = a.rebeam(&c, &locks, lambda);
                let states = original_states(&a, &before, &locks, lambda);
                match case {
                    0 => a.set_user(None),
                    1 => a.set_user(Some(Arc::new(UserLexicon::default()))),
                    2 => set(&a, &[("她她", "ㄊㄚ ㄊㄚ")]),
                    _ => set(&a, &[("權利", "ㄑㄩㄢˊ ㄌㄧˋ")]),
                }
                let locked: Vec<_> = (0..s.len()).map(|i| locks.get(i).copied().flatten().map(|c| a.vocab_id(c))).collect();
                assert!(a.user_select(&s, &c.inf.sets, &c.inf.scores, &locked, lambda, a.pn.as_ref().map(|(_, cfg)| cfg), None, &[], &crate::convert::ShortView::OFF).is_none());
                let after = a.rebeam(&c, &locks, lambda);
                assert_eq!(before.chars, after.chars, "pn={pn_on}, lambda={lambda}, case={case}");
                assert!(Arc::ptr_eq(&before.inf, &after.inf));
                assert_eq!(states, original_states(&a, &after, &locks, lambda));
                if case < 3 {
                    let p = CandParams { lambda, ..CandParams::default() };
                    let with_user = candidate_bits(&a.candidates(&after, 2, &p));
                    a.set_user(None);
                    assert_eq!(candidate_bits(&a.candidates(&before, 2, &p)), with_user);
                }
            }
        }
    }
}

#[test]
fn user_wins_without_pn_and_respects_lock_and_explicit_pn_off() {
    let mut a = tiny(true);
    let c = base(&a, &syl("ㄑㄩㄢˊ ㄌㄧˋ"), "全力");
    assert_eq!(c.text().unwrap(), "全力");
    set(&a, &[("權利", "ㄑㄩㄢˊ ㄌㄧˋ")]);
    assert_eq!(a.rebeam(&c, &[], 0.0).text().unwrap(), "權利");
    assert_eq!(a.rebeam(&c, &[Some('全'), None], 0.0).text().unwrap(), "全力");
    a.set_pn(Some(lex(&a, &[("全力", "ㄑㄩㄢˊ ㄌㄧˋ")])), cfg());
    assert_eq!(a.rebeam_with(&c, &[], 0.5, false, None, true).text().unwrap(), "權利");
    assert_eq!(a.rebeam(&c, &[], 0.5).text().unwrap(), "權利");
}

#[test]
fn folded_lock_writes_locked_glyph_and_preserves_other_user_glyphs() {
    let mut a = tiny(true);
    a.set_norm(NormCfg::ALL);
    let s = syl("ㄊㄞˊ ㄨㄢ ㄊㄞˊ ㄐㄧ ㄉㄧㄢˋ");
    let c = base(&a, &s, "台灣台積電");
    set(&a, &[("臺灣臺積電", "ㄊㄞˊ ㄨㄢ ㄊㄞˊ ㄐㄧ ㄉㄧㄢˋ")]);
    assert_eq!(a.rebeam(&c, &[Some('台')], 0.0).text().unwrap(), "台灣臺積電");
    a.set_norm(NormCfg::default());
    assert_eq!(a.rebeam(&c, &[Some('台')], 0.0).text().unwrap(), "台灣台積電");
}

#[test]
fn ta_user_word_is_protected_but_one_char_user_is_not() {
    let mut a = tiny(true);
    a.set_ta_default(true);
    let c = base(&a, &syl("ㄊㄚ ㄌㄧˋ"), "他力");
    set(&a, &[("她利", "ㄊㄚ ㄌㄧˋ")]);
    assert_eq!(a.rebeam(&c, &[], 0.0).text().unwrap(), "她利");
    set(&a, &[("她", "ㄊㄚ")]);
    assert_eq!(a.rebeam(&c, &[], 0.0).text().unwrap(), "他力");
    let single = a.convert("", &syl("ㄊㄚ"), &[Some('他')], 0.0);
    let cs = a.candidates(&single, 1, &CandParams::default());
    assert_eq!(cs[0].text, "他"); assert_eq!(cs[1].text, "她");
}

#[test]
fn fallback_only_changes_candidates_and_user_span_ignores_max_span() {
    for model in [false, true] {
        let a = tiny(model);
        let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
        let c = if model { base(&a, &s, "全力") } else { a.convert("", &s, &[], 0.0) };
        let p = CandParams { max_span: 1, word_cap: 0, beta: 2.0, ..CandParams::default() };
        set(&a, &[("權利", "ㄑㄩㄢˊ ㄌㄧˋ"), ("立", "ㄌㄧˋ")]);
        let cs = a.candidates(&c, 2, &p);
        assert!(cs[0].current);
        assert!(cs[1..3].iter().any(|x| x.text == "權利"));
        assert!(cs[1..3].iter().any(|x| x.text == "立"));
        if !model { assert_eq!(a.rebeam(&c, &[], 0.0).chars, c.chars); }
        let user_score = cs.iter().find(|x| x.text == "權利").unwrap().score;
        let plain = a.candidates(&c, 2, &CandParams { beta: 0.0, ..p });
        assert_eq!(user_score.to_bits(), plain.iter().find(|x| x.text == "權利").unwrap().score.to_bits());
    }
}

#[test]
fn user_dedup_across_layers_free_caps_and_hides_equivalent_single() {
    let mut a = tiny(true);
    let l = lex(&a, &[("權利", "ㄑㄩㄢˊ ㄌㄧˋ"), ("泉立", "ㄑㄩㄢˊ ㄌㄧˋ")]);
    a.set_pn(Some(l.clone()), cfg()); a.genw = Some(l);
    let c = base(&a, &syl("ㄑㄩㄢˊ ㄌㄧˋ"), "全力");
    set(&a, &[("權利", "ㄑㄩㄢˊ ㄌㄧˋ"), ("泉立", "ㄑㄩㄢˊ ㄌㄧˋ"), ("全利", "ㄑㄩㄢˊ ㄌㄧˋ")]);
    let cs = a.candidates(&c, 2, &CandParams { word_cap: 0, ..CandParams::default() });
    assert!(cs[0].current);
    for word in ["權利", "泉立", "全利"] { assert_eq!(cs.iter().filter(|c| c.text == word).count(), 1); }
    assert!(!cs.iter().any(|c| c.text == "利"));
}

#[test]
fn current_tai_mate_precedes_user_and_user_current_does_not_use_word_cap() {
    let mut a = tiny(false); a.set_norm(NormCfg::ALL);
    let c = a.convert("", &syl("ㄊㄞˊ ㄌㄧˋ"), &[], 0.0);
    set(&a, &[("台力", "ㄊㄞˊ ㄌㄧˋ"), ("臺力", "ㄊㄞˊ ㄌㄧˋ"), ("臺利", "ㄊㄞˊ ㄌㄧˋ")]);
    let cs = a.candidates(&c, 2, &CandParams { word_cap: 0, ..CandParams::default() });
    assert_eq!(cs[0].text, "台力"); assert!(cs[0].current);
    assert_eq!(cs[1].text, "臺力"); assert_eq!(cs[2].text, "臺利");
}

#[test]
fn user_candidates_truncate_twenty_without_spilling_to_other_groups() {
    let chars: Vec<char> = (0..26).map(|i| char::from_u32('一' as u32 + i).unwrap()).collect();
    let mut dict = String::from("ㄅ 北 -1\n");
    for (i, c) in chars.iter().enumerate() { dict.push_str(&format!("ㄚ {c} -{}\nㄅ-ㄚ 北{c} -2\n", i + 1)); }
    let a = Assets::from_dictionary_text(&dict, "").unwrap();
    let words: Vec<_> = chars.iter().skip(1).map(|c| a.user_word(&format!("北{c}"), &syl("ㄅ ㄚ"), 10.0).unwrap()).collect();
    let c = a.convert("", &syl("ㄅ ㄚ"), &[], 0.0);
    a.set_user(Some(Arc::new(a.build_user_lexicon(words))));
    let cs = a.candidates(&c, 2, &CandParams::default());
    assert_eq!(cs.iter().filter(|c| c.span == 2 && !c.current).count(), 20);
}

#[test]
fn missing_character_blocks_user_span_and_incomplete_conversion_keeps_old_path() {
    let a = tiny(true);
    let s = syl("ㄑㄩㄢˊ bad ㄌㄧˋ");
    let c = a.convert("", &s, &[], 0.5);
    let before = candidate_bits(&a.candidates(&c, 3, &CandParams::default()));
    set(&a, &[("權利", "ㄑㄩㄢˊ ㄌㄧˋ")]);
    assert_eq!(c.chars, a.rebeam(&c, &[], 0.5).chars);
    assert_eq!(before, candidate_bits(&a.candidates(&c, 3, &CandParams::default())));
}

#[test]
fn user_outside_top_k_survives_pn_h_zero_and_uses_identical_cache_beam_scores() {
    let chars: Vec<_> = (0..12).map(|i| char::from_u32('一' as u32 + i).unwrap()).collect();
    let dict: String = chars.iter().map(|c| format!("ㄚ {c} -1\n")).collect();
    let mut a = Assets::from_dictionary_text(&dict, "").unwrap();
    a.model = Some(crate::n1::N1::random(a.vocab.len(), 32, 1, 11));
    let s = syl("ㄚ ㄚ");
    let c = base(&a, &s, &format!("{}{}", chars[0], chars[0]));
    let word = format!("{}{}", chars[11], chars[11]);
    let entry = a.user_word(&word, &s, 30.0).unwrap();
    let ids = entry.1.ids.clone();
    a.set_user(Some(Arc::new(a.build_user_lexicon([entry]))));
    a.set_pn(Some(lex(&a, &[(&word, "ㄚ ㄚ")])), pn::PnCfg { h: Some(0), ..cfg() });
    a.prior = Some(pack(&a));
    let inf = &c.inf;
    let (cs, ls): (Vec<_>, Vec<_>) = inf.scores.iter().zip(&inf.sets).map(|(sc, ids)| prior::top_k(sc, ids)).unzip();
    assert!(!cs[0].contains(&ids[0]));
    let lp = pn::log_softmax(&inf.scores);
    let locked = vec![None; 2];
    let jump = pn::Jump { start: 0, ids: ids.clone(), bonus: 30.0 };
    let cache = crate::cache::CacheSet::from_pairs(&[(ids[0], ids[1])], (a.vocab_id('臺'), a.vocab_id('台')), 2.0, Some(1));
    let pr = |h: &[u32], id| a.prior.as_ref().unwrap().logp(h, id);
    let states = pn::beam_states(&cs, &ls, &inf.sets, &lp, &locked, &[jump], 0.5, &pr, cache.bonus()).unwrap();
    let (selected, mask) = a.user_select(&s, &inf.sets, &inf.scores, &locked, 0.5, a.pn.as_ref().map(|(_, cfg)| cfg), cache.bonus(), &[], &crate::convert::ShortView::OFF).unwrap();
    assert_eq!(selected, states[0].1);
    assert_eq!(selected, ids);
    assert_eq!(mask, vec![true; 2]);
    let expected = ((lp[0][inf.sets[0].binary_search(&ids[0]).unwrap()] + 0.5 * pr(&[], ids[0]))
        + lp[1][inf.sets[1].binary_search(&ids[1]).unwrap()]) + 0.5 * pr(&ids[..1], ids[1]);
    assert_eq!(states[0].0.to_bits(), ((expected + 2.0) + 30.0).to_bits());
    assert_eq!(a.rebeam_with_cache(&c, &[], 0.5, false, a.pn.as_ref().map(|(_, cfg)| cfg), true, Some(&cache)).text().unwrap(), word);
}

#[test]
fn illegal_unlocked_user_word_is_rejected_without_entering_beam() {
    let a = tiny(true);
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
    let c = base(&a, &s, "全力");
    let codes: Vec<_> = a.syl_codes(&s).into_iter().map(Option::unwrap).collect();
    a.set_user(Some(Arc::new(a.build_user_lexicon([(codes, UserWord { ids: vec![a.vocab_id('她'), a.vocab_id('利')], bonus: Some(30.0), custom: 0 })]))));
    assert!(a.user_select(&s, &c.inf.sets, &c.inf.scores, &[None, None], 0.0, None, None, &[], &crate::convert::ShortView::OFF).is_none());
    assert_eq!(a.rebeam(&c, &[], 0.0).chars, c.chars);
}

#[test]
fn pn_and_user_jumps_compete_and_only_matching_user_output_is_protected() {
    let mut a = tiny(true);
    let c = base(&a, &syl("ㄑㄩㄢˊ ㄌㄧˋ"), "全力");
    a.set_pn(Some(lex(&a, &[("全力", "ㄑㄩㄢˊ ㄌㄧˋ")])), pn::PnCfg { b2: Some(40.0), ..cfg() });
    set(&a, &[("權利", "ㄑㄩㄢˊ ㄌㄧˋ")]);
    let (ids, mask) = a.user_select(c.syllables(), &c.inf.sets, &c.inf.scores, &[None, None], 0.0, a.pn.as_ref().map(|(_, cfg)| cfg), None, &[], &crate::convert::ShortView::OFF).unwrap();
    assert_eq!(ids, vec![a.vocab_id('全'), a.vocab_id('力')]);
    assert_eq!(mask, vec![false, false]);
    assert_eq!(a.rebeam(&c, &[], 0.0).text().unwrap(), "全力");
}

#[test]
fn empty_and_unrelated_candidate_lexicons_are_bit_identical_in_both_modes() {
    for model in [false, true] {
        let a = tiny(model);
        let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
        let c = if model { base(&a, &s, "全力") } else { a.convert("", &s, &[], 0.0) };
        for end in 1..=2 {
            let p = CandParams { beta: 0.7, word_cap: 1, ..CandParams::default() };
            a.set_user(None);
            let plain = candidate_bits(&a.candidates(&c, end, &p));
            a.set_user(Some(Arc::new(UserLexicon::default())));
            assert_eq!(candidate_bits(&a.candidates(&c, end, &p)), plain);
            set(&a, &[("她她", "ㄊㄚ ㄊㄚ")]);
            assert_eq!(candidate_bits(&a.candidates(&c, end, &p)), plain);
        }
        set(&a, &[("全力", "ㄑㄩㄢˊ ㄌㄧˋ")]);
        let cs = a.candidates(&c, 2, &CandParams { word_cap: 1, ..CandParams::default() });
        assert!(cs[0].current);
        assert!(cs.iter().any(|x| x.text == "權利"), "a user current item does not consume the dictionary word slot");
    }
}
