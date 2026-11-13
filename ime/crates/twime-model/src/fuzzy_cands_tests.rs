use super::*;
use std::sync::Arc;

const DICT: &str = "\
ㄉㄤˇ 檔 -3
ㄉㄤˇ 黨 -2.5
ㄉㄢˇ 膽 -3
ㄢˋ 案 -3
ㄢˋ 岸 -3.2
ㄢ 安 -2.5
ㄉㄤˇ-ㄢˋ 檔案 -4
ㄉㄤˇ-ㄢ 黨安 -3
ㄕˊ 時 -2
ㄕˊ 實 -2.5
ㄐㄧㄢ 間 -2.5
ㄐㄧㄢˋ 踐 -4
ㄕˊ-ㄐㄧㄢ 時間 -3
ㄕˊ-ㄐㄧㄢˋ 實踐 -4.5
";

fn syl(s: &str) -> Vec<String> {
    s.split(' ').map(str::to_string).collect()
}

fn alts(n: usize, v: &[(usize, &str, u8)]) -> Vec<Vec<(String, u8)>> {
    let mut a = vec![vec![]; n];
    for (j, g, c) in v {
        a[*j].push((g.to_string(), *c));
    }
    a
}

fn texts(h: &[FuzzyHit]) -> Vec<&str> {
    h.iter().map(|x| x.text.as_str()).collect()
}

#[test]
fn one_change_finds_the_word_and_marks_it_strong_without_base_word() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let s = syl("ㄉㄢˇ ㄢˋ");
    let h = a.fuzzy_words(&s, 2, 0, &alts(2, &[(0, "ㄉㄤˇ", 2)]), &FuzzyParams::default());
    assert_eq!(texts(&h), ["檔案"]);
    let x = &h[0];
    assert_eq!((x.span, x.at, x.syl.as_str(), x.cost, x.source), (2, 0, "ㄉㄤˇ", 2, Source::Dict));
    assert!(x.strong && (x.score - (-4.0 - 1.0)).abs() < 1e-9);
}

#[test]
fn exactly_one_position_changes() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    // 黨安 needs ㄉㄤˇ and ㄢ: two changes, never listed
    let h = a.fuzzy_words(&syl("ㄉㄢˇ ㄢˋ"), 2, 0, &alts(2, &[(0, "ㄉㄤˇ", 2), (1, "ㄢ", 3)]), &FuzzyParams::default());
    assert_eq!(texts(&h), ["檔案"]);
    // an alternative equal to the syllable itself is ignored; no alternatives = nothing
    assert!(a.fuzzy_words(&syl("ㄉㄤˇ ㄢˋ"), 2, 0, &alts(2, &[(0, "ㄉㄤˇ", 2)]), &FuzzyParams::default()).is_empty());
    assert!(a.fuzzy_words(&syl("ㄉㄢˇ ㄢˋ"), 2, 0, &alts(2, &[]), &FuzzyParams::default()).is_empty());
}

#[test]
fn strong_only_above_the_best_word_of_the_typed_reading() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    // typed 時間 (-3); 實踐 one tone away: -4.5 - 1.5 < -3, listed but not strong
    let h = a.fuzzy_words(&syl("ㄕˊ ㄐㄧㄢ"), 2, 0, &alts(2, &[(1, "ㄐㄧㄢˋ", 3)]), &FuzzyParams::default());
    assert_eq!(texts(&h), ["實踐"]);
    assert!(!h[0].strong);
    // with no penalty and a weaker typed word it would be strong: penalty matters
    let h = a.fuzzy_words(&syl("ㄕˊ ㄐㄧㄢ"), 2, 0, &alts(2, &[(1, "ㄐㄧㄢˋ", 3)]), &FuzzyParams { penalty: -1.0, max_len: 6 });
    assert!(h[0].strong);
}

#[test]
fn span_limits_first_and_max_len() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let s = syl("ㄕˊ ㄉㄢˇ ㄢˋ");
    let al = alts(3, &[(1, "ㄉㄤˇ", 2)]);
    assert_eq!(texts(&a.fuzzy_words(&s, 3, 0, &al, &FuzzyParams::default())), ["檔案"]);
    // the run of characters starts after the changed position: nothing
    assert!(a.fuzzy_words(&s, 3, 2, &al, &FuzzyParams::default()).is_empty());
    // spans shorter than 2 are never produced
    assert!(a.fuzzy_words(&s, 3, 0, &al, &FuzzyParams { penalty: 0.5, max_len: 1 }).is_empty());
    // the cursor before the word: nothing ends there
    assert!(a.fuzzy_words(&s, 2, 0, &al, &FuzzyParams::default()).is_empty());
}

#[test]
fn illegal_character_for_the_changed_reading_is_dropped() {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    let id = a.vocab.char_id('檔');
    a.legal.get_mut("ㄉㄤˇ").unwrap().retain(|&x| x != id);
    assert!(a.fuzzy_words(&syl("ㄉㄢˇ ㄢˋ"), 2, 0, &alts(2, &[(0, "ㄉㄤˇ", 2)]), &FuzzyParams::default()).is_empty());
}

#[test]
fn user_words_come_first_and_are_strong_and_dedup_keeps_one_item() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let w = |t: &str, s: &str| a.user_word(t, &syl(s), 30.0).unwrap();
    a.set_user(Some(Arc::new(a.build_user_lexicon([w("黨案", "ㄉㄤˇ ㄢˋ"), w("檔案", "ㄉㄤˇ ㄢˋ")]))));
    let h = a.fuzzy_words(&syl("ㄉㄢˇ ㄢˋ"), 2, 0, &alts(2, &[(0, "ㄉㄤˇ", 2)]), &FuzzyParams::default());
    // 檔案 is a user word and a data.txt word: one item, the user one (data.txt log10)
    assert_eq!(texts(&h), ["檔案", "黨案"]);
    assert!(h.iter().all(|x| x.source == Source::User && x.strong));
    assert!((h[0].log10 + 4.0).abs() < 1e-9);
}

#[test]
fn user_word_outside_data_txt_scores_at_the_length_floor_without_pn_setup() {
    let a = Assets::from_dictionary_text(DICT, "").unwrap();
    let w = a.user_word("黨案", &syl("ㄉㄤˇ ㄢˋ"), 30.0).unwrap();
    a.set_user(Some(Arc::new(a.build_user_lexicon([w]))));
    let h = a.fuzzy_words(&syl("ㄉㄢˇ ㄢˋ"), 2, 0, &alts(2, &[(0, "ㄉㄤˇ", 2)]), &FuzzyParams::default());
    let x = h.iter().find(|x| x.text == "黨案").unwrap();
    // the lowest data.txt log10 of a 2-character word in DICT (實踐 -4.5)
    assert!((x.log10 + 4.5).abs() < 1e-9, "{}", x.log10);
}

#[test]
fn order_is_user_score_span_text() {
    let mk = |t: &str, span, score, source| FuzzyHit { text: t.into(), span, at: 0, syl: String::new(), cost: 2, log10: score, source, score, strong: false };
    let mut v = vec![
        mk("乙乙", 2, -3.0, Source::Dict),
        mk("甲甲甲", 3, -3.0, Source::Dict),
        mk("丙丙", 2, -9.0, Source::User),
        mk("甲甲", 2, -2.0, Source::Pn),
        mk("乙乙", 2, -3.0, Source::Pn),
    ];
    order(&mut v);
    let t: Vec<(&str, Source)> = v.iter().map(|x| (x.text.as_str(), x.source)).collect();
    assert_eq!(t, [("丙丙", Source::User), ("甲甲", Source::Pn), ("甲甲甲", Source::Dict), ("乙乙", Source::Dict)]);
    // the same text with another changed reading is another item
    let mut w = vec![mk("乙乙", 2, -3.0, Source::Dict), FuzzyHit { syl: "ㄅ".into(), ..mk("乙乙", 2, -3.5, Source::Dict) }];
    order(&mut w);
    assert_eq!(w.len(), 2);
}
