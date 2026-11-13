//! Session on the real product weights (TRAIN T-model since 2026-10-22; KE4 before) + prior (skipped when the local data files are absent; they are not in git).

use std::{path::PathBuf, sync::Arc};

use twime_core::{Config, Engine, keymap::key_of};
use twime_model::{Assets, ModelPaths};
use twime_proto::{Key, KeyEvent};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn load() -> Option<Assets> {
    let r = root();
    let p = ModelPaths {
        data_txt: r.join("third_party/mcbopomofo/data.txt"),
        aliases: r.join("engine/data/common-readings.txt"),
        model: r.join("data/train/T-model.twg1"),
        prior: Some(r.join("data/vd/prior/freq.pack")),
    };
    [&p.data_txt, &p.aliases, &p.model, p.prior.as_ref().unwrap()].iter().all(|f| f.exists()).then(|| Assets::load(&p).unwrap())
}

fn type_str(s: &mut twime_core::Session, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { Key::Space } else { key_of(c).unwrap() };
        s.on_key(KeyEvent::press(key));
    }
}

#[test]
fn session_matches_direct_conversion_and_is_repeatable() {
    let Some(assets) = load() else {
        eprintln!("skipped: local model files not present");
        return;
    };
    let assets = Arc::new(assets);
    let engine = Engine::new(assets.clone(), Config { candidates: true, ..Config::default() });
    let syl: Vec<String> = "ㄐㄧㄣ ㄊㄧㄢ ㄊㄧㄢ ㄑㄧˋ ㄏㄣˇ ㄏㄠˇ".split(' ').map(String::from).collect();
    let direct = assets.convert("", &syl, &[], 1.0).text().unwrap();
    assert_eq!(direct, "今天天氣很好");
    let mut outs = vec![];
    for _ in 0..3 {
        let mut s = engine.new_session();
        type_str(&mut s, "ㄐㄧㄣ ㄊㄧㄢ ㄊㄧㄢ ㄑㄧˋㄏㄣˇㄏㄠˇ");
        let u = s.on_key(KeyEvent::press(Key::Enter));
        outs.push(u.commit.unwrap());
        assert_eq!(s.context(), outs[0]);
    }
    assert!(outs.iter().all(|o| *o == direct));
    // Candidates come from the same model: the chosen character is first and all are legal.
    let mut s = engine.new_session();
    type_str(&mut s, "ㄏㄠˇ");
    let u = s.on_key(KeyEvent::press(Key::Down));
    let c = u.candidates.unwrap();
    assert_eq!(c.items[0], "好");
    // the last item is the typed bopomofo (ZHUYIN-OUT)
    assert_eq!(c.items.last().map(String::as_str), Some("ㄏㄠˇ"));
    assert!(c.items[..c.items.len() - 1].iter().all(|x| assets.is_legal_char("ㄏㄠˇ", x.chars().next().unwrap())));
}

/// `rebeam` against a full conversion with the real weights and the full prior (beam with lambda != 0), on the
/// hand-written homophone sentences with random locks.
#[test]
fn rebeam_equals_convert_with_the_real_prior() {
    let Some(assets) = load() else {
        eprintln!("skipped: local model files not present");
        return;
    };
    let text = std::fs::read_to_string(root().join("ime/crates/twime-tools/data/homophone-cases.tsv")).unwrap();
    let mut seed = 99u32;
    let mut next = |m: usize| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((seed >> 8) as usize) % m
    };
    let mut checked = 0;
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let syl: Vec<String> = f[4].split(' ').map(String::from).collect();
        let chars: Vec<char> = f[3].chars().collect();
        for lambda in [0.5, 1.0] {
            let base = assets.convert("", &syl, &[], lambda);
            for _ in 0..20 {
                // Locks drawn from the sentence itself or from the candidate list of the position.
                let locks: Vec<Option<char>> = (0..syl.len())
                    .map(|i| match next(4) {
                        0 => Some(chars[i]),
                        1 => assets.alternatives(&base, &syl, i).get(next(5)).copied(),
                        _ => None,
                    })
                    .collect();
                let direct = assets.convert("", &syl, &locks, lambda);
                let re = assets.rebeam(&base, &locks, lambda);
                assert_eq!(direct.chars, re.chars, "{} locks {locks:?} lambda {lambda}", f[0]);
                checked += 1;
            }
        }
    }
    assert!(checked > 500);
}

/// With word candidates off the list is the new model-based single-character list (N3b decision 2026-10-12), not the old
/// ranking: the first page equals `Assets::candidates` with a span of 1, on the hand-written homophone sentences.
#[test]
fn words_off_lists_single_characters_by_the_new_score() {
    let Some(assets) = load() else {
        eprintln!("skipped: local model files not present");
        return;
    };
    let assets = Arc::new(assets);
    let engine = Engine::new(assets.clone(), Config { word_candidates: false, ..Config::default() });
    let text = std::fs::read_to_string(root().join("ime/crates/twime-tools/data/homophone-cases.tsv")).unwrap();
    let (mut checked, mut differs_from_old) = (0, 0);
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let syl: Vec<String> = f[4].split(' ').map(String::from).collect();
        let mut s = engine.new_session();
        for sy in &syl {
            type_str(&mut s, sy);
            if !sy.ends_with(['ˊ', 'ˇ', 'ˋ', '˙']) {
                type_str(&mut s, " ");
            }
        }
        let u = s.on_key(KeyEvent::press(Key::Down));
        let mut got = u.candidates.expect("window").items;
        if got.last() == syl.last() {
            got.pop(); // the typed bopomofo (ZHUYIN-OUT)
        }
        let conv = assets.convert("", &syl, &[], 0.5);
        let p = twime_model::CandParams { lambda: 0.5, beta: 0.0, max_span: 1, word_cap: 40, style_tai: false };
        let want: Vec<String> = assets.candidates(&conv, syl.len(), &p).into_iter().take(9).map(|c| c.text).collect();
        assert_eq!(got, want, "{}", f[0]);
        let old: Vec<String> = assets.alternatives(&conv, &syl, syl.len() - 1).iter().take(9).map(|c| c.to_string()).collect();
        differs_from_old += usize::from(old != got);
        checked += 1;
    }
    assert!(checked >= 10);
    println!("{checked} sentences, {differs_from_old} differ from the old ranking");
}

/// NORM A-5: with the pairs on, every candidate (single or word, current item included) that has 台/臺 at a ㄊㄞˊ position
/// has its other form in the list too (and next to it, apart from the current item), for 30 hand-written readings.
#[test]
fn tai_pairs_are_listed_together_with_the_real_dictionary() {
    let Some(mut assets) = load() else {
        eprintln!("skipped: local model files not present");
        return;
    };
    assets.set_norm(twime_model::NormCfg::ALL);
    let cases = [
        "ㄊㄞˊ ㄨㄢ", "ㄊㄞˊ ㄅㄟˇ", "ㄊㄞˊ ㄓㄨㄥ", "ㄊㄞˊ ㄋㄢˊ", "ㄨˇ ㄊㄞˊ", "ㄊㄞˊ ㄕˋ", "ㄊㄞˊ ㄕㄤˋ", "ㄊㄞˊ ㄒㄧㄚˋ",
        "ㄆㄧㄥˊ ㄊㄞˊ", "ㄧㄩㄝˋ ㄊㄞˊ", "ㄉㄧㄢˋ ㄕˋ ㄊㄞˊ", "ㄊㄞˊ ㄉㄨㄥ", "ㄊㄞˊ ㄧㄣˊ ㄕㄨ", "ㄊㄞˊ ㄅㄟˇ ㄕˋ", "ㄍㄨㄛˊ ㄌㄧˋ ㄊㄞˊ ㄨㄢ ㄉㄚˋ ㄒㄩㄝˊ",
        "ㄊㄞˊ ㄨㄢ ㄧㄣˊ ㄏㄤˊ", "ㄊㄞˊ ㄓㄨㄥ ㄕˋ", "ㄊㄞˊ ㄍㄨㄢˇ", "ㄊㄞˊ ㄏㄞˇ", "ㄊㄞˊ ㄐㄧㄝ", "ㄍㄠ ㄊㄞˊ", "ㄊㄞˊ ㄓㄨㄛ", "ㄒㄧㄣ ㄊㄞˊ ㄅㄧˋ",
        "ㄊㄞˊ ㄇㄧㄣˊ", "ㄊㄞˊ ㄅㄟˇ ㄕˋ ㄓㄥˋ ㄈㄨˇ", "ㄊㄞˊ ㄨㄢ ㄕㄥˇ", "ㄧˋ ㄊㄞˊ", "ㄊㄞˊ ㄊㄞˊ", "ㄊㄞˊ ㄏㄨㄚˋ", "ㄏㄨㄚˋ ㄊㄞˊ", "ㄊㄞˊ ㄈㄥㄥ",
    ];
    assert!(cases.len() >= 30);
    let mut checked = 0;
    for style in [false, true] {
        for c in cases.iter().chain(&["ㄊㄞˊ ㄍㄨㄢˇ ㄏㄨㄟˋ"]) {
            let syl: Vec<String> = c.split(' ').map(String::from).collect();
            if syl.iter().any(|s| !assets.is_legal_syllable(s)) {
                continue;
            }
            let conv = assets.convert_ex("", &syl, &[], 0.5, style);
            for end in 1..=syl.len() {
                let p = twime_model::CandParams { lambda: 0.5, beta: 0.0, max_span: 6, word_cap: 40, style_tai: style };
                let list: Vec<String> = assets.candidates(&conv, end, &p).into_iter().map(|c| c.text).collect();
                for (i, t) in list.iter().enumerate() {
                    let cs: Vec<char> = t.chars().collect();
                    let first = end - cs.len();
                    let swap = |to: char| -> String {
                        cs.iter().enumerate().map(|(k, &ch)| if (ch == '台' || ch == '臺') && assets.is_legal_char(&syl[first + k], to) { to } else { ch }).collect()
                    };
                    for to in ['台', '臺'] {
                        let w = swap(to);
                        // a form equal to the sentence is the "current" item (shown as the shortest equal word), not listed again
                        let same_as_sentence = w.chars().enumerate().all(|(k, ch)| conv.chars[first + k] == Some(ch));
                        if w != *t && !same_as_sentence {
                            assert!(list.contains(&w), "{c} end {end}: {t} lacks {w} in {list:?}");
                            let j = list.iter().position(|x| *x == w).unwrap();
                            checked += 1;
                            // the partner of the item sits within the same neighbourhood (pairs are adjacent unless it is the current item's mate)
                            assert_eq!(i.abs_diff(j), 1, "{c} {t} {w} {i} {j}");
                        }
                    }
                }
            }
        }
    }
    println!("{checked} pair checks");
    assert!(checked > 100);
}
