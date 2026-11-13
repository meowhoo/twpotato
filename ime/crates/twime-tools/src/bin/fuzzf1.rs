//! FUZZY F1 evaluation driver (FUZZY-plan 3.7): drives real `Session`s key by key (product settings: PN, int8,
//! NORM, TA as the engine) with `repair` off / auto / suggest. Prints counts only; text stays in local output files.
//!
//!   fuzzf1 clean   <settings> <units.tsv> <out.tsv> <threads> <mode>   key, hash of every Update, final output, committable
//!   fuzzf1 cands   <settings> <pos.tsv>   <out.tsv> <threads> <mode>   key, every page of the list (shown), items (text:span)
//!   fuzzf1 quality <settings> <units.tsv> <out.jsonl> <threads>        off / auto / suggest outputs and the first two pages
//!   fuzzf1 select  <settings> <sel.tsv>   <out.tsv> <threads>          suggest: output after choosing item idx of the list
//!   fuzzf1 events  <settings> <units.tsv> <out.tsv> <threads> <mode>   fixed edit / choice / commit chain, hash of each step
//! units.tsv: key, context, syllables; pos.tsv: + cursor; quality units: + repaired position; sel.tsv: + position, item.
//! The same file builds against the source before F1 (`with_repair` / `cand_items` reduced to the off behaviour there).

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

fn settings(p: &str) -> HashMap<String, String> {
    pn::read_settings(std::path::Path::new(p)).expect("settings")
}

/// `pneval`'s load with the engine's TA default (on).
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
    a
}

// ---- the two places that differ in the build before F1 ---------------------------------------------------------------
fn with_repair(cfg: Config, mode: &str, a: &Assets, data_txt: &str) -> Config {
    let m = match mode {
        "off" => return cfg,
        "auto" => twime_core::RepairMode::Auto,
        "suggest" => twime_core::RepairMode::Suggest,
        _ => panic!("mode must be off, auto or suggest"),
    };
    Config { repair: Some(twime_core::Repair { mode: m, index: Arc::new(fuzzy::Index::from_data_txt(a, data_txt)) }), ..cfg }
}
fn cand_items(s: &Session) -> Option<Vec<(String, usize, String)>> {
    s.candidate_items()
}
// ----------------------------------------------------------------------------------------------------------------------

fn engine(m: &HashMap<String, String>, assets: &Arc<Assets>, mode: &str) -> Arc<Engine> {
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let data = fs::read_to_string(&m["data_txt"]).expect("data.txt");
    let cfg = with_repair(Config { lambda, candidates: true, ..Config::default() }, mode, assets, &data);
    Engine::new(assets.clone(), cfg)
}

fn keys_of(s: &str) -> Vec<Key> {
    fuzzy::keys_of(&fuzzy::Slots::parse(s).unwrap_or_else(|| panic!("unparsable syllable at a unit")))
}

fn press(s: &mut Session, k: Key) -> Update {
    s.on_key(KeyEvent::press(k))
}

/// A fresh session with the unit's context, every syllable typed; the Updates of every event.
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

/// One character per position (U+FFFD where unconverted) from the composition.
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

/// Runs `f` over the rows on `threads` threads; output lines in input order.
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

fn clean(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let e = engine(&m, &assets, &a[6]);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let (mut s, mut ups) = type_unit(&e, &r[1], &syl);
        let last = ups.last().unwrap().clone();
        let text = chars(&last, syl.len());
        ups.push(press(&mut s, Key::Enter));
        format!("{}\t{:016x}\t{}\t{}", r[0], hash_updates(&ups), text, u8::from(last.committable))
    });
    write(&a[4], &out);
    println!("clean units {} mode {}", out.len(), a[6]);
}

fn cands(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let e = engine(&m, &assets, &a[6]);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let cursor: usize = r[3].parse().unwrap();
        let (mut s, _) = type_unit(&e, &r[1], &syl);
        for _ in cursor..syl.len() {
            press(&mut s, Key::Left);
        }
        let mut u = press(&mut s, Key::Down);
        let internal = cand_items(&s).map_or("-".into(), |v| v.iter().map(|(t, n, d)| format!("{t}:{n}:{d}")).collect::<Vec<_>>().join("\u{1f}"));
        let mut pages = vec![];
        while let Some(c) = u.candidates.clone() {
            pages.push(c.items.join("\u{1f}"));
            u = press(&mut s, Key::PageDown);
            if u.candidates.as_ref().is_none_or(|x| x.page == 0) || pages.len() > 40 {
                break;
            }
        }
        format!("{}\t{}\t{}", r[0], pages.join("\u{1e}"), internal)
    });
    write(&a[4], &out);
    println!("cand positions {} mode {}", out.len(), a[6]);
}

fn quality(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let es: Vec<Arc<Engine>> = ["off", "auto", "suggest"].iter().map(|x| engine(&m, &assets, x)).collect();
    let ps = es[0].config().page_size.max(1);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let pos: usize = r[3].parse().unwrap();
        let mut j = serde_json::json!({"key": r[0]});
        for (e, name) in es.iter().zip(["off", "auto", "sug"]) {
            let (mut s, ups) = type_unit(e, &r[1], &syl);
            let last = ups.last().unwrap();
            j[name] = serde_json::json!(chars(last, syl.len()));
            j[format!("{name}_c")] = serde_json::json!(last.committable);
            if name != "off" {
                for _ in pos + 1..syl.len() {
                    press(&mut s, Key::Left);
                }
                press(&mut s, Key::Down);
                let items = cand_items(&s).unwrap_or_default();
                j[format!("{name}_items")] = serde_json::json!(items.iter().take(2 * ps).map(|(t, n, d)| serde_json::json!([t, n, d])).collect::<Vec<_>>());
                j[format!("{name}_n")] = serde_json::json!(items.len());
            }
        }
        j.to_string()
    });
    write(&a[4], &out);
    println!("quality units {}", out.len());
}

fn select(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let e = engine(&m, &assets, "suggest");
    let ps = e.config().page_size.max(1);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let pos: usize = r[3].parse().unwrap();
        let idx: usize = r[4].parse().unwrap();
        let (mut s, _) = type_unit(&e, &r[1], &syl);
        for _ in pos + 1..syl.len() {
            press(&mut s, Key::Left);
        }
        press(&mut s, Key::Down);
        for _ in 0..idx / ps {
            press(&mut s, Key::PageDown);
        }
        let mut u = press(&mut s, Key::Digit((idx % ps + 1) as u8));
        // the composition after the choice (cursor moves do not change it)
        if u.candidates_open {
            u = press(&mut s, Key::Esc);
        }
        format!("{}\t{}\t{}", r[0], chars(&u, syl.len()), u8::from(u.committable))
    });
    write(&a[4], &out);
    println!("selected units {}", out.len());
}

/// Gate 1(d): a fixed event chain over each clean unit (edits, paging, choices, insert / delete, commit), on a version 1/2
/// session and on a version 3 session; the hash of every step's Update.
fn events(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let e = engine(&m, &assets, &a[6]);
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let mut steps = vec![];
        // gate 1(d) is about chains without illegal syllables: flag a chain where any composition shows an unconverted
        // (illegal) syllable (a digit typed as a bopomofo key when no list opened, ...)
        let mut illegal = false;
        for ux in [false, true] {
            let mut s = e.new_session();
            s.set_ux(ux);
            let mut ups = vec![s.handle(1, None, Event::SetContext(r[1].clone()))];
            let typ = |s: &mut Session, x: &str, ups: &mut Vec<Update>| {
                for k in keys_of(x) {
                    ups.push(press(s, k));
                }
            };
            for x in &syl {
                typ(&mut s, x, &mut ups);
            }
            for k in [Key::Down, Key::Down, Key::PageDown, Key::Digit(2), Key::Left, Key::Left, Key::Down, Key::Digit(1), Key::Home] {
                ups.push(press(&mut s, k));
            }
            typ(&mut s, &syl[0], &mut ups);
            for k in [Key::Right, Key::Backspace, Key::End, Key::Left, Key::Delete, Key::Space, Key::Esc, Key::Down, Key::Right, Key::Enter, Key::Enter] {
                ups.push(press(&mut s, k));
            }
            typ(&mut s, &syl[syl.len() - 1], &mut ups);
            ups.push(press(&mut s, Key::Esc));
            illegal |= ups.iter().any(|u| u.composition.as_ref().is_some_and(|c| c.segments.iter().any(|g| g.unconverted)));
            steps.extend(ups.iter().map(|u| format!("{:016x}", hash_updates(std::slice::from_ref(u)))));
        }
        format!("{}\t{}\t{}", r[0], steps.join(","), u8::from(illegal))
    });
    write(&a[4], &out);
    println!("event chains {} mode {}", out.len(), a[6]);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("events") if a.len() >= 7 => events(&a),
        Some("clean") if a.len() >= 7 => clean(&a),
        Some("cands") if a.len() >= 7 => cands(&a),
        Some("quality") if a.len() >= 6 => quality(&a),
        Some("select") if a.len() >= 6 => select(&a),
        _ => {
            eprintln!("usage: see the header of fuzzf1.rs");
            std::process::exit(2);
        }
    }
}
