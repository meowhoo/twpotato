//! NORM-MO (NORM-MO-plan): the model sees the light-tone syllable ㄇㄛ˙ as ㄇㄜ˙; everything else keeps the original
//! reading. Synthetic data only.
use super::*;

const DICT: &str = "ㄕㄜˊ 蛇 -1\nㄕㄜˊ 什 -2\nㄇㄛ˙ 麼 -1\nㄇㄛ˙ 摸 -5\nㄇㄜ˙ 麼 -1\nㄇㄜ˙ 麽 -5\nㄊㄡˊ 頭 -1\nㄊㄡˊ 投 -2\nㄕˋ 是 -1\nㄕˋ 事 -2\n";

fn assets(mo: bool) -> Assets {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    a.model = Some(crate::n1::n1_prefix_tests::model(a.vocab.len()));
    a.set_mo_norm(mo);
    a
}

fn syl(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

fn bits(s: &[Vec<f32>]) -> Vec<Vec<u32>> {
    s.iter().map(|v| v.iter().map(|x| x.to_bits()).collect()).collect()
}

/// Reference: encode the original syllables, replace the base syllable tokens by hand, run the same forward path.
fn reference(a: &Assets, context: &str, s: &[String]) -> Vec<Vec<f32>> {
    let (mut tokens, first) = n1::encode(&a.vocab, context, s);
    let (mo, me) = (a.vocab.syllable_id("ㄇㄛ˙"), a.vocab.syllable_id("ㄇㄜ˙"));
    for t in &mut tokens[first..] {
        if *t == mo {
            *t = me;
        }
    }
    let sets = a.full_sets(s);
    let refs: Vec<&[u32]> = sets.iter().map(Vec::as_slice).collect();
    a.model.as_ref().unwrap().scores(&tokens, first, &refs).unwrap()
}

#[test]
fn off_is_the_old_path_and_on_without_mo_changes_nothing() {
    let (off, on) = (assets(false), assets(true));
    for (ctx, s) in [("", "ㄕㄜˊ ㄊㄡˊ"), ("是", "ㄕˋ ㄕㄜˊ ㄊㄡˊ"), ("", "ㄕˋ")] {
        let (c0, c1) = (off.convert(ctx, &syl(s), &[], 0.0), on.convert(ctx, &syl(s), &[], 0.0));
        assert_eq!(bits(&c0.inf.scores), bits(&c1.inf.scores), "{s}");
        assert_eq!(c0.text(), c1.text());
    }
    // off with a ㄇㄛ˙ input = direct forward on the original tokens
    let s = syl("ㄕㄜˊ ㄇㄛ˙");
    let c = off.convert("", &s, &[], 0.0);
    let (tokens, first) = n1::encode(&off.vocab, "", &s);
    let sets = off.full_sets(&s);
    let refs: Vec<&[u32]> = sets.iter().map(Vec::as_slice).collect();
    assert_eq!(bits(&c.inf.scores), bits(&off.model.as_ref().unwrap().scores(&tokens, first, &refs).unwrap()));
}

#[test]
fn on_feeds_the_model_me_but_keeps_reading_and_legal_sets() {
    let (off, on) = (assets(false), assets(true));
    for (ctx, s) in [("", "ㄕㄜˊ ㄇㄛ˙"), ("是", "ㄕˋ ㄕㄜˊ ㄇㄛ˙ ㄊㄡˊ"), ("", "ㄇㄛ˙ ㄇㄛ˙")] {
        let sy = syl(s);
        let (c0, c1) = (off.convert(ctx, &sy, &[], 0.0), on.convert(ctx, &sy, &[], 0.0));
        assert_eq!(c1.inf.syllables, sy, "original readings are kept");
        assert_eq!(c0.inf.sets, c1.inf.sets, "legal sets are those of the original reading");
        assert_eq!(c0.inf.ctx_tail, c1.inf.ctx_tail);
        assert_eq!(bits(&c1.inf.scores), bits(&reference(&on, ctx, &sy)), "{s}");
        assert_ne!(bits(&c0.inf.scores), bits(&c1.inf.scores), "the ㄇㄛ˙ positions are scored differently");
    }
    // the output character set of a ㄇㄛ˙ position stays {麼, 摸} (not ㄇㄜ˙'s {麼, 麽})
    let c = on.convert("", &syl("ㄇㄛ˙"), &[], 0.0);
    let chars: Vec<char> = c.inf.sets[0].iter().filter_map(|&i| on.vocab.char_of(i)).collect();
    assert!(chars.contains(&'摸') && chars.contains(&'麼') && !chars.contains(&'麽'));
    assert!(matches!(c.text().unwrap().as_str(), "麼" | "摸"));
}

#[test]
fn mo_input_scores_equal_me_input_scores_on_shared_characters() {
    let on = assets(true);
    let (a, b) = (on.convert("", &syl("ㄕㄜˊ ㄇㄛ˙"), &[], 0.0), on.convert("", &syl("ㄕㄜˊ ㄇㄜ˙"), &[], 0.0));
    let id = on.vocab_id('麼');
    let pick = |c: &Conversion| {
        let j = c.inf.sets[1].binary_search(&id).unwrap();
        c.inf.scores[1][j].to_bits()
    };
    assert_eq!(pick(&a), pick(&b), "the same model input gives the same score of 麼");
    assert_eq!(bits(&a.inf.scores[..1]), bits(&b.inf.scores[..1]));
}

#[test]
fn locks_user_words_and_readings_are_untouched() {
    let on = assets(true);
    let c = on.convert("", &syl("ㄕㄜˊ ㄇㄛ˙"), &[Some('蛇'), Some('摸')], 0.0);
    assert_eq!(c.text().unwrap(), "蛇摸", "a lock wins; the lock is checked against the original reading's legal set");
}

#[test]
fn cand_assembles_from_the_original_tokens_and_only_the_base_is_mapped() {
    use std::{fs, sync::Arc};
    let mut a = assets(true);
    let d = a.model_dim().unwrap();
    let dir = std::env::temp_dir().join(format!("twime-normmo-cand-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let base = dir.join("c");
    let mut lex = b"TWCL".to_vec();
    for v in [1u32, 2, 1, 0] {
        lex.extend_from_slice(&v.to_le_bytes());
    }
    lex.extend_from_slice(&[2, 0, 0, 0, 1]);
    for v in [a.vocab.syllable_id("ㄕㄜˊ") as u16, a.vocab.syllable_id("ㄇㄛ˙") as u16, a.vocab.char_id('什') as u16, a.vocab.char_id('麼') as u16, 1u16] {
        lex.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(base.with_extension("clex"), lex).unwrap();
    let mut emb = b"TWCE".to_vec();
    for v in [1u32, d as u32, 2, crate::cand::NSLOT as u32, 0] {
        emb.extend_from_slice(&v.to_le_bytes());
    }
    for _ in 0..(1 + crate::cand::NSLOT + 2 + 3 + 5) * d {
        emb.extend_from_slice(&1.0f32.to_le_bytes());
    }
    fs::write(base.with_extension("cemb"), emb).unwrap();
    a.set_cand(Some(Arc::new(crate::cand::CandCfg::open(&base, &a, None).unwrap())));
    let s = syl("ㄕㄜˊ ㄇㄛ˙");
    let (tokens, first) = n1::encode(&a.vocab, "", &s);
    let cand = a.cand.as_ref().unwrap();
    let (mut toks, pos, extra, kept) = cand.assemble(&tokens, first, d).expect("the entry matches the ORIGINAL ㄇㄛ˙ tokens");
    assert_eq!(kept.len(), 1);
    // reference: base tokens mapped by hand, appended candidate tokens, positions and extra untouched
    let (mo, me) = (a.vocab.syllable_id("ㄇㄛ˙"), a.vocab.syllable_id("ㄇㄜ˙"));
    for t in &mut toks[first..tokens.len()] {
        if *t == mo {
            *t = me;
        }
    }
    let sets = a.full_sets(&s);
    let refs: Vec<&[u32]> = sets.iter().map(Vec::as_slice).collect();
    let want = a.model.as_ref().unwrap().scores_ext(&toks, &pos, &extra, n1::POSITIONS + cand.m, first, &refs).unwrap();
    let got = a.convert("", &s, &[], 0.0);
    assert_eq!(bits(&got.inf.scores), bits(&want));
    // and with the switch off the same entry still matches and the old path is taken
    a.set_mo_norm(false);
    let (t2, p2, e2, _) = a.cand.as_ref().unwrap().assemble(&tokens, first, d).unwrap();
    let want_off = a.model.as_ref().unwrap().scores_ext(&t2, &p2, &e2, n1::POSITIONS + a.cand.as_ref().unwrap().m, first, &refs).unwrap();
    assert_eq!(bits(&a.convert("", &s, &[], 0.0).inf.scores), bits(&want_off));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn missing_syllable_empty_input_and_no_model_are_harmless() {
    // a dictionary without ㄇㄜ˙: nothing to map to, the old path
    let mut a = Assets::from_dictionary_text("ㄕㄜˊ 蛇 -1\nㄇㄛ˙ 麼 -1\n", "").unwrap();
    a.model = Some(crate::n1::n1_prefix_tests::model(a.vocab.len()));
    let s = syl("ㄕㄜˊ ㄇㄛ˙");
    a.set_mo_norm(false);
    let off = bits(&a.convert("", &s, &[], 0.0).inf.scores);
    a.set_mo_norm(true);
    assert_eq!(bits(&a.convert("", &s, &[], 0.0).inf.scores), off);
    // empty input
    let b = assets(true);
    assert_eq!(b.convert("", &[], &[], 0.0).chars.len(), 0);
    // no model (fallback): the conversion does not depend on the switch
    let mut c = Assets::from_dictionary_text(DICT, "").unwrap();
    c.set_mo_norm(false);
    let t0 = c.convert("", &syl("ㄕㄜˊ ㄇㄛ˙"), &[], 0.0);
    c.set_mo_norm(true);
    let t1 = c.convert("", &syl("ㄕㄜˊ ㄇㄛ˙"), &[], 0.0);
    assert!(t0.fallback && t1.fallback && t0.text() == t1.text());
}

#[test]
fn prefix_cache_cold_and_hot_give_the_same_scores() {
    let a = assets(true);
    let s = syl("ㄕˋ ㄕㄜˊ ㄇㄛ˙");
    a.model.as_ref().unwrap().set_prefix_cache(false);
    let cold = bits(&a.convert("是", &s, &[], 0.0).inf.scores);
    a.model.as_ref().unwrap().set_prefix_cache(true);
    let first = bits(&a.convert("是", &s, &[], 0.0).inf.scores);
    let hot = bits(&a.convert("是", &s, &[], 0.0).inf.scores);
    assert_eq!(cold, first);
    assert_eq!(first, hot);
    assert_eq!(hot, bits(&reference(&a, "是", &s)));
}

#[test]
fn user_words_and_pn_lookups_keep_the_original_reading() {
    let mut a = assets(true);
    a.set_user_rules(true, crate::user::DEFAULT_BONUS).unwrap();
    let w = a.user_word_opt("蛇麼是", &syl("ㄕㄜˊ ㄇㄛ˙ ㄕˋ"), None).unwrap();
    a.set_user(Some(std::sync::Arc::new(a.build_user_lexicon([w]))));
    let c = a.convert("", &syl("ㄕㄜˊ ㄇㄛ˙ ㄕˋ"), &[], 0.0);
    assert_eq!(c.text().unwrap(), "蛇麼是", "a forced user word typed with ㄇㄛ˙ still matches");
    // the same word under the ㄇㄜ˙ spelling is a different reading and does not match
    let c = a.convert("", &syl("ㄕㄜˊ ㄇㄜ˙ ㄕˋ"), &[], 0.0);
    assert_ne!(c.inf.syllables, syl("ㄕㄜˊ ㄇㄛ˙ ㄕˋ"));
    a.set_user(None);
    let base = std::env::temp_dir().join(format!("twime-normmo-pn-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let lex = base.join("lex");
    let words = [crate::pn::BuildWord { word: "什麼".into(), reading: "ㄕㄜˊ-ㄇㄛ˙".into(), class: 0 }];
    crate::pn::build(&a, &words, &lex).unwrap();
    a.set_pn(Some(std::sync::Arc::new(crate::pn::PnLexicon::open(&lex, &a).unwrap())), crate::pn::PnCfg::default());
    assert_eq!(a.pn_words(&syl("ㄕㄜˊ ㄇㄛ˙")).into_iter().map(|x| x.0).collect::<Vec<_>>(), vec!["什麼".to_string()]);
    assert!(a.pn_words(&syl("ㄕㄜˊ ㄇㄜ˙")).is_empty());
    let _ = std::fs::remove_dir_all(&base);
}
