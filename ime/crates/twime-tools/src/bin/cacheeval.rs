//! CACHE evaluation: the same-document bigram cache against B0 with the product code
//! (`Assets`, `Session`, `CacheSet::from_text`). Prints counts only; unit and document text stay in the local files it reads
//! (data/cache/work) and nothing of them is written except ids and per-group counts.
//!
//! Usage (paths relative to the repository root, the working directory):
//!   cacheeval oracle <settings> <set> <configs.txt> <out.tsv> <threads> [nonl]
//!   cacheeval replay <settings> <set> <configs.txt> <out.tsv> <threads> <closed|cont> [groups.txt]
//!   cacheeval ids    <settings> <units5.tsv> <out.tsv> <threads>          (G8(a): raw ids, cache off / cache on with an empty set)
//!   cacheeval detail <settings+cache> <set> <ids.txt> <out.jsonl> <threads> (G9: oracle scenario with legal scores)
//!   cacheeval session <settings+cache> <set> <groups.txt> <threads>        (G8(d): oracle decoder == Session with DocText)
//! <set> = data/cache/work/<name> (.units.tsv, .docs.tsv, .script.tsv). configs.txt: `name window gamma cap` per line (cap `inf`
//! or a count); B0 (cache off) is always evaluated and is the reference of every comparison.
//! Output rows: scen cfg group units chars err err_t ok resc brok chars_r0 err_r0 resc_r0 brok_r0 f_units f_chars f_err f_resc
//! f_brok fail (err = two-sided fold_eval errors; err_t = 臺 read as 台 only; r0 = units whose gold was not already in the
//! document before them; f_ = the first unit after the continuation point; fail = compositions that could not be committed).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Write,
    path::PathBuf,
    sync::Arc,
};

use twime_core::{
    Config, Engine, Session,
    keymap::{char_of, full_of, key_of},
};
use twime_model::{
    Assets, Conversion, ModelPaths, NormCfg,
    cache::{CacheCfg, CacheSet},
    norm,
    pn::{self, PnCfg, PnLexicon},
};
use twime_proto::{Applied, CommitResult, Event, Key, KeyEvent, Mode, Mods, Update, is_zhuyin_key};

const LS: char = '\u{1e}';

fn settings(p: &str) -> HashMap<String, String> {
    pn::read_settings(std::path::Path::new(p)).expect("settings")
}

fn load(m: &HashMap<String, String>) -> Assets {
    let p = ModelPaths {
        data_txt: m["data_txt"].clone().into(),
        aliases: m["aliases"].clone().into(),
        model: m["model"].clone().into(),
        prior: m.get("prior").map(PathBuf::from),
    };
    let mut a = Assets::load(&p).expect("assets");
    if m.get("norm").map_or("all", String::as_str) == "all" {
        a.set_norm(NormCfg::ALL);
    }
    assert!(m.get("cand").is_none_or(|c| c == "off") && m.get("gen").is_none_or(|g| g == "off"), "CACHE runs B0 only (no CAND, no gen)");
    let lex = PnLexicon::open(std::path::Path::new(&m["lexicon"]), &a).expect("lexicon");
    let cfg = PnCfg::from_settings(m).expect("pn settings");
    a.set_pn(Some(Arc::new(lex)), cfg);
    a
}

fn lambda_of(m: &HashMap<String, String>) -> f64 {
    let l: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    assert!(l == 0.5, "the engine default lambda is 0.5");
    l
}

#[derive(Clone)]
struct Unit {
    id: String,
    group: String,
    ctx: String,
    syl: Vec<String>,
    gold: Vec<char>,
    start: usize,
    rep: bool,
}

struct Doc {
    text: Vec<char>,
}

fn units(p: &str) -> Vec<Unit> {
    fs::read_to_string(p)
        .expect("units")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert!(f.len() >= 9, "units file of cache_units.py expected");
            Unit {
                id: f[0].into(),
                group: f[1].into(),
                ctx: f[2].into(),
                syl: f[3].split(' ').map(String::from).collect(),
                gold: f[4].chars().collect(),
                start: f[5].parse().unwrap(),
                rep: f[7] == "1",
            }
        })
        .collect()
}

fn docs(p: &str) -> HashMap<String, Doc> {
    fs::read_to_string(p)
        .expect("docs")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut f = l.splitn(3, '\t');
            let g = f.next().unwrap().to_string();
            let rule = f.next().unwrap();
            assert!(rule == "drop" || rule == "space");
            (g, Doc { text: f.next().unwrap_or("").chars().collect() })
        })
        .collect()
}

fn scripts(p: &str) -> HashMap<String, Vec<(String, String)>> {
    let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut cur = String::new();
    for l in fs::read_to_string(p).expect("scripts").lines() {
        if let Some(g) = l.strip_prefix("ART\t") {
            cur = g.to_string();
            out.insert(cur.clone(), vec![]);
            continue;
        }
        let (k, v) = l.split_once('\t').unwrap_or((l, ""));
        out.get_mut(&cur).expect("ART line first").push((k.to_string(), v.to_string()));
    }
    out
}

#[derive(Clone)]
struct Cfg {
    name: String,
    cache: Option<CacheCfg>,
}

fn configs(p: &str) -> Vec<Cfg> {
    let mut v = vec![Cfg { name: "B0".into(), cache: None }];
    for l in fs::read_to_string(p).expect("configs").lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = l.split_whitespace().collect();
        let mut m: HashMap<String, String> = [("cache_window", f[1]), ("cache_gamma", f[2]), ("cache_cap", f[3])].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        // CACHE-c: optional 5th column cache_source (doc | confirmed)
        if let Some(src) = f.get(4) {
            m.insert("cache_source".into(), src.to_string());
        }
        let c = CacheCfg::from_settings(&m).expect("config").expect("gamma");
        assert!(f[0] != "B0");
        // a grid name carries its parameters: W<window>_g<gamma>_k<cap> must be what is measured
        if f[0].starts_with('W') && f[0].contains("_g") {
            assert_eq!(f[0], format!("W{}_g{}_k{}", f[1], f[2], f[3]), "grid name and parameters differ");
        }
        v.push(Cfg { name: f[0].into(), cache: Some(c) });
    }
    v
}

/// Two-sided fold_eval (TRAIN 5.3): ent_norm.fold_out without locks on both sides.
fn fold_eval(v: &[Option<char>]) -> Vec<Option<char>> {
    let mut x = v.to_vec();
    let n = x.len();
    norm::fold_out(&mut x, &vec![false; n], &vec![false; n], false);
    x
}

fn tai(c: Option<char>) -> Option<char> {
    c.map(|c| if c == norm::TAI_TRAD { norm::TAI } else { c })
}

#[derive(Default, Clone, Copy)]
struct Row {
    units: u64,
    chars: u64,
    err: u64,
    err_t: u64,
    ok: u64,
    resc: u64,
    brok: u64,
    chars_r0: u64,
    err_r0: u64,
    resc_r0: u64,
    brok_r0: u64,
    f_units: u64,
    f_chars: u64,
    f_err: u64,
    f_resc: u64,
    f_brok: u64,
    fail: u64,
}

impl Row {
    fn add(&mut self, o: &Row) {
        self.units += o.units;
        self.chars += o.chars;
        self.err += o.err;
        self.err_t += o.err_t;
        self.ok += o.ok;
        self.resc += o.resc;
        self.brok += o.brok;
        self.chars_r0 += o.chars_r0;
        self.err_r0 += o.err_r0;
        self.resc_r0 += o.resc_r0;
        self.brok_r0 += o.brok_r0;
        self.f_units += o.f_units;
        self.f_chars += o.f_chars;
        self.f_err += o.f_err;
        self.f_resc += o.f_resc;
        self.f_brok += o.f_brok;
        self.fail += o.fail;
    }
}

/// Scores one unit's output against the gold and the B0 output (None output = not committed: every position wrong).
fn score(row: &mut Row, out: Option<&[Option<char>]>, b0: Option<&[Option<char>]>, gold: &[char], rep: bool, first: bool) {
    let n = gold.len();
    let g: Vec<Option<char>> = fold_eval(&gold.iter().copied().map(Some).collect::<Vec<_>>());
    let none = vec![None; n];
    let o = fold_eval(out.unwrap_or(&none));
    let b = fold_eval(b0.unwrap_or(&none));
    assert_eq!(o.len(), n);
    let (mut e, mut et, mut rs, mut bk) = (0u64, 0u64, 0u64, 0u64);
    let raw = out.unwrap_or(&none);
    for i in 0..n {
        let ok = o[i].is_some() && o[i] == g[i];
        let okb = b[i].is_some() && b[i] == g[i];
        e += u64::from(!ok);
        et += u64::from(!(raw[i].is_some() && tai(raw[i]) == tai(Some(gold[i]))));
        rs += u64::from(ok && !okb);
        bk += u64::from(!ok && okb);
    }
    row.units += 1;
    row.chars += n as u64;
    row.err += e;
    row.err_t += et;
    row.ok += u64::from(e == 0);
    row.resc += rs;
    row.brok += bk;
    row.fail += u64::from(out.is_none());
    if !rep {
        row.chars_r0 += n as u64;
        row.err_r0 += e;
        row.resc_r0 += rs;
        row.brok_r0 += bk;
    }
    if first {
        row.f_units += 1;
        row.f_chars += n as u64;
        row.f_err += e;
        row.f_resc += rs;
        row.f_brok += bk;
    }
}

fn write_rows(path: &str, rows: &BTreeMap<(String, String, String), Row>) {
    let mut f = std::io::BufWriter::new(fs::File::create(path).unwrap());
    writeln!(f, "scen\tcfg\tgroup\tunits\tchars\terr\terr_t\tok\tresc\tbrok\tchars_r0\terr_r0\tresc_r0\tbrok_r0\tf_units\tf_chars\tf_err\tf_resc\tf_brok\tfail").unwrap();
    for ((s, c, g), r) in rows {
        writeln!(
            f,
            "{s}\t{c}\t{g}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            r.units, r.chars, r.err, r.err_t, r.ok, r.resc, r.brok, r.chars_r0, r.err_r0, r.resc_r0, r.brok_r0, r.f_units, r.f_chars, r.f_err, r.f_resc, r.f_brok, r.fail
        )
        .unwrap();
    }
}

fn last_chars(t: &[char], w: usize) -> String {
    t[t.len().saturating_sub(w)..].iter().collect()
}

/// The last `units` UTF-16 units of a text (what the shell sends; a split surrogate pair becomes U+FFFD there).
fn last_utf16(t: &str, units: usize) -> String {
    let v: Vec<u16> = t.encode_utf16().collect();
    String::from_utf16_lossy(&v[v.len().saturating_sub(units)..])
}

fn by_group(us: Vec<Unit>) -> Vec<(String, Vec<Unit>)> {
    let mut order: Vec<String> = vec![];
    let mut m: HashMap<String, Vec<Unit>> = HashMap::new();
    for u in us {
        if !m.contains_key(&u.group) {
            order.push(u.group.clone());
        }
        m.entry(u.group.clone()).or_default().push(u);
    }
    order.into_iter().map(|g| {
        let v = m.remove(&g).unwrap();
        (g, v)
    }).collect()
}

fn chars_of(c: &Conversion) -> Vec<Option<char>> {
    c.chars.clone()
}

// ---------------------------------------------------------------------------------------------------------------
// oracle scenario (offline decoding, the pneval path)

fn oracle(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda = lambda_of(&m);
    let set = &a[3];
    let cfgs = Arc::new(configs(&a[4]));
    let threads: usize = a[6].parse().unwrap();
    let nonl = a.get(7).is_some_and(|x| x == "nonl");
    let ds = Arc::new(docs(&format!("{set}.docs.tsv")));
    let groups = Arc::new(by_group(units(&format!("{set}.units.tsv"))));
    let pn_cfg = assets.pn().unwrap().1.clone();
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, cfgs, ds, groups, pn_cfg) = (assets.clone(), cfgs.clone(), ds.clone(), groups.clone(), pn_cfg.clone());
        hs.push(std::thread::spawn(move || {
            let mut rows: BTreeMap<(String, String, String), Row> = BTreeMap::new();
            let mut skipped = 0u64;
            for (g, us) in groups.iter().skip(t).step_by(threads) {
                let d = &ds[g];
                let text: Vec<char> = if nonl { d.text.iter().copied().filter(|&c| c != LS).collect() } else { d.text.clone() };
                for u in us {
                    if u.gold.len() != u.syl.len() {
                        skipped += 1;
                        continue;
                    }
                    let style = norm::style_of(&u.ctx).unwrap_or(false);
                    let ctx: String = last_chars(&u.ctx.chars().collect::<Vec<_>>(), 64);
                    let conv = assets.convert_ex(&ctx, &u.syl, &[], lambda, style);
                    let b0 = chars_of(&assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), true, None));
                    // the document before the unit (with nonl: the same text without line breaks)
                    let st = if nonl { d.text[..u.start].iter().filter(|&&c| c != LS).count() } else { u.start };
                    let mut built: HashMap<(usize, u64, Option<usize>), CacheSet> = HashMap::new();
                    for c in cfgs.iter() {
                        let out = match &c.cache {
                            None => b0.clone(),
                            Some(cc) => {
                                let s = built.entry((cc.window, cc.gamma.to_bits(), cc.cap)).or_insert_with(|| CacheSet::from_text(&assets, &last_chars(&text[..st], cc.window), cc));
                                chars_of(&assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), true, Some(s)))
                            }
                        };
                        score(rows.entry(("oracle".into(), c.name.clone(), g.clone())).or_default(), Some(&out), Some(&b0), &u.gold, u.rep, false);
                    }
                }
            }
            (rows, skipped)
        }));
    }
    let mut all: BTreeMap<(String, String, String), Row> = BTreeMap::new();
    let mut skipped = 0;
    for h in hs {
        let (r, s) = h.join().unwrap();
        skipped += s;
        for (k, v) in r {
            all.entry(k).or_default().add(&v);
        }
    }
    write_rows(&a[5], &all);
    println!("oracle groups {} configs {} skipped(len) {skipped}", groups.len(), cfgs.len());
}

// ---------------------------------------------------------------------------------------------------------------
// CACHE-c offline estimate (plan 7 section C3): the oracle scenario with a cache restricted to "confirmed" pairs. The
// simulated user corrects a unit with errors with probability p (fixed draw per unit); the corrected unit is the gold text;
// its confirmed positions are the positions that were wrong. Pairs recorded per rule: `any` = pairs with at least one
// confirmed position, `unit` = every pair of a corrected unit. The cache = pairs of the last W characters of the (gold)
// document that were recorded. Each configuration keeps its own record from its own errors.

/// Fixed uniform draw in [0, 1) per (group, unit index): the same for every configuration and for B0.
fn draw(group: &str, k: usize, seed: u64) -> f64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
    for b in group.bytes().chain((k as u64).to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn estconf(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda = lambda_of(&m);
    let set = &a[3];
    // name W gamma cap rule p
    let cfgs: Arc<Vec<(String, CacheCfg, String, f64)>> = Arc::new(
        fs::read_to_string(&a[4])
            .unwrap()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                let mm: HashMap<String, String> = [("cache_window", f[1]), ("cache_gamma", f[2]), ("cache_cap", f[3])].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                assert!(["any", "unit", "run", "run2"].contains(&f[4]));
                (f[0].to_string(), CacheCfg::from_settings(&mm).unwrap().unwrap(), f[4].to_string(), f[5].parse::<f64>().unwrap())
            })
            .collect(),
    );
    let threads: usize = a[6].parse().unwrap();
    let ds = Arc::new(docs(&format!("{set}.docs.tsv")));
    let groups = Arc::new(by_group(units(&format!("{set}.units.tsv"))));
    let pn_cfg = assets.pn().unwrap().1.clone();
    let tai = (assets.vocab_id(norm::TAI_TRAD), assets.vocab_id(norm::TAI));
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, cfgs, ds, groups, pn_cfg) = (assets.clone(), cfgs.clone(), ds.clone(), groups.clone(), pn_cfg.clone());
        hs.push(std::thread::spawn(move || {
            let mut rows: BTreeMap<(String, String, String), Row> = BTreeMap::new();
            let fold = |id: u32| if id == tai.0 { tai.1 } else { id };
            let idof = |c: char| -> Option<u32> {
                let id = assets.vocab_id(c);
                (id != twime_model::vocab::UNK).then(|| fold(id))
            };
            let (mut npairs, mut nconf) = (0u64, 0u64);
            for (g, us) in groups.iter().skip(t).step_by(threads) {
                let d = &ds[g];
                let ids: Vec<Option<u32>> = d.text.iter().map(|&c| idof(c)).collect();
                let mut rec: Vec<HashSet<u64>> = vec![HashSet::new(); cfgs.len()];
                // run rules (plan 7th edition r7): confirmed runs (>= 2 consecutive confirmed positions, gold ids); a run counts
                // only if it occurs in the window; no run in the window clears them all. run = wrong positions; run2 = wrong
                // positions and their left neighbours (a two-character word chosen)
                let mut runs: Vec<Vec<Vec<u32>>> = vec![vec![]; cfgs.len()];
                for (k, u) in us.iter().enumerate() {
                    if u.gold.len() != u.syl.len() {
                        continue;
                    }
                    let style = norm::style_of(&u.ctx).unwrap_or(false);
                    let ctx: String = last_chars(&u.ctx.chars().collect::<Vec<_>>(), 64);
                    let conv = assets.convert_ex(&ctx, &u.syl, &[], lambda, style);
                    let b0 = chars_of(&assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), true, None));
                    score(rows.entry(("estconf".into(), "B0".into(), g.clone())).or_default(), Some(&b0), Some(&b0), &u.gold, u.rep, false);
                    let gold_f = fold_eval(&u.gold.iter().copied().map(Some).collect::<Vec<_>>());
                    let corrects = draw(g, k, 20261701);
                    for (ci, (name, cc, rule, p)) in cfgs.iter().enumerate() {
                        let lo = u.start.saturating_sub(cc.window);
                        let mut pairs: Vec<(u32, u32)> = vec![];
                        if rule.starts_with("run") && !runs[ci].is_empty() {
                            let win = &ids[lo..u.start];
                            let occurs = |r: &[u32]| win.windows(r.len()).any(|w| w.iter().zip(r).all(|(a, b)| *a == Some(*b)));
                            let valid: Vec<&Vec<u32>> = runs[ci].iter().filter(|r| occurs(r)).collect();
                            if valid.is_empty() {
                                runs[ci].clear();
                            } else {
                                for r in valid {
                                    pairs.extend(r.windows(2).map(|w| (w[0], w[1])));
                                }
                            }
                        }
                        for i in (lo + 1..u.start).filter(|_| !rule.starts_with("run")) {
                            if let (Some(x), Some(y)) = (ids[i - 1], ids[i]) {
                                if rec[ci].contains(&((u64::from(x) << 32) | u64::from(y))) {
                                    pairs.push((x, y));
                                }
                            }
                        }
                        let s = CacheSet::from_pairs(&pairs, tai, cc.gamma, cc.cap);
                        npairs += pairs.len() as u64;
                        let out = if s.is_empty() { b0.clone() } else { chars_of(&assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), true, Some(&s))) };
                        score(rows.entry(("estconf".into(), name.clone(), g.clone())).or_default(), Some(&out), Some(&b0), &u.gold, u.rep, false);
                        // the user's correction of this unit and what it records
                        let of = fold_eval(&out);
                        let wrong: Vec<bool> = (0..u.gold.len()).map(|i| !(of[i].is_some() && of[i] == gold_f[i])).collect();
                        if wrong.iter().any(|&w| w) && corrects < *p && rule.starts_with("run") {
                            let n = u.gold.len();
                            let mut conf = wrong.clone();
                            if rule == "run2" {
                                for i in 1..n {
                                    if wrong[i] {
                                        conf[i - 1] = true;
                                    }
                                }
                            }
                            let mut i = 0;
                            while i < n {
                                let mut j = i;
                                while j < n && conf[j] && ids[u.start + j].is_some() {
                                    j += 1;
                                }
                                if j - i >= 2 {
                                    let r: Vec<u32> = (i..j).map(|x| ids[u.start + x].unwrap()).collect();
                                    if !runs[ci].contains(&r) {
                                        runs[ci].push(r);
                                        if runs[ci].len() > 256 {
                                            runs[ci].remove(0);
                                        }
                                        nconf += 1;
                                    }
                                }
                                i = j.max(i + 1);
                            }
                        } else if wrong.iter().any(|&w| w) && corrects < *p {
                            for i in 0..u.gold.len().saturating_sub(1) {
                                if rule == "unit" || wrong[i] || wrong[i + 1] {
                                    if let (Some(x), Some(y)) = (ids[u.start + i], ids[u.start + i + 1]) {
                                        rec[ci].insert((u64::from(x) << 32) | u64::from(y));
                                        nconf += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            (rows, npairs, nconf)
        }));
    }
    let mut all: BTreeMap<(String, String, String), Row> = BTreeMap::new();
    let (mut np, mut nc) = (0, 0);
    for h in hs {
        let (r, a_, b_) = h.join().unwrap();
        np += a_;
        nc += b_;
        for (k, v) in r {
            all.entry(k).or_default().add(&v);
        }
    }
    write_rows(&a[5], &all);
    println!("estconf groups {} configs {} cache pairs used {np} pairs recorded {nc}", groups.len(), cfgs.len());
}

// ---------------------------------------------------------------------------------------------------------------
// product replay (closed loop / continuation): the ART typist policy v1 (ime-art/src/main.rs, same project) through a real
// Session, with the shell's behaviour emulated (plan 5.1)

struct Typist {
    s: Session,
    seq: u32,
    /// Emulated host document (line breaks as U+001E).
    doc: String,
    /// The host changed the text by itself: the shell sends `ExternalEdit` before the next event.
    ext: bool,
    /// Index of the zh op whose composition is open, and its syllable count.
    cur: Option<(usize, usize)>,
    outs: Vec<Option<Vec<Option<char>>>>,
    /// CACHE-c user model (plan 7th edition 3.1): the composition before the user's corrections, per zh op.
    pre: Vec<Option<Vec<Option<char>>>>,
    /// Candidate selections made and positions whose gold item was not in the list.
    picks: u64,
    unreachable: u64,
    /// `committable` of the last Update (every position has a character).
    committable: bool,
}

impl Typist {
    fn new(engine: &Arc<Engine>, doc: String, nzh: usize) -> Typist {
        let mut s = engine.new_session();
        s.set_ux(true);
        let mut t = Typist { s, seq: 0, doc, ext: false, cur: None, outs: vec![None; nzh], pre: vec![None; nzh], picks: 0, unreachable: 0, committable: false };
        t.raw(Event::Focus(true));
        t
    }

    fn raw(&mut self, ev: Event) -> Update {
        let ack = (self.seq > 0).then(|| CommitResult { seq: self.seq, commit: Applied::Applied, comp: Applied::Applied });
        self.seq += 1;
        let u = self.s.handle(self.seq, ack, ev);
        self.committable = u.committable;
        if let Some(c) = &u.commit {
            if let Some((k, n)) = self.cur.take() {
                let v: Vec<Option<char>> = c.chars().take(n).map(Some).collect();
                self.outs[k] = (v.len() == n).then_some(v);
            }
            self.doc.push_str(c);
        }
        u
    }

    /// The shell sends a pending `ExternalEdit` before anything else (so every decision sees the state after it).
    fn flush(&mut self) {
        if self.ext {
            self.ext = false;
            self.raw(Event::ExternalEdit);
            // an external edit ends any open composition without a commit
            self.cur = None;
        }
    }

    fn send(&mut self, ev: Event) -> Update {
        self.flush();
        self.raw(ev)
    }

    fn host_insert(&mut self, c: char) {
        self.doc.push(c);
        self.ext = true;
    }

    fn view(&self) -> twime_proto::PeekView {
        self.s.peek_view()
    }

    fn key(&mut self, k: Key, mods: u8) -> Update {
        self.flush();
        let v = self.view();
        if !v.composing && !v.run && !v.symbols && self.s.mode() == Mode::Chinese && mods & Mods::SHIFT == 0 && is_zhuyin_key(k) {
            let d = last_utf16(&self.doc, twime_proto::DOC_TEXT_UNITS);
            self.send(Event::DocText(d));
        }
        let u = self.send(Event::Key(KeyEvent { key: k, mods: Mods(mods), down: true }));
        if !u.handled {
            match k {
                Key::Space => self.host_insert(' '),
                Key::Enter => self.host_insert(LS),
                _ => {
                    if let Some(c) = char_of(k, mods & Mods::SHIFT != 0) {
                        self.host_insert(c)
                    }
                }
            }
        }
        u
    }

    fn composing(&mut self) -> bool {
        self.flush();
        self.view().composing
    }

    /// Ends whatever composition is open: committed when it can be (the open zh op gets its output), else dropped (that
    /// op keeps None: every position counts as wrong).
    fn finish(&mut self) {
        // NEUTRAL: Enter would now commit a position without a character as its typed bopomofo; the measurement keeps the
        // old rule (such a composition is dropped and scored None)
        // (an open list keeps the old Enter: it takes the highlighted item, then Esc drops the composition)
        if self.composing() && (self.committable || self.view().candidates_open) {
            self.key(Key::Enter, 0);
        }
        // Esc twice at most: the first one may only close a list
        for _ in 0..2 {
            if self.composing() {
                self.key(Key::Esc, 0);
            }
        }
        self.cur = None;
    }
}

const SHIFT: u8 = Mods::SHIFT;

fn ascii_key(c: char) -> Option<(Key, bool)> {
    let mut keys: Vec<Key> = (b'a'..=b'z').map(Key::Letter).collect();
    keys.extend((0..=9).map(Key::Digit));
    keys.extend([Key::Minus, Key::Comma, Key::Period, Key::Slash, Key::Semicolon, Key::Quote, Key::Backquote, Key::LBracket, Key::RBracket, Key::Backslash, Key::Equals]);
    for k in keys {
        for shift in [false, true] {
            if char_of(k, shift) == Some(c) {
                return Some((k, shift));
            }
        }
    }
    None
}

fn full_key(s: &str) -> Option<(Key, bool)> {
    let mut keys: Vec<Key> = (0..=9).map(Key::Digit).collect();
    keys.extend([Key::Minus, Key::Comma, Key::Period, Key::Slash, Key::Semicolon, Key::Quote, Key::LBracket, Key::RBracket]);
    for k in keys {
        for shift in [false, true] {
            if full_of(k, shift) == Some(s) {
                return Some((k, shift));
            }
        }
    }
    None
}

fn commits_composition(op: Option<&(String, String)>) -> bool {
    match op {
        Some((k, t)) => match k.as_str() {
            "punct" => full_key(t).is_some(),
            "num" => !t.contains(','),
            "en" => true,
            _ => false,
        },
        None => false,
    }
}

/// A character the typist policy cannot type: the host gets it by other means (an external edit).
fn by_host(t: &mut Typist, s: &str) {
    for c in s.chars() {
        t.host_insert(c);
    }
}

/// The positions of a composition (None = unconverted syllable), from the engine's display.
fn comp_chars(u: &Update) -> Option<Vec<Option<char>>> {
    let c = u.composition.as_ref()?;
    c.segments.iter().filter(|g| !g.pending).map(|g| if g.unconverted { Some(None) } else { g.text.chars().next().map(Some) }).collect()
}

fn same_tai(a: &str, b: &[char]) -> bool {
    let a: Vec<char> = a.chars().collect();
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| tai(Some(*x)) == tai(Some(*y)))
}

/// CACHE-c user model (plan 7th edition 3.1): with errors (two-sided fold_eval) the user moves the cursor after the leftmost
/// wrong position j, opens the list (Down), takes the longest item ending at j equal to the gold (臺 read as 台; the first such
/// item in list order), turning pages with Right; none: Esc and j is unreachable. Each position at most once; then End.
fn correct(t: &mut Typist, mut cur: Vec<Option<char>>, gold: &[char]) {
    let n = gold.len();
    let g = fold_eval(&gold.iter().copied().map(Some).collect::<Vec<_>>());
    let mut tried = vec![false; n];
    loop {
        if cur.len() != n {
            return;
        }
        let o = fold_eval(&cur);
        let Some(j) = (0..n).find(|&i| !tried[i] && !(o[i].is_some() && o[i] == g[i])) else { break };
        tried[j] = true;
        t.key(Key::Home, 0);
        for _ in 0..=j {
            t.key(Key::Right, 0);
        }
        let u = t.key(Key::Down, 0);
        let Some(c) = u.candidates else {
            t.unreachable += 1;
            continue;
        };
        let per = usize::from(c.per_page.max(1));
        let pages = (usize::from(c.total).max(c.items.len())).div_ceil(per).max(1);
        // scan every page once (back on page 0 after the last Right), keep the longest match, first in list order
        let mut best: Option<(usize, usize, usize)> = None; // (len, page, index)
        let mut items = c.items;
        for pg in 0..pages {
            for (i, it) in items.iter().enumerate() {
                let len = it.chars().count();
                if len >= 1 && len <= j + 1 && same_tai(it, &gold[j + 1 - len..=j]) && best.is_none_or(|b| len > b.0) {
                    best = Some((len, pg, i));
                }
            }
            let u = t.key(Key::Right, 0);
            items = u.candidates.map(|c| c.items).unwrap_or_default();
        }
        match best {
            Some((_, pg, i)) => {
                for _ in 0..pg {
                    t.key(Key::Right, 0);
                }
                let u = t.key(Key::Digit(i as u8 + 1), 0);
                t.picks += 1;
                match comp_chars(&u) {
                    Some(v) => cur = v,
                    None => return,
                }
            }
            None => {
                t.key(Key::Esc, 0);
                t.unreachable += 1;
            }
        }
    }
    t.key(Key::End, 0);
}

fn type_zh(t: &mut Typist, syls: &str) -> Update {
    let mut last = Update::default();
    for syl in syls.split(' ').filter(|x| !x.is_empty()) {
        let mut has_tone = false;
        for c in syl.chars() {
            if "ˊˇˋ˙".contains(c) {
                has_tone = true;
            }
            if let Some(k) = key_of(c) {
                last = t.key(k, 0);
            }
        }
        if !has_tone {
            last = t.key(Key::Space, 0);
        }
    }
    last
}

fn english_mode(t: &mut Typist, text: &str) {
    let toggled = t.s.mode() == Mode::Chinese;
    if toggled {
        t.send(Event::ToggleMode);
    }
    for c in text.chars() {
        match ascii_key(c) {
            Some((k, sh)) => {
                t.key(k, if sh { SHIFT } else { 0 });
            }
            None => by_host(t, &c.to_string()),
        }
    }
    if toggled {
        t.send(Event::ToggleMode);
    }
}

fn type_en(t: &mut Typist, text: &str, next: Option<&(String, String)>) {
    if text.chars().count() > 63 {
        by_host(t, text);
        return;
    }
    if !text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        english_mode(t, text);
        return;
    }
    for (i, c) in text.chars().enumerate() {
        match ascii_key(c) {
            Some((k, sh)) => {
                let shift = sh || (i == 0 && c.is_ascii_uppercase());
                t.key(k, if shift { SHIFT } else { 0 });
            }
            None => by_host(t, &c.to_string()),
        }
    }
    let ends_by_mark = matches!(next, Some((k, c)) if k == "punct" && full_key(c).is_some_and(|(_, sh)| sh));
    if !ends_by_mark {
        t.key(Key::Space, 0);
    }
}

fn type_punct(t: &mut Typist, txt: &str) {
    let c = txt.chars().next().unwrap();
    if let Some((k, sh)) = full_key(txt) {
        t.key(k, if sh { SHIFT } else { 0 });
        return;
    }
    if let Some(i) = twime_core::keymap::SYMBOL_TABLE.iter().position(|s| s.chars().count() == 1 && s.chars().next() == Some(c)) {
        if t.composing() {
            t.key(Key::Enter, 0);
        }
        t.key(Key::Backquote, 0);
        for _ in 0..i / twime_core::keymap::SYMBOL_PAGE {
            t.key(Key::PageDown, 0);
        }
        t.key(Key::Digit((i % twime_core::keymap::SYMBOL_PAGE) as u8 + 1), 0);
        return;
    }
    match c {
        '…' | '—' => {
            let k = if c == '…' { Key::Digit(6) } else { Key::Minus };
            t.key(k, SHIFT);
        }
        c if c.is_ascii() => {
            if t.composing() {
                t.key(Key::Enter, 0);
            }
            english_mode(t, &c.to_string());
        }
        _ => by_host(t, txt),
    }
}

fn type_num(t: &mut Typist, text: &str) {
    if text.contains(',') {
        if t.composing() {
            t.key(Key::Enter, 0);
        }
        english_mode(t, text);
        return;
    }
    for c in text.chars() {
        match c {
            '0'..='9' => {
                t.key(Key::NumpadDigit(c as u8 - b'0'), 0);
            }
            '.' => {
                t.key(Key::NumpadDecimal, 0);
            }
            _ => by_host(t, &c.to_string()),
        }
    }
}

/// Types the script from op index `from` (a zh op for the continuation; 0 for the closed loop). Returns the outputs of every
/// zh op (None before `from`, and for compositions that could not be committed).
/// CACHE-c user model (plan 7th edition 3.1): the units of the document (gold per zh op), its id and the correction rate p.
#[derive(Clone, Copy)]
struct User<'a> {
    units: &'a [Unit],
    group: &'a str,
    p: f64,
    /// `closed:half` (plan 8th edition 3.2): units from this index on are never corrected (p = 0 there).
    stop: Option<usize>,
}

impl User<'_> {
    fn p_of(&self, k: usize) -> f64 {
        if self.stop.is_some_and(|s| k >= s) { 0.0 } else { self.p }
    }
}

const USER_SEED: u64 = 20261701;

/// Replays one document. Without a user model the output of a zh op is its commit (first phase); with one, it is the
/// composition before the user's corrections, when the composition was committed. Returns (outputs, document, picks,
/// unreachable positions).
#[cfg(test)]
fn replay_doc(engine: &Arc<Engine>, ops: &[(String, String)], from: usize, doc: String, nzh: usize, zh_before: usize) -> (Vec<Option<Vec<Option<char>>>>, String) {
    let (o, d, _, _) = replay_user(engine, ops, from, doc, nzh, zh_before, None);
    (o, d)
}

#[allow(clippy::too_many_arguments)]
fn replay_user(engine: &Arc<Engine>, ops: &[(String, String)], from: usize, doc: String, nzh: usize, zh_before: usize, user: Option<User>) -> (Vec<Option<Vec<Option<char>>>>, String, u64, u64) {
    let mut t = Typist::new(engine, doc, nzh);
    let mut zi = zh_before;
    // line index of the script position: LINE<TAB>index (empty lines have no block, so the index can jump)
    let line_of = |o: &(String, String)| -> usize { o.1.parse().expect("LINE index") };
    let mut line = if from == 0 { 0 } else { ops[..from].iter().rev().find(|o| o.0 == "LINE").map(line_of).expect("a LINE before the start") };
    for i in from..ops.len() {
        let (k, txt) = (&ops[i].0, &ops[i].1);
        let next = ops.get(i + 1);
        match k.as_str() {
            "LINE" => {
                let li = line_of(&ops[i]);
                assert!(li >= line);
                // the line breaks of the document: Enter outside a composition goes to the host
                for _ in line..li {
                    t.finish();
                    t.key(Key::Enter, 0);
                }
                line = li;
            }
            "END" => {
                t.finish();
            }
            "zh" => {
                let syl = txt.split('\t').next().unwrap();
                let n = syl.split(' ').filter(|x| !x.is_empty()).count();
                assert!(n <= 39, "a unit over 39 syllables (plan 4.4)");
                t.finish();
                let u = type_zh(&mut t, syl);
                if let Some(us) = user {
                    let pre = comp_chars(&u).filter(|v| v.len() == n);
                    t.pre[zi] = pre.clone();
                    let unit = &us.units[zi];
                    // only scored units (gold length == syllables) can be compared with the gold
                    if let Some(cur) = pre.filter(|_| unit.gold.len() == n) {
                        let o = fold_eval(&cur);
                        let g = fold_eval(&unit.gold.iter().copied().map(Some).collect::<Vec<_>>());
                        let wrong = (0..n).any(|i| !(o[i].is_some() && o[i] == g[i]));
                        if wrong && draw(us.group, zi, USER_SEED) < us.p_of(zi) {
                            correct(&mut t, cur, &unit.gold);
                        }
                    }
                }
                t.cur = Some((zi, n));
                if !commits_composition(next) {
                    t.finish();
                }
                zi += 1;
            }
            "keep" => by_host(&mut t, txt),
            "en" => type_en(&mut t, txt, next),
            "num" => type_num(&mut t, txt),
            "punct" => type_punct(&mut t, txt),
            "sp" => {
                for _ in txt.chars() {
                    t.key(Key::Space, 0);
                }
            }
            _ => by_host(&mut t, txt),
        }
        // a zh op whose composition the next op ended without committing it
        if t.cur.is_some() && !t.composing() && k != "zh" {
            t.cur = None;
        }
    }
    t.finish();
    assert_eq!(zi, nzh, "script zh ops differ from the units");
    let outs = match user {
        None => t.outs,
        Some(us) => {
            let mut v = vec![None; nzh];
            for k in 0..nzh {
                // committed: scored on what the user saw before correcting; with p = 0 that is the commit itself
                if t.outs[k].is_some() {
                    if us.p_of(k) == 0.0 {
                        assert_eq!(t.pre[k], t.outs[k], "p = 0: the composition is committed unchanged");
                    }
                    v[k] = t.pre[k].clone();
                }
            }
            v
        }
    };
    (outs, t.doc, t.picks, t.unreachable)
}

fn replay(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda = lambda_of(&m);
    let set = &a[3];
    let cfgs = configs(&a[4]);
    let threads: usize = a[6].parse().unwrap();
    let scen = a[7].clone();
    // closed | cont (first phase, no user model) or closed:<p> | cont:<p> (CACHE-c user model, plan 7th edition 3.1)
    // closed:half = p 1 before the middle of the document, 0 from the first scored unit at or after it; only units from there on
    // are scored (plan 8th edition 3.2)
    let half_mode = scen == "closed:half";
    let (kind, user_p): (String, Option<f64>) = match scen.split_once(':') {
        Some((k, _)) if half_mode => (k.to_string(), Some(1.0)),
        Some((k, p)) => (k.to_string(), Some(p.parse().expect("p"))),
        None => (scen.clone(), None),
    };
    assert!(kind == "closed" || kind == "cont");
    assert!(user_p.is_none_or(|p| (0.0..=1.0).contains(&p)));
    let only: Option<HashSet<String>> = a.get(8).map(|p| fs::read_to_string(p).unwrap().lines().map(String::from).collect());
    let ds = Arc::new(docs(&format!("{set}.docs.tsv")));
    let sc = Arc::new(scripts(&format!("{set}.script.tsv")));
    let mut groups = by_group(units(&format!("{set}.units.tsv")));
    if let Some(o) = &only {
        groups.retain(|(g, _)| o.contains(g));
    }
    let groups = Arc::new(groups);
    let engines: Arc<Vec<(String, Arc<Engine>)>> =
        Arc::new(cfgs.iter().map(|c| (c.name.clone(), Engine::new(assets.clone(), Config { lambda, cache: c.cache.clone(), ..Config::default() }))).collect());
    let mut hs = vec![];
    for t in 0..threads {
        let (ds, sc, groups, engines, scen, kind) = (ds.clone(), sc.clone(), groups.clone(), engines.clone(), scen.clone(), kind.clone());
        hs.push(std::thread::spawn(move || {
            let mut rows: BTreeMap<(String, String, String), Row> = BTreeMap::new();
            let (mut nosplit, mut skipped) = (0u64, 0u64);
            let mut effort = vec![(0u64, 0u64); engines.len()];
            // G8(e): scored units compared with B0 on the raw output (before any normalisation), and how many differ
            let mut rawdiff = vec![(0u64, 0u64); engines.len()];
            for (g, us) in groups.iter().skip(t).step_by(threads) {
                let ops = &sc[g];
                let d = &ds[g];
                // continuation point: the first unit whose original start is at least half of the original document length
                // the first scored unit at or after the middle of the original document (continuation point, closed:half switch)
                let mid = us.iter().position(|u| u.start >= d.text.len().div_ceil(2) && u.gold.len() == u.syl.len());
                if half_mode && mid.is_none() {
                    nosplit += 1;
                    continue;
                }
                let score_from = if half_mode { mid.unwrap() } else { 0 };
                let (from_op, from_unit, doc0) = if kind == "cont" {
                    let half = d.text.len().div_ceil(2);
                    // the first SCORED unit (gold length == syllables) at or after the middle (plan 5.1)
                    let Some(k) = us.iter().position(|u| u.start >= half && u.gold.len() == u.syl.len()) else {
                        nosplit += 1;
                        continue;
                    };
                    let zh_ops: Vec<usize> = ops.iter().enumerate().filter(|(_, o)| o.0 == "zh").map(|(i, _)| i).collect();
                    (zh_ops[k], k, d.text[..us[k].start].iter().collect::<String>())
                } else {
                    (0, 0, String::new())
                };
                let mut outs: Vec<Vec<Option<Vec<Option<char>>>>> = vec![];
                for (ei, (_, e)) in engines.iter().enumerate() {
                    let user = user_p.map(|p| User { units: us, group: g, p, stop: half_mode.then_some(score_from) });
                    let (o, doc, pk, un) = replay_user(e, ops, from_op, doc0.clone(), us.len(), from_unit, user);
                    effort[ei].0 += pk;
                    effort[ei].1 += un;
                    // the emulated document has exactly the line breaks of the original
                    // (line breaks after the last line with text are not typed)
                    let orig: String = d.text.iter().collect();
                    let n = |t: &str| t.trim_end_matches(LS).chars().filter(|&c| c == LS).count();
                    assert_eq!(n(&doc), n(&orig), "line breaks of the replayed document");
                    outs.push(o);
                }
                for (ci, (name, _)) in engines.iter().enumerate() {
                    let mut row = Row::default();
                    let mut first = kind == "cont";
                    for (k, u) in us.iter().enumerate().skip(from_unit.max(score_from)) {
                        // typed (the document and the engine state need it) but not scored: gold length != syllables (plan 5.2)
                        if u.gold.len() != u.syl.len() {
                            skipped += u64::from(ci == 0);
                            continue;
                        }
                        score(&mut row, outs[ci][k].as_deref(), outs[0][k].as_deref(), &u.gold, u.rep, first);
                        rawdiff[ci].0 += 1;
                        rawdiff[ci].1 += u64::from(outs[ci][k] != outs[0][k]);
                        first = false;
                    }
                    // a group without any scored unit has no row (as in the oracle scenario)
                    if row.units > 0 {
                        rows.insert((scen.clone(), name.clone(), g.clone()), row);
                    }
                }
            }
            (rows, nosplit, skipped, effort, rawdiff)
        }));
    }
    let mut all: BTreeMap<(String, String, String), Row> = BTreeMap::new();
    let (mut nosplit, mut skipped) = (0, 0);
    let mut effort = vec![(0u64, 0u64); cfgs.len()];
    let mut rawdiff = vec![(0u64, 0u64); cfgs.len()];
    for h in hs {
        let (r, n, sk, ef, rd) = h.join().unwrap();
        for (i, (a_, b_)) in rd.into_iter().enumerate() {
            rawdiff[i].0 += a_;
            rawdiff[i].1 += b_;
        }
        nosplit += n;
        skipped += sk;
        for (i, (p, u)) in ef.into_iter().enumerate() {
            effort[i].0 += p;
            effort[i].1 += u;
        }
        for (k, v) in r {
            all.entry(k).or_default().add(&v);
        }
    }
    write_rows(&a[5], &all);
    // CACHE-c 5.5: what was actually measured (parsed settings of every engine, the user model), for cache_c_stats.py
    let meta = serde_json::json!({
        "scen": scen,
        "settings": a[2],
        "configs": cfgs.iter().map(|c| serde_json::json!({
            "name": c.name,
            "cache": c.cache.as_ref().map(|k| serde_json::json!({"window": k.window, "gamma": k.gamma, "cap": k.cap,
                "source": match k.source { twime_model::cache::CacheSource::Doc => "doc", twime_model::cache::CacheSource::Confirmed => "confirmed" }})),
            "picks": effort[cfgs.iter().position(|x| x.name == c.name).unwrap()].0,
            "unreachable": effort[cfgs.iter().position(|x| x.name == c.name).unwrap()].1,
            "raw_compared": rawdiff[cfgs.iter().position(|x| x.name == c.name).unwrap()].0,
            "raw_differ": rawdiff[cfgs.iter().position(|x| x.name == c.name).unwrap()].1,
        })).collect::<Vec<_>>(),
        "user_seed": USER_SEED,
    });
    fs::write(format!("{}.meta.json", a[5]), serde_json::to_string_pretty(&meta).unwrap()).unwrap();
    println!("replay {scen} groups {} configs {} without continuation point {nosplit} skipped(len) {skipped}", groups.len(), cfgs.len());
    if user_p.is_some() {
        for (c, (p, u)) in cfgs.iter().zip(effort.iter().copied()) {
            println!("user {scen} {} picks {p} unreachable {u}", c.name);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// G8(a), G9, G8(d)

fn ids(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda = lambda_of(&m);
    let lines: Arc<Vec<String>> = Arc::new(fs::read_to_string(&a[3]).unwrap().lines().filter(|l| !l.is_empty()).map(String::from).collect());
    let threads: usize = a[5].parse().unwrap();
    let pn_cfg = assets.pn().unwrap().1.clone();
    let empty = Arc::new(CacheSet::from_text(&assets, "", &CacheCfg { gamma: 4.0, window: 64, cap: None, source: Default::default() }));
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, lines, pn_cfg, empty) = (assets.clone(), lines.clone(), pn_cfg.clone(), empty.clone());
        hs.push(std::thread::spawn(move || {
            let mut out = vec![];
            for l in lines.iter().skip(t).step_by(threads) {
                let f: Vec<&str> = l.split('\t').collect();
                let syl: Vec<String> = f[3].split(' ').map(String::from).collect();
                let style = norm::style_of(f[2]).unwrap_or(false);
                let ctx: String = last_chars(&f[2].chars().collect::<Vec<_>>(), 64);
                let conv = assets.convert_ex(&ctx, &syl, &[], lambda, style);
                let id = |c: &Conversion| -> String { c.chars.iter().map(|x| x.map_or("-".to_string(), |ch| assets.vocab_id(ch).to_string())).collect::<Vec<_>>().join(",") };
                let off = assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), false, None);
                let emp = assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), false, Some(&empty));
                let cc = assets.convert_cached(&ctx, Some(empty.clone()), &syl, &[], lambda, style);
                let prod = assets.rebeam_with(&cc, &[], lambda, style, Some(&pn_cfg), false);
                out.push(format!("{}\t{}\t{}\t{}", f[0], id(&off), id(&emp), id(&prod)));
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
    println!("ids units {n}");
}

fn cache_cfg(m: &HashMap<String, String>) -> CacheCfg {
    CacheCfg::from_settings(m).expect("cache settings").expect("cache_gamma in the settings")
}

fn detail(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda = lambda_of(&m);
    let cc = cache_cfg(&m);
    let set = &a[3];
    let keep: HashSet<String> = fs::read_to_string(&a[4]).unwrap().lines().map(String::from).collect();
    let ds = docs(&format!("{set}.docs.tsv"));
    let us: Vec<Unit> = units(&format!("{set}.units.tsv")).into_iter().filter(|u| keep.contains(&u.id)).collect();
    let pn_cfg = assets.pn().unwrap().1.clone();
    let mut f = std::io::BufWriter::new(fs::File::create(&a[5]).unwrap());
    let mut n = 0;
    for u in &us {
        let style = norm::style_of(&u.ctx).unwrap_or(false);
        let ctx: String = last_chars(&u.ctx.chars().collect::<Vec<_>>(), 64);
        let conv = assets.convert_ex(&ctx, &u.syl, &[], lambda, style);
        let s = CacheSet::from_text(&assets, &last_chars(&ds[&u.group].text[..u.start], cc.window), &cc);
        let ids = |c: &Conversion| -> Option<Vec<u32>> { c.chars.iter().map(|x| x.map(|ch| assets.vocab_id(ch))).collect() };
        let on = assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), false, Some(&s));
        let off = assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), false, None);
        let r = ids(&on);
        let hits = r.as_ref().map_or(0, |v| v.windows(2).filter(|w| s.contains(w[0], w[1])).count());
        let j = serde_json::json!({"id": u.id, "syl": u.syl, "raw": r, "raw_off": ids(&off), "scores": assets.legal_scores(&conv), "pairs": s.len(), "hits": hits});
        writeln!(f, "{j}").unwrap();
        n += 1;
    }
    println!("detail units {n}");
}

fn syllable_keys(s: &str) -> Vec<Key> {
    let mut keys: Vec<Key> = s.chars().filter_map(key_of).collect();
    if !s.chars().any(|c| "ˊˇˋ˙".contains(c)) {
        keys.push(Key::Space);
    }
    keys
}

fn session(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let lambda = lambda_of(&m);
    let cc = cache_cfg(&m);
    let set = &a[3];
    let only: HashSet<String> = fs::read_to_string(&a[4]).unwrap().lines().map(String::from).collect();
    let threads: usize = a[5].parse().unwrap();
    let ds = Arc::new(docs(&format!("{set}.docs.tsv")));
    let us: Arc<Vec<Unit>> = Arc::new(units(&format!("{set}.units.tsv")).into_iter().filter(|u| only.contains(&u.group) && u.gold.len() == u.syl.len()).collect());
    let found: HashSet<&str> = us.iter().map(|u| u.group.as_str()).collect();
    assert!(!us.is_empty() && only.iter().all(|g| found.contains(g.as_str())), "session: empty or missing groups");
    let engine = Engine::new(assets.clone(), Config { lambda, cache: Some(cc.clone()), ..Config::default() });
    let pn_cfg = assets.pn().unwrap().1.clone();
    let mut hs = vec![];
    for t in 0..threads {
        let (assets, ds, us, engine, cc, pn_cfg) = (assets.clone(), ds.clone(), us.clone(), engine.clone(), cc.clone(), pn_cfg.clone());
        hs.push(std::thread::spawn(move || {
            let (mut n, mut bad, mut changed) = (0u64, 0u64, 0u64);
            for u in us.iter().skip(t).step_by(threads) {
                let d: String = ds[&u.group].text[..u.start].iter().collect();
                let style = norm::style_of(&u.ctx).unwrap_or(false);
                let ctx: String = last_chars(&u.ctx.chars().collect::<Vec<_>>(), 64);
                let conv = assets.convert_ex(&ctx, &u.syl, &[], lambda, style);
                let s = CacheSet::from_text(&assets, &last_chars(&ds[&u.group].text[..u.start], cc.window), &cc);
                let off: String = assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), true, Some(&s)).chars.iter().map(|c| tai(*c).unwrap_or('?')).collect();
                let b0: String = assets.rebeam_with_cache(&conv, &[], lambda, style, Some(&pn_cfg), true, None).chars.iter().map(|c| tai(*c).unwrap_or('?')).collect();
                let mut se = engine.new_session();
                se.set_ux(true);
                let mut seq = 0u32;
                let mut call = |se: &mut Session, ev: Event| {
                    seq += 1;
                    se.handle(seq, None, ev)
                };
                call(&mut se, Event::SetContext(u.ctx.clone()));
                call(&mut se, Event::DocText(last_utf16(&d, twime_proto::DOC_TEXT_UNITS)));
                let mut last = Update::default();
                for y in &u.syl {
                    for k in syllable_keys(y) {
                        last = call(&mut se, Event::Key(KeyEvent::press(k)));
                    }
                }
                let got: String = last.composition.map(|c| c.text()).unwrap_or_default().chars().map(|c| tai(Some(c)).unwrap()).collect();
                n += 1;
                bad += u64::from(got != off);
                changed += u64::from(off != b0);
            }
            (n, bad, changed)
        }));
    }
    let (mut n, mut bad, mut ch) = (0, 0, 0);
    for h in hs {
        let (a_, b_, c_) = h.join().unwrap();
        n += a_;
        bad += b_;
        ch += c_;
    }
    println!("session units {n} cache changed the output {ch} mismatch {bad}");
    assert_eq!(bad, 0, "Session with DocText differs from the offline decoder");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("oracle") if a.len() >= 7 => oracle(&a),
        Some("replay") if a.len() >= 8 => replay(&a),
        Some("estconf") if a.len() >= 7 => estconf(&a),
        Some("ids") if a.len() >= 6 => ids(&a),
        Some("detail") if a.len() >= 6 => detail(&a),
        Some("session") if a.len() >= 6 => session(&a),
        _ => {
            eprintln!("usage: see the header of cacheeval.rs");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DICT: &str = "\
ㄐㄧㄣ 今 -2
ㄊㄧㄢ 天 -2
ㄊㄧㄢ 添 -3
";

    fn ops(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// Plan 5.1: a commit delayed to the next mark, an Enter commit, a composition that cannot be committed, host text.
    #[test]
    fn typist_attributes_commits_to_zh_ops() {
        let assets = Arc::new(Assets::from_dictionary_text(DICT, "").unwrap());
        let engine = Engine::new(assets, Config { lambda: 0.0, ..Config::default() });
        let script = ops(&[("LINE", "0"), ("zh", "ㄐㄧㄣ ㄊㄧㄢ"), ("punct", "，"), ("zh", "ㄊㄧㄢ"), ("END", ""), ("LINE", "2"), ("zh", "ㄅㄧㄤ"), ("keep", "鑫"), ("zh", "ㄐㄧㄣ"), ("END", "")]);
        let (outs, doc) = replay_doc(&engine, &script, 0, String::new(), 4, 0);
        assert_eq!(doc, "今天，天\u{1e}\u{1e}鑫今");
        let s = |o: &Option<Vec<Option<char>>>| o.as_ref().map(|v| v.iter().map(|c| c.unwrap()).collect::<String>());
        assert_eq!(s(&outs[0]).as_deref(), Some("今天"));
        assert_eq!(s(&outs[1]).as_deref(), Some("天"));
        assert_eq!(outs[2], None, "not committable");
        assert_eq!(s(&outs[3]).as_deref(), Some("今"));
        // a long English word typed by the host after a composition: the pending external edit ends that composition
        // first (no commit, no stray line break), then the next composition starts normally
        let long = "a".repeat(70);
        let script = ops(&[("LINE", "0"), ("zh", "ㄐㄧㄣ"), ("en", &long), ("zh", "ㄊㄧㄢ"), ("END", "")]);
        let (outs, doc) = replay_doc(&engine, &script, 0, String::new(), 2, 0);
        assert_eq!(outs[0], None);
        assert_eq!(outs[1], Some(vec![Some('天')]));
        assert_eq!(doc, format!("{long}天"));
        // NEUTRAL: a composition without a character and with its list open is still dropped, not committed
        let mut t = Typist::new(&engine, String::new(), 1);
        t.cur = Some((0, 1));
        type_zh(&mut t, "ㄅㄧㄤ˙");
        t.key(Key::Down, 0);
        assert!(t.view().candidates_open);
        t.finish();
        assert!(!t.composing() && t.doc.is_empty() && t.outs[0].is_none());
        // the host document: commits, the line break and the kept character
        let mut t = Typist::new(&engine, String::new(), 0);
        t.host_insert('鑫');
        assert!(t.ext && t.doc == "鑫");
    }
}

#[cfg(test)]
mod user_tests {
    use super::*;

    const DICT: &str = "\
ㄐㄧㄣ 今 -2
ㄐㄧㄣ 金 -2.5
ㄊㄧㄢ 天 -2
ㄊㄧㄢ 添 -3
";

    fn unit(gold: &str, syl: &str) -> Unit {
        Unit { id: "u".into(), group: "G".into(), ctx: String::new(), syl: syl.split(' ').map(String::from).collect(), gold: gold.chars().collect(), start: 0, rep: false }
    }

    /// Plan 7th edition 3.1: scored output = before the corrections; the commit = after; p = 0 changes nothing; an
    /// unreachable gold is counted; the choices reach the confirmed record of the product session.
    #[test]
    fn user_model_corrects_before_the_commit() {
        let assets = Arc::new(Assets::from_dictionary_text(DICT, "").unwrap());
        let cc = CacheCfg { gamma: 4.0, window: 64, cap: None, source: twime_model::cache::CacheSource::Confirmed };
        let engine = Engine::new(assets, Config { lambda: 1.0, candidates: true, page_size: 1, cache: Some(cc), ..Config::default() });
        let script = |n: usize| {
            let mut v = vec![("LINE".to_string(), "0".to_string())];
            for _ in 0..n {
                v.push(("zh".into(), "ㄐㄧㄣ ㄊㄧㄢ".into()));
                v.push(("punct".into(), "，".into()));
            }
            v.push(("END".into(), String::new()));
            v
        };
        let us = vec![unit("金添", "ㄐㄧㄣ ㄊㄧㄢ"), unit("金添", "ㄐㄧㄣ ㄊㄧㄢ")];
        let s = |o: &Option<Vec<Option<char>>>| o.as_ref().map(|v| v.iter().map(|c| c.unwrap()).collect::<String>());
        // p = 1: both positions corrected (page size 1: the second item is on page 2), the document has the gold
        let (outs, doc, picks, un) = replay_user(&engine, &script(2), 0, String::new(), 2, 0, Some(User { units: &us, group: "G", p: 1.0, stop: None }));
        assert_eq!(s(&outs[0]).as_deref(), Some("今天"));
        assert_eq!(doc, "金添，金添，");
        assert_eq!(un, 0);
        assert!(picks >= 2);
        // p = 0: nothing corrected, output = commit
        let (outs, doc, picks, _) = replay_user(&engine, &script(2), 0, String::new(), 2, 0, Some(User { units: &us, group: "G", p: 0.0, stop: None }));
        assert_eq!((s(&outs[0]).as_deref(), s(&outs[1]).as_deref(), picks), (Some("今天"), Some("今天"), 0));
        assert_eq!(doc, "今天，今天，");
        // a gold that is not a candidate: unreachable, committed as converted
        let bad = vec![unit("金田", "ㄐㄧㄣ ㄊㄧㄢ")];
        let (_, doc, _, un) = replay_user(&engine, &script(1), 0, String::new(), 1, 0, Some(User { units: &bad, group: "G", p: 1.0, stop: None }));
        assert_eq!((doc.as_str(), un), ("金天，", 1));
    }
}
