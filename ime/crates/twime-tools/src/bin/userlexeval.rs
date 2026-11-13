//! USERLEX evaluation runner (USERLEX-plan 3): decodes units like `pneval detail` (same settings file, same
//! context handling, the product path `rebeam_ex` with the settings' PN), with an optional user lexicon and USERLEX rules.
//!
//!   userlexeval detail <settings> <units.tsv> <out.jsonl> <threads> <userlex.tsv|-> <force on|off> <bonus2>
//!   userlexeval cands  <settings> <units.tsv> <out.jsonl> <threads> <userlex.tsv|-> <force on|off> <bonus2>
//!
//! settings / units.tsv: as for pneval (paths relative to the repository root). userlex.tsv: `word<TAB>reading[<TAB>weight]`
//! (reading space separated, the user-lexicon file format). `detail` writes {id, group, gold, on, off (= on), fallback}
//! per unit (pneval's detail fields the score tools read); `cands` additionally checks that every candidate score at every
//! cursor is finite and writes {id, nonfinite} (functional check of USERLEX 1.6). Prints counts only, never text.
use std::{collections::HashMap, fs, io::Write, path::PathBuf, sync::Arc};
use twime_model::{Assets, Conversion, ModelPaths, NormCfg, pn::{self, PnCfg, PnLexicon}};

fn load(m: &HashMap<String, String>) -> Assets {
    let p = ModelPaths {
        data_txt: m["data_txt"].clone().into(),
        aliases: m["aliases"].clone().into(),
        model: m["model"].clone().into(),
        prior: m.get("prior").map(PathBuf::from),
    };
    let mut a = Assets::load(&p).expect("assets");
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
    a
}

struct Unit {
    id: String,
    group: String,
    ctx: String,
    syl: Vec<String>,
    gold: String,
}

fn units(p: &str) -> Vec<Unit> {
    fs::read_to_string(p)
        .expect("units")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Unit { id: f[0].into(), group: f[1].into(), ctx: f[2].into(), syl: f[3].split(' ').map(String::from).collect(), gold: f[4].into() }
        })
        .collect()
}

fn convert(a: &Assets, u: &Unit, lambda: f64) -> (Conversion, bool) {
    let style = twime_model::norm::style_of(&u.ctx).unwrap_or(false);
    let ctx: String = {
        let c: Vec<char> = u.ctx.chars().collect();
        c[c.len().saturating_sub(64)..].iter().collect()
    };
    (a.convert_ex(&ctx, &u.syl, &[], lambda, style), style)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 9 || !["detail", "cands"].contains(&a[1].as_str()) {
        eprintln!("usage: userlexeval detail|cands <settings> <units.tsv> <out.jsonl> <threads> <userlex.tsv|-> <force on|off> <bonus2>");
        std::process::exit(2);
    }
    let m = pn::read_settings(std::path::Path::new(&a[2])).expect("settings");
    let mut assets = load(&m);
    let force = match a[7].as_str() { "on" => true, "off" => false, _ => panic!("force must be on or off") };
    assets.set_user_rules(force, a[8].parse().expect("bonus2")).expect("rules");
    let (mut kept, mut rejected) = (0usize, 0usize);
    if a[6] != "-" {
        let mut words = vec![];
        for l in fs::read_to_string(&a[6]).expect("userlex").lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let f: Vec<&str> = l.split('\t').collect();
            let syl: Vec<String> = f[1].split(' ').map(String::from).collect();
            let w = f.get(2).map(|x| x.parse::<f64>().expect("weight"));
            match assets.user_word_opt(f[0], &syl, w) {
                Ok(x) => { words.push(x); kept += 1; }
                Err(_) => rejected += 1,
            }
        }
        assets.set_user(Some(Arc::new(assets.build_user_lexicon(words))));
    }
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let assets = Arc::new(assets);
    let us = Arc::new(units(&a[3]));
    let threads: usize = a[5].parse().unwrap();
    let cands = a[1] == "cands";
    let t0 = std::time::Instant::now();
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, us) = (assets.clone(), us.clone());
        hs.push(std::thread::spawn(move || {
            let mut out = vec![];
            for u in us.iter().skip(t).step_by(threads) {
                let (conv, style) = convert(&assets, u, lambda);
                let on = assets.rebeam_ex(&conv, &[], lambda, style);
                let s: String = on.chars.iter().map(|x| x.unwrap_or('\u{FFFD}')).collect();
                let mut j = serde_json::json!({"id": u.id, "group": u.group, "gold": u.gold, "on": s, "off": s, "fallback": conv.fallback});
                if cands {
                    let p = twime_model::CandParams { lambda, style_tai: style, ..Default::default() };
                    let mut bad = 0usize;
                    for end in 1..=u.syl.len() {
                        bad += assets.candidates(&on, end, &p).iter().filter(|c| !c.score.is_finite()).count();
                    }
                    j["nonfinite"] = serde_json::json!(bad);
                }
                out.push(j.to_string());
            }
            out
        }));
    }
    let mut f = std::io::BufWriter::new(fs::File::create(&a[4]).expect("out"));
    let mut n = 0;
    for h in hs {
        for l in h.join().unwrap() {
            writeln!(f, "{l}").unwrap();
            n += 1;
        }
    }
    println!("userlexeval {} units {n} userlex kept {kept} rejected {rejected} force {force} bonus2 {} sec {:.1}", a[1], a[8], t0.elapsed().as_secs_f64());
}
