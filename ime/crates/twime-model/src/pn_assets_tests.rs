//! Assets-level PN tests (tiny dictionary, random weights, lexicon files in a temp dir).

use std::sync::Arc;

use super::*;
use crate::{CandParams, NormCfg, n1::N1};

const DICT: &str = "\
ㄊㄞˊ 台 -2.5
ㄊㄞˊ 臺 -2
ㄊㄞˊ 抬 -3
ㄅㄟˇ 北 -2
ㄅㄟˇ 貝 -3
ㄅㄚ 八 -2
ㄅㄚ 巴 -2.2
ㄅㄚ 吧 -2.4
ㄅㄚ 疤 -3
ㄅㄚ 芭 -3.1
ㄇㄚ 媽 -2
ㄇㄚ 麻 -2.5
ㄑㄩ 區 -2
ㄑㄩ 屈 -3
ㄊㄞˊ-ㄅㄟˇ 臺北 -3
ㄅㄚ-ㄅㄚ 爸爸 -2.6
ㄅㄟˇ-ㄑㄩ 北區 -4
ㄅㄚ 爸 -2.1
";

fn syl(s: &str) -> Vec<String> {
    s.split(' ').map(str::to_string).collect()
}

fn tiny(norm: NormCfg) -> Assets {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    let v = a.vocab.len();
    a.model = Some(N1::random(v, 32, 1, 7));
    a.set_norm(norm);
    a
}

fn lexicon(a: &Assets, name: &str, words: &[(&str, &str)]) -> Arc<PnLexicon> {
    let dir = std::env::temp_dir().join(format!("twime-pn-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("lex");
    let ws: Vec<BuildWord> = words.iter().map(|(w, r)| BuildWord { word: w.to_string(), reading: r.to_string(), class: 0 }).collect();
    build(a, &ws, &base).unwrap();
    Arc::new(PnLexicon::open(&base, a).unwrap())
}

fn big(b: f64) -> PnCfg {
    PnCfg { b2: Some(b), b3: Some(b), h: None, cands: true }
}

const NAMES: [(&str, &str); 4] = [("台北", "ㄊㄞˊ-ㄅㄟˇ"), ("巴媽", "ㄅㄚ-ㄇㄚ"), ("芭疤巴吧八", "ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ"), ("貝區", "ㄅㄟˇ-ㄑㄩ")];

#[test]
fn disabled_or_empty_lexicon_is_bit_identical() {
    let inputs = ["ㄊㄞˊ ㄅㄟˇ ㄅㄚ ㄇㄚ", "ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄑㄩ", "ㄅㄟˇ ㄑㄩ ㄊㄞˊ", "ㄅㄚ ㄒㄧ ㄇㄚ"];
    for norm in [NormCfg::default(), NormCfg::ALL] {
        let plain = tiny(norm);
        let mut empty = tiny(norm);
        let e = lexicon(&empty, "empty", &[]);
        assert!(e.is_empty());
        empty.set_pn(Some(e), big(50.0));
        assert!(empty.pn().is_none(), "an empty lexicon is the old path");
        let mut off = tiny(norm);
        off.set_pn(None, big(50.0));
        let mut fb = Assets::from_dictionary_text(DICT, "").unwrap();
        fb.set_norm(norm);
        let fbl = lexicon(&fb, "fb", &NAMES);
        let mut fb_on = Assets::from_dictionary_text(DICT, "").unwrap();
        fb_on.set_norm(norm);
        fb_on.set_pn(Some(fbl), big(50.0));
        assert!(fb_on.pn().is_some());
        for inp in inputs {
            let s = syl(inp);
            let one = plain.convert("", &s, &[], 0.0).chars[1];
            let lock_sets: Vec<Vec<Option<char>>> = vec![vec![None; s.len()], (0..s.len()).map(|i| if i == 1 { one } else { None }).collect()];
            for locks in lock_sets {
                for lambda in [0.0, 1.0] {
                    for style in [false, true] {
                        let x = plain.convert_ex("台", &s, &locks, lambda, style);
                        for other in [&empty, &off] {
                            let y = other.convert_ex("台", &s, &locks, lambda, style);
                            assert_eq!(x.chars, y.chars);
                            for end in 1..=s.len() {
                                let p = CandParams { lambda, ..CandParams::default() };
                                assert_eq!(plain.candidates(&x, end, &p), other.candidates(&y, end, &p));
                            }
                        }
                        // fallback mode (no model): PN does nothing, conversion and list unchanged
                        let (a, b) = (fb.convert_ex("", &s, &locks, lambda, style), fb_on.convert_ex("", &s, &locks, lambda, style));
                        assert!(a.fallback && b.fallback);
                        assert_eq!(a.chars, b.chars);
                        for end in 1..=s.len() {
                            assert_eq!(fb.candidates(&a, end, &CandParams::default()), fb_on.candidates(&b, end, &CandParams::default()));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn jumps_work_without_prior_and_lambda_zero_and_skip_empty_sets() {
    let mut a = tiny(NormCfg::default());
    let lex = lexicon(&a, "j", &NAMES);
    a.set_pn(Some(lex), big(1000.0));
    // no prior (tiny assets) and lambda 0 or not: the PN beam with prior term 0; the huge bonus forces the words
    for lambda in [0.0, 0.5] {
        assert_eq!(a.convert("", &syl("ㄅㄚ ㄇㄚ"), &[], lambda).text().unwrap(), "巴媽");
        assert_eq!(a.convert("", &syl("ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ"), &[], lambda).text().unwrap(), "芭疤巴吧八");
    }
    // a syllable without any legal character: PN is not applied (same as the old path)
    let s = syl("ㄅㄚ ㄇㄚ ㄒㄧ");
    let plain = tiny(NormCfg::default());
    assert_eq!(a.convert("", &s, &[], 0.0).chars, plain.convert("", &s, &[], 0.0).chars);
    // a length switched off: no jump of that length
    let mut b = tiny(NormCfg::default());
    let lb = lexicon(&b, "j2", &NAMES);
    b.set_pn(Some(lb), PnCfg { b2: None, b3: Some(1000.0), h: None, cands: true });
    assert_eq!(b.convert("", &syl("ㄅㄚ ㄇㄚ"), &[], 0.0).chars, plain.convert("", &syl("ㄅㄚ ㄇㄚ"), &[], 0.0).chars);
    assert_eq!(b.convert("", &syl("ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ"), &[], 0.0).text().unwrap(), "芭疤巴吧八");
}

#[test]
fn locks_are_kept_and_fold_matches_tai() {
    for norm in [NormCfg::ALL, NormCfg::default()] {
        let mut a = tiny(norm);
        let lex = lexicon(&a, &format!("lock{}", norm.out), &[("台貝", "ㄊㄞˊ-ㄅㄟˇ")]);
        a.set_pn(Some(lex), PnCfg { b2: Some(1000.0), b3: None, h: None, cands: true });
        let s = syl("ㄊㄞˊ ㄅㄟˇ");
        let locks = [Some('臺'), None];
        let c = a.convert("", &s, &locks, 0.0);
        let raw = a.rebeam_with(&c, &locks, 0.0, false, a.pn().map(|x| &x.1), false);
        assert_eq!(raw.chars[0], Some('臺'), "locked character not overwritten");
        if norm.out {
            // with the output fold the 台 word matches the locked 臺 (folded key) and the locked 臺 is written
            assert_eq!(raw.chars[1], Some('貝'));
            assert_eq!(c.text().unwrap(), "臺貝");
        } else {
            // without it the word does not fit the lock: the old path
            let plain = tiny(norm);
            assert_eq!(c.chars, plain.convert("", &s, &locks, 0.0).chars);
        }
    }
    // style following writes the unlocked 台 of a jump as 臺; default 台
    let mut a = tiny(NormCfg::ALL);
    let l = lexicon(&a, "style", &[("台貝", "ㄊㄞˊ-ㄅㄟˇ")]);
    a.set_pn(Some(l), big(1000.0));
    assert_eq!(a.convert_ex("", &syl("ㄊㄞˊ ㄅㄟˇ"), &[], 0.0, true).text().unwrap(), "臺貝");
    assert_eq!(a.convert_ex("", &syl("ㄊㄞˊ ㄅㄟˇ"), &[], 0.0, false).text().unwrap(), "台貝");
}

#[test]
fn candidate_list_lexicon_words() {
    let mut a = tiny(NormCfg::default());
    let mut words: Vec<(String, String)> = NAMES.iter().map(|(w, r)| (w.to_string(), r.to_string())).collect();
    // 8 syllables, longer than max_span 6
    words.push(("八巴吧疤芭八巴吧".into(), "ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ".into()));
    // 25 two-syllable ㄅㄚ-ㄅㄚ words (more than the PN cap of 20) and 爸爸, which data.txt also has
    let cs = ['八', '巴', '吧', '疤', '芭'];
    for x in cs {
        for y in cs {
            words.push((format!("{x}{y}"), "ㄅㄚ-ㄅㄚ".into()));
        }
    }
    words.push(("爸爸".into(), "ㄅㄚ-ㄅㄚ".into()));
    let wr: Vec<(&str, &str)> = words.iter().map(|(w, r)| (w.as_str(), r.as_str())).collect();
    let lex = lexicon(&a, "cands", &wr);
    let p = CandParams::default();
    let s8 = syl("ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ");
    let off = tiny(NormCfg::default());
    let base = off.convert("", &s8, &[], 0.0);
    let before = off.candidates(&base, 8, &p);
    a.set_pn(Some(lex.clone()), PnCfg { b2: None, b3: None, h: None, cands: true });
    let conv = a.convert("", &s8, &[], 0.0);
    assert_eq!(conv.chars, base.chars, "no jumps configured: same conversion");
    let c = a.candidates(&conv, 8, &p);
    let t: Vec<&str> = c.iter().map(|x| x.text.as_str()).collect();
    assert!(c.iter().all(|x| x.span <= 8));
    // an 8-syllable lexicon word is listed (span beyond max_span 6) when the PN cap leaves room for it
    let mut a8 = tiny(NormCfg::default());
    let l8 = lexicon(&a8, "eight", &[("八巴吧疤芭八巴吧", "ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ"), ("芭疤巴吧八巴吧", "ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ")]);
    a8.set_pn(Some(l8), PnCfg::default());
    let c8 = a8.candidates(&a8.convert("", &s8, &[], 0.0), 8, &p);
    assert!(c8.iter().any(|x| x.text == "八巴吧疤芭八巴吧" && x.span == 8) && c8.iter().any(|x| x.span == 7), "7/8-syllable lexicon words listed");
    // lexicon-only two-syllable words: at most 20 (own cap); 爸爸 (also in data.txt) is one item
    let pn_only2 = c.iter().filter(|x| x.span == 2 && !x.current && x.text != "爸爸").count();
    assert!(pn_only2 <= 20 && pn_only2 >= 19, "{pn_only2}");
    assert_eq!(t.iter().filter(|x| **x == "爸爸").count(), 1);
    // the old items are all still there (the dictionary words keep their own cap)
    for b in &before {
        assert!(c.iter().any(|x| x.text == b.text && x.span == b.span), "{} lost", b.text);
    }
    // PN words kept out of the list: the list is the old one item for item
    a.set_pn(Some(lex.clone()), PnCfg { b2: None, b3: None, h: None, cands: false });
    let c2 = a.convert("", &s8, &[], 0.0);
    assert_eq!(a.candidates(&c2, 8, &p), before);
    // long word + same-tail short word: the current item is the shortest multi-character word (rule unchanged)
    let mut b = tiny(NormCfg::default());
    let lt = lexicon(&b, "tail", &[("芭疤巴吧八", "ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ-ㄅㄚ"), ("吧八", "ㄅㄚ-ㄅㄚ")]);
    b.set_pn(Some(lt), PnCfg { b2: None, b3: Some(1000.0), h: None, cands: true });
    let s5 = syl("ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ ㄅㄚ");
    let cv = b.convert("", &s5, &[], 0.0);
    assert_eq!(cv.text().unwrap(), "芭疤巴吧八");
    let cl = b.candidates(&cv, 5, &p);
    assert!(cl[0].current && cl[0].text == "吧八" && cl[0].span == 2, "{:?}", cl[0]);
    // NORM pairs: a lexicon 台 word gets its 臺 partner
    let mut n = tiny(NormCfg::ALL);
    let lp = lexicon(&n, "pair", &[("台貝", "ㄊㄞˊ-ㄅㄟˇ")]);
    n.set_pn(Some(lp), PnCfg::default());
    let cn = n.convert("", &syl("ㄊㄞˊ ㄅㄟˇ"), &[], 0.0);
    let tn: Vec<String> = n.candidates(&cn, 2, &p).into_iter().map(|x| x.text).collect();
    assert!(tn.contains(&"台貝".to_string()) && tn.contains(&"臺貝".to_string()), "{tn:?}");
}
