//! PN evaluation: lexicon build, grid of bonus settings, per-unit detail for the
//! Python equivalence check and the diagnostics. Prints counts only; unit text stays in the files it was read from and
//! the outputs it writes (local, under data/pn/work).
//!
//! Usage:
//!   pneval build  <settings>                                         -> <lexicon>.fst / .lst from <lexicon>.tsv
//!   pneval grid   <settings> <units.tsv> <configs.txt> <out.tsv> <threads>
//!   pneval detail <settings> <units.tsv> <out.jsonl> <threads> [ids.txt] [scores]
//!   pneval gencheck <settings> <units.tsv> <threads>                 (SRCH mode 0: conversions equal with / without `gen`)
//!   pneval gensel   <settings> <units.tsv> <out.tsv> <threads>       (SRCH mode 0: gold word in the candidate list)
//! `gen=<lexicon base>` in the settings adds the general-word layer.
//! `cand=<export base>` [`cand_m=<M>`] switches CAND on: `raw_off` = C, `raw` = C + PN.
//! `int8=on|off` selects G1 W8A8 inference (default off; other values are errors).
//! SHORT-PRIOR (SHORT-PRIOR-plan 1): `lambda_short_c`, `lambda_short0`, `word_short_bonus`, `prior_ctx` as in the
//! engine's model.cfg (absent = off); `word_short_table=<path>` the T2 word weights.
//!   pneval spgrid <settings> <units.tsv> <configs.txt> <out.tsv> <threads>
//!   pneval genjgrid <settings> <units.tsv> <configs.txt> <out.tsv> <threads>
//!   pneval genjdump <settings> <units.tsv> <configs.txt> <out.jsonl> <threads>
//! SRCH-JUMP: genj=<base> loads .fst/.lst/.tier/.cfg; optional complete settings override:
//! b3_a/b3_b/b3_c/b3_d (off or nonnegative finite bonus), genj_h (inf or positive count).
//! configs: a b c d h; names c0, c1, ... . genjgrid writes #cfg, all ref rows without J, then differing rows per config.
//! genjdump writes vocabulary/legal metadata, then numeric constraints/logits and the merged jumps per unit/config.
//! genj_build=on selects the J TSV builder (tier + jump assertions); ordinary builds retain their existing behavior.
//!     one inference per unit, then the product rebeam with each SHORT-PRIOR config (`name c l0|- wb ctx(on|off) table(0|1)`
//!     per line); out.tsv: `ref<TAB>id<TAB>output` (settings without SHORT-PRIOR, table off) for every unit, then
//!     `<name><TAB>id<TAB>output` only where a config's output differs from ref, after one
//!     `#cfg<TAB><name><TAB>c<TAB>l0|-<TAB>wb<TAB>ctx<TAB>table` line per config run (parsed values). Text stays in the local file.
//! settings: key=value (data_txt, aliases, model, prior, lambda, norm=all|off, lexicon, b2, b3, h, cands), paths relative
//! to the repository root (the working directory). units.tsv: id, group, context, syllables (space separated), gold.
//! configs.txt: one `b2 b3 h` per line (off / number, inf / count). The grid compares every config with PN off on the
//! same inference (the conversion of the full syllable sequence, as committed by the typist in oracle-context mode).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Write,
    path::PathBuf,
    sync::Arc,
};

use twime_model::{
    Assets, Conversion, ModelPaths, NormCfg,
    pn::{self, PnCfg, PnLexicon},
};

fn settings(p: &str) -> HashMap<String, String> {
    pn::read_settings(std::path::Path::new(p)).expect("settings")
}

fn load(m: &HashMap<String, String>, with_lex: bool) -> Assets {
    let p = ModelPaths {
        data_txt: m["data_txt"].clone().into(),
        aliases: m["aliases"].clone().into(),
        model: m["model"].clone().into(),
        prior: m.get("prior").map(PathBuf::from),
    };
    let mut a = Assets::load(&p).expect("assets");
    match m.get("int8").map(String::as_str) {
        Some("on") => a.set_int8().expect("int8"),
        None | Some("off") => {},
        _ => panic!("int8 must be on or off"),
    }
    if m.get("norm").map_or("all", String::as_str) == "all" {
        a.set_norm(NormCfg::ALL);
    }
    // NORM-MO: `norm_mo=on|off` (default on)
    match m.get("norm_mo").map(String::as_str) {
        None | Some("on") => a.set_mo_norm(true),
        Some("off") => a.set_mo_norm(false),
        _ => panic!("norm_mo must be on or off"),
    }
    // TA: only an explicit `ta_default=on` switches the ㄊㄚ default on
    if m.get("ta_default").is_some_and(|v| v == "on") {
        a.set_ta_default(true);
    }
    if with_lex && m.get("lexicon").is_some_and(|v| v != "off") {
        let lex = PnLexicon::open(std::path::Path::new(&m["lexicon"]), &a).expect("lexicon");
        let cfg = PnCfg::from_settings(m).expect("pn settings");
        a.set_pn(Some(Arc::new(lex)), cfg);
    }
    // CAND: `cand=<export base>` (.clex / .cemb next to it), optional `cand_m=<M>`
    if let Some(c) = m.get("cand").filter(|c| !c.is_empty() && *c != "off") {
        let mm = m.get("cand_m").map(|x| x.parse::<usize>().expect("cand_m"));
        let cfg = twime_model::cand::CandCfg::open(std::path::Path::new(c), &a, mm).expect("cand");
        a.set_cand(Some(Arc::new(cfg)));
    }
    let short = twime_model::prior::ShortCfg::parse(&|k| m.get(k).cloned()).expect("SHORT-PRIOR settings");
    a.set_short(short).expect("SHORT-PRIOR settings");
    if let Some(t) = m.get("word_short_table").filter(|t| !t.is_empty()) {
        a.set_short_table(Some(Assets::load_short_table(std::path::Path::new(t)).expect("word_short_table")));
    }
    if let Some(g) = m.get("gen").filter(|g| !g.is_empty() && *g != "off") {
        let lex = PnLexicon::open(std::path::Path::new(g), &a).expect("gen lexicon");
        a.set_gen(Some(Arc::new(lex)));
    }
    if let Some(g) = m.get("genj").filter(|g| !g.is_empty() && *g != "off") {
        let mut j = pn::genj::open(std::path::Path::new(g), &a).expect("genj assets");
        if m.keys().any(|k| ["b3_a", "b3_b", "b3_c", "b3_d"].contains(&k.as_str())) || m.contains_key("genj_h") {
            let cfg = pn::genj::GenjCfg::parse(m, "genj_h").expect("genj settings");
            if let Some(j) = &mut j { j.1 = cfg; }
        }
        a.set_genj(j);
    }
    // WORDBONUS: `wordbonus=<path of wordbonus.bin>` (unset / empty = off); a missing or mismatching file is an error.
    if let Some(w) = m.get("wordbonus").filter(|w| !w.is_empty()) {
        let t = twime_model::wordbonus::WordBonus::open(&a, std::path::Path::new(w)).expect("wordbonus");
        a.set_wordbonus(Some(Arc::new(t)));
    }
    let user_force = match m.get("user_force").map(String::as_str) {
        None | Some("off") => false,
        Some("on") => true,
        _ => panic!("user_force"),
    };
    let user_bonus = m.get("user_bonus2").map_or(10.0, |v| v.parse().expect("user_bonus2"));
    a.set_user_rules(user_force, user_bonus).expect("user rules");
    a.set_lexux(twime_model::lexux::Config::parse(m).expect("LEXUX settings")).expect("LEXUX settings");
    a
}

const CLASSES: [&str; 9] = ["人名", "地名", "機構團體", "事件與其他專名", "作品", "品牌商品", "店家品牌", "科技專業", "醫藥生物"];

fn build(a: &[String]) {
    let m = settings(&a[2]);
    let mut bm = m.clone(); bm.remove("genj");
    let assets = load(&bm, false);
    let tsv = fs::read_to_string(format!("{}.tsv", m["lexicon"])).expect("lexicon tsv");
    let mut lines = tsv.lines();
    let hdr: Vec<&str> = lines.next().unwrap().split('\t').collect();
    let col = |n: &str| hdr.iter().position(|h| *h == n).unwrap_or_else(|| panic!("column {n}"));
    let (cw, cr) = (col("word"), col("reading"));
    // Opt in explicitly: existing general-word builds also have a tier column and may include 2-character words.
    let tier = if m.get("genj_build").is_some_and(|v| v == "on") { Some(col("tier")) } else { None };
    let (cc, cl) = if tier.is_none() { (col("class"), col("layer")) } else { (0,0) };
    let mut words = vec![];
    let mut unknown_class = 0;
    for l in lines {
        let f: Vec<&str> = l.split('\t').collect();
        if let Some(t) = tier {
            assert!(f[col("jump")] == "1" && f[cw].chars().count() >= 3, "invalid J row");
            let class = ["甲", "乙", "丙", "丁"].iter().position(|v| *v == f[t]).expect("invalid tier") as u8;
            words.push(pn::BuildWord { word:f[cw].into(), reading:f[cr].into(), class });
            continue;
        }
        let class = CLASSES.iter().position(|c| *c == f[cc]).unwrap_or_else(|| {
            unknown_class += 1;
            15
        }) as u8;
        let layer: u8 = match f[cl] {
            "L1" => 0,
            "L2a" => 1,
            "L2b" => 2,
            _ => 3,
        };
        words.push(pn::BuildWord { word: f[cw].to_string(), reading: f[cr].to_string(), class: (layer << 4) | class });
    }
    let builder = if tier.is_some() { pn::build_j } else { pn::build };
    let st = builder(&assets, &words, std::path::Path::new(&m["lexicon"])).expect("build");
    if tier.is_some() {
        println!("genj build {st:?}");
        assert_eq!(st.kept, words.len(), "J build must not drop rows");
        let b = fs::read(format!("{}.lst", m["lexicon"])).unwrap();
        let (mut p, mut tiers) = (28usize, vec![]);
        while p < b.len() { let n=b[p]; p+=1; for _ in 0..n { tiers.push(b[p+1]); p+=2+2*b[p] as usize; } }
        fs::write(format!("{}.tier", m["lexicon"]), tiers).unwrap();
    }
    let lex = PnLexicon::open(std::path::Path::new(&m["lexicon"]), &assets).expect("reopen");
    println!("input rows {} unknown_class {unknown_class} {st:?} file_bytes {}", words.len(), lex.file_bytes());
}

struct Unit {
    id: String,
    group: String,
    ctx: String,
    syl: Vec<String>,
    gold: Vec<char>,
    /// Optional sixth column `pos:char,...` (equivalence fixed cases).
    locks: Vec<Option<char>>,
}

fn units(p: &str) -> Vec<Unit> {
    fs::read_to_string(p)
        .expect("units")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let syl: Vec<String> = f[3].split(' ').map(String::from).collect();
            let mut locks = vec![None; syl.len()];
            for item in f.get(5).map_or("", |x| *x).split(',').filter(|x| !x.is_empty()) {
                let (p, c) = item.split_once(':').unwrap();
                locks[p.parse::<usize>().unwrap()] = c.chars().next();
            }
            Unit { id: f[0].into(), group: f[1].into(), ctx: f[2].into(), syl, gold: f[4].chars().collect(), locks }
        })
        .collect()
}

fn fold(c: char) -> char {
    if c == '臺' { '台' } else { c }
}

fn lev(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(x != y)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[derive(Default, Clone)]
struct Stats {
    units: u64,
    chars: u64,
    err_off: u64,
    err_on: u64,
    rescued: u64,
    broken: u64,
    changed: u64,
    ok_off: u64,
    ok_on: u64,
    lev_off: u64,
    lev_on: u64,
}

fn parse_cfg(line: &str) -> PnCfg {
    let f: Vec<&str> = line.split_whitespace().collect();
    PnCfg { b2: pn::parse_bonus(f[0]).unwrap(), b3: pn::parse_bonus(f[1]).unwrap(), h: pn::parse_h(f[2]).unwrap(), cands: true }
}

fn convert(assets: &Assets, u: &Unit, lambda: f64) -> (Conversion, bool) {
    let style = twime_model::norm::style_of(&u.ctx).unwrap_or(false);
    let ctx: String = { let c: Vec<char> = u.ctx.chars().collect(); c[c.len().saturating_sub(64)..].iter().collect() };
    (assets.convert_ex(&ctx, &u.syl, &u.locks, lambda, style), style)
}

fn grid(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m, true));
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let us = Arc::new(units(&a[3]));
    let cfgs: Arc<Vec<PnCfg>> = Arc::new(fs::read_to_string(&a[4]).unwrap().lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(parse_cfg).collect());
    let threads: usize = a[6].parse().unwrap();
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, us, cfgs) = (assets.clone(), us.clone(), cfgs.clone());
        hs.push(std::thread::spawn(move || {
            let mut acc: HashMap<(usize, String), Stats> = HashMap::new();
            let mut skipped = 0u64;
            for u in us.iter().skip(t).step_by(threads) {
                if u.gold.len() != u.syl.len() {
                    skipped += 1;
                    continue;
                }
                let (conv, style) = convert(&assets, u, lambda);
                let off = assets.rebeam_with(&conv, &[], lambda, style, None, true);
                let gold: Vec<char> = u.gold.iter().map(|&c| fold(c)).collect();
                let off_c: Vec<char> = off.chars.iter().map(|c| fold(c.unwrap_or('\u{FFFD}'))).collect();
                for (k, cfg) in cfgs.iter().enumerate() {
                    let on = assets.rebeam_with(&conv, &[], lambda, style, Some(cfg), true);
                    let on_c: Vec<char> = on.chars.iter().map(|c| fold(c.unwrap_or('\u{FFFD}'))).collect();
                    let s = acc.entry((k, u.group.clone())).or_default();
                    s.units += 1;
                    s.chars += gold.len() as u64;
                    for i in 0..gold.len() {
                        let (a0, a1) = (off_c[i] == gold[i], on_c[i] == gold[i]);
                        s.err_off += u64::from(!a0);
                        s.err_on += u64::from(!a1);
                        s.rescued += u64::from(!a0 && a1);
                        s.broken += u64::from(a0 && !a1);
                        s.changed += u64::from(off_c[i] != on_c[i]);
                    }
                    s.ok_off += u64::from(off_c == gold);
                    s.ok_on += u64::from(on_c == gold);
                    s.lev_off += lev(&off_c, &gold) as u64;
                    s.lev_on += lev(&on_c, &gold) as u64;
                }
            }
            (acc, skipped)
        }));
    }
    let mut all: BTreeMap<(usize, String), Stats> = BTreeMap::new();
    let mut skipped = 0;
    for h in hs {
        let (acc, sk) = h.join().unwrap();
        skipped += sk;
        for (k, v) in acc {
            let e = all.entry(k).or_default();
            e.units += v.units;
            e.chars += v.chars;
            e.err_off += v.err_off;
            e.err_on += v.err_on;
            e.rescued += v.rescued;
            e.broken += v.broken;
            e.changed += v.changed;
            e.ok_off += v.ok_off;
            e.ok_on += v.ok_on;
            e.lev_off += v.lev_off;
            e.lev_on += v.lev_on;
        }
    }
    let mut f = std::io::BufWriter::new(fs::File::create(&a[5]).unwrap());
    writeln!(f, "cfg\tgroup\tunits\tchars\terr_off\terr_on\trescued\tbroken\tchanged\tok_off\tok_on\tlev_off\tlev_on").unwrap();
    for ((k, g), s) in &all {
        writeln!(f, "{k}\t{g}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", s.units, s.chars, s.err_off, s.err_on, s.rescued, s.broken, s.changed, s.ok_off, s.ok_on, s.lev_off, s.lev_on).unwrap();
    }
    println!("units {} skipped(len) {skipped} configs {}", us.len(), cfgs.len());
}

/// Per unit: raw (unfolded) PN-on and PN-off ids, folded outputs, and every hit whose characters equal the gold text at
/// its span (diagnostics: length, class/layer code, kept under each H, gold characters outside the top 8, score gap).
fn detail(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m, true));
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let mut us = units(&a[3]);
    if let Some(p) = a.get(6).filter(|p| *p != "-") {
        let keep: HashSet<String> = fs::read_to_string(p).unwrap().lines().map(String::from).collect();
        us.retain(|u| keep.contains(&u.id));
    }
    let with_scores = a.get(7).is_some_and(|x| x == "scores");
    let us = Arc::new(us);
    let threads: usize = a[5].parse().unwrap();
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, us) = (assets.clone(), us.clone());
        hs.push(std::thread::spawn(move || {
            let mut out = vec![];
            for u in us.iter().skip(t).step_by(threads) {
                let (conv, style) = convert(&assets, u, lambda);
                let cfg = &assets.pn().unwrap().1;
                let lk = &u.locks;
                let raw_on = assets.rebeam_with(&conv, lk, lambda, style, Some(cfg), false);
                let raw_off = assets.rebeam_with(&conv, lk, lambda, style, None, false);
                let on = assets.rebeam_with(&conv, lk, lambda, style, Some(cfg), true);
                let off = assets.rebeam_with(&conv, lk, lambda, style, None, true);
                // the product path (rebeam_ex with the settings' PN) must equal the explicit-settings path
                assert_eq!(on.chars, assets.rebeam_ex(&conv, lk, lambda, style).chars);
                let ids = |c: &Conversion| -> Option<Vec<u32>> { c.chars.iter().map(|x| x.map(|ch| assets.vocab_id(ch))).collect() };
                let s = |c: &Conversion| -> String { c.chars.iter().map(|x| x.unwrap_or('\u{FFFD}')).collect() };
                let mut j = serde_json::json!({"id": u.id, "group": u.group, "raw": ids(&raw_on), "raw_off": ids(&raw_off), "on": s(&on), "off": s(&off), "fallback": conv.fallback});
                if let Some(d) = assets.pn_diag(&conv, &u.gold, lambda) {
                    j["diag"] = serde_json::json!(d.iter().map(|g| serde_json::json!([g.start, g.len, g.class, g.rank, g.group, g.out8, g.gap])).collect::<Vec<_>>());
                }
                if let Some(r) = ids(&raw_on) {
                    j["applied"] = serde_json::json!(assets.pn_matches(&u.syl, &r).iter().map(|(s, l, c)| [*s, *l, *c as usize]).collect::<Vec<_>>());
                }
                if let Some(r) = ids(&raw_off) {
                    j["present_off"] = serde_json::json!(assets.pn_matches(&u.syl, &r).iter().map(|(s, l, c)| [*s, *l, *c as usize]).collect::<Vec<_>>());
                }
                j["gold"] = serde_json::json!(u.gold.iter().collect::<String>());
                if with_scores {
                    j["syl"] = serde_json::json!(u.syl);
                    j["locked"] = serde_json::json!(u.locks.iter().enumerate().map(|(i, c)| c.and_then(|c| { let id = assets.vocab_id(c); assets.is_legal_char(&u.syl[i], c).then_some(id) })).collect::<Vec<_>>());
                    j["scores"] = serde_json::json!(assets.legal_scores(&conv));
                }
                out.push(j.to_string());
            }
            out
        }));
    }
    let mut f = std::io::BufWriter::new(fs::File::create(&a[4]).unwrap());
    let mut n = 0;
    for h in hs {
        for l in h.join().unwrap() {
            writeln!(f, "{l}").unwrap();
            n += 1;
        }
    }
    println!("detail units {n}");
}

/// SRCH mode 0 acceptance 1: every unit's conversion (PN off and PN on paths) is the same with and without the layer.
fn gencheck(a: &[String]) {
    let m = settings(&a[2]);
    assert!(m.contains_key("gen"), "settings need gen=");
    let with = Arc::new(load(&m, true));
    let mut m0 = m.clone();
    m0.remove("gen");
    let without = Arc::new(load(&m0, true));
    assert!(with.genw().is_some() && without.genw().is_none());
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let us = Arc::new(units(&a[3]));
    let threads: usize = a[4].parse().unwrap();
    assert!((1..=6).contains(&threads), "threads must be 1..6");
    let mut hs = vec![];
    for t in 0..threads {
        let (w, o, us) = (with.clone(), without.clone(), us.clone());
        hs.push(std::thread::spawn(move || {
            let (mut n, mut diff) = (0u64, [0u64; 4]);
            for u in us.iter().skip(t).step_by(threads) {
                let (cw, sw) = convert(&w, u, lambda);
                let (co, so) = convert(&o, u, lambda);
                let cfg = w.pn().map(|p| p.1.clone());
                let same = [cw.chars == co.chars,
                    w.rebeam_with(&cw, &u.locks, lambda, sw, None, true).chars == o.rebeam_with(&co, &u.locks, lambda, so, None, true).chars,
                    w.rebeam_with(&cw, &u.locks, lambda, sw, cfg.as_ref(), true).chars == o.rebeam_with(&co, &u.locks, lambda, so, cfg.as_ref(), true).chars,
                    w.rebeam_ex(&cw, &u.locks, lambda, sw).chars == o.rebeam_ex(&co, &u.locks, lambda, so).chars];
                n += 1;
                for (d, equal) in diff.iter_mut().zip(same) { *d += u64::from(!equal); }
            }
            (n, diff)
        }));
    }
    let (mut n, mut diff) = (0, [0u64; 4]);
    for h in hs {
        let (x, y) = h.join().unwrap();
        n += x;
        for (d, v) in diff.iter_mut().zip(y) { *d += v; }
    }
    for (name, d) in ["convert", "pn_off", "pn_on", "product"].into_iter().zip(diff) {
        println!("gencheck path {name} units {n} differing {d}");
    }
    assert_eq!(diff, [0; 4], "the general-word layer changed an automatic conversion");
}

/// SRCH mode 0 acceptance 2: each unit (one lexicon word, its reading, no context) converted alone; is the gold word
/// in the candidate list of the full span (product parameters: beta 0, word_cap 40, max_span 6)? Writes id, rank or -,
/// list length, and whether the conversion itself already is the word.
fn gensel(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m, true));
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let us = Arc::new(units(&a[3]));
    let threads: usize = a[5].parse().unwrap();
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, us) = (assets.clone(), us.clone());
        hs.push(std::thread::spawn(move || {
            let mut out = vec![];
            for u in us.iter().skip(t).step_by(threads) {
                let (conv, style) = convert(&assets, u, lambda);
                let conv = assets.rebeam_ex(&conv, &u.locks, lambda, style);
                let n = u.syl.len();
                let p = twime_model::CandParams { lambda, beta: 0.0, max_span: 6, word_cap: 40, style_tai: style };
                let list = assets.candidates(&conv, n, &p);
                let gold: Vec<char> = u.gold.iter().map(|&c| fold(c)).collect();
                let rank = list.iter().position(|c| c.span == n && c.text.chars().map(fold).eq(gold.iter().copied())).map(|i| i + 1);
                let first = conv.chars.iter().map(|c| fold(c.unwrap_or('\u{FFFD}'))).eq(gold.iter().copied());
                out.push(format!("{}\t{}\t{}\t{}", u.id, rank.map_or("-".into(), |r| r.to_string()), list.len(), u8::from(first)));
            }
            out
        }));
    }
    let mut f = std::io::BufWriter::new(fs::File::create(&a[4]).unwrap());
    writeln!(f, "id\trank\tlist_len\tfirst").unwrap();
    let (mut n, mut found) = (0, 0);
    for h in hs {
        for l in h.join().unwrap() {
            n += 1;
            found += usize::from(l.split('\t').nth(1) != Some("-"));
            writeln!(f, "{l}").unwrap();
        }
    }
    println!("gensel units {n} in list {found}");
}

/// SHORT-PRIOR tuning grid (see the header): differing outputs only, so 273 configs stay small.
fn spgrid(a: &[String]) {
    use twime_model::prior::ShortCfg;
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m, true));
    assert!(assets.short().is_off(), "spgrid: the settings must not set SHORT-PRIOR keys (the configs do)");
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let us = Arc::new(units(&a[3]));
    let cfgs: Vec<(String, ShortCfg, bool)> = fs::read_to_string(&a[4]).unwrap().lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        assert_eq!(f.len(), 6, "config line: name c l0|- wb ctx table");
        let c = ShortCfg { c: f[1].parse().unwrap(), l0: (f[2] != "-").then(|| f[2].parse().unwrap()), wb: f[3].parse().unwrap(), ctx: f[4] == "on" };
        assert!(c.valid() && (f[4] == "on" || f[4] == "off") && (f[5] == "0" || f[5] == "1"), "bad config {}", f[0]);
        assert!(f[5] == "0" || m.contains_key("word_short_table"), "table config without word_short_table");
        (f[0].to_string(), c, f[5] == "1")
    }).collect();
    let cfgs = Arc::new(cfgs);
    let threads: usize = a[6].parse().unwrap();
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, us, cfgs) = (assets.clone(), us.clone(), cfgs.clone());
        hs.push(std::thread::spawn(move || {
            let mut out = vec![];
            for u in us.iter().skip(t).step_by(threads) {
                let (conv, style) = convert(&assets, u, lambda);
                let s = |c: &Conversion| -> String { c.chars.iter().map(|x| x.unwrap_or('\u{FFFD}')).collect() };
                let r = assets.rebeam_short(&conv, &u.locks, lambda, style, ShortCfg::default(), false);
                assert_eq!(r.chars, assets.rebeam_ex(&conv, &u.locks, lambda, style).chars);
                let rs = s(&r);
                for (name, c, tb) in cfgs.iter() {
                    let o = s(&assets.rebeam_short(&conv, &u.locks, lambda, style, *c, *tb));
                    if o != rs {
                        out.push(format!("{name}\t{}\t{o}", u.id));
                    }
                }
                out.push(format!("ref\t{}\t{rs}", u.id));
            }
            out
        }));
    }
    let mut f = std::io::BufWriter::new(fs::File::create(&a[5]).unwrap());
    for (name, c, tb) in cfgs.iter() {
        // the parameters as parsed (checked against the plan grid by train/short_prior.py grid)
        let l0 = c.l0.map_or("-".to_string(), |x| x.to_string());
        writeln!(f, "#cfg\t{name}\t{}\t{l0}\t{}\t{}\t{}", c.c, c.wb, if c.ctx { "on" } else { "off" }, u8::from(*tb)).unwrap();
    }
    let (mut n, mut d) = (0usize, 0usize);
    for h in hs {
        for l in h.join().unwrap() {
            if l.starts_with("ref\t") { n += 1 } else { d += 1 }
            writeln!(f, "{l}").unwrap();
        }
    }
    println!("spgrid units {n} configs {} differing outputs {d}", cfgs.len());
}

include!("pneval/genj.rs");
include!("pneval/lexux.rs");

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("lexux") => std::process::exit(lexux_python(&a[2..])),
        Some("lexux-run") => {
            std::panic::set_hook(Box::new(|_| eprintln!("lexux_errors=1")));
            lexux_run(&a)
        },
        Some("lexux-build") => {
            std::panic::set_hook(Box::new(|_| eprintln!("lexux_errors=1")));
            lexux_build(&a)
        },
        Some("build") if a.len() >= 3 => build(&a),
        Some("grid") if a.len() >= 7 => grid(&a),
        Some("detail") if a.len() >= 6 => detail(&a),
        Some("gencheck") if a.len() >= 5 => gencheck(&a),
        Some("gensel") if a.len() >= 6 => gensel(&a),
        Some("genjgrid") if a.len() == 7 => genjgrid(&a, false),
        Some("genjdump") if a.len() == 7 => genjgrid(&a, true),
        Some("spgrid") if a.len() >= 7 => spgrid(&a),
        _ => {
            eprintln!("usage: see the header of pneval.rs");
            std::process::exit(2);
        }
    }
}
