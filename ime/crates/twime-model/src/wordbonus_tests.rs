//! WORDBONUS (WORDBONUS-plan): table format and validation, jumps, locks, forced user words, empty table = off.
//! Synthetic data only.
use super::*;
use crate::{Assets, Conversion};
use std::sync::Arc;

const DICT: &str = "ㄕㄜˊ 蛇 -1\nㄕㄜˊ 什 -2\nㄕㄜˊ 舌 -3\nㄇㄛ˙ 麼 -1\nㄇㄛ˙ 嘛 -2\nㄇㄜ˙ 麼 -1\nㄇㄜ˙ 嘛 -2\n\
ㄊㄡˊ 頭 -1\nㄊㄡˊ 投 -2\nㄕˋ 是 -1\nㄕˋ 事 -2\nㄕㄜˊ-ㄊㄡˊ 蛇頭 -3\n";

fn syl(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

fn assets() -> Assets {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    a.model = Some(crate::n1::N1::random(a.vocab.len(), 32, 1, 7));
    a.set_user_rules(true, crate::user::DEFAULT_BONUS).unwrap();
    a
}

fn row(reading: &str, word: &str, bonus: f64) -> WbRow {
    WbRow { reading: reading.into(), word: word.into(), bonus }
}

fn table(a: &Assets, rows: &[WbRow]) -> Arc<WordBonus> {
    Arc::new(WordBonus::from_bytes(a, &build(a, rows).unwrap()).unwrap())
}

fn what(b: f64) -> Vec<WbRow> {
    vec![row("ㄕㄜˊ-ㄇㄛ˙", "什麼", b), row("ㄕㄜˊ-ㄇㄜ˙", "什麼", b)]
}

/// Model scores that strongly prefer `preferred` (one char per syllable), as in the USERLEX tests.
fn conv(a: &Assets, s: &str, preferred: &str, locks: &[Option<char>]) -> Conversion {
    let mut c = a.convert("", &syl(s), &[], 0.0);
    let inf = Arc::get_mut(&mut c.inf).unwrap();
    for ((ids, scores), ch) in inf.sets.iter().zip(&mut inf.scores).zip(preferred.chars()) {
        for (&id, score) in ids.iter().zip(scores) {
            *score = if id == a.vocab_id(ch) { 20.0 } else { 0.0 };
        }
    }
    a.rebeam(&c, locks, 0.0)
}

fn text(c: &Conversion) -> String {
    c.text().unwrap()
}

#[test]
fn tsv_parsing() {
    let rows = parse_tsv("# c\n\nㄕㄜˊ-ㄇㄛ˙\t什麼\t6 # tail\r\nㄕㄜˊ-ㄇㄜ˙\t什麼\t6\n").unwrap();
    assert_eq!(rows, what(6.0));
    for bad in ["ㄕㄜˊ-ㄇㄛ˙\t什麼\n", "ㄕㄜˊ-ㄇㄛ˙\t什麼\tx\n", "ㄕㄜˊ-ㄇㄛ˙\t什麼\t6\textra\n", "\t什麼\t6\n"] {
        assert!(parse_tsv(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn build_rejects_bad_rows() {
    let a = assets();
    for r in [
        row("ㄕㄜˊ-ㄇㄛ˙", "什嘛x", 1.0),          // length mismatch
        row("ㄕㄜˊ", "什", 1.0),                    // too short
        row("ㄕㄜˊ-ㄇㄛ˙", "什頭", 1.0),            // 頭 is not legal for ㄇㄛ˙
        row("ㄕㄜˊ-ㄏㄏ", "什麼", 1.0),             // unknown syllable
        row("ㄕㄜˊ-ㄇㄛ˙", "什麼", f64::NAN),
        row("ㄕㄜˊ-ㄇㄛ˙", "什麼", f64::INFINITY),
    ] {
        assert!(build(&a, &[r.clone()]).is_err(), "{r:?}");
    }
    assert!(build(&a, &[row("ㄕㄜˊ-ㄇㄛ˙", "什麼", 1.0), row("ㄕㄜˊ-ㄇㄛ˙", "什麼", 2.0)]).is_err(), "duplicate row");
    // same reading with different words, or the same word with another reading, are fine
    assert!(build(&a, &[row("ㄕㄜˊ-ㄇㄛ˙", "什麼", 1.0), row("ㄕㄜˊ-ㄇㄛ˙", "蛇麼", 2.0), row("ㄕㄜˊ-ㄇㄜ˙", "什麼", 1.0)]).is_ok());
}

#[test]
fn loader_rejects_damaged_files() {
    let a = assets();
    let good = build(&a, &what(6.0)).unwrap();
    assert_eq!(WordBonus::from_bytes(&a, &good).unwrap().len(), 2);
    for n in 0..good.len() {
        assert!(WordBonus::from_bytes(&a, &good[..n]).is_err(), "prefix {n}");
    }
    let mut t = good.clone();
    t.push(0);
    assert!(WordBonus::from_bytes(&a, &t).is_err(), "trailing byte");
    let patch = |off: usize, bytes: &[u8]| {
        let mut v = good.clone();
        v[off..off + bytes.len()].copy_from_slice(bytes);
        v
    };
    assert!(WordBonus::from_bytes(&a, &patch(0, b"X")).is_err(), "magic");
    assert!(WordBonus::from_bytes(&a, &patch(8, &2u32.to_le_bytes())).is_err(), "version");
    assert!(WordBonus::from_bytes(&a, &patch(12, &[0xff])).is_err(), "fingerprint");
    assert!(WordBonus::from_bytes(&a, &patch(20, &3u32.to_le_bytes())).is_err(), "count too large");
    assert!(WordBonus::from_bytes(&a, &patch(20, &1u32.to_le_bytes())).is_err(), "count too small leaves trailing bytes");
    let row_len = 1 + 8 + 8;
    assert!(WordBonus::from_bytes(&a, &patch(24 + 1, &f64::NAN.to_le_bytes())).is_err(), "NaN bonus");
    assert!(WordBonus::from_bytes(&a, &patch(24 + row_len + 1, &f64::INFINITY.to_le_bytes())).is_err(), "inf bonus");
    assert!(WordBonus::from_bytes(&a, &patch(24, &[9])).is_err(), "bad row length");
    // duplicate: the second row copied over the first; out of order: rows swapped
    let (r0, r1) = (good[24..24 + row_len].to_vec(), good[24 + row_len..].to_vec());
    let dup = [&good[..24], &r1[..], &r1[..]].concat();
    assert!(WordBonus::from_bytes(&a, &dup).is_err(), "duplicate");
    let swapped = [&good[..24], &r1[..], &r0[..]].concat();
    assert!(WordBonus::from_bytes(&a, &swapped).is_err(), "order");
    // built for another vocabulary / legal table
    let other = Assets::from_dictionary_text(&format!("{DICT}ㄊㄡˊ 骰 -4\n"), "").unwrap();
    assert!(WordBonus::from_bytes(&other, &good).is_err(), "other assets");
    assert!(WordBonus::open(&a, std::path::Path::new("definitely-missing.wordbonus.bin")).is_err());
}

#[test]
fn hits_use_each_rows_own_bonus_and_any_number_of_rows() {
    let a = assets();
    let t = table(&a, &[row("ㄕㄜˊ-ㄇㄛ˙", "什麼", 7.5), row("ㄕㄜˊ-ㄊㄡˊ", "舌頭", 3.0), row("ㄕㄜˊ-ㄇㄛ˙-ㄕˋ", "什麼是", 1.25)]);
    assert_eq!(t.len(), 3);
    let codes = a.syl_codes(&syl("ㄕˋ ㄕㄜˊ ㄇㄛ˙ ㄕˋ ㄕㄜˊ ㄊㄡˊ"));
    let mut h = t.hits(&codes);
    h.sort_by(|x, y| (x.0, x.1.len()).cmp(&(y.0, y.1.len())));
    let id = |s: &str| s.chars().map(|c| a.vocab_id(c)).collect::<Vec<_>>();
    assert_eq!(h, vec![(1, id("什麼"), 7.5), (1, id("什麼是"), 1.25), (4, id("舌頭"), 3.0)]);
    assert!(t.hits(&a.syl_codes(&syl("ㄕㄜˊ ㄕㄜˊ ㄇㄛ˙"))).iter().all(|x| x.0 == 1));
    assert!(t.hits(&[]).is_empty());
}

#[test]
fn a_large_enough_bonus_turns_snake_into_what_and_only_there() {
    let mut a = assets();
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙", "蛇麼", &[])), "蛇麼");
    a.set_wordbonus(Some(table(&a, &what(30.0))));
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙", "蛇麼", &[])), "什麼");
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄜ˙", "蛇麼", &[])), "什麼", "the ㄇㄜ˙ row");
    assert_eq!(text(&conv(&a, "ㄕˋ ㄕㄜˊ ㄇㄛ˙", "是蛇麼", &[])), "是什麼");
    // negatives: real 蛇 / 舌 / 蛇頭 and a different second syllable are not touched
    assert_eq!(text(&conv(&a, "ㄕㄜˊ", "蛇", &[])), "蛇");
    assert_eq!(text(&conv(&a, "ㄕㄜˊ", "舌", &[])), "舌");
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄊㄡˊ", "蛇頭", &[])), "蛇頭");
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄕˋ", "蛇是", &[])), "蛇是");
    assert_eq!(text(&conv(&a, "ㄇㄛ˙ ㄕㄜˊ", "麼蛇", &[])), "麼蛇");
    // a small bonus does not beat a confident model; the bonus is per row
    a.set_wordbonus(Some(table(&a, &what(5.0))));
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙", "蛇麼", &[])), "蛇麼");
    a.set_wordbonus(Some(table(&a, &[row("ㄕㄜˊ-ㄇㄛ˙", "什麼", 5.0), row("ㄕㄜˊ-ㄊㄡˊ", "舌頭", 100.0)])));
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙", "蛇麼", &[])), "蛇麼");
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄊㄡˊ", "蛇頭", &[])), "舌頭");
}

#[test]
fn locks_and_forced_user_words_win_over_the_bonus() {
    let mut a = assets();
    a.set_wordbonus(Some(table(&a, &what(30.0))));
    // a lock that contradicts the word drops the jump
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙", "蛇麼", &[Some('蛇'), None])), "蛇麼");
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙", "蛇麼", &[None, Some('嘛')])), "蛇嘛");
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙", "蛇麼", &[Some('什'), None])), "什麼");
    // a forced user word of >= 3 characters (full reading typed) stays first, also where the table word would overlap it
    let w = |x: &str, r: &str| a.user_word_opt(x, &syl(r), None).unwrap();
    a.set_user(Some(Arc::new(a.build_user_lexicon([w("蛇麼是", "ㄕㄜˊ ㄇㄛ˙ ㄕˋ")]))));
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙ ㄕˋ", "什麼是", &[])), "蛇麼是");
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙ ㄕˋ", "蛇麼事", &[])), "蛇麼是");
    a.set_user(Some(Arc::new(a.build_user_lexicon([w("麼是事", "ㄇㄛ˙ ㄕˋ ㄕˋ")]))));
    assert_eq!(text(&conv(&a, "ㄕㄜˊ ㄇㄛ˙ ㄕˋ ㄕˋ", "蛇麼是事", &[])), "蛇麼是事", "the jump that enters the forced span is never used");
}

#[test]
fn empty_or_unmatched_table_is_bit_for_bit_the_old_output() {
    let base = assets();
    let inputs = [("ㄕㄜˊ ㄇㄛ˙", "蛇麼"), ("ㄕㄜˊ ㄇㄜ˙", "蛇麼"), ("ㄕㄜˊ ㄊㄡˊ", "蛇頭"), ("ㄕˋ ㄕㄜˊ", "是蛇"), ("ㄕㄜˊ", "蛇")];
    let jumps = |a: &Assets, c: &Conversion| {
        let lp = crate::pn::log_softmax(&c.inf.scores);
        let n = c.inf.syllables.len();
        a.merged_jumps(&c.inf.syllables, &c.inf.sets, &lp, &vec![None; n], &vec![false; n], None)
    };
    let off: Vec<_> = inputs.iter().map(|(s, p)| { let c = conv(&base, s, p, &[]); (text(&c), jumps(&base, &c)) }).collect();
    // empty table
    let mut a = assets();
    a.set_wordbonus(Some(table(&a, &[])));
    assert!(a.wordbonus().is_none() && a.wordbonus.is_none());
    let got: Vec<_> = inputs.iter().map(|(s, p)| { let c = conv(&a, s, p, &[]); (text(&c), jumps(&a, &c)) }).collect();
    assert_eq!(got, off);
    // a table none of whose readings occurs: the same outputs and the same jump lists (the beam path is taken, results equal)
    a.set_wordbonus(Some(table(&a, &[row("ㄕㄜˊ-ㄕˋ", "什事", 50.0)])));
    for ((s, p), (t, j)) in inputs.iter().zip(&off) {
        let c = conv(&a, s, p, &[]);
        if *s != "ㄕㄜˊ ㄕˋ" { assert_eq!((&text(&c), &jumps(&a, &c)), (t, j)); }
    }
}

#[test]
fn bonus_jump_is_in_the_merged_list_with_its_row_bonus_and_pn_duplicates_keep_the_larger() {
    let mut a = assets();
    a.set_wordbonus(Some(table(&a, &what(6.0))));
    let c = conv(&a, "ㄕˋ ㄕㄜˊ ㄇㄛ˙", "是蛇麼", &[]);
    let lp = crate::pn::log_softmax(&c.inf.scores);
    let l = a.merged_jumps(&c.inf.syllables, &c.inf.sets, &lp, &[None; 3], &[false; 3], None);
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].start, l[0].ids.clone(), l[0].bonus), (1, vec![a.vocab_id('什'), a.vocab_id('麼')], 6.0));
    // a forced span blocks it
    let l = a.merged_jumps(&c.inf.syllables, &c.inf.sets, &lp, &[None; 3], &[false, true, false], None);
    assert!(l.is_empty());
}

#[test]
fn loader_rejects_rows_with_unknown_or_illegal_content_even_with_a_good_fingerprint() {
    let a = assets();
    let good = build(&a, &[row("ㄕㄜˊ-ㄇㄛ˙", "什麼", 6.0)]).unwrap();
    assert!(WordBonus::from_bytes(&a, &good).is_ok());
    // layout of the only row: len u8 @24, bonus f64 @25..33, codes @33..37, ids @37..41
    let patch = |off: usize, v: u16| {
        let mut b = good.clone();
        b[off..off + 2].copy_from_slice(&v.to_le_bytes());
        b
    };
    assert!(WordBonus::from_bytes(&a, &patch(33, 0xffff)).is_err(), "syllable code out of range");
    assert!(WordBonus::from_bytes(&a, &patch(33, a.vocab.syllable_id("ㄊㄡˊ") as u16)).is_err(), "legal syllable but 什 is not legal for it");
    assert!(WordBonus::from_bytes(&a, &patch(37, a.vocab_id('頭') as u16)).is_err(), "character not legal for its syllable");
    assert!(WordBonus::from_bytes(&a, &patch(37, 0xffff)).is_err(), "character id out of range");
}

#[test]
fn pn_duplicate_keeps_the_larger_bonus_and_h_does_not_cut_the_table() {
    let mut a = assets();
    let base = std::env::temp_dir().join(format!("twime-wb-pn-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let lex = base.join("lex");
    let words = [("什麼", "ㄕㄜˊ-ㄇㄛ˙"), ("蛇麼", "ㄕㄜˊ-ㄇㄛ˙"), ("舌麼", "ㄕㄜˊ-ㄇㄛ˙")]
        .map(|(w, r)| crate::pn::BuildWord { word: w.into(), reading: r.into(), class: 0 });
    crate::pn::build(&a, &words, &lex).unwrap();
    a.set_pn(Some(Arc::new(crate::pn::PnLexicon::open(&lex, &a).unwrap())), crate::pn::PnCfg { b2: Some(9.0), b3: Some(9.0), h: Some(1), cands: true });
    let c = conv(&a, "ㄕㄜˊ ㄇㄛ˙", "蛇麼", &[]);
    let lp = crate::pn::log_softmax(&c.inf.scores);
    let jumps = |a: &Assets| a.merged_jumps(&c.inf.syllables, &c.inf.sets, &lp, &[None; 2], &[false; 2], a.pn().map(|p| &p.1));
    let what_ids = vec![a.vocab_id('什'), a.vocab_id('麼')];
    let bonus_of = |l: &[crate::pn::Jump]| l.iter().filter(|j| j.ids == what_ids).map(|j| j.bonus).collect::<Vec<_>>();
    let pn_only = jumps(&a);
    assert_eq!(pn_only.len(), 1, "H = 1 keeps one PN word of this reading");
    // 什麼 is not the PN hit that H keeps (the model prefers 蛇麼), so the table adds it with its own bonus
    a.set_wordbonus(Some(table(&a, &what(30.0))));
    let l = jumps(&a);
    assert_eq!(bonus_of(&l), vec![30.0]);
    assert_eq!(l.len(), 2, "the PN jump kept by H and the table jump");
    // when PN carries the same (start, ids) with a smaller / larger bonus, the larger one stays, one entry only
    for (table_bonus, want) in [(3.0, 9.0), (30.0, 30.0)] {
        a.set_pn(a.pn().map(|p| p.0.clone()), crate::pn::PnCfg { b2: Some(9.0), b3: Some(9.0), h: None, cands: true });
        a.set_wordbonus(Some(table(&a, &what(table_bonus))));
        assert_eq!(bonus_of(&jumps(&a)), vec![want], "table {table_bonus}");
    }
    let _ = std::fs::remove_dir_all(&base);
}
