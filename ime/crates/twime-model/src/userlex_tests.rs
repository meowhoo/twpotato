//! USERLEX functional matrix (USERLEX-plan 3.6): forced user words of >= 3 characters, 2-character weights,
//! candidates and reselection, fallback and no-legal-character paths, glyph / TA protection. Synthetic data only.
use super::*;
use crate::{CandParams, Conversion, NormCfg, pn};

const DICT: &str = "ㄚ 阿 -1\nㄚ 啊 -2\nㄈㄨˊ 福 -1\nㄈㄨˊ 伏 -3\nㄑㄧㄝˊ 茄 -1\nㄑㄧㄝˊ 伽 -3\nㄌㄨㄛˋ 落 -1\nㄌㄨㄛˋ 洛 -3\n\
ㄉㄜˊ 得 -1\nㄉㄜˊ 德 -3\nㄔㄤˊ 常 -1\nㄔㄤˊ 長 -2\nㄕㄨˋ 數 -1\nㄕㄨˋ 樹 -2\nㄇㄚ 媽 -1\nㄇㄚ 麻 -2\nㄊㄞˊ 台 -1\nㄊㄞˊ 臺 -2\n\
ㄨㄢ 灣 -1\nㄨㄢ 彎 -2\nㄊㄚ 他 -1\nㄊㄚ 她 -2\nㄓˋ 志 -1\nㄓˋ 指 -2\nㄇㄧㄥˊ 明 -1\nㄇㄧㄥˊ 名 -2\nㄓˋ-ㄇㄧㄥˊ 指明 -1\nㄕˋ 是 -1\nㄕˋ 事 -2\n";

fn syl(s: &str) -> Vec<String> { s.split_whitespace().map(str::to_owned).collect() }

// A tiny uniform unigram prior in the existing pack format (as in user_tests), so lambda > 0 runs the real prior path.
fn with_prior(a: &mut Assets) {
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
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!("twime-userlex-prior-{}-{}.pack", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::write(&path, b).unwrap();
    a.prior = Some(crate::prior::Pack::load(&path).unwrap());
}

fn pn_words(a: &mut Assets, w: &[(&str, &str)], bonus: f64) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let base = std::env::temp_dir().join(format!("twime-userlex-pnw-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir_all(&base).unwrap();
    let lexb = base.join(format!("lex{}", w.len()));
    let words: Vec<_> = w.iter().map(|(x, r)| pn::BuildWord { word: x.to_string(), reading: r.to_string(), class: 0 }).collect();
    pn::build(a, &words, &lexb).unwrap();
    a.set_pn(Some(Arc::new(pn::PnLexicon::open(&lexb, a).unwrap())), pn::PnCfg { b2: Some(bonus), b3: Some(bonus), h: Some(1), cands: true });
}

fn assets(model: bool) -> Assets {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    if model { a.model = Some(crate::n1::N1::random(a.vocab.len(), 32, 1, 7)); }
    a.set_user_rules(true, DEFAULT_BONUS).unwrap();
    a
}

fn words(a: &Assets, w: &[(&str, &str, Option<f64>)]) {
    a.set_user(Some(Arc::new(a.build_user_lexicon(w.iter().map(|(x, s, b)| a.user_word_opt(x, &syl(s), *b).unwrap())))));
}

/// Model scores that strongly prefer `preferred` (one char per syllable), so a forced word must beat the model.
fn conv(a: &Assets, s: &str, preferred: &str, locks: &[Option<char>], lambda: f64) -> Conversion {
    let s = syl(s);
    let mut c = a.convert("", &s, &[], 0.0);
    if !c.fallback {
        let inf = Arc::get_mut(&mut c.inf).unwrap();
        for ((ids, scores), ch) in inf.sets.iter().zip(&mut inf.scores).zip(preferred.chars()) {
            for (&id, score) in ids.iter().zip(scores) { *score = if id == a.vocab_id(ch) { 20.0 } else { 0.0 }; }
        }
    }
    a.rebeam(&c, locks, lambda)
}

const AVO: &str = "ㄚ ㄈㄨˊ ㄑㄧㄝˊ ㄌㄨㄛˋ ㄉㄜˊ";

#[test]
fn forced_word_beats_the_model_with_and_without_context() {
    let a = assets(true);
    words(&a, &[("阿伏伽洛德", AVO, None)]);
    for lambda in [0.0, 0.5] {
        assert_eq!(conv(&a, AVO, "阿福茄落得", &[], lambda).text().unwrap(), "阿伏伽洛德");
        assert_eq!(conv(&a, &format!("{AVO} ㄔㄤˊ ㄕㄨˋ"), "阿福茄落得常數", &[], lambda).text().unwrap(), "阿伏伽洛德常數");
        // In the middle, text on both sides.
        assert_eq!(conv(&a, &format!("ㄕˋ {AVO} ㄔㄤˊ"), "是阿福茄落得常", &[], lambda).text().unwrap(), "是阿伏伽洛德常");
    }
}

#[test]
fn rules_off_is_the_old_behaviour() {
    let mut a = assets(true);
    words(&a, &[("阿伏伽洛德", AVO, Some(1.0))]);
    a.set_user_rules(false, DEFAULT_BONUS).unwrap();
    let off = conv(&a, &format!("{AVO} ㄔㄤˊ ㄕㄨˋ"), "阿福茄落得常數", &[], 0.5);
    assert_eq!(off.text().unwrap(), "阿福茄落得常數");
    assert!(off.forced.is_empty());
}

#[test]
fn real_locks_win_and_compatible_locks_keep_the_word() {
    let a = assets(true);
    words(&a, &[("阿伏伽洛德", AVO, None)]);
    // A lock on another character: the word is not forced, the lock stays.
    let c = conv(&a, AVO, "阿福茄落得", &[None, Some('福')], 0.0);
    assert!(c.forced.is_empty());
    assert_eq!(c.chars[1], Some('福'));
    // A lock on the word's own character: still forced.
    let c = conv(&a, AVO, "阿福茄落得", &[None, Some('伏')], 0.0);
    assert_eq!(c.text().unwrap(), "阿伏伽洛德");
}

#[test]
fn overlaps_longest_then_earliest_then_lexicon_order() {
    let a = assets(true);
    // 3-char words overlapping in a chain: the earlier one wins at equal length; the longer one wins otherwise.
    words(&a, &[("伽洛德", "ㄑㄧㄝˊ ㄌㄨㄛˋ ㄉㄜˊ", None), ("阿伏伽", "ㄚ ㄈㄨˊ ㄑㄧㄝˊ", None)]);
    let c = conv(&a, AVO, "啊福茄落得", &[], 0.0);
    assert_eq!(c.forced.len(), 1);
    assert_eq!(c.forced[0].0, 0);
    assert!(c.text().unwrap().starts_with("阿伏伽"));
    words(&a, &[("伽洛德", "ㄑㄧㄝˊ ㄌㄨㄛˋ ㄉㄜˊ", None), ("阿伏伽", "ㄚ ㄈㄨˊ ㄑㄧㄝˊ", None), ("阿伏伽洛德", AVO, None)]);
    let c = conv(&a, AVO, "啊福茄落得", &[], 0.0);
    assert_eq!(c.text().unwrap(), "阿伏伽洛德");
    assert_eq!(c.forced, vec![(0, "阿伏伽洛德".chars().map(|x| a.vocab_id(x)).collect())]);
}

#[test]
fn same_reading_first_added_then_the_next_takes_over_after_reselection() {
    let a = assets(true);
    words(&a, &[("阿伏伽洛德", AVO, None), ("啊福伽洛德", AVO, None)]);
    assert_eq!(conv(&a, AVO, "阿福茄落得", &[], 0.0).text().unwrap(), "阿伏伽洛德");
    // The user picks 啊 at the first position: the first word no longer fits, the second one is forced.
    assert_eq!(conv(&a, AVO, "阿福茄落得", &[Some('啊')], 0.0).text().unwrap(), "啊福伽洛德");
}

#[test]
fn repeated_syllables_and_two_occurrences() {
    let a = assets(true);
    words(&a, &[("麻媽麻", "ㄇㄚ ㄇㄚ ㄇㄚ", None), ("阿伏伽洛德", AVO, None)]);
    assert_eq!(conv(&a, "ㄇㄚ ㄇㄚ ㄇㄚ", "媽媽媽", &[], 0.0).text().unwrap(), "麻媽麻");
    let c = conv(&a, &format!("{AVO} ㄕˋ {AVO}"), "阿福茄落得是阿福茄落得", &[], 0.5);
    assert_eq!(c.text().unwrap(), "阿伏伽洛德是阿伏伽洛德");
    assert_eq!(c.forced.len(), 2);
}

#[test]
fn two_character_words_use_the_omitted_default_or_their_weight() {
    let mut a = assets(true);
    words(&a, &[("志明", "ㄓˋ ㄇㄧㄥˊ", None)]);
    // Not forced (2 characters); omitted weight -> the deployment default.
    a.set_user_rules(true, 0.0).unwrap();
    let low = conv(&a, "ㄓˋ ㄇㄧㄥˊ", "指明", &[], 0.0);
    assert!(low.forced.is_empty());
    assert_eq!(low.text().unwrap(), "指明");
    a.set_user_rules(true, 30.0).unwrap();
    assert_eq!(conv(&a, "ㄓˋ ㄇㄧㄥˊ", "指明", &[], 0.0).text().unwrap(), "志明");
    // An explicit weight is used as written, whatever the default.
    words(&a, &[("志明", "ㄓˋ ㄇㄧㄥˊ", Some(0.0))]);
    assert_eq!(conv(&a, "ㄓˋ ㄇㄧㄥˊ", "指明", &[], 0.0).text().unwrap(), "指明");
}

#[test]
fn fallback_and_no_legal_character_paths_keep_the_forced_word() {
    let a = assets(false);
    words(&a, &[("阿伏伽洛德", AVO, None)]);
    let c = conv(&a, AVO, "", &[], 0.0);
    assert!(c.fallback);
    assert_eq!(c.text().unwrap(), "阿伏伽洛德");
    let a = assets(true);
    words(&a, &[("阿伏伽洛德", AVO, None)]);
    // A syllable without any legal character elsewhere in the composition.
    let c = conv(&a, &format!("{AVO} ㄅㄧㄤ"), "阿福茄落得", &[], 0.5);
    assert_eq!(c.chars[..5].iter().map(|x| x.unwrap()).collect::<String>(), "阿伏伽洛德");
    assert_eq!(c.chars[5], None);
}

#[test]
fn pn_jumps_crossing_a_forced_span_are_not_used() {
    let mut a = assets(true);
    let base = std::env::temp_dir().join(format!("twime-userlex-pn-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let lexb = base.join("lex");
    // A PN word compatible with the forced 德 that starts inside the span and leaves it: it would pull 長 in if used.
    let pw = vec![pn::BuildWord { word: "德長".into(), reading: "ㄉㄜˊ-ㄔㄤˊ".into(), class: 0 }];
    pn::build(&a, &pw, &lexb).unwrap();
    a.set_pn(Some(Arc::new(pn::PnLexicon::open(&lexb, &a).unwrap())), pn::PnCfg { b2: Some(60.0), b3: Some(60.0), h: Some(1), cands: true });
    words(&a, &[("阿伏伽洛德", AVO, None)]);
    let c = conv(&a, &format!("{AVO} ㄔㄤˊ"), "阿福茄落得常", &[], 0.5);
    assert_eq!(c.text().unwrap(), "阿伏伽洛德常");
    // Control: without the user word the PN jump is used.
    a.set_user(None);
    assert_eq!(conv(&a, "ㄉㄜˊ ㄔㄤˊ", "得常", &[], 0.5).text().unwrap(), "德長");
}

#[test]
fn glyph_and_ta_protection_keep_the_user_spelling() {
    let mut a = assets(true);
    a.set_norm(NormCfg::ALL);
    a.set_ta_default(true);
    words(&a, &[("臺灣她", "ㄊㄞˊ ㄨㄢ ㄊㄚ", None)]);
    assert_eq!(conv(&a, "ㄊㄞˊ ㄨㄢ ㄊㄚ", "台灣他", &[], 0.0).text().unwrap(), "臺灣她");
    // Without the user word the same input folds to the defaults.
    a.set_user(None);
    assert_eq!(conv(&a, "ㄊㄞˊ ㄨㄢ ㄊㄚ", "台灣他", &[], 0.0).text().unwrap(), "台灣他");
}

#[test]
fn candidates_put_the_forced_word_first_at_its_end() {
    let a = assets(true);
    words(&a, &[("阿伏伽洛德", AVO, None), ("啊福伽洛德", AVO, None)]);
    let c = conv(&a, &format!("{AVO} ㄔㄤˊ"), "阿福茄落得常", &[], 0.0);
    let p = CandParams::default();
    let at_end = a.candidates(&c, 5, &p);
    assert_eq!(at_end[0].text, "阿伏伽洛德");
    assert!(at_end[0].current);
    assert!(at_end.iter().any(|x| x.text == "啊福伽洛德"));
    // Cursor after the word: the usual rule (single character at the cursor is current).
    let after = a.candidates(&c, 6, &p);
    assert!(after[0].current);
    assert_eq!(after[0].text.chars().last(), Some('常'));
}

#[test]
fn typing_on_and_backspace_recompute_the_spans() {
    let a = assets(true);
    words(&a, &[("阿伏伽洛德", AVO, None)]);
    // Not yet the full reading: nothing forced.
    let partial = conv(&a, "ㄚ ㄈㄨˊ ㄑㄧㄝˊ ㄌㄨㄛˋ", "阿福茄落", &[], 0.0);
    assert!(partial.forced.is_empty());
    assert_eq!(conv(&a, AVO, "阿福茄落得", &[], 0.0).text().unwrap(), "阿伏伽洛德");
    let more = conv(&a, &format!("{AVO} ㄕㄨˋ"), "阿福茄落得數", &[], 0.0);
    assert_eq!(more.text().unwrap(), "阿伏伽洛德數");
}

#[test]
fn validation_rejects_bad_entries() {
    let a = assets(true);
    assert_eq!(a.user_word_opt("阿伏伽洛德阿伏伽洛", &syl("ㄚ ㄈㄨˊ ㄑㄧㄝˊ ㄌㄨㄛˋ ㄉㄜˊ ㄚ ㄈㄨˊ ㄑㄧㄝˊ ㄌㄨㄛˋ"), None), Err(UserWordError::Length));
    assert_eq!(a.user_word_opt("阿伏", &syl("ㄚ ㄅㄧㄤ"), None), Err(UserWordError::IllegalSyllable(1)));
    assert_eq!(a.user_word_opt("阿常", &syl("ㄚ ㄈㄨˊ"), None).unwrap().1.custom, 0b10, "an illegal pair is a custom reading (1.6)");
    assert!(matches!(a.user_word_opt("阿龘", &syl("ㄚ ㄈㄨˊ"), None), Err(UserWordError::UnknownChar(1))));
    assert_eq!(a.user_word_opt("阿伏", &syl("ㄚ ㄈㄨˊ"), Some(31.0)), Err(UserWordError::Weight));
}

// ---------------------------------------------------------------- USERLEX 1.6 custom readings
// 常 is not legal for ㄈㄨˊ and 樹 is not legal for ㄈㄨˊ: both are custom pairs inside the words below.

#[test]
fn custom_reading_is_validated_and_marked() {
    let a = assets(true);
    let (_, w) = a.user_word_opt("阿常伽", &syl("ㄚ ㄈㄨˊ ㄑㄧㄝˊ"), None).unwrap();
    assert_eq!(w.custom, 0b010);
    assert!(w.is_custom(1) && !w.is_custom(0));
    // A syllable outside the model's syllable table is still rejected.
    assert_eq!(a.user_word_opt("阿常", &syl("ㄚ ㄅㄧㄤ"), None), Err(UserWordError::IllegalSyllable(1)));
}

#[test]
fn custom_three_char_word_is_forced_and_never_spills() {
    let a = assets(true);
    words(&a, &[("阿常伽洛德", AVO, None)]);
    for lambda in [0.0, 0.5] {
        assert_eq!(conv(&a, &format!("{AVO} ㄔㄤˊ"), "阿福茄落得常", &[], lambda).text().unwrap(), "阿常伽洛德常");
        // Outside the word's full reading 常 never appears for ㄈㄨˊ.
        for s in ["ㄈㄨˊ", "ㄚ ㄈㄨˊ", "ㄚ ㄈㄨˊ ㄑㄧㄝˊ ㄌㄨㄛˋ", "ㄕˋ ㄈㄨˊ ㄉㄜˊ"] {
            let c = conv(&a, s, "", &[], lambda);
            assert!(!c.text().unwrap().contains('常'), "{s}");
        }
    }
}

#[test]
fn custom_two_char_word_scores_with_the_floor_plus_its_bonus() {
    let a = assets(true);
    words(&a, &[("阿常", "ㄚ ㄈㄨˊ", Some(30.0))]);
    assert_eq!(conv(&a, "ㄚ ㄈㄨˊ", "阿福", &[], 0.0).text().unwrap(), "阿常");
    words(&a, &[("阿常", "ㄚ ㄈㄨˊ", Some(0.0))]);
    assert_eq!(conv(&a, "ㄚ ㄈㄨˊ", "阿福", &[], 0.0).text().unwrap(), "阿福");
}

#[test]
fn custom_single_char_is_a_candidate_only_and_kept_after_reselection() {
    let a = assets(true);
    words(&a, &[("常", "ㄈㄨˊ", None)]);
    let c = conv(&a, "ㄚ ㄈㄨˊ", "阿福", &[], 0.5);
    assert_eq!(c.text().unwrap(), "阿福");
    let cands = a.candidates(&c, 2, &CandParams::default());
    assert!(cands.iter().any(|x| x.text == "常" && x.score.is_finite()));
    // Choosing it locks it; recomputation keeps it, also after typing on.
    assert_eq!(conv(&a, "ㄚ ㄈㄨˊ", "阿福", &[None, Some('常')], 0.5).chars[1], Some('常'));
    assert_eq!(conv(&a, "ㄚ ㄈㄨˊ ㄉㄜˊ", "阿福得", &[None, Some('常')], 0.5).chars[1], Some('常'));
    // The same character locked where no custom word matches (another syllable) is ignored as before.
    assert_ne!(conv(&a, "ㄚ ㄕㄨˋ", "阿數", &[None, Some('常')], 0.5).chars[1], Some('常'));
}

#[test]
fn custom_lock_dies_with_the_match() {
    let a = assets(true);
    words(&a, &[("阿常伽洛德", AVO, None)]);
    // Locked 常 at position 1 while the full reading matches: kept.
    assert_eq!(conv(&a, AVO, "阿福茄落得", &[None, Some('常')], 0.0).chars[1], Some('常'));
    // Backspace removes the last syllable: the word no longer matches, the lock is refused, 1 decodes normally.
    let c = conv(&a, "ㄚ ㄈㄨˊ ㄑㄧㄝˊ ㄌㄨㄛˋ", "阿福茄落", &[None, Some('常')], 0.0);
    assert_eq!(c.chars[1], Some('福'));
}

#[test]
fn homophone_custom_words_reselect_each_other() {
    let a = assets(true);
    words(&a, &[("阿常伽", "ㄚ ㄈㄨˊ ㄑㄧㄝˊ", None), ("阿樹伽", "ㄚ ㄈㄨˊ ㄑㄧㄝˊ", None)]);
    let c = conv(&a, "ㄚ ㄈㄨˊ ㄑㄧㄝˊ", "阿福茄", &[], 0.0);
    assert_eq!(c.text().unwrap(), "阿常伽");
    let cands = a.candidates(&c, 3, &CandParams::default());
    assert_eq!(cands[0].text, "阿常伽");
    assert!(cands.iter().any(|x| x.text == "阿樹伽"));
    assert!(cands.iter().all(|x| x.score.is_finite()), "every score finite with a custom character in the output");
    // Picking 樹 (a lock the second word allows) makes the second word forced; picking 常 back returns.
    assert_eq!(conv(&a, "ㄚ ㄈㄨˊ ㄑㄧㄝˊ", "阿福茄", &[None, Some('樹')], 0.0).text().unwrap(), "阿樹伽");
    assert_eq!(conv(&a, "ㄚ ㄈㄨˊ ㄑㄧㄝˊ", "阿福茄", &[None, Some('常')], 0.0).text().unwrap(), "阿常伽");
}


// ---------------------------------------------------------------- review r1 additions

#[test]
fn rules_off_keeps_the_old_validation_and_no_custom_locks() {
    let mut a = assets(true);
    a.set_user_rules(false, DEFAULT_BONUS).unwrap();
    assert_eq!(a.user_word_opt("阿常", &syl("ㄚ ㄈㄨˊ"), Some(30.0)), Err(UserWordError::IllegalChar(1)));
    words(&a, &[("阿伏", "ㄚ ㄈㄨˊ", Some(1.0))]);
    assert_ne!(conv(&a, "ㄚ ㄈㄨˊ", "阿福", &[None, Some('常')], 0.0).chars[1], Some('常'));
}

#[test]
fn forced_word_with_a_real_prior_and_a_document_cache() {
    let mut a = assets(true);
    with_prior(&mut a);
    words(&a, &[("阿伏伽洛德", AVO, None)]);
    assert_eq!(conv(&a, &format!("ㄕˋ {AVO} ㄔㄤˊ"), "是阿福茄落得常", &[], 0.5).text().unwrap(), "是阿伏伽洛德常");
    // A document cache that rewards the model's characters inside the span does not move the forced word.
    let ids = |t: &str| t.chars().map(|c| a.vocab_id(c)).collect::<Vec<_>>();
    let f = ids("福茄落得");
    let cache = crate::cache::CacheSet::from_pairs(&[(f[0], f[1]), (f[1], f[2]), (f[2], f[3])], (a.vocab_id('臺'), a.vocab_id('台')), 50.0, None);
    let s = syl(AVO);
    let base = conv(&a, AVO, "阿福茄落得", &[], 0.5);
    let c = a.rebeam_with_cache(&base, &[], 0.5, false, None, true, Some(&cache));
    assert_eq!(c.text().unwrap(), "阿伏伽洛德");
    let _ = s;
}

#[test]
fn more_than_twenty_same_reading_words_are_all_listed() {
    let a = assets(true);
    let alts = [("阿", "啊"), ("福", "伏"), ("茄", "伽"), ("落", "洛"), ("得", "德")];
    let mut list: Vec<String> = vec![];
    for m in 0..32u32 {
        let w: String = alts.iter().enumerate().map(|(k, (x, y))| if m & (1 << k) != 0 { *y } else { *x }).collect();
        if w != "阿福茄落得" { list.push(w); }
    }
    list.truncate(25);
    let entries: Vec<_> = list.iter().map(|w| (w.as_str(), AVO, None)).collect();
    words(&a, &entries);
    let c = conv(&a, AVO, "阿福茄落得", &[], 0.0);
    assert_eq!(c.text().unwrap(), list[0]);
    let cands = a.candidates(&c, 5, &CandParams::default());
    assert_eq!(cands[0].text, list[0]);
    for w in &list {
        assert!(cands.iter().any(|x| &x.text == w), "{w} listed");
    }
}

#[test]
fn pn_jumps_entering_or_covering_a_forced_span_are_not_used() {
    let mut a = assets(true);
    pn_words(&mut a, &[("事阿", "ㄕˋ-ㄚ"), ("事阿伏伽洛德", "ㄕˋ-ㄚ-ㄈㄨˊ-ㄑㄧㄝˊ-ㄌㄨㄛˋ-ㄉㄜˊ")], 60.0);
    words(&a, &[("阿伏伽洛德", AVO, None)]);
    assert_eq!(conv(&a, &format!("ㄕˋ {AVO}"), "是阿福茄落得", &[], 0.5).text().unwrap(), "是阿伏伽洛德");
    // Control: the entering jump alone (no user word) is used.
    a.set_user(None);
    assert_eq!(conv(&a, "ㄕˋ ㄚ", "是阿", &[], 0.5).text().unwrap(), "事阿");
}

#[test]
fn user_jumps_crossing_a_forced_span_must_fit_it() {
    let a = assets(true);
    // 德長 fits the forced 德 and may be used; 得長 does not fit and is dropped.
    // (The forced word gets weight 0 so its own jump bonus does not tie with 德長's net gain.)
    words(&a, &[("阿伏伽洛德", AVO, Some(0.0)), ("德長", "ㄉㄜˊ ㄔㄤˊ", Some(30.0))]);
    assert_eq!(conv(&a, &format!("{AVO} ㄔㄤˊ"), "阿福茄落得常", &[], 0.0).text().unwrap(), "阿伏伽洛德長");
    words(&a, &[("阿伏伽洛德", AVO, Some(0.0)), ("得長", "ㄉㄜˊ ㄔㄤˊ", Some(30.0))]);
    assert_eq!(conv(&a, &format!("{AVO} ㄔㄤˊ"), "阿福茄落得常", &[], 0.0).text().unwrap(), "阿伏伽洛德常");
}

