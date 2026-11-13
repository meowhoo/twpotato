//! FUZZY F2 step 2 (FUZZY-F2-plan 5, S5): fuzzy options in the LEXUX `\` cycle. Synthetic dictionary only;
//! assertions never print text.
use super::*;
use crate::fuzzy::Index;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use twime_model::{ModelPaths, lexicon::Dictionary, pn::{self, BuildWord, PnLexicon}, vocab::Vocab};

static NEXT: AtomicUsize = AtomicUsize::new(0);
// ㄉㄢˇ-ㄢˋ has no word, so the output 膽案 is not a word; one PA change (ㄉㄢˇ -> ㄉㄤˇ) gives the common word 檔案.
const DICT: &str = "\
ㄉㄢˇ 膽 -2.5
ㄉㄤˇ 檔 -3
ㄢˋ 案 -2
ㄉㄤˇ-ㄢˋ 檔案 -3
ㄕˊ 時 -1
ㄐㄧㄢ 間 -1
ㄕˊ-ㄐㄧㄢ 時間 -4
";

fn assets(dict_text: &str) -> Assets {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/fuzzy/f2/core-tests")
        .join(format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("data.txt"), dict_text).unwrap();
    std::fs::write(p.join("aliases.txt"), "").unwrap();
    let (dict, _) = Dictionary::parse(dict_text);
    let n = Vocab::new(&dict).len();
    // zero layers and embeddings: equal scores, deterministic first legal character
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

fn layer(a: &Assets, words: &[(&str, &str)], name: &str) -> Arc<PnLexicon> {
    let base = std::path::Path::new(&std::env::temp_dir()).join(format!("twime-f2s2-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let words: Vec<BuildWord> = words.iter().map(|(w, r)| BuildWord { word: (*w).into(), reading: (*r).into(), class: twime_model::lexux::JUMP | twime_model::lexux::META }).collect();
    pn::build(a, &words, &base).unwrap();
    Arc::new(PnLexicon::open(&base, a).unwrap())
}

fn session_with(hints: bool, lexux: bool) -> Session {
    session_full(DICT, &[], &[], hints, lexux)
}

/// `gen`: general-word layer words (LEXUX hints); `pn`: proper-name words (no jumps).
fn session_full(dict: &str, gen_words: &[(&str, &str)], pn_words: &[(&str, &str)], hints: bool, lexux: bool) -> Session {
    let mut a = assets(dict);
    if !gen_words.is_empty() {
        let g = layer(&a, gen_words, "gen");
        a.set_gen(Some(g));
    }
    if !pn_words.is_empty() {
        let p = layer(&a, pn_words, "pn");
        a.set_pn(Some(p), twime_model::pn::PnCfg { b2: None, b3: None, h: None, cands: false });
    }
    let index = Arc::new(Index::from_data_txt(&a, dict));
    let cfg = Config {
        lambda: 0.,
        lexux: lexux.then_some(twime_model::lexux::Config { a: 100., b: None, w: false }),
        fuzzy_words: Some(index),
        fuzzy_hints: hints,
        ..Config::default()
    };
    let mut s = Engine::new(Arc::new(a), cfg).new_session();
    s.set_ux(true);
    s.set_lexux_peer(true);
    s
}

fn typed(s: &mut Session, syllables: &[&str]) {
    for syl in syllables {
        for c in syl.chars() {
            s.on_key(KeyEvent::press(keymap::key_of(c).unwrap()));
        }
        // a tone mark ends the syllable; the first tone is Space
        if !syl.ends_with(['ˊ', 'ˇ', 'ˋ', '˙']) {
            s.on_key(KeyEvent::press(Key::Space));
        }
    }
    // the last syllable is not hinted while the cursor is at the end (plan v8): step back over it
    if syllables.len() >= 2 {
        s.on_key(KeyEvent::press(Key::Left));
    }
}

fn text(s: &Session) -> String {
    s.comp().text()
}

#[test]
fn hint_lists_original_and_fuzzy_option_with_reading() {
    let mut s = session_with(true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    assert!(text(&s) == "膽案");
    let h = s.lexux_hint().expect("fuzzy hint");
    assert!(h.options == ["膽案".to_string(), "檔案（ㄉㄤˇ）".to_string()]);
    assert!(h.current == 0 && !h.auto);
    let labels = h.labels();
    assert!(labels[0].starts_with('\\') && labels[0].contains("（原）"));
    assert!(labels[1].starts_with('\\') && labels[1].contains("ㄉㄤˇ"));
}

#[test]
fn backslash_switches_reading_and_cycles_back() {
    let mut s = session_with(true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    let u = s.on_key(KeyEvent::press(Key::Backslash));
    assert!(u.handled && u.commit.is_none());
    assert!(text(&s) == "檔案");
    assert!(s.st.repair[0].as_deref() == Some("ㄉㄤˇ") && s.st.repair[1].is_none());
    assert!(s.eff_syl() == ["ㄉㄤˇ".to_string(), "ㄢˋ".to_string()]);
    assert!(s.lexux_hint().unwrap().current == 1);
    assert!(s.st.swaps.len() == 1 && s.st.locks.iter().all(Option::is_some));
    // back to option 0: text and repair restored, the span stays locked
    s.on_key(KeyEvent::press(Key::Backslash));
    assert!(text(&s) == "膽案");
    assert!(s.st.repair.iter().all(Option::is_none));
    assert!(s.st.swaps.len() == 1 && s.st.swaps[0].index == 0 && s.st.locks.iter().all(Option::is_some));
    s.on_key(KeyEvent::press(Key::Backslash));
    assert!(text(&s) == "檔案");
}

#[test]
fn enter_commits_the_shown_option_without_reading_label() {
    let mut s = session_with(true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    s.on_key(KeyEvent::press(Key::Backslash));
    let u = s.on_key(KeyEvent::press(Key::Enter));
    assert!(u.commit.as_deref() == Some("檔案"));
}

#[test]
fn off_or_without_lexux_gives_no_fuzzy_hint() {
    for (hints, lexux) in [(false, true), (true, false)] {
        let mut s = session_with(hints, lexux);
        typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
        assert!(text(&s) == "膽案");
        assert!(s.lexux_hint().is_none());
        assert!(s.st.conv.as_ref().unwrap().hints.is_empty());
    }
}

#[test]
fn a_word_output_gets_no_fuzzy_hint() {
    let mut s = session_with(true, true);
    typed(&mut s, &["ㄉㄤˇ", "ㄢˋ"]);
    assert!(text(&s) == "檔案");
    assert!(s.lexux_hint().is_none());
}

#[test]
fn editing_inside_the_span_dissolves_the_record_and_keeps_the_reading() {
    let mut s = session_with(true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    s.on_key(KeyEvent::press(Key::Backslash));
    assert!(text(&s) == "檔案");
    s.on_key(KeyEvent::press(Key::Right));
    s.on_key(KeyEvent::press(Key::Backspace));
    assert!(s.st.swaps.is_empty() && s.st.locks.iter().all(Option::is_none));
    assert!(s.st.repair.first().and_then(|r| r.as_deref()) == Some("ㄉㄤˇ"));
}

// 膽岸 / 膽案 share ㄉㄢˇ-ㄢˋ (the zero model shows 膽岸, the first legal id); 膽案 is a general-layer word, so LEXUX hints it on the same span and the fuzzy 檔案 joins
// the same cycle as the third option.
const DICT3: &str = "ㄉㄢˇ 膽 -2.5
ㄉㄤˇ 檔 -3
ㄢˋ 案 -2
ㄢˋ 岸 -2.5
ㄉㄤˇ-ㄢˋ 檔案 -3
ㄕˊ 時 -1
ㄐㄧㄢ 間 -1
ㄕˊ-ㄐㄧㄢ 時間 -4
";

#[test]
fn identical_lexux_span_merges_into_one_three_option_cycle() {
    let mut s = session_full(DICT3, &[("膽案", "ㄉㄢˇ-ㄢˋ")], &[], true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    assert!(text(&s) == "膽岸");
    let h = s.lexux_hint().expect("merged hint");
    assert!(h.options == ["膽岸".to_string(), "膽案".to_string(), "檔案（ㄉㄤˇ）".to_string()]);
    let want = [("膽案", None), ("檔案", Some("ㄉㄤˇ")), ("膽岸", None), ("膽案", None)];
    for (i, (t, rep)) in want.iter().enumerate() {
        s.on_key(KeyEvent::press(Key::Backslash));
        assert!(text(&s) == *t, "step {i}");
        assert!(s.st.repair[0].as_deref() == *rep, "step {i}");
        assert!(s.st.swaps.len() == 1 && s.st.swaps[0].index == (i + 1) % 3);
        // lock sources are legal for the current effective reading
        let eff = s.eff_syl();
        assert!(s.st.locks.iter().enumerate().all(|(k, c)| c.is_some_and(|c| s.engine.assets.is_legal_char(&eff[k], c))));
    }
}

#[test]
fn below_median_or_tied_variants_give_no_hint() {
    // 檔案 below tau_2 (the other 2-character words are more common)
    let low = "ㄉㄢˇ 膽 -2.5
ㄉㄤˇ 檔 -3
ㄢˋ 案 -2
ㄉㄤˇ-ㄢˋ 檔案 -6
ㄕˊ 時 -1
ㄐㄧㄢ 間 -1
ㄕˊ-ㄐㄧㄢ 時間 -1
ㄕˊ-ㄐㄧㄢˋ 實踐 -2
ㄐㄧㄢˋ 踐 -3
ㄕˊ 實 -2
";
    let mut s = session_full(low, &[], &[], true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    assert!(s.lexux_hint().is_none());
    // two variants of the same class cost and log10 (ㄉㄢˇ->ㄉㄤˇ, ㄢˋ->ㄤˋ): a tie, no hint
    let tie = "ㄉㄢˇ 膽 -2.5
ㄉㄤˇ 檔 -3
ㄢˋ 案 -2
ㄤˋ 盎 -3
ㄉㄤˇ-ㄢˋ 檔案 -3
ㄉㄢˇ-ㄤˋ 膽盎 -3
ㄕˊ 時 -1
ㄐㄧㄢ 間 -1
ㄕˊ-ㄐㄧㄢ 時間 -4
";
    let mut s = session_full(tie, &[], &[], true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    assert!(text(&s) == "膽案");
    assert!(s.lexux_hint().is_none());
}

#[test]
fn proper_name_output_is_excluded() {
    let mut s = session_full(DICT, &[], &[("膽案", "ㄉㄢˇ-ㄢˋ")], true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    assert!(text(&s) == "膽案");
    assert!(s.lexux_hint().is_none());
}

#[test]
fn hints_off_and_on_are_identical_without_backslash() {
    let mut on = session_with(true, true);
    let mut off = session_with(false, true);
    for syl in ["ㄕˊ", "ㄉㄢˇ", "ㄢˋ", "ㄐㄧㄢ"] {
        typed(&mut on, &[syl]);
        typed(&mut off, &[syl]);
        assert!(text(&on) == text(&off));
        assert!(on.eff_syl() == off.eff_syl());
    }
    let (a, b) = (on.on_key(KeyEvent::press(Key::Enter)), off.on_key(KeyEvent::press(Key::Enter)));
    assert!(a.commit == b.commit);
}

#[test]
fn insertion_inside_the_span_dissolves_and_keeps_the_reading() {
    let mut s = session_with(true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄢˋ"]);
    s.on_key(KeyEvent::press(Key::Backslash));
    assert!(text(&s) == "檔案");
    typed(&mut s, &["ㄕˊ"]);
    assert!(s.st.swaps.is_empty() && s.st.locks.iter().all(Option::is_none));
    assert!(s.st.repair.first().and_then(|r| r.as_deref()) == Some("ㄉㄤˇ"));
}

#[test]
fn existing_repair_inside_the_span_is_kept_and_restored() {
    let dict = format!("{DICT}ㄤˋ 盎 -3\n");
    let mut s = session_full(&dict, &[], &[], true, true);
    typed(&mut s, &["ㄉㄢˇ", "ㄤˋ"]);
    // as if a step-1 suggestion had changed position 1 to ㄢˋ
    s.st.repair[1] = Some("ㄢˋ".to_string());
    s.reconvert();
    assert!(text(&s) == "膽案");
    assert!(s.eff_syl() == ["ㄉㄢˇ".to_string(), "ㄢˋ".to_string()]);
    s.on_key(KeyEvent::press(Key::Backslash));
    assert!(text(&s) == "檔案");
    assert!(s.st.repair[0].as_deref() == Some("ㄉㄤˇ") && s.st.repair[1].as_deref() == Some("ㄢˋ"));
    let eff = s.eff_syl();
    assert!(s.st.locks.iter().enumerate().all(|(k, c)| c.is_some_and(|c| s.engine.assets.is_legal_char(&eff[k], c))));
    s.on_key(KeyEvent::press(Key::Backslash));
    assert!(text(&s) == "膽案");
    assert!(s.st.repair[0].is_none() && s.st.repair[1].as_deref() == Some("ㄢˋ"));
}

#[test]
fn last_syllable_is_not_hinted_while_typing_at_the_end() {
    let mut s = session_with(true, true);
    for c in "ㄉㄢˇㄢˋ".chars() {
        s.on_key(KeyEvent::press(keymap::key_of(c).unwrap()));
    }
    assert!(text(&s) == "膽案" && s.lexux_hint().is_none());
    s.on_key(KeyEvent::press(Key::Left));
    assert!(s.lexux_hint().is_some());
}
