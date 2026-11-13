//! AUTOCOMP (AUTOCOMP-plan 3, A6): completions in the LEXUX `\` cycle. Synthetic dictionary only; assertions
//! never print text.
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use twime_model::{ModelPaths, autocomp::AutoParams, lexicon::Dictionary, vocab::Vocab};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const DICT: &str = "\
ㄓㄨㄥ 中 -2
ㄏㄨㄚˊ 華 -2
ㄇㄧㄣˊ 民 -2
ㄍㄨㄛˊ 國 -2
ㄒㄧㄢˋ 憲 -3
ㄈㄚˇ 法 -2
ㄉㄜ˙ 的 -1
ㄓㄨㄥ-ㄏㄨㄚˊ-ㄇㄧㄣˊ-ㄍㄨㄛˊ 中華民國 -3
ㄓㄨㄥ-ㄏㄨㄚˊ-ㄇㄧㄣˊ-ㄍㄨㄛˊ-ㄒㄧㄢˋ-ㄈㄚˇ 中華民國憲法 -3.3
";

fn assets(dict_text: &str) -> Assets {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/autocomp/core-tests")
        .join(format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("data.txt"), dict_text).unwrap();
    std::fs::write(p.join("aliases.txt"), "").unwrap();
    let (dict, _) = Dictionary::parse(dict_text);
    let n = Vocab::new(&dict).len();
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

fn session(on: bool) -> Session {
    let cfg = Config {
        lambda: 0.,
        lexux: Some(twime_model::lexux::Config { a: 100., b: None, w: false }),
        autocomp: on.then_some(AutoParams { theta: 0.6, theta2: 0.3 }),
        ..Config::default()
    };
    let mut s = Engine::new(Arc::new(assets(DICT)), cfg).new_session();
    s.set_ux(true);
    s.set_lexux_peer(true);
    s
}

fn typed(s: &mut Session, syllables: &[&str]) {
    for syl in syllables {
        for c in syl.chars() {
            s.on_key(KeyEvent::press(keymap::key_of(c).unwrap()));
        }
        if !syl.ends_with(['ˊ', 'ˇ', 'ˋ', '˙']) {
            s.on_key(KeyEvent::press(Key::Space));
        }
    }
}

fn text(s: &Session) -> String {
    s.comp().text()
}

fn bs(s: &mut Session) -> Update {
    s.on_key(KeyEvent::press(Key::Backslash))
}

const ZHM: [&str; 3] = ["ㄓㄨㄥ", "ㄏㄨㄚˊ", "ㄇㄧㄣˊ"];

#[test]
fn completions_hide_the_original_and_list_both_words() {
    let mut s = session(true);
    typed(&mut s, &ZHM);
    assert!(text(&s) == "中華民");
    let h = s.lexux_hint().expect("completion hint");
    assert!(h.hide_first && !h.auto && h.current == 0);
    assert!(h.options[1] == "中華民國" && h.options[2] == "中華民國憲法");
    let labels = h.labels();
    assert!(labels.len() == 2 && labels.iter().all(|l| l.starts_with('\\') && !l.contains("（原）")));
}

#[test]
fn backslash_appends_switches_lengths_and_returns() {
    let mut s = session(true);
    typed(&mut s, &ZHM);
    let u = bs(&mut s);
    assert!(u.handled && u.commit.is_none());
    assert!(text(&s) == "中華民國" && s.st.syl.len() == 4 && s.st.cursor == 4);
    assert!(s.st.locks.iter().all(Option::is_some));
    bs(&mut s);
    assert!(text(&s) == "中華民國憲法" && s.st.syl.len() == 6 && s.st.cursor == 6);
    bs(&mut s);
    assert!(text(&s) == "中華民" && s.st.syl.len() == 3 && s.st.cursor == 3);
    assert!(s.st.swaps.len() == 1 && s.st.swaps[0].index == 0 && s.st.locks.iter().all(Option::is_some));
    bs(&mut s);
    assert!(text(&s) == "中華民國");
}

#[test]
fn typing_at_the_end_settles_and_keeps_the_word() {
    let mut s = session(true);
    typed(&mut s, &ZHM);
    bs(&mut s);
    typed(&mut s, &["ㄉㄜ˙"]);
    assert!(text(&s) == "中華民國的");
    assert!(s.st.swaps.len() == 1 && s.st.swaps[0].settled);
    assert!(s.st.locks[..4].iter().all(Option::is_some));
    // a settled record is out of the cycle
    assert!(s.lexux_spans().iter().all(|h| h.start + h.len != 4 || h.options.iter().all(|o| o.extra.is_empty())));
}

#[test]
fn incomplete_zhuyin_does_not_settle() {
    let mut s = session(true);
    typed(&mut s, &ZHM);
    bs(&mut s);
    s.on_key(KeyEvent::press(keymap::key_of('ㄉ').unwrap()));
    assert!(!s.st.swaps[0].settled);
    s.on_key(KeyEvent::press(Key::Backspace));
    assert!(!s.st.swaps[0].settled && text(&s) == "中華民國");
}

#[test]
fn backspace_dissolves_the_group_and_keeps_the_rest() {
    let mut s = session(true);
    typed(&mut s, &ZHM);
    bs(&mut s);
    bs(&mut s);
    assert!(text(&s) == "中華民國憲法");
    s.on_key(KeyEvent::press(Key::Backspace));
    assert!(s.st.swaps.is_empty() && s.st.locks.iter().all(Option::is_none));
    assert!(s.st.syl.len() == 5);
}

#[test]
fn completions_only_at_the_end_and_off_gives_none() {
    let mut s = session(true);
    typed(&mut s, &ZHM);
    assert!(s.lexux_hint().is_some());
    s.on_key(KeyEvent::press(Key::Left));
    assert!(s.lexux_hint().is_none());
    let mut off = session(false);
    typed(&mut off, &ZHM);
    assert!(off.lexux_hint().is_none());
}

#[test]
fn off_and_on_type_the_same_without_backslash() {
    let mut on = session(true);
    let mut off = session(false);
    for syl in ["ㄓㄨㄥ", "ㄏㄨㄚˊ", "ㄇㄧㄣˊ", "ㄍㄨㄛˊ"] {
        typed(&mut on, &[syl]);
        typed(&mut off, &[syl]);
        assert!(text(&on) == text(&off));
    }
    assert!(on.on_key(KeyEvent::press(Key::Enter)).commit == off.on_key(KeyEvent::press(Key::Enter)).commit);
}
