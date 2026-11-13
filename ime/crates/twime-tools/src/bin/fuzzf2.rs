//! FUZZY F2 evaluation driver (FUZZY-F2-plan 3): drives real `Session`s key by key (product settings: PN, int8,
//! NORM, TA as the engine; `repair=suggest`) with `fuzzy_words` off and on. Prints counts only; text stays in local
//! output files.
//!
//!   fuzzf2 clean   <settings> <units.tsv> <out.tsv> <threads>     key, hash of every typing Update + Enter: off, on
//!   fuzzf2 cands   <settings> <pos.tsv>   <out.tsv> <threads>     key, off list, on list (text:span:display, U+1F joined)
//!   fuzzf2 quality <settings> <q.tsv>     <out.jsonl> <threads>   off / on outputs and lists at the cursor (+ a choice)
//!   fuzzf2 emoji <settings> <units.tsv> <out.jsonl> <threads> [emoji.tsv]   EMOJI equivalence (fuzzf2/emoji.rs)
//!   fuzzf2 emoji-keys <settings> <emoji.tsv>                                EMOJI representative keywords
//!   fuzzf2 zyout <settings> <units.tsv> <out.jsonl> <threads> [emoji.tsv]   ZHUYIN-OUT equivalence (fuzzf2/zyout.rs)
//! units.tsv: key, context, syllables; pos.tsv: + cursor; q.tsv: key, context, syllables, cursor [, display to choose in
//! the on list ("*" = the first marked item that is not in the off list)].

#[path = "fuzzf2/pop.rs"]
mod pop;
#[path = "fuzzf2/emoji.rs"]
mod emoji;
#[path = "fuzzf2/emoji_shim.rs"]
mod emoji_shim;
#[path = "fuzzf2/zyout.rs"]
mod zyout;
#[path = "fuzzf2/zyout_shim.rs"]
mod zyout_shim;

use std::{
    collections::HashMap,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Write,
    path::PathBuf,
    sync::Arc,
};

use twime_core::{Config, Engine, Session, fuzzy};
use twime_model::{
    Assets, ModelPaths, NormCfg,
    pn::{self, PnCfg, PnLexicon},
};
use twime_proto::{Event, Key, KeyEvent, Update};

const FFFD: char = '\u{FFFD}';
/// Items of each list kept in the quality output (two pages of 9 and more).
const KEEP: usize = 27;

fn settings(p: &str) -> HashMap<String, String> {
    pn::read_settings(std::path::Path::new(p)).expect("settings")
}

/// `fuzzf1`'s load (pneval's with the engine's TA default).
fn load(m: &HashMap<String, String>) -> Assets {
    let p = ModelPaths { data_txt: m["data_txt"].clone().into(), aliases: m["aliases"].clone().into(), model: m["model"].clone().into(), prior: m.get("prior").map(PathBuf::from) };
    let mut a = Assets::load(&p).expect("assets");
    match m.get("int8").map(String::as_str) {
        Some("on") => a.set_int8().expect("int8"),
        None | Some("off") => {}
        _ => panic!("int8 must be on or off"),
    }
    if m.get("norm").map_or("all", String::as_str) == "all" {
        a.set_norm(NormCfg::ALL);
    }
    a.set_ta_default(m.get("ta_default").is_none_or(|v| v != "off"));
    let lex = PnLexicon::open(std::path::Path::new(&m["lexicon"]), &a).expect("lexicon");
    a.set_pn(Some(Arc::new(lex)), PnCfg::from_settings(m).expect("pn settings"));
    if let Some(g) = m.get("gen") {
        let lex = PnLexicon::open(std::path::Path::new(g), &a).expect("gen lexicon");
        a.set_gen(Some(Arc::new(lex)));
    }
    a
}

/// Step 2 engines: LEXUX from the settings, `fuzzy_words` on, `fuzzy_hints` off / on (the only difference).
fn engines2(m: &HashMap<String, String>, assets: &Arc<Assets>) -> [Arc<Engine>; 2] {
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let data = fs::read_to_string(&m["data_txt"]).expect("data.txt");
    let index = Arc::new(fuzzy::Index::from_data_txt(assets, &data));
    let lexux = twime_model::lexux::Config::parse(m).expect("lexux keys").expect("lexux_a / lexux_b required");
    let base = Config { lambda, candidates: true, lexux: Some(lexux), fuzzy_words: Some(index.clone()),
        repair: Some(twime_core::Repair { mode: twime_core::RepairMode::Suggest, index }), ..Config::default() };
    [Engine::new(assets.clone(), base.clone()), Engine::new(assets.clone(), Config { fuzzy_hints: true, ..base })]
}

/// A step-2 session: LEXUX peer, UX on, the unit's context.
fn hint_session(e: &Arc<Engine>, ctx: &str) -> Session {
    let mut s = e.new_session();
    s.set_ux(true);
    s.set_lexux_peer(true);
    s.handle(1, None, Event::SetContext(ctx.to_string()));
    s
}


fn comp_text(u: &Update) -> Option<String> {
    u.composition.as_ref().map(|c| c.segments.iter().map(|g| g.text.clone()).collect::<String>())
}

/// Plan 5.3: S1 (typing text / committability / commit identical off vs on), S2 (hints and appearances while typing),
/// S3 (the output with every fuzzy hint switched to its fuzzy option, left to right).
fn hints(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let es = engines2(&m, &assets);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let n = syl.len();
        let (mut off, mut on) = (hint_session(&es[0], &r[1]), hint_session(&es[1], &r[1]));
        // S2 (LEXUX 3.3 metric): at each completed syllable, the fuzzy groups (start, len, word) new against the previous one
        let (mut same, mut appear, mut appear_fz) = (true, 0usize, 0usize);
        let mut prev: Vec<(usize, usize, String)> = vec![];
        let mut last_on = Update::default();
        for x in &syl {
            for k in keys_of(x) {
                let uo = press(&mut off, k);
                let un = press(&mut on, k);
                same &= comp_text(&uo) == comp_text(&un) && uo.committable == un.committable && uo.commit == un.commit;
                last_on = un;
            }
            let now: Vec<(usize, usize, String)> = on.fuzzy_hint_spans().into_iter().map(|(s0, l0, _, w)| (s0, l0, w)).collect();
            let new = now.iter().filter(|g| !prev.contains(g)).count();
            appear += new;
            appear_fz += new;
            prev = now;
        }
        let mut j = serde_json::json!({"key": r[0], "n": n});
        j["out"] = serde_json::json!(comp_text(&last_on));
        j["hints_fz"] = serde_json::json!(on.fuzzy_hint_spans().len());
        j["appear"] = serde_json::json!(appear);
        j["appear_fz"] = serde_json::json!(appear_fz);
        // S1: the commits without any backslash
        let (co, cn) = (press(&mut off, Key::Enter), press(&mut hint_session_typed(&es[1], &r[1], &syl), Key::Enter));
        same &= co.commit == cn.commit;
        j["same_typing"] = serde_json::json!(same);
        // S3: the fuzzy hints of the finished composition (frozen here), left to right, each switched to its fuzzy option;
        // a hint that is gone or changed after an earlier switch, or a switch that does not show the word, is a failure
        let frozen = on.fuzzy_hint_spans();
        let mut u = last_on;
        let mut fail = 0usize;
        for (st, len, _, word) in &frozen {
            for _ in st + len..n {
                press(&mut on, Key::Left);
            }
            match on.fuzzy_hint_spans().into_iter().find(|(s0, l0, _, w)| s0 == st && l0 == len && w == word) {
                Some((_, _, idx, _)) => {
                    for _ in 0..idx {
                        u = press(&mut on, Key::Backslash);
                    }
                    let shown: String = comp_text(&u).unwrap_or_default().chars().skip(*st).take(*len).collect();
                    fail += usize::from(shown != *word);
                }
                None => fail += 1,
            }
            for _ in st + len..n {
                u = press(&mut on, Key::Right);
            }
        }
        j["fixed"] = serde_json::json!(comp_text(&u));
        j["s3_fail"] = serde_json::json!(fail);
        j.to_string()
    });
    write(&a[4], &out);
    println!("hint rows {}", out.len());
}

/// AUTOCOMP (AUTOCOMP-plan 3): `autocomp <settings> <units.tsv> <out.jsonl> <threads> <theta> <theta2>`.
/// units.tsv: key, context, syllables (the gold reading), gold text. Observation trajectory (A1-A3): off / on typed
/// syllable by syllable, never `\`; each completed syllable's new completion group and whether its first completion
/// equals the gold over the whole k + m span (characters and syllables). Acceptance trajectory (A4): a correct first
/// completion is taken with `\` (checked: text, syllable count, cursor), the m gold syllables are skipped; commits of
/// the baseline (off) and of the acceptance trajectory are written for the scorer.
fn autocomp(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let theta: f64 = a[6].parse().expect("theta");
    let theta2: f64 = a[7].parse().expect("theta2");
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let lexux = twime_model::lexux::Config::parse(&m).expect("lexux keys").expect("lexux_a / lexux_b required");
    let base = Config { lambda, candidates: true, lexux: Some(lexux), ..Config::default() };
    let es = [Engine::new(assets.clone(), base.clone()),
        Engine::new(assets.clone(), Config { autocomp: Some(twime_model::autocomp::AutoParams { theta, theta2 }), ..base })];
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let gold: Vec<char> = r[3].chars().collect();
        let n = syl.len();
        let keys: Vec<Vec<Key>> = syl.iter().map(|x| keys_of(x)).collect();
        let mut j = serde_json::json!({"key": r[0], "n": n, "keys": keys.iter().map(Vec::len).sum::<usize>()});
        // observation
        let (mut off, mut on) = (hint_session(&es[0], &r[1]), hint_session(&es[1], &r[1]));
        let mut same = true;
        let (mut shown, mut correct) = (0usize, 0usize);
        let mut prev: Option<(usize, usize, String)> = None;
        for ks in &keys {
            for &k in ks {
                let uo = press(&mut off, k);
                let un = press(&mut on, k);
                same &= comp_text(&uo) == comp_text(&un) && uo.committable == un.committable && uo.commit == un.commit;
            }
            let now = on.autocomp_hint_now().map(|(s0, k0, c)| (s0, k0, c[0].1.clone(), c[0].2.clone()));
            let key = now.as_ref().map(|(s0, k0, w, _)| (*s0, *k0, w.clone()));
            if let Some((s0, k0, w, extra)) = &now {
                if key != prev {
                    shown += 1;
                    let end = s0 + k0 + extra.len();
                    let wc: Vec<char> = w.chars().collect();
                    let ok = end <= n && gold.get(*s0..end).is_some_and(|g| g == wc.as_slice())
                        && syl[*s0..s0 + k0].iter().chain(extra.iter()).eq(syl[*s0..end.min(n)].iter()) && end <= n;
                    correct += usize::from(ok);
                }
            }
            prev = key;
        }
        let base_commit = press(&mut off, Key::Enter).commit;
        let on_commit = press(&mut on, Key::Enter).commit;
        same &= base_commit == on_commit;
        j["same_typing"] = serde_json::json!(same);
        j["shown"] = serde_json::json!(shown);
        j["correct"] = serde_json::json!(correct);
        j["base_commit"] = serde_json::json!(base_commit);
        // acceptance
        let mut acc = hint_session(&es[1], &r[1]);
        let (mut i, mut typed_keys, mut presses, mut taken, mut fail) = (0usize, 0usize, 0usize, 0usize, 0usize);
        while i < n {
            for &k in &keys[i] {
                press(&mut acc, k);
            }
            typed_keys += keys[i].len();
            i += 1;
            if let Some((s0, k0, c)) = acc.autocomp_hint_now() {
                let (idx, w, extra) = c[0].clone();
                let end = s0 + k0 + extra.len();
                let wc: Vec<char> = w.chars().collect();
                let ok = s0 + k0 == i && end <= n && gold.get(s0..end).is_some_and(|g| g == wc.as_slice())
                    && extra.iter().eq(syl[i..end].iter());
                if ok {
                    let mut u = Update::default();
                    for _ in 0..idx {
                        u = press(&mut acc, Key::Backslash);
                        presses += 1;
                    }
                    let shown_text = comp_text(&u).unwrap_or_default();
                    let ok2 = shown_text.chars().skip(s0).take(wc.len()).eq(wc.iter().copied()) && shown_text.chars().count() == end;
                    if ok2 {
                        taken += 1;
                        i = end;
                    } else {
                        fail += 1;
                        // put the original back and keep typing
                        let opts = c.len() + 1;
                        for _ in idx..opts {
                            press(&mut acc, Key::Backslash);
                        }
                    }
                }
            }
        }
        j["acc_commit"] = serde_json::json!(press(&mut acc, Key::Enter).commit);
        j["acc_keys"] = serde_json::json!(typed_keys);
        j["acc_presses"] = serde_json::json!(presses);
        j["taken"] = serde_json::json!(taken);
        j["fail"] = serde_json::json!(fail);
        j.to_string()
    });
    write(&a[4], &out);
    println!("autocomp rows {}", out.len());
}

/// PRED plan 3: `acdump <settings> <units.tsv> <out.jsonl> <threads>`. Observation trajectory (typed syllable by syllable,
/// never `\`, LEXUX on, autocomp off so the composition is today's); at each completed syllable every unblocked k's top
/// 20 candidates (word, appended syllables, dictionary score, user) and the left text h (context + all shown characters).
/// One output line per unit; local files only.
fn acdump(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let lexux = twime_model::lexux::Config::parse(&m).expect("lexux keys");
    let e = Engine::new(assets, Config { lambda, candidates: true, lexux, ..Config::default() });
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let mut s = hint_session(&e, &r[1]);
        let mut obs = vec![];
        for (i, x) in syl.iter().enumerate() {
            let mut u = Update::default();
            for k in keys_of(x) {
                u = press(&mut s, k);
            }
            let lists = s.autocomp_lists_now(20);
            if lists.is_empty() {
                continue;
            }
            let shown = comp_text(&u).unwrap_or_default();
            // h = context + every shown character, left-truncated to 64 characters (PRED plan 3)
            let h: Vec<char> = format!("{}{}", r[1], shown).chars().collect();
            let h: String = h[h.len().saturating_sub(64)..].iter().collect();
            obs.push(serde_json::json!({
                "pos": i + 1,
                "h": h,
                "lists": lists.iter().map(|(k, l)| serde_json::json!({"k": k, "c": l.iter().map(|c| serde_json::json!([c.word, c.extra, c.score, c.user])).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            }));
        }
        serde_json::json!({"key": r[0], "n": syl.len(), "obs": obs}).to_string()
    });
    write(&a[4], &out);
    println!("acdump rows {}", out.len());
}

/// AUTOCOMP-LIST (AUTOCOMP-LIST-plan 5.2): `aclist <settings> <units.tsv> <out.jsonl> <threads>`; units.tsv:
/// key, context, syllables, gold text. L1: off / on typed key by key without opening the list (text, committability,
/// commit). L3: after every completed syllable both open the list at the end (Down), then Esc; ordinary items = the
/// off list's items, identity (text, span, display); H = the first page holds an item whose text equals the last `span`
/// gold characters. Counts only.
fn aclist(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let base = Config { lambda, candidates: true, ..Config::default() };
    let es = [Engine::new(assets.clone(), base.clone()), Engine::new(assets, Config { autocomp_list: true, ..base })];
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let gold: Vec<char> = r[3].chars().collect();
        let ps = es[0].config().page_size.max(1);
        // L1
        let (mut off, mut on) = (es[0].new_session(), es[1].new_session());
        off.handle(1, None, Event::SetContext(r[1].to_string()));
        on.handle(1, None, Event::SetContext(r[1].to_string()));
        let mut same = true;
        // off fingerprint (compared with the pre-construction snapshot, plan 5.2 L1)
        let mut fp: Vec<String> = vec![];
        for x in &syl {
            for k in keys_of(x) {
                let uo = press(&mut off, k);
                let un = press(&mut on, k);
                same &= comp_text(&uo) == comp_text(&un) && uo.committable == un.committable && uo.commit == un.commit;
                fp.push(format!("{:?}|{}|{:?}", comp_text(&uo), uo.committable, uo.commit));
            }
        }
        same &= press(&mut off, Key::Enter).commit == press(&mut on, Key::Enter).commit;
        // the off list at the end and the composition after choosing its second item
        let mut o2 = es[0].new_session();
        o2.handle(1, None, Event::SetContext(r[1].to_string()));
        for x in &syl {
            for k in keys_of(x) {
                press(&mut o2, k);
            }
        }
        press(&mut o2, Key::Down);
        fp.push(format!("{:?}", o2.candidate_items()));
        let u2 = press(&mut o2, Key::Digit(2));
        fp.push(format!("{:?}|{}|{:?}", comp_text(&u2), u2.committable, u2.composition));
        let off_fp = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            fp.hash(&mut h);
            format!("{:016x}", h.finish())
        };
        // L3
        let (mut off, mut on) = (es[0].new_session(), es[1].new_session());
        off.handle(1, None, Event::SetContext(r[1].to_string()));
        on.handle(1, None, Event::SetContext(r[1].to_string()));
        let mut end_comp = serde_json::Value::Null;
        let (mut obs, mut h_off, mut h_on, mut fell, mut with_comp) = (0usize, 0usize, 0usize, 0usize, 0usize);
        for (i, x) in syl.iter().enumerate() {
            for k in keys_of(x) {
                press(&mut off, k);
                press(&mut on, k);
            }
            let pos = i + 1;
            press(&mut off, Key::Down);
            press(&mut on, Key::Down);
            let lo = off.candidate_items().unwrap_or_default();
            let ln = on.candidate_items().unwrap_or_default();
            press(&mut off, Key::Esc);
            press(&mut on, Key::Esc);
            obs += 1;
            with_comp += usize::from(ln.len() > lo.len());
            let correct = |it: &(String, usize, String)| it.1 <= pos && it.0.chars().count() == it.1
                && gold.get(pos - it.1..pos).is_some_and(|g| g.iter().copied().eq(it.0.chars())) && it.0 == it.2;
            let good: Vec<&(String, usize, String)> = lo.iter().filter(|it| correct(it)).collect();
            let a_off = lo.iter().take(ps).any(|it| good.contains(&it));
            let a_on = ln.iter().take(ps).any(|it| good.contains(&it));
            h_off += usize::from(a_off);
            h_on += usize::from(a_on);
            fell += usize::from(a_off && !a_on);
            if pos == syl.len() {
                let texts: Vec<&String> = lo.iter().map(|it| &it.0).collect();
                if let Some(c) = ln.iter().find(|it| !texts.contains(&&it.0)) {
                    end_comp = serde_json::json!({"word": c.0, "m": c.0.chars().count() - c.1});
                }
            }
        }
        serde_json::json!({"key": r[0], "n": syl.len(), "same_typing": same, "obs": obs, "h_off": h_off, "h_on": h_on,
            "fell_off": fell, "with_completions": with_comp, "off_fp": off_fp, "end_completion": end_comp}).to_string()
    });
    write(&a[4], &out);
    println!("aclist rows {}", out.len());
}

/// AUTOCOMP-LIST latency probe (in process, not a gate): `acbench <settings> <manifest.json>`; per unit, Down at the
/// end timed 15 times (Esc between) on and off; prints p50 / p99 of the per-unit median difference and of on.
fn acbench(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let base = Config { lambda, candidates: true, ..Config::default() };
    let es = [Engine::new(assets.clone(), base.clone()), Engine::new(assets, Config { autocomp_list: true, ..base })];
    let man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).unwrap()).unwrap();
    let mut on_t = vec![];
    let mut d_t = vec![];
    for u in man["units"].as_array().unwrap() {
        let syl: Vec<String> = u["syllables"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        let mut med = [0f64; 2];
        for (i, e) in es.iter().enumerate() {
            let mut s = e.new_session();
            s.handle(1, None, Event::SetContext(u["context"].as_str().unwrap().to_string()));
            for x in &syl {
                for k in keys_of(x) {
                    press(&mut s, k);
                }
            }
            let mut v = vec![];
            for _ in 0..15 {
                let t = std::time::Instant::now();
                press(&mut s, Key::Down);
                v.push(t.elapsed().as_secs_f64() * 1000.0);
                press(&mut s, Key::Esc);
            }
            v.sort_by(f64::total_cmp);
            med[i] = v[7];
        }
        on_t.push(med[1]);
        d_t.push(med[1] - med[0]);
    }
    let q = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(f64::total_cmp);
        v[((v.len() - 1) as f64 * p).round() as usize]
    };
    println!("acbench n={} on p50={:.3} p99={:.3} diff p50={:.3} p99={:.3} max={:.3}", on_t.len(), q(&mut on_t, 0.5), q(&mut on_t, 0.99),
        q(&mut d_t, 0.5), q(&mut d_t, 0.99), q(&mut d_t, 1.0));
}

fn hint_session_typed(e: &Arc<Engine>, ctx: &str, syl: &[String]) -> Session {
    let mut s = hint_session(e, ctx);
    for x in syl {
        for k in keys_of(x) {
            press(&mut s, k);
        }
    }
    s
}

/// The off and on engines: `repair=suggest` (the deployment), `fuzzy_words` off / on, one shared index.
fn engines(m: &HashMap<String, String>, assets: &Arc<Assets>) -> [Arc<Engine>; 2] {
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let data = fs::read_to_string(&m["data_txt"]).expect("data.txt");
    let index = Arc::new(fuzzy::Index::from_data_txt(assets, &data));
    let base = Config { lambda, candidates: true, repair: Some(twime_core::Repair { mode: twime_core::RepairMode::Suggest, index: index.clone() }), ..Config::default() };
    [Engine::new(assets.clone(), base.clone()), Engine::new(assets.clone(), Config { fuzzy_words: Some(index), ..base })]
}

fn keys_of(s: &str) -> Vec<Key> {
    fuzzy::keys_of(&fuzzy::Slots::parse(s).unwrap_or_else(|| panic!("unparsable syllable at a unit")))
}

fn press(s: &mut Session, k: Key) -> Update {
    s.on_key(KeyEvent::press(k))
}

fn type_unit(e: &Arc<Engine>, ctx: &str, syl: &[String]) -> (Session, Vec<Update>) {
    let mut s = e.new_session();
    let mut ups = vec![s.handle(1, None, Event::SetContext(ctx.to_string()))];
    for x in syl {
        for k in keys_of(x) {
            ups.push(press(&mut s, k));
        }
    }
    (s, ups)
}

fn chars(u: &Update, n: usize) -> String {
    let segs = &u.composition.as_ref().expect("composition").segments;
    assert_eq!(segs.len(), n, "one segment per syllable");
    segs.iter().map(|g| if g.unconverted { FFFD } else { g.text.chars().next().unwrap() }).collect()
}

fn hash_updates(ups: &[Update]) -> u64 {
    let mut h = DefaultHasher::new();
    for u in ups {
        format!("{u:?}").hash(&mut h);
    }
    h.finish()
}

fn rows(p: &str) -> Vec<Vec<String>> {
    fs::read_to_string(p).expect("input").lines().filter(|l| !l.is_empty()).map(|l| l.split('\t').map(String::from).collect()).collect()
}

fn syls(f: &str) -> Vec<String> {
    f.split(' ').map(String::from).collect()
}

fn par(rs: Vec<Vec<String>>, threads: usize, f: impl Fn(&[String]) -> String + Send + Sync + 'static) -> Vec<String> {
    let rs = Arc::new(rs);
    let f = Arc::new(f);
    let hs: Vec<_> = (0..threads)
        .map(|t| {
            let (rs, f) = (rs.clone(), f.clone());
            std::thread::spawn(move || rs.iter().enumerate().skip(t).step_by(threads).map(|(i, r)| (i, f(r))).collect::<Vec<_>>())
        })
        .collect();
    let mut all: Vec<(usize, String)> = hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
    all.sort_by_key(|x| x.0);
    all.into_iter().map(|x| x.1).collect()
}

fn write(p: &str, lines: &[String]) {
    let mut f = std::io::BufWriter::new(fs::File::create(p).unwrap());
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
}

/// Types the unit, moves the cursor to `cursor` and opens the list; the Update before opening and the list.
fn open_at(e: &Arc<Engine>, ctx: &str, syl: &[String], cursor: usize) -> (Session, Update, Vec<(String, usize, String)>) {
    let (mut s, ups) = type_unit(e, ctx, syl);
    let last = ups.last().unwrap().clone();
    for _ in cursor..syl.len() {
        press(&mut s, Key::Left);
    }
    press(&mut s, Key::Down);
    let items = s.candidate_items().unwrap_or_default();
    (s, last, items)
}

fn clean(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let es = engines(&m, &assets);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let hs: Vec<String> = es
            .iter()
            .map(|e| {
                let (mut s, mut ups) = type_unit(e, &r[1], &syl);
                ups.push(press(&mut s, Key::Enter));
                format!("{:016x}", hash_updates(&ups))
            })
            .collect();
        format!("{}\t{}\t{}", r[0], hs[0], hs[1])
    });
    write(&a[4], &out);
    println!("clean units {}", out.len());
}

fn cands(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let es = engines(&m, &assets);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let cursor: usize = r[3].parse().unwrap();
        let ls: Vec<String> = es
            .iter()
            .map(|e| {
                let (s, _, items) = open_at(e, &r[1], &syl, cursor);
                if s.candidate_items().is_none() {
                    return "-".into();
                }
                items.iter().map(|(t, n, d)| format!("{t}:{n}:{d}")).collect::<Vec<_>>().join("\u{1f}")
            })
            .collect();
        format!("{}\t{}\t{}", r[0], ls[0], ls[1])
    });
    write(&a[4], &out);
    println!("cand positions {}", out.len());
}

fn quality(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let es = engines(&m, &assets);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let cursor: usize = r[3].parse().unwrap();
        let mut j = serde_json::json!({"key": r[0]});
        let (_, u_off, off_items) = open_at(&es[0], &r[1], &syl, cursor);
        let (mut s_on, u_on, on_items) = open_at(&es[1], &r[1], &syl, cursor);
        let it = |v: &[(String, usize, String)]| v.iter().take(KEEP).map(|(t, n, d)| serde_json::json!([t, n, d])).collect::<Vec<_>>();
        j["off"] = serde_json::json!(chars(&u_off, syl.len()));
        j["on"] = serde_json::json!(chars(&u_on, syl.len()));
        j["off_c"] = serde_json::json!(u_off.committable);
        j["on_c"] = serde_json::json!(u_on.committable);
        j["same_typing"] = serde_json::json!(format!("{u_off:?}") == format!("{u_on:?}"));
        j["off_items"] = serde_json::json!(it(&off_items));
        j["on_items"] = serde_json::json!(it(&on_items));
        j["off_n"] = serde_json::json!(off_items.len());
        j["on_n"] = serde_json::json!(on_items.len());
        // the added items (in the on list, not in the off list) and their places
        let added: Vec<usize> = on_items.iter().enumerate().filter(|(_, x)| !off_items.contains(x)).map(|(i, _)| i).collect();
        j["added_at"] = serde_json::json!(added);
        if let Some(want) = r.get(4).filter(|x| !x.is_empty()) {
            let idx = if want == "*" { added.first().copied() } else { on_items.iter().position(|x| &x.2 == want) };
            if let Some(idx) = idx {
                let ps = es[1].config().page_size.max(1);
                for _ in 0..idx / ps {
                    press(&mut s_on, Key::PageDown);
                }
                let mut u = press(&mut s_on, Key::Digit((idx % ps + 1) as u8));
                if u.candidates_open {
                    u = press(&mut s_on, Key::Esc);
                }
                j["pick"] = serde_json::json!(on_items[idx].2);
                j["pick_out"] = serde_json::json!(chars(&u, syl.len()));
                j["pick_c"] = serde_json::json!(u.committable);
            }
        }
        j.to_string()
    });
    write(&a[4], &out);
    println!("quality rows {}", out.len());
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("acpop-show") if a.len() >= 5 => pop::show(&a),
        Some("acpop-expected") if a.len() >= 5 => pop::expected(&a),
        Some("acpop-p1") if a.len() >= 6 => pop::run(&a, false),
        Some("acpop") if a.len() >= 6 => pop::run(&a, true),
        Some("emoji") if a.len() >= 6 => emoji::run(&a),
        Some("emoji-keys") if a.len() >= 4 => emoji::keys(&a),
        Some("zyout") if a.len() >= 6 => zyout::run(&a),
        Some("clean") if a.len() >= 6 => clean(&a),
        Some("cands") if a.len() >= 6 => cands(&a),
        Some("quality") if a.len() >= 6 => quality(&a),
        Some("hints") if a.len() >= 6 => hints(&a),
        Some("autocomp") if a.len() >= 8 => autocomp(&a),
        Some("acdump") if a.len() >= 6 => acdump(&a),
        Some("aclist") if a.len() >= 6 => aclist(&a),
        Some("acbench") if a.len() >= 4 => acbench(&a),
        Some("vocab") if a.len() >= 4 => {
            // PRED: the conversion model's characters in id order, one per line
            let assets = load(&settings(&a[2]));
            let chars: String = assets.vocab_chars().iter().map(|c| format!("{c}\n")).collect();
            fs::write(&a[3], chars).expect("write vocab");
            println!("vocab chars {}", assets.vocab_chars().len());
        }
        _ => {
            eprintln!("usage: see the header of fuzzf2.rs");
            std::process::exit(2);
        }
    }
}
