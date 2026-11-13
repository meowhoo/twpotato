//! LEXUX v6 interface matrix. All text is synthetic and assertions never print text.
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use twime_model::{
    ModelPaths,
    lexicon::Dictionary,
    pn::{self, BuildWord, PnLexicon},
    vocab::Vocab,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const DICT: &str = "ㄚ 一 -1\nㄚ 乙 -2\nㄚ 丙 -3\nㄧ 丁 -1\n";

fn root() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/lexux/core-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

pub(super) fn assets() -> Assets { assets_with(DICT, false) }

fn assets_with(dict_text: &str, norm: bool) -> Assets {
    let p = root();
    std::fs::write(p.join("data.txt"), dict_text).unwrap();
    std::fs::write(p.join("aliases.txt"), "").unwrap();
    let (dict, _) = Dictionary::parse(dict_text);
    let n = Vocab::new(&dict).len();
    // A zero-layer, zero-embedding model gives deterministic equal scores; baseline picks the first legal id.
    let mut b = b"TWG1".to_vec();
    for v in [n as u32, 32, 0, 32] {
        b.extend(v.to_le_bytes());
    }
    b.extend(vec![0u8; n * 32 * 4]);
    for _ in 0..32 {
        b.extend(1f32.to_le_bytes());
    }
    std::fs::write(p.join("model.bin"), b).unwrap();
    let mut a = Assets::load(&ModelPaths {
        data_txt: p.join("data.txt"),
        aliases: p.join("aliases.txt"),
        model: p.join("model.bin"),
        prior: None,
    })
    .unwrap();
    let base = p.join("gen");
    let words =
        [("乙乙", "ㄚ-ㄚ"), ("丙丙", "ㄚ-ㄚ"), ("乙乙乙", "ㄚ-ㄚ-ㄚ")].map(|(w, r)| BuildWord {
            word: w.into(),
            reading: r.into(),
            class: twime_model::lexux::JUMP | twime_model::lexux::META,
        });
    pn::build(&a, &words, &base).unwrap();
    a.set_gen(Some(Arc::new(PnLexicon::open(&base, &a).unwrap())));
    if norm { a.set_norm(twime_model::norm::NormCfg::ALL); }
    a
}

pub(super) fn session(auto: bool) -> Session {
    let cfg = Config {
        lambda: 0.,
        lexux: Some(twime_model::lexux::Config {
            a: 100.,
            b: auto.then_some(100.),
            w: false,
        }),
        ..Config::default()
    };
    let mut s = Engine::new(Arc::new(assets()), cfg).new_session();
    s.set_ux(true);
    s.set_lexux_peer(true);
    s
}
fn key(s: &mut Session, k: Key) -> Update {
    s.on_key(KeyEvent::press(k))
}
pub(super) fn typed(s: &mut Session, syllables: &[&str]) -> Update {
    let mut u = Update::default();
    for syl in syllables {
        for c in syl.chars() {
            key(s, keymap::key_of(c).unwrap());
        }
        u = key(s, Key::Space);
    }
    u
}
pub(super) fn pair() -> Session {
    let mut s = session(false);
    typed(&mut s, &["ㄚ", "ㄚ"]);
    assert!(s.lexux_next().is_some());
    s
}
fn text(s: &Session) -> String {
    s.comp().text()
}
fn no_swaps(s: &Session) {
    assert!(s.st.swaps.is_empty());
    assert!(s.st.locks.iter().all(Option::is_none));
    assert!(s.st.groups.iter().all(|&g| g == 0));
}

#[test]
fn active_span_inside_right_left_and_multiple() {
    let mut s = pair();
    let spans = [1, 5, 9].map(|start| twime_model::lexux::Hint {
        start,
        len: 2,
        options: vec![],
        auto: false,
    });
    for (c, want) in [
        (0, 0),
        (1, 0),
        (2, 0),
        (3, 0),
        (4, 0),
        (5, 0),
        (6, 1),
        (8, 1),
        (9, 1),
        (10, 2),
        (12, 2),
    ] {
        s.st.cursor = c;
        assert!(s.active_lexux(&spans) == Some(want));
    }
    assert!(s.active_lexux(&[]).is_none());
}

#[test]
fn several_hints_and_swaps_mark_only_the_active_span() {
    let mut s = pair();
    typed(&mut s, &["ㄧ", "ㄚ", "ㄚ"]);
    let spans = s.lexux_spans();
    assert!(spans.len() == 2 && spans[0].start == 0 && spans[1].start == 3);
    let hints: Vec<u8> = s.comp().segments.iter().map(|g| g.hint).collect();
    assert!(hints == vec![1, 1, 0, 2, 2]);
    key(&mut s, Key::Backslash);
    s.st.cursor = 1;
    key(&mut s, Key::Backslash);
    assert!(s.st.swaps.len() == 2);
    let hints: Vec<u8> = s.comp().segments.iter().map(|g| g.hint).collect();
    assert!(hints == vec![2, 2, 0, 1, 1]);
    key(&mut s, Key::Backslash);
    assert!(
        s.st.swaps
            .iter()
            .find(|r| s.st.groups[0] == r.group)
            .unwrap()
            .index
            == 2
    );
    assert!(
        s.st.swaps
            .iter()
            .find(|r| s.st.groups[3] == r.group)
            .unwrap()
            .index
            == 1
    );
}

#[test]
fn userlex_custom_cover_appears_and_disappears_without_losing_unchanged_effective_readings() {
    let mut a = assets();
    a.set_user_rules(true, twime_model::user::DEFAULT_BONUS)
        .unwrap();
    let reading: Vec<String> = ["ㄚ", "ㄚ", "ㄧ"].into_iter().map(String::from).collect();
    let w = a.user_word_opt("乙乙乙", &reading, None).unwrap();
    let user = a.build_user_lexicon([w]);
    a.set_user(Some(Arc::new(user)));
    let mut s = Engine::new(
        Arc::new(a),
        Config {
            lambda: 0.,
            lexux: Some(twime_model::lexux::Config {
                a: 100.,
                b: None,
                w: false,
            }),
            ..Config::default()
        },
    )
    .new_session();
    s.set_ux(true);
    s.set_lexux_peer(true);
    typed(&mut s, &["ㄚ", "ㄚ"]);
    key(&mut s, Key::Backslash);
    assert!(s.st.swaps.len() == 1 && s.engine.assets.user_cover(&s.st.syl) == vec![false, false]);
    let saved = s.st.swaps[0].syllables.clone();
    typed(&mut s, &["ㄧ"]);
    assert!(s.engine.assets.user_cover(&s.st.syl) == vec![true, true, true]);
    assert!(s.st.swaps.len() == 1 && s.st.swaps[0].syllables == saved);
    key(&mut s, Key::Backspace);
    assert!(s.engine.assets.user_cover(&s.st.syl) == vec![false, false]);
    assert!(s.st.swaps.len() == 1 && s.st.swaps[0].syllables == saved);
}

#[test]
fn switch_cycles_and_zero_stays_locked() {
    let mut s = pair();
    let options = s.lexux_spans()[0].options.clone();
    assert!(options.len() == 3);
    assert!(options.iter().all(|o| o.reading.is_none()));
    let inf = s.st.conv.as_ref().unwrap().clone();
    for i in 1..=6 {
        let u = key(&mut s, Key::Backslash);
        assert!(u.handled && u.commit.is_none());
        assert!(text(&s) == options[i % 3].display);
        assert!(s.st.swaps.len() == 1 && s.st.swaps[0].index == i % 3);
        assert!(s.st.locks.iter().all(Option::is_some) && s.st.explicit.is_empty());
        assert!(s.st.conv.as_ref().unwrap().syllables() == inf.syllables());
    }
}

#[test]
fn continuation_then_zero_then_cycle_preserves_frozen_options() {
    let mut s = pair();
    let options = s.lexux_spans()[0].options.clone();
    key(&mut s, Key::Backslash);
    typed(&mut s, &["ㄧ"]);
    assert!(s.st.swaps.len() == 1 && s.st.locks[..2].iter().all(Option::is_some));
    for i in [2, 0, 1] {
        key(&mut s, Key::Backslash);
        assert!(text(&s).starts_with(&options[i].display) && s.st.swaps[0].index == i);
    }
}

#[test]
fn automatic_span_restores_original_and_back_with_locks() {
    let mut s = session(true);
    let u = typed(&mut s, &["ㄚ", "ㄚ", "ㄚ"]);
    let h = s.lexux_spans()[0].clone();
    assert!(h.auto && h.options.len() == 2 && u.hint.as_ref().is_some_and(|v| v.current == 0 && v.options == h.options.iter().map(|o| o.display.clone()).collect::<Vec<_>>()));
    assert_original_marker(u.hint.as_ref().unwrap(), true, 0);
    assert!(u.composition.unwrap().segments.iter().all(|g| g.hint == 2));
    for i in [1, 0, 1, 0] {
        let u = key(&mut s, Key::Backslash);
        let hint = u.hint.unwrap();
        assert_original_marker(&hint, true, i);
        assert!(hint.current == i);
        assert!(hint.options == h.options.iter().map(|o| o.display.clone()).collect::<Vec<_>>());
        assert!(text(&s) == h.options[i].display && s.st.locks.iter().all(Option::is_some));
    }
}

#[test]
fn deletion_inside_swap_dissolves_the_whole_group() {
    for k in [Key::Backspace, Key::Delete] {
        let mut s = pair();
        key(&mut s, Key::Backslash);
        s.st.cursor = 1;
        key(&mut s, k);
        no_swaps(&s);
    }
}

#[test]
fn insertion_inside_swap_dissolves_the_whole_group() {
    let mut s = pair();
    key(&mut s, Key::Backslash);
    s.st.cursor = 1;
    typed(&mut s, &["ㄧ"]);
    no_swaps(&s);
}

#[test]
fn insertion_and_deletion_before_swap_shift_it() {
    for delete in [Key::Backspace, Key::Delete] {
        let mut s = pair();
        key(&mut s, Key::Backslash);
        let g = s.st.swaps[0].group;
        s.st.cursor = 0;
        typed(&mut s, &["ㄧ"]);
        assert!(s.st.swaps.len() == 1 && s.st.groups == vec![0, g, g]);
        s.st.cursor = if delete == Key::Backspace { 1 } else { 0 };
        key(&mut s, delete);
        assert!(s.st.swaps.len() == 1 && s.st.groups == vec![g, g]);
        assert!(s.lexux_spans()[0].start == 0);
    }
}

#[test]
fn partial_candidate_overlap_releases_all_swap_locks_before_selection() {
    let mut s = pair();
    key(&mut s, Key::Backslash);
    s.st.cand = Some(Cand {
        pos: 1,
        selected: 0,
        items: vec![CandItem {
            text: "一".into(),
            span: 1,
            repairs: vec![],
            extra: vec![],
            raw: false,
            lit: false,
        }],
    });
    s.select(0, true);
    assert!(s.st.swaps.is_empty() && s.st.locks[0].is_none() && s.st.groups[0] == 0);
    assert!(s.st.locks[1] == Some('一') && s.st.explicit.contains(&s.st.groups[1]));
}

#[test]
fn candidate_escape_keeps_swap_and_resumes_cycle() {
    let mut s = pair();
    key(&mut s, Key::Backslash);
    let before = text(&s);
    let u = key(&mut s, Key::Down);
    assert!(u.candidates_open && u.hint.is_none());
    let u = key(&mut s, Key::Esc);
    assert!(!u.candidates_open && text(&s) == before && s.st.swaps[0].index == 1);
    key(&mut s, Key::Backslash);
    assert!(s.st.swaps[0].index == 2);
}

#[test]
fn composition_escape_clears_records() {
    let mut s = pair();
    key(&mut s, Key::Backslash);
    key(&mut s, Key::Esc);
    no_swaps(&s);
    assert!(!s.st.composing());
}

#[test]
fn enter_commits_display_and_failed_host_commit_restores_records() {
    let mut s = pair();
    key(&mut s, Key::Backslash);
    let before = text(&s);
    let u = key(&mut s, Key::Enter);
    assert!(u.commit.as_ref() == Some(&before) && s.st.swaps.is_empty());
    let seq = s.last_seq;
    s.handle(
        seq + 1,
        Some(CommitResult {
            seq,
            commit: Applied::Failed,
            comp: Applied::Applied,
        }),
        Event::Ping,
    );
    assert!(s.st.swaps.len() == 1 && text(&s) == before && s.st.swaps[0].index == 1);
    key(&mut s, Key::Backslash);
    assert!(s.st.swaps[0].index == 2);
}

#[test]
fn incomplete_commit_keeps_records() {
    let mut s = pair();
    key(&mut s, Key::Backslash);
    key(&mut s, keymap::key_of('ㄧ').unwrap());
    assert!(s.take_commit().is_none() && s.st.swaps.len() == 1 && s.st.swaps[0].index == 1);
}

#[test]
fn continuation_recomputes_hints_and_pending_typing_only_hides_text() {
    let mut s = pair();
    let u = key(&mut s, keymap::key_of('ㄧ').unwrap());
    assert!(
        u.hint.is_none()
            && u.composition
                .unwrap()
                .segments
                .iter()
                .filter(|g| g.hint != 0)
                .count()
                == 2
    );
    key(&mut s, Key::Backspace);
    assert!(s.lexux_next().is_some());
    typed(&mut s, &["ㄚ"]);
    assert!(s.lexux_spans()[0].len == 3);
    assert!(s.update(false, None).hint == s.lexux_hint());
    s.st.cursor = 1;
    key(&mut s, Key::Delete);
    key(&mut s, Key::Delete);
    assert!(s.lexux_spans().is_empty() && s.lexux_next().is_none());
}

#[test]
fn invalid_group_shape_length_and_effective_syllables_release_all_members() {
    for mode in 0..3 {
        let mut s = pair();
        key(&mut s, Key::Backslash);
        match mode {
            0 => {
                s.st.groups[0] = 0;
                s.st.locks[0] = None;
            }
            1 => {
                s.st.swaps[0].syllables.push("ㄚ".into());
            }
            _ => {
                s.st.swaps[0].syllables[0] = "ㄧ".into();
            }
        }
        s.reselect();
        no_swaps(&s);
    }
}

#[test]
fn repaired_reading_change_invalidates_swap_before_reconversion() {
    let a = assets();
    let mut index = crate::fuzzy::Index::from_data_txt(&a, DICT);
    index.rev.insert(
        "ㄜ".into(),
        vec![crate::fuzzy::Repair {
            syl: "ㄚ".into(),
            class: crate::fuzzy::Class::Sa,
        }],
    );
    let index = Arc::new(index);
    let mut s = Engine::new(
        Arc::new(a),
        Config {
            lambda: 0.,
            repair: Some(Repair {
                mode: RepairMode::Auto,
                index,
            }),
            lexux: Some(twime_model::lexux::Config {
                a: 100.,
                b: None,
                w: false,
            }),
            ..Config::default()
        },
    )
    .new_session();
    s.set_ux(true);
    s.set_lexux_peer(true);
    typed(&mut s, &["ㄜ", "ㄚ"]);
    assert!(s.eff_syl() == vec!["ㄚ", "ㄚ"]);
    key(&mut s, Key::Backslash);
    assert!(s.st.swaps.len() == 1);
    s.st.repair[0] = Some("ㄧ".into());
    s.reselect();
    no_swaps(&s);
}

#[test]
fn userlex_exemption_effective_reading_transitions_invalidate_in_both_directions() {
    // Supply the coverage boundary directly: production obtains it from the USERLEX snapshot.
    // This also covers future dictionaries that retain syllable codes without legal characters.
    for appears in [true, false] {
        let a = assets();
        let mut index = crate::fuzzy::Index::from_data_txt(&a, DICT);
        index.rev.insert(
            "ㄜ".into(),
            vec![crate::fuzzy::Repair {
                syl: "ㄚ".into(),
                class: crate::fuzzy::Class::Sa,
            }],
        );
        let mut s = Engine::new(
            Arc::new(a),
            Config {
                lambda: 0.,
                repair: Some(Repair {
                    mode: RepairMode::Auto,
                    index: Arc::new(index),
                }),
                lexux: Some(twime_model::lexux::Config {
                    a: 100.,
                    b: None,
                    w: false,
                }),
                ..Config::default()
            },
        )
        .new_session();
        s.set_ux(true);
        s.set_lexux_peer(true);
        typed(&mut s, &["ㄜ", "ㄚ"]);
        key(&mut s, Key::Backslash);
        assert!(s.st.swaps.len() == 1);
        let before = s.eff_syl_from_kinds(s.repair_kinds_for_cover(&[!appears, !appears]));
        let after = s.eff_syl_from_kinds(s.repair_kinds_for_cover(&[appears, appears]));
        assert!(before != after);
        s.st.swaps[0].syllables = before;
        s.validate_swaps_with(&after);
        no_swaps(&s);
    }
}

#[test]
fn recent_output_context_disables_rules_but_document_context_allows_them() {
    let mut s = pair();
    key(&mut s, Key::Enter);
    typed(&mut s, &["ㄚ", "ㄚ"]);
    assert!(s.st.recent_output && s.lexux_spans().is_empty());
    let n = s.last_seq + 1;
    s.handle(n, None, Event::SetContext("一".into()));
    typed(&mut s, &["ㄚ", "ㄚ"]);
    assert!(!s.st.recent_output && !s.lexux_spans().is_empty());
}

#[test]
fn fallback_and_missing_keys_and_old_peers_have_no_hints() {
    let mut a = pair();
    a.set_lexux_peer(false);
    a.reselect();
    assert!(a.lexux_spans().is_empty() && a.lexux_next().is_none());
    let u = key(&mut a, Key::Backslash);
    assert!(u.commit.is_some() && a.st.swaps.is_empty());
    let mut a = session(false);
    let e = Engine::new(a.engine.assets.clone(), Config::default());
    a = e.new_session();
    a.set_ux(true);
    a.set_lexux_peer(true);
    typed(&mut a, &["ㄚ", "ㄚ"]);
    assert!(a.lexux_next().is_none() && a.lexux_spans().is_empty());
    let e = Engine::new(
        Arc::new(Assets::from_dictionary_text(DICT, "").unwrap()),
        Config {
            lexux: Some(twime_model::lexux::Config {
                a: 100.,
                b: Some(100.),
                w: true,
            }),
            ..Config::default()
        },
    );
    let mut a = e.new_session();
    a.set_ux(true);
    a.set_lexux_peer(true);
    typed(&mut a, &["ㄚ", "ㄚ", "ㄚ"]);
    assert!(a.st.conv.as_ref().unwrap().fallback && a.lexux_next().is_none());
}

// Every ineligible Backslash case must match the existing session behaviour byte for byte for v1..v5.
#[test]
fn ineligible_backslash_matches_old_behaviour_and_hint_is_hidden() {
    for case in 0..12 {
        let mut s = pair();
        let mut mods = 0;
        match case {
            0 => {
                key(&mut s, Key::Esc);
                typed(&mut s, &["ㄧ"]);
            }
            1 => mods = twime_proto::Mods::SHIFT,
            2 => mods = twime_proto::Mods::CAPS,
            3 => {
                key(&mut s, Key::Down);
            }
            4 => {
                s.st.symbols = true;
            }
            5 => {
                key(&mut s, keymap::key_of('ㄧ').unwrap());
            }
            6 => {
                s.st.conv.as_mut().unwrap().chars[0] = None;
            }
            7 => s.st.mode = Mode::English,
            8 => s.st.run = "abc".into(),
            9 => s.st.clear_composition(),
            10 => mods = twime_proto::Mods::CTRL,
            _ => mods = twime_proto::Mods::ALT,
        }
        let mut old = s.engine.new_session();
        old.set_ux(true);
        old.st = s.st.clone();
        let ev = KeyEvent {
            key: Key::Backslash,
            mods: twime_proto::Mods(mods),
            down: true,
        };
        s.hint_mods = ev.mods;
        assert!(s.lexux_next().is_none());
        let u = s.on_key(ev);
        let v = old.on_key(ev);
        for version in 1..=5 {
            let pack = |update| {
                twime_proto::Reply::Update {
                    session: 1,
                    seq: 1,
                    update,
                }
                .encode_for(version)
            };
            assert!(pack(u.clone()) == pack(v.clone()));
        }
        assert!(s.st.swaps.is_empty());
    }
}

#[test]
fn norm_illegal_display_cycles_and_automatic_restore_keep_legal_locks() {
    // Output word folding produces a display glyph absent from its legal set.
    // All fixtures are synthetic and boolean assertions never disclose text.
    let dict = "\u{3112}\u{3129}\u{3122} \u{5ba3} -1\n\u{3112}\u{3129}\u{3122} \u{6f22} -2\n\u{3112}\u{3129}\u{3122} \u{6f23} -3\n\u{3105}\u{3128}\u{02cb} \u{4f48} -1\n\u{3105}\u{3128}\u{02cb} \u{6f22} -2\n\u{3105}\u{3128}\u{02cb} \u{6f23} -3\n";
    let mut a = assets_with(dict, true);
    let base = root().join("norm-gen");
    let words = [2, 3].map(|n| BuildWord {
        word: "\u{6f22}".repeat(n),
        reading: if n == 2 { "\u{3112}\u{3129}\u{3122}-\u{3105}\u{3128}\u{02cb}".into() } else { "\u{3112}\u{3129}\u{3122}-\u{3105}\u{3128}\u{02cb}-\u{3112}\u{3129}\u{3122}".into() },
        class: twime_model::lexux::JUMP | twime_model::lexux::META,
    });
    pn::build(&a, &words, &base).unwrap();
    a.set_gen(Some(Arc::new(PnLexicon::open(&base, &a).unwrap())));
    assert!(!a.is_legal_char("\u{3105}\u{3128}\u{02cb}", '\u{5e03}'));
    let a = Arc::new(a);
    for auto in [false, true] {
        let cfg = Config { lambda: 0., lexux: Some(twime_model::lexux::Config { a: 100., b: auto.then_some(100.), w: false }), ..Config::default() };
        let mut s = Engine::new(a.clone(), cfg).new_session();
        s.set_ux(true); s.set_lexux_peer(true);
        let mut syllables = vec!["\u{3112}\u{3129}\u{3122}", "\u{3105}\u{3128}\u{02cb}"];
        if auto { syllables.push("\u{3112}\u{3129}\u{3122}"); }
        for syl in &syllables {
            for c in syl.chars() { key(&mut s, keymap::key_of(c).unwrap()); }
            if !syl.ends_with('\u{02cb}') { key(&mut s, Key::Space); }
        }
        let frozen = text(&s);
        assert!(s.lexux_next().is_some());
        key(&mut s, Key::Backslash);
        if auto { assert!(text(&s).starts_with("\u{5ba3}\u{5e03}")); }
        key(&mut s, Key::Backslash);
        assert!(text(&s) == frozen);
        assert!(s.st.swaps[0].index == 0);
        assert!(s.st.locks.iter().all(Option::is_some));
        for (i, ch) in s.st.locks.iter().enumerate() { assert!(a.is_legal_char(syllables[i], ch.unwrap())); }
        s.reconvert(); assert!(text(&s) == frozen);
        key(&mut s, Key::Backslash);
        assert!(s.st.swaps[0].index == 1);
        let displayed = text(&s); let committed = key(&mut s, Key::Enter);
        assert!(committed.commit.as_deref() == Some(displayed.as_str()));
    }
}

#[test]
fn frozen_hint_options_and_index_follow_full_cycle() {
        let mut s = pair();
        let initial = s.update(false, None).hint.unwrap();
        assert!(initial.valid() && initial.current == 0);
        assert_original_marker(&initial, false, 0);
        for i in 1..=initial.options.len() * 2 {
            let u = key(&mut s, Key::Backslash);
            let h = u.hint.unwrap();
            assert_original_marker(&h, false, i % initial.options.len());
            assert!(h.options == initial.options);
            assert!(h.current == i % h.options.len());
            assert!(u.composition.unwrap().text() == h.options[h.current]);
            assert!(!u.candidates_open && u.candidates.is_none());
        }
}

fn assert_original_marker(hint: &twime_proto::LexuxHint, auto: bool, current: usize) {
    assert!(hint.auto == auto && hint.current == current);
    for (i, label) in hint.labels().iter().enumerate() {
        assert!(label.contains("（原）") == (i == usize::from(auto)));
        assert!(label.contains("（目前）") == (i == current));
    }
}
