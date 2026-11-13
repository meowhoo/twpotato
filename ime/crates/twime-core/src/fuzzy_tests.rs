use super::*;
use std::{path::PathBuf, sync::Arc};

use twime_model::{Assets, ModelPaths};
use twime_proto::{KeyEvent, Update};

use crate::{Config, Engine};

fn sym(cs: &str) -> Vec<char> {
    cs.chars().collect()
}

fn nb_syms(c: char) -> Vec<char> {
    neighbours(keymap::key_of(c).unwrap()).into_iter().map(|k| keymap::symbol(k).unwrap()).collect()
}

#[test]
fn fixed_cases() {
    // S-a: ㄅㄛ -> ㄅㄨㄛ, and back
    assert_eq!(forward("ㄅㄛ", Class::Sa), vec!["ㄅㄨㄛ".to_string()]);
    assert_eq!(forward("ㄅㄨㄛ", Class::Sa), vec!["ㄅㄛ".to_string()]);
    // PN: ㄧㄥ -> ㄧㄣ
    assert_eq!(forward("ㄧㄥ", Class::Pn), vec!["ㄧㄣ".to_string()]);
    // K: ㄙㄨ typed `n j Space`, `n` -> `m` gives ㄩ which ㄨ then overwrites
    assert_eq!(replay(&[Key::Letter(b'm'), Key::Letter(b'j'), Key::Space]), Replay::One("ㄨ".into()));
    assert!(forward("ㄙㄨ", Class::K).contains(&"ㄨ".to_string()));
    // neighbour table
    let mut a = nb_syms('ㄢ');
    a.sort();
    let mut e = sym("ㄞㄦㄟㄣ");
    e.sort();
    assert_eq!(a, e);
    let mut a = nb_syms('ㄤ');
    a.sort();
    let mut e = sym("ㄣㄠㄡㄥ");
    e.sort();
    assert_eq!(a, e);
    assert!(!nb_syms('ㄢ').contains(&'ㄤ'));
}

#[test]
fn class_conditions() {
    assert_eq!(forward("ㄐㄩ", Class::Sb), vec!["ㄐㄨ".to_string()]);
    assert!(forward("ㄉㄨ", Class::Sb).is_empty());
    assert_eq!(forward("ㄅㄥ", Class::Sc), vec!["ㄅㄨㄥ".to_string()]);
    assert!(forward("ㄉㄥ", Class::Sc).is_empty());
    assert_eq!(forward("ㄓ", Class::Sd), vec!["ㄓㄧ".to_string()]);
    assert_eq!(forward("ㄧㄝˋ", Class::Se), vec!["ㄧㄜˋ".to_string()]);
    assert_eq!(forward("ㄕㄢ", Class::Pz), vec!["ㄙㄢ".to_string()]);
    assert_eq!(forward("ㄖㄣˊ", Class::Pr), vec!["ㄌㄣˊ".to_string()]);
    assert_eq!(forward("ㄈㄚ", Class::Pf), vec!["ㄏㄚ".to_string()]);
    assert_eq!(forward("ㄇㄚ", Class::T).len(), 4);
    assert!(!forward("ㄇㄚ", Class::T).contains(&"ㄇㄚ".to_string()));
    // neutral tone first or last parses the same way
    assert_eq!(replay(&[keymap::key_of('˙').unwrap(), keymap::key_of('ㄉ').unwrap(), keymap::key_of('ㄜ').unwrap(), Key::Space]), Replay::One("ㄉㄜ˙".into()));
    // a tone key replaced by a body key: nothing completes
    assert_eq!(replay(&[Key::Digit(1), Key::Digit(8), Key::Digit(2)]), Replay::None);
    // a body key replaced by ˙ completes early
    assert_eq!(replay(&[Key::Digit(1), Key::Digit(7), Key::Digit(3)]), Replay::Early);
}

#[test]
fn ranking_rules() {
    // t = ㄙㄢ is reached by PA (ㄙㄤ), PZ (ㄕㄢ, larger log10 but a later class), and T from three tones
    let ix = Index::build(vec![
        ("ㄙㄤ".to_string(), Some(-4.0)),
        ("ㄕㄢ".to_string(), Some(-1.0)),
        ("ㄙㄢˊ".to_string(), Some(-2.0)),
        ("ㄙㄢˇ".to_string(), None),
        ("ㄙㄢˋ".to_string(), Some(-1.5)),
        ("ㄙㄢ˙".to_string(), None),
        ("ㄅㄛ".to_string(), Some(-3.0)),
    ]);
    let r: Vec<&str> = ix.repairs_all("ㄙㄢ").iter().map(|x| x.syl.as_str()).collect();
    let at = |s: &str| r.iter().position(|x| *x == s).unwrap_or_else(|| panic!("{s} missing"));
    // same cost: class order PA before PZ, whatever the log10
    assert!(at("ㄙㄤ") < at("ㄕㄢ"));
    // cost 2 before cost 3
    assert!(at("ㄕㄢ") < at("ㄙㄢˊ"));
    // same class: larger finite log10 first even against the Unicode order (ˋ U+02CB after ˊ U+02CA), no single-character
    // entry last, then Unicode order (ˇ U+02C7 before ˙ U+02D9)
    assert!(at("ㄙㄢˋ") < at("ㄙㄢˊ"));
    assert!(at("ㄙㄢˊ") < at("ㄙㄢˇ") && at("ㄙㄢˇ") < at("ㄙㄢ˙"));
    assert_eq!(ix.repairs_all("ㄙㄢ")[at("ㄙㄤ")].class, Class::Pa);
    // S-a beats everything else reaching ㄅㄨㄛ
    assert_eq!(ix.repairs("ㄅㄨㄛ")[0], Repair { syl: "ㄅㄛ".into(), class: Class::Sa });
    // never the typed syllable itself
    assert!(ix.repairs("ㄅㄛ").iter().all(|x| x.syl != "ㄅㄛ"));
}

#[test]
fn cheapest_class_and_truncation_on_the_real_table() {
    let Some(a) = real_assets() else {
        eprintln!("skipped: local data files not present");
        return;
    };
    let raw = std::fs::read_to_string(root().join("third_party/mcbopomofo/data.txt")).unwrap();
    let ix = Index::from_data_txt(&a, &raw);
    let mut long = 0;
    for (t, all) in &ix.rev {
        let cut = ix.repairs(t);
        assert_eq!(cut, &all[..all.len().min(MAX_REPAIRS)]);
        long += usize::from(all.len() > MAX_REPAIRS);
        for w in all.windows(2) {
            assert!((w[0].class.cost(), w[0].class.family()) <= (w[1].class.cost(), w[1].class.family()), "{t}");
        }
        for r in all {
            // the class kept is the cheapest one that reaches t from r.syl
            let per = &ix.fwd[&r.syl];
            for (ci, ts) in per.iter().enumerate() {
                if ts.contains(t) {
                    let c = CLASSES[ci];
                    assert!((r.class.cost(), r.class.family()) <= (c.cost(), c.family()), "{t} {}", r.syl);
                }
            }
        }
    }
    assert!(long > 0);
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn real_assets() -> Option<Assets> {
    let r = root();
    let p = ModelPaths {
        data_txt: r.join("third_party/mcbopomofo/data.txt"),
        aliases: r.join("data/evo/r3/legal/common-readings.txt"),
        model: r.join("none"),
        prior: None,
    };
    (p.data_txt.exists() && p.aliases.exists()).then(|| Assets::load_dictionary_only(&p).unwrap())
}

/// Replays keys on a real fresh `Session` (dictionary mode, no model, no user words) and classifies like `replay`.
fn session_replay(engine: &Arc<Engine>, keys: &[Key]) -> Replay {
    let mut s = engine.new_session();
    let done = |u: &Update| -> Vec<String> {
        u.composition.as_ref().map_or(vec![], |c| c.segments.iter().filter(|g| !g.pending).map(|g| g.text.clone()).collect())
    };
    let mut before = vec![];
    let mut last = vec![];
    for (j, &k) in keys.iter().enumerate() {
        let u = s.on_key(KeyEvent::press(k));
        if j + 1 == keys.len() {
            last = done(&u);
        } else {
            before = done(&u);
        }
    }
    match (before.len(), last.len()) {
        (_, 0) => Replay::None,
        (0, 1) => Replay::One(last[0].clone()),
        (1, 1) => Replay::Early,
        _ => Replay::Many,
    }
}

#[test]
fn k_replay_matches_session_for_every_legal_syllable() {
    let Some(_) = real_assets() else {
        eprintln!("skipped: local data files not present");
        return;
    };
    // An empty dictionary: every completed syllable stays visible as an unconverted segment with its own text.
    let engine = Engine::new(Arc::new(Assets::from_dictionary_text("", "").unwrap()), Config::default());
    let a = real_assets().unwrap();
    let (_, syls) = a.dictionary().vocabulary();
    let mut checked = 0;
    for g in syls.iter().filter(|s| a.is_legal_syllable(s)) {
        let s = Slots::parse(g).unwrap_or_else(|| panic!("unparsable legal syllable {g}"));
        let keys = keys_of(&s);
        assert_eq!(session_replay(&engine, &keys), Replay::One(g.clone()), "{g}");
        for j in 0..keys.len() {
            if keys[j] == Key::Space {
                continue;
            }
            for nb in neighbours(keys[j]) {
                let mut v = keys.clone();
                v[j] = nb;
                assert_eq!(replay(&v), session_replay(&engine, &v), "{g} key {j}");
                checked += 1;
            }
        }
    }
    assert!(checked > 10_000, "{checked}");
}

#[test]
fn neighbour_relation_is_symmetric_and_within_layout() {
    let keys = layout_keys();
    assert_eq!(keys.len(), 41);
    for &k in &keys {
        for n in neighbours(k) {
            assert!(neighbours(n).contains(&k));
            assert_ne!(n, k);
        }
        assert!(!neighbours(k).is_empty());
    }
}
