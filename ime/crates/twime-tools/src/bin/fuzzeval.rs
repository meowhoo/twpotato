//! FUZZY F0 (FUZZY-plan 2): confusion-class export and decoding of (possibly error-injected) units. Prints
//! counts only; unit text stays in the input files and the per-unit outputs it writes (local, under data/fuzzy).
//!
//! Usage:
//!   fuzzeval classes <settings> <out.json>                          -> classes, neighbour table, forward index, R(t)
//!   fuzzeval decode  <settings> <units.tsv> <out.tsv> <threads>     -> key, output (U+FFFD where no character), complete
//! settings: the deployment's key=value file (data_txt, aliases, model, prior, lambda, norm, lexicon, b2, b3, h, cands,
//! int8), loaded exactly as `pneval` loads it. units.tsv: key, context, syllables (space separated).

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::PathBuf,
    sync::Arc,
};

use twime_core::fuzzy::{self, CLASSES, FAMILIES, Index};
use twime_model::{
    Assets, ModelPaths, NormCfg,
    pn::{self, PnCfg, PnLexicon},
};

fn settings(p: &str) -> HashMap<String, String> {
    pn::read_settings(std::path::Path::new(p)).expect("settings")
}

fn paths(m: &HashMap<String, String>) -> ModelPaths {
    ModelPaths {
        data_txt: m["data_txt"].clone().into(),
        aliases: m["aliases"].clone().into(),
        model: m["model"].clone().into(),
        prior: m.get("prior").map(PathBuf::from),
    }
}

/// Same as `pneval`'s `load(m, true)`.
fn load(m: &HashMap<String, String>) -> Assets {
    let mut a = Assets::load(&paths(m)).expect("assets");
    match m.get("int8").map(String::as_str) {
        Some("on") => a.set_int8().expect("int8"),
        None | Some("off") => {}
        _ => panic!("int8 must be on or off"),
    }
    if m.get("norm").map_or("all", String::as_str) == "all" {
        a.set_norm(NormCfg::ALL);
    }
    if m.get("ta_default").is_some_and(|v| v == "on") {
        a.set_ta_default(true);
    }
    let lex = PnLexicon::open(std::path::Path::new(&m["lexicon"]), &a).expect("lexicon");
    let cfg = PnCfg::from_settings(m).expect("pn settings");
    a.set_pn(Some(Arc::new(lex)), cfg);
    if let Some(c) = m.get("cand").filter(|c| !c.is_empty() && *c != "off") {
        let mm = m.get("cand_m").map(|x| x.parse::<usize>().expect("cand_m"));
        let cfg = twime_model::cand::CandCfg::open(std::path::Path::new(c), &a, mm).expect("cand");
        a.set_cand(Some(Arc::new(cfg)));
    }
    if let Some(g) = m.get("gen").filter(|g| !g.is_empty() && *g != "off") {
        let lex = PnLexicon::open(std::path::Path::new(g), &a).expect("gen lexicon");
        a.set_gen(Some(Arc::new(lex)));
    }
    a
}

fn classes(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Assets::load_dictionary_only(&paths(&m)).expect("dictionary");
    let ix = Index::from_data_txt(&assets, &fs::read_to_string(&m["data_txt"]).expect("data.txt"));
    let sym = |k| twime_core::keymap::symbol(k).unwrap().to_string();
    let neighbours: BTreeMap<String, Vec<String>> =
        fuzzy::layout_keys().into_iter().map(|k| (sym(k), fuzzy::neighbours(k).into_iter().map(sym).collect())).collect();
    let mut fwd = BTreeMap::new();
    let mut n_fwd = [0usize; 12];
    for (g, per) in &ix.fwd {
        let mut e = BTreeMap::new();
        for (ci, ts) in per.iter().enumerate() {
            n_fwd[ci] += ts.len();
            if !ts.is_empty() {
                e.insert(CLASSES[ci].code(), ts.clone());
            }
        }
        fwd.insert(g.clone(), e);
    }
    let rev: BTreeMap<&String, Vec<[&str; 2]>> =
        ix.rev.iter().map(|(t, v)| (t, v.iter().map(|r| [r.syl.as_str(), r.class.code()]).collect())).collect();
    let single: BTreeMap<&String, Option<f64>> = ix.single.iter().map(|(k, v)| (k, *v)).collect();
    let illegal = ix.rev.keys().filter(|t| !assets.is_legal_syllable(t)).count();
    let j = serde_json::json!({
        "plan": "FUZZY-plan v6 section 1",
        "classes": CLASSES.iter().map(|c| serde_json::json!({"code": c.code(), "family": FAMILIES[c.family()], "cost": c.cost()})).collect::<Vec<_>>(),
        "family_order": FAMILIES,
        "max_repairs": fuzzy::MAX_REPAIRS,
        "neighbours": neighbours,
        "legal_syllables": ix.fwd.len(),
        "k_excluded": {"none_completed": ix.k_excluded[0], "completed_early": ix.k_excluded[1], "several": ix.k_excluded[2]},
        "fwd": fwd,
        "rev": rev,
        "single_log10": single,
    });
    fs::write(&a[3], serde_json::to_string(&j).unwrap()).expect("write");
    println!(
        "legal {} typed {} (illegal {illegal}) fwd per class {:?} k_excluded {:?}",
        ix.fwd.len(),
        ix.rev.len(),
        CLASSES.iter().zip(n_fwd).map(|(c, n)| format!("{}={n}", c.code())).collect::<Vec<_>>(),
        ix.k_excluded
    );
}

struct Unit {
    key: String,
    ctx: String,
    syl: Vec<String>,
}

fn units(p: &str) -> Vec<Unit> {
    fs::read_to_string(p)
        .expect("units")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Unit { key: f[0].into(), ctx: f[1].into(), syl: f[2].split(' ').map(String::from).collect() }
        })
        .collect()
}

fn decode(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let us = Arc::new(units(&a[3]));
    let threads: usize = a[5].parse().unwrap();
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, us) = (assets.clone(), us.clone());
        hs.push(std::thread::spawn(move || {
            let mut out = vec![];
            for (i, u) in us.iter().enumerate().skip(t).step_by(threads) {
                // as pneval: the last 64 context characters, the 台/臺 style of the context
                let style = twime_model::norm::style_of(&u.ctx).unwrap_or(false);
                let c: Vec<char> = u.ctx.chars().collect();
                let ctx: String = c[c.len().saturating_sub(64)..].iter().collect();
                let conv = assets.convert_ex(&ctx, &u.syl, &[], lambda, style);
                let s: String = conv.chars.iter().map(|x| x.unwrap_or('\u{FFFD}')).collect();
                out.push((i, format!("{}\t{}\t{}", u.key, s, u8::from(conv.complete()))));
            }
            out
        }));
    }
    let mut all: Vec<(usize, String)> = hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
    all.sort_by_key(|x| x.0);
    let mut f = std::io::BufWriter::new(fs::File::create(&a[4]).unwrap());
    let mut incomplete = 0;
    for (_, l) in &all {
        incomplete += usize::from(l.ends_with("\t0"));
        writeln!(f, "{l}").unwrap();
    }
    println!("decoded {} incomplete {incomplete}", all.len());
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("classes") if a.len() >= 4 => classes(&a),
        Some("decode") if a.len() >= 6 => decode(&a),
        _ => {
            eprintln!("usage: see the header of fuzzeval.rs");
            std::process::exit(2);
        }
    }
}
