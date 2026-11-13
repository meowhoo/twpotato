//! TA: ㄊㄚ variants of the automatic conversion become 他, except locked positions, confirmed cache
//! support, proper names of the PN lexicon that the output spells (臺 read as 台), and the switch off.

use std::sync::Arc;

use super::*;
use crate::{
    NormCfg,
    cache::{CacheCfg, CacheSet, CacheSource, Confirmed},
    pn::{BuildWord, PnCfg, PnLexicon, build},
};

// No model: the conversion is the best single character of each syllable (她 over 他, 它 over 他 in 其它, 台 over 臺).
const DICT: &str = "\
ㄊㄚ 她 -1
ㄊㄚ 他 -2
ㄊㄚ 它 -3
ㄊㄚ 牠 -4
ㄊㄚ 祂 -5
ㄕㄨㄛ 說 -1
ㄇㄣˊ 們 -1
ㄑㄧˊ 其 -1
ㄇㄚ 媽 -1
ㄊㄞˊ 台 -1
ㄊㄞˊ 臺 -2
";

fn syl(s: &str) -> Vec<String> {
    s.split(' ').map(String::from).collect()
}

fn assets(ta: bool) -> Assets {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    a.set_norm(NormCfg::ALL);
    a.set_ta_default(ta);
    a
}

fn text(c: &Conversion) -> String {
    c.chars.iter().map(|c| c.unwrap_or('?')).collect()
}

fn with_lexicon(mut a: Assets, words: &[(&str, &str)]) -> Assets {
    let dir = std::env::temp_dir().join(format!("twime-ta-test-{}-{}", std::process::id(), words.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("lex");
    let ws: Vec<BuildWord> = words.iter().map(|(w, r)| BuildWord { word: w.to_string(), reading: r.to_string(), class: 0 }).collect();
    build(&a, &ws, &base).unwrap();
    let lex = PnLexicon::open(&base, &a).unwrap();
    a.set_pn(Some(Arc::new(lex)), PnCfg { b2: Some(4.0), b3: Some(4.0), h: None, cands: true });
    a
}

#[test]
fn single_characters_and_variant_words_become_ta() {
    let (on, off) = (assets(true), assets(false));
    let s = syl("ㄊㄚ ㄕㄨㄛ ㄊㄚ ㄇㄣˊ ㄑㄧˊ ㄊㄚ");
    assert_eq!(text(&off.convert_ex("", &s, &[], 0.0, false)), "她說她們其她");
    assert_eq!(text(&on.convert_ex("", &s, &[], 0.0, false)), "他說他們其他");
    // a locked variant stays; other positions are untouched
    let locks = [Some('它'), None, None, None, None, None];
    assert_eq!(text(&on.convert_ex("", &s, &locks, 0.0, false)), "它說他們其他");
    // other readings never change
    assert_eq!(text(&on.convert_ex("", &syl("ㄇㄚ ㄕㄨㄛ"), &[], 0.0, false)), "媽說");
}

#[test]
fn switch_off_is_bit_identical() {
    let (a, b) = (assets(false), Assets::from_dictionary_text(DICT, "").unwrap());
    let mut b = b;
    b.set_norm(NormCfg::ALL);
    for s in ["ㄊㄚ ㄕㄨㄛ", "ㄑㄧˊ ㄊㄚ", "ㄊㄚ ㄇㄣˊ ㄊㄞˊ"] {
        assert_eq!(a.convert_ex("", &syl(s), &[], 0.0, true).chars, b.convert_ex("", &syl(s), &[], 0.0, true).chars);
    }
}

#[test]
fn confirmed_cache_supports_the_variant_but_the_document_cache_does_not() {
    let a = assets(true);
    let s = syl("ㄊㄚ ㄕㄨㄛ");
    let cfg = |source| CacheCfg { gamma: 1.0, window: 64, cap: None, source };
    let mut r = Confirmed::default();
    r.commit(&a, &['她', '說'], &[true, true]);
    let conf = CacheSet::from_text_confirmed(&a, "她說", &cfg(CacheSource::Confirmed), &mut r);
    assert!(conf.is_confirmed() && !conf.is_empty());
    assert_eq!(text(&a.convert_cached("", Some(Arc::new(conf)), &s, &[], 0.0, false)), "她說");
    let doc = CacheSet::from_text(&a, "她說", &cfg(CacheSource::Doc));
    assert!(!doc.is_confirmed() && !doc.is_empty());
    assert_eq!(text(&a.convert_cached("", Some(Arc::new(doc)), &s, &[], 0.0, false)), "他說");
}

#[test]
fn proper_names_stay_variant_words_do_not() {
    // 她媽 is a name of the lexicon; 她們 is a variant word even when it is in the lexicon
    let a = with_lexicon(assets(true), &[("她媽", "ㄊㄚ-ㄇㄚ"), ("她們", "ㄊㄚ-ㄇㄣˊ"), ("台她", "ㄊㄞˊ-ㄊㄚ")]);
    assert_eq!(text(&a.convert_ex("", &syl("ㄊㄚ ㄇㄚ ㄊㄚ ㄕㄨㄛ"), &[], 0.0, false)), "她媽他說");
    assert_eq!(text(&a.convert_ex("", &syl("ㄊㄚ ㄇㄣˊ"), &[], 0.0, false)), "他們");
    // the 臺 style changes the name's other character: still the name (臺 read as 台 when comparing)
    let c = a.convert_ex("", &syl("ㄊㄞˊ ㄊㄚ"), &[], 0.0, true);
    assert_eq!(text(&c), "臺她");
}
