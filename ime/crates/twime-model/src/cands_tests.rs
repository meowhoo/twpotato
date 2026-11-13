use super::*;
use crate::n1::N1;

const DICT: &str = "\
ㄑㄩㄢˊ 全 -3
ㄑㄩㄢˊ 權 -3.5
ㄑㄩㄢˊ 泉 -4
ㄌㄧˋ 力 -2.5
ㄌㄧˋ 利 -2.6
ㄌㄧˋ 立 -2.7
ㄌㄧˋ 歷 -3
ㄑㄩㄢˊ-ㄌㄧˋ 權力 -4.13
ㄑㄩㄢˊ-ㄌㄧˋ 權利 -4.134
ㄑㄩㄢˊ-ㄌㄧˋ 全力 -4.52
ㄧˇ 以 -2
ㄧˇ 已 -2.2
ㄑㄩㄢˊ-ㄌㄧˋ-ㄧˇ 權力以 -9
ㄌㄧˋ-ㄧˇ 力以 -9
";

fn syl(s: &str) -> Vec<String> {
    s.split(' ').map(str::to_string).collect()
}

fn texts(c: &[Candidate]) -> Vec<String> {
    c.iter().map(|x| x.text.clone()).collect()
}

fn with_random_model() -> Assets {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    let v = a.vocab.len();
    a.model = Some(N1::random(v, 32, 1, 11));
    a
}

#[test]
fn fallback_order_current_first_words_before_singles() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
    let conv = a.convert("", &s, &[], 0.0);
    assert!(conv.fallback);
    // The fallback picks the best single characters 全 + 力.
    assert_eq!(conv.text().unwrap(), "全力");
    let c = a.candidates(&conv, 2, &CandParams::default());
    // The current item is the word 全力 (the shortest multi-character word equal to the sentence).
    assert!(c[0].current && c[0].text == "全力" && c[0].span == 2);
    let t = texts(&c);
    assert_eq!(&t[..3], &["全力", "權力", "權利"]);
    assert!(t.contains(&"利".to_string()) && t.contains(&"立".to_string()));
    // 力 is the current single: replaced by the current word, not repeated.
    assert!(!t.contains(&"力".to_string()));
    // Words come before singles.
    let first_single = t.iter().position(|x| x.chars().count() == 1).unwrap();
    assert!(t[..first_single].iter().all(|x| x.chars().count() >= 2));
}

#[test]
fn single_hidden_when_a_shown_word_has_the_same_effect() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
    // Lock 權 so the sentence is 權力; the word 權利 and the single 利 change the same last character.
    let conv = a.convert("", &s, &[Some('權'), None], 0.0);
    assert_eq!(conv.text().unwrap(), "權力");
    let t = texts(&a.candidates(&conv, 2, &CandParams::default()));
    assert_eq!(t[0], "權力");
    assert!(t.contains(&"權利".to_string()));
    assert!(!t.contains(&"利".to_string()), "利 has the same effect as the shown word 權利: {t:?}");
    assert!(t.contains(&"立".to_string()), "no word covers 立, it stays: {t:?}");
    // Reachability: each legal character of the last syllable is a single or the end of a shown word whose first
    // character is the current one.
    for c in ["力", "利", "立", "歷"] {
        let ok = t.iter().any(|x| x == c) || t.iter().any(|x| x.chars().count() >= 2 && x.ends_with(c) && x.starts_with('權'));
        assert!(ok, "{c} unreachable in {t:?}");
    }
}

#[test]
fn spans_stop_at_syllables_without_characters_and_respect_limits() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ ㄧˇ");
    let conv = a.convert("", &s, &[], 0.0);
    let all = texts(&a.candidates(&conv, 3, &CandParams::default()));
    assert!(all.contains(&"權力以".to_string()) && all.contains(&"力以".to_string()));
    let two = texts(&a.candidates(&conv, 3, &CandParams { max_span: 2, ..CandParams::default() }));
    assert!(!two.contains(&"權力以".to_string()) && two.contains(&"力以".to_string()));
    let one = texts(&a.candidates(&conv, 3, &CandParams { max_span: 1, ..CandParams::default() }));
    assert!(one.iter().all(|x| x.chars().count() == 1));
    // A syllable with no legal character in the middle stops the span; single candidates still work.
    let s2 = syl("ㄑㄩㄢˊ ㄅㄅㄅ ㄧˇ");
    let conv2 = a.convert("", &s2, &[], 0.0);
    let t2 = texts(&a.candidates(&conv2, 3, &CandParams::default()));
    assert!(!t2.is_empty() && t2.iter().all(|x| x.chars().count() == 1));
    assert!(a.candidates(&conv2, 2, &CandParams::default()).is_empty());
    assert!(a.candidates(&conv, 0, &CandParams::default()).is_empty());
    assert!(a.candidates(&conv, 4, &CandParams::default()).is_empty());
    // word_cap = 0 leaves only the current item among the words.
    let capped = texts(&a.candidates(&conv, 2, &CandParams { word_cap: 0, ..CandParams::default() }));
    assert!(capped.iter().filter(|x| x.chars().count() >= 2).count() <= 1);
}

#[test]
fn model_candidates_are_unique_current_first_and_sorted() {
    let a = with_random_model();
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ ㄧˇ ㄑㄩㄢˊ ㄌㄧˋ");
    for lambda in [0.0, 0.5] {
        let conv = a.convert("", &s, &[], lambda);
        assert!(!conv.fallback);
        for end in 1..=s.len() {
            let c = a.candidates(&conv, end, &CandParams { lambda, beta: 0.3, ..CandParams::default() });
            assert!(c[0].current, "end {end}: {c:?}");
            let mut seen = std::collections::HashSet::new();
            assert!(c.iter().all(|x| seen.insert((x.span, x.text.clone()))));
            let cur: String = conv.chars[end - c[0].span..end].iter().map(|c| c.unwrap()).collect();
            assert_eq!(c[0].text, cur);
            assert!(c[1..].windows(2).all(|w| w[0].score >= w[1].score));
        }
    }
}

#[test]
fn beta_penalizes_rarer_homophone_words_only() {
    let a = with_random_model();
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
    let conv = a.convert("", &s, &[], 0.0);
    let find = |c: &[Candidate], t: &str| c.iter().find(|x| x.text == t).map(|x| x.score);
    let c0 = a.candidates(&conv, 2, &CandParams { lambda: 0.0, beta: 0.0, ..CandParams::default() });
    let c1 = a.candidates(&conv, 2, &CandParams { lambda: 0.0, beta: 1.0, ..CandParams::default() });
    // 權力 is the most common word of the reading (R = 0); 全力 is rarer by ln(10) * 0.39.
    let (p0, p1) = (find(&c0, "權力"), find(&c1, "權力"));
    if let (Some(p0), Some(p1)) = (p0, p1) {
        assert!((p0 - p1).abs() < 1e-12);
    }
    let (q0, q1) = (find(&c0, "全力"), find(&c1, "全力"));
    if let (Some(q0), Some(q1)) = (q0, q1) {
        assert!((q0 - q1 - std::f64::consts::LN_10 * (4.52 - 4.13)).abs() < 1e-9, "{q0} {q1}");
    }
    // Single characters are never penalized.
    assert_eq!(find(&c0, "立"), find(&c1, "立"));
}

#[test]
fn rebeam_equals_convert_for_any_locks() {
    let a = with_random_model();
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ ㄧˇ ㄑㄩㄢˊ ㄅㄅㄅ ㄌㄧˋ");
    let base = a.convert("今天", &s, &[], 0.0);
    let chars = ['權', '力', '以', '全', '利', '立', '泉', '歷', '甲'];
    let mut seed = 5u32;
    let mut next = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as usize
    };
    for _ in 0..200 {
        let locks: Vec<Option<char>> = (0..s.len()).map(|_| (next() % 3 == 0).then(|| chars[next() % chars.len()])).collect();
        for lambda in [0.0, 0.5] {
            let direct = a.convert("今天", &s, &locks, lambda);
            let re = a.rebeam(&base, &locks, lambda);
            assert_eq!(direct.chars, re.chars, "locks {locks:?}");
        }
    }
    // Fallback mode: unlocking returns to the unlocked first choice, not to the previously locked character.
    let fb = Assets::from_dictionary_text(DICT, "").unwrap();
    let s2 = syl("ㄑㄩㄢˊ ㄌㄧˋ");
    let c0 = fb.convert("", &s2, &[], 0.0);
    let c1 = fb.rebeam(&c0, &[Some('泉'), Some('歷')], 0.0);
    assert_eq!(c1.text().unwrap(), "泉歷");
    let c2 = fb.rebeam(&c1, &[None, None], 0.0);
    assert_eq!(c2.chars, c0.chars);
}

struct Fake;
impl WordSource for Fake {
    fn words(&self, syllables: &[String]) -> Vec<WordHit> {
        if syllables.join("-") == "ㄑㄩㄢˊ-ㄌㄧˋ" { vec![WordHit { word: "泉立".into(), log10: -3.0 }] } else { vec![] }
    }
}

#[test]
fn word_source_adds_words() {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    a.set_word_source(Some(std::sync::Arc::new(Fake)));
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
    let conv = a.convert("", &s, &[], 0.0);
    let t = texts(&a.candidates(&conv, 2, &CandParams::default()));
    // Fallback order is by log10: the extra word (-3.0) is above the dictionary words (about -4.1).
    assert_eq!(&t[..2], &["全力", "泉立"], "{t:?}");
}

/// Plan 2.4 invariant: every legal character of the last syllable can be reached, as its own item or as the end of a
/// shown word whose earlier characters are the ones already in the sentence (so the word has the same effect).
#[test]
fn every_legal_character_stays_reachable() {
    let a = with_random_model();
    let sentences = ["ㄑㄩㄢˊ ㄌㄧˋ", "ㄑㄩㄢˊ ㄌㄧˋ ㄧˇ", "ㄧˇ ㄑㄩㄢˊ ㄌㄧˋ", "ㄌㄧˋ ㄧˇ ㄑㄩㄢˊ ㄌㄧˋ", "ㄑㄩㄢˊ ㄌㄧˋ ㄧˇ ㄑㄩㄢˊ ㄌㄧˋ ㄧˇ"];
    let legal = |s: &str| -> Vec<char> {
        let mut v: Vec<char> = DICT.lines().filter_map(|l| {
            let mut p = l.split(' ');
            let (r, w) = (p.next()?, p.next()?);
            (r == s && w.chars().count() == 1).then(|| w.chars().next().unwrap())
        }).collect();
        v.sort();
        v
    };
    for s in sentences {
        let syl = syl(s);
        for lambda in [0.0, 0.5] {
            for locked in [false, true] {
                let locks: Vec<Option<char>> = syl.iter().enumerate().map(|(i, _)| (locked && i % 2 == 0).then(|| a.convert("", &syl, &[], lambda).chars[i]).flatten()).collect();
                let conv = a.convert("", &syl, &locks, lambda);
                for cap in [0usize, 1, 40] {
                    for end in 1..=syl.len() {
                        let shown = texts(&a.candidates(&conv, end, &CandParams { lambda, word_cap: cap, ..CandParams::default() }));
                        for c in legal(&syl[end - 1]) {
                            let single = shown.iter().any(|x| x.chars().eq([c]));
                            // A shown word ending in c whose earlier characters equal the sentence's.
                            let by_word = shown.iter().any(|x| {
                                let w: Vec<char> = x.chars().collect();
                                w.len() >= 2 && w.len() <= end && w[w.len() - 1] == c && w[..w.len() - 1].iter().enumerate().all(|(k, ch)| conv.chars[end - w.len() + k] == Some(*ch))
                            });
                            let is_current = conv.chars[end - 1] == Some(c);
                            let current_shown = shown.first().is_some_and(|x| x.chars().last() == Some(c));
                            assert!(single || by_word || (is_current && current_shown), "{c} unreachable at end {end} in {shown:?} ({s}, lambda {lambda}, locked {locked}, cap {cap})");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn word_cap_counts_the_current_word() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let s = syl("ㄑㄩㄢˊ ㄌㄧˋ");
    let conv = a.convert("", &s, &[], 0.0); // 全力 is the current word
    let count = |cap| texts(&a.candidates(&conv, 2, &CandParams { word_cap: cap, ..CandParams::default() })).iter().filter(|x| x.chars().count() >= 2).count();
    assert_eq!(count(0), 1); // the current item is always shown
    assert_eq!(count(1), 1);
    assert_eq!(count(2), 2);
    assert_eq!(count(40), 3);
}

// ---- NORM: 台/臺 pairs ----

const TDICT: &str = "\
ㄊㄞˊ 臺 -2
ㄊㄞˊ 台 -2.5
ㄊㄞˊ 抬 -3
ㄨㄢ 灣 -2
ㄨㄢ 玩 -3
ㄊㄞˊ-ㄨㄢ 臺灣 -3
ㄊㄞˊ-ㄨㄢ 台灣 -3.5
ㄊㄞˊ-ㄨㄢ 抬玩 -9
ㄇㄧㄥˊ-ㄊㄞˊ 明臺 -5
ㄇㄧㄥˊ 明 -2
";

fn tai_assets(pair: bool, model: bool) -> Assets {
    let mut a = Assets::from_dictionary_text(TDICT, "").unwrap();
    if model {
        let v = a.vocab.len();
        a.model = Some(N1::random(v, 32, 1, 11));
    }
    a.set_norm(crate::norm::NormCfg { out: true, ctx: true, pair });
    a
}

#[test]
fn pairs_sit_next_to_each_other_and_share_the_score() {
    for model in [false, true] {
        for beta in [0.0, 1.0] {
            let a = tai_assets(true, model);
            let conv = a.convert("", &syl("ㄊㄞˊ ㄨㄢ"), &[], 0.0);
            assert_eq!(conv.text().unwrap(), if model { conv.text().unwrap() } else { "台灣".to_string() });
            let c = a.candidates(&conv, 2, &CandParams { beta, ..CandParams::default() });
            let t = texts(&c);
            let (i, j) = (t.iter().position(|x| x == "台灣").unwrap(), t.iter().position(|x| x == "臺灣").unwrap());
            assert_eq!(j, i + 1, "{t:?}");
            assert_eq!(c[i].score, c[j].score);
            // the sentence is the current item and its partner follows it directly
            assert!(c[0].current);
            let ci = t.iter().position(|x| *x == c[0].text).unwrap();
            assert_eq!(ci, 0);
            assert_eq!(t[1].replace('臺', "台"), t[0].replace('臺', "台"));
        }
    }
}

#[test]
fn missing_partner_is_synthesized_and_pairs_off_changes_nothing() {
    let a = tai_assets(true, false);
    let conv = a.convert("", &syl("ㄇㄧㄥˊ ㄊㄞˊ"), &[], 0.0);
    let t = texts(&a.candidates(&conv, 2, &CandParams::default()));
    // 明臺 is in the dictionary only with 臺: the 台 form is added; singles 台/臺 are both listed
    assert!(t.contains(&"明臺".to_string()) && t.contains(&"明台".to_string()), "{t:?}");
    let off = tai_assets(false, false);
    let conv = off.convert("", &syl("ㄇㄧㄥˊ ㄊㄞˊ"), &[], 0.0);
    let t = texts(&off.candidates(&conv, 2, &CandParams::default()));
    assert!(!t.contains(&"明台".to_string()), "{t:?}");
}

#[test]
fn a_pair_counts_once_for_the_word_cap_and_style_picks_the_first_glyph() {
    let a = tai_assets(true, false);
    let conv = a.convert("", &syl("ㄊㄞˊ ㄨㄢ"), &[], 0.0);
    // cap 2 words: the current word's pair is free, the next group (抬玩) takes one place
    let t = texts(&a.candidates(&conv, 2, &CandParams { word_cap: 2, ..CandParams::default() }));
    assert!(t.contains(&"台灣".to_string()) && t.contains(&"臺灣".to_string()), "{t:?}");
    // A pair that is not the current item: the preferred glyph of the style comes first, the partner right after it.
    let conv = a.convert("", &syl("ㄊㄞˊ ㄨㄢ"), &[Some('抬'), None], 0.0);
    for (style, first) in [(false, "台灣"), (true, "臺灣")] {
        let t = texts(&a.candidates(&conv, 2, &CandParams { style_tai: style, ..CandParams::default() }));
        let i = t.iter().position(|x| x == first).unwrap();
        let other = if style { "台灣" } else { "臺灣" };
        assert_eq!(t[i + 1], other, "{t:?}");
    }
}

#[test]
fn two_pairs_with_equal_scores_do_not_interleave() {
    let dict = "ㄊㄞˊ 抬 -3\nㄊㄞˊ 臺 -2\nㄊㄞˊ 台 -2.5\nㄨㄢ 灣 -2\nㄨㄢ 玩 -2\nㄊㄞˊ-ㄨㄢ 臺灣 -3\nㄊㄞˊ-ㄨㄢ 台灣 -3.5\nㄊㄞˊ-ㄨㄢ 臺玩 -3\nㄊㄞˊ-ㄨㄢ 台玩 -3.2\n";
    let mut a = Assets::from_dictionary_text(dict, "").unwrap();
    a.set_norm(crate::norm::NormCfg { out: true, ctx: true, pair: true });
    let conv = a.convert("", &syl("ㄊㄞˊ ㄨㄢ"), &[Some('抬'), None], 0.0);
    for style in [false, true] {
        let c = a.candidates(&conv, 2, &CandParams { style_tai: style, ..CandParams::default() });
        let words: Vec<(String, f64)> = c.iter().filter(|x| x.span == 2).map(|x| (x.text.clone(), x.score)).collect();
        assert_eq!(words.len(), 4, "{words:?}");
        assert_eq!(words[0].1, words[1].1);
        assert_eq!(words[2].1, words[3].1);
        assert_eq!(words[0].0.chars().nth(1), words[1].0.chars().nth(1), "{words:?}"); // same second character = same group
        assert_eq!(words[2].0.chars().nth(1), words[3].0.chars().nth(1), "{words:?}");
        let (p, q) = (if style { '臺' } else { '台' }, if style { '台' } else { '臺' });
        assert_eq!((words[0].0.chars().next(), words[1].0.chars().next()), (Some(p), Some(q)), "{words:?}");
        assert_eq!((words[2].0.chars().next(), words[3].0.chars().next()), (Some(p), Some(q)), "{words:?}");
    }
}
