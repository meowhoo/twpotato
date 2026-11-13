//! Synthetic user-lexicon latency: no corpus input and no text output.
use std::{collections::{HashMap, HashSet}, path::PathBuf, sync::Arc, time::Instant};
use twime_model::{Assets, CandParams, ModelPaths, pn::{PnCfg, PnLexicon}, user::UserLexicon};

struct Rng(u64);
impl Rng {
    fn pick(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((z ^ (z >> 31)) % n as u64) as usize
    }
}

#[derive(Default)]
struct Stats { ms: Vec<f64>, hits: usize }
impl Stats {
    fn print(&mut self, label: &str) {
        self.ms.sort_by(f64::total_cmp);
        let percentile = |p: usize| self.ms[(self.ms.len() * p).div_ceil(100).saturating_sub(1)];
        println!("{label} p50_ms={:.6} p99_ms={:.6} max_ms={:.6} hits={}",
            percentile(50), percentile(99), self.ms.last().unwrap(), self.hits);
    }
}

fn measure(a: &Assets, vocab: &twime_model::vocab::Vocab, segments: &[Vec<String>], lex: Option<&UserLexicon>, lambda: f64, stats: &mut Stats) {
    let params = CandParams { lambda, ..Default::default() };
    for syllables in segments {
        for end in 1..=syllables.len() {
            let t = Instant::now();
            let converted = a.convert("", &syllables[..end], &[], lambda);
            std::hint::black_box(&converted);
            stats.ms.push(t.elapsed().as_secs_f64() * 1000.0);
            if let Some(lex) = lex {
                let codes: Vec<_> = syllables[..end].iter().map(|s| u16::try_from(vocab.syllable_id(s)).ok()).collect();
                stats.hits += lex.hits(&codes, 2).len();
            }
            if end == syllables.len() {
                let t = Instant::now();
                std::hint::black_box(a.candidates(&converted, end, &params));
                stats.ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = HashMap::new();
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        if !["--data-txt", "--aliases", "--model", "--prior", "--pn", "--pn-b2", "--pn-b3", "--pn-h", "--lambda", "--seed"].contains(&flag.as_str()) {
            return Err("unknown argument".into());
        }
        let value = raw.next().ok_or("missing argument value")?;
        if args.insert(flag, value).is_some() { return Err("duplicate argument".into()); }
    }
    let required = |name: &str| args.get(name).map(PathBuf::from).ok_or("missing required asset argument");
    let paths = ModelPaths {
        data_txt: required("--data-txt")?, aliases: required("--aliases")?, model: required("--model")?,
        prior: args.get("--prior").map(PathBuf::from),
    };
    let number = |name: &str, default: f64| -> Result<f64, String> {
        let x = args.get(name).map_or(Ok(default), |v| v.parse().map_err(|_| "invalid numeric argument".to_string()))?;
        if !x.is_finite() { return Err("non-finite numeric argument".into()); }
        Ok(x)
    };
    let lambda = number("--lambda", 0.5)?;
    let seed = args.get("--seed").map_or(Ok(11), |s| s.parse::<u64>().map_err(|_| "invalid seed"))?;
    let mut rng = Rng(seed);
    let mut a = Assets::load(&paths).map_err(|_| "asset loading failed")?;
    if let Some(base) = args.get("--pn") {
        let cfg = PnCfg {
            b2: Some(number("--pn-b2", 10.0)?), b3: Some(number("--pn-b3", 10.0)?),
            h: args.get("--pn-h").map(|s| s.parse::<usize>().map_err(|_| "invalid PN limit")).transpose()?,
            cands: true,
        };
        let lex = PnLexicon::open(std::path::Path::new(base), &a).map_err(|_| "PN loading failed")?;
        a.set_pn(Some(Arc::new(lex)), cfg);
    } else if ["--pn-b2", "--pn-b3", "--pn-h"].iter().any(|k| args.contains_key(*k)) {
        return Err("PN options require --pn".into());
    }
    // Derive legal syllables and characters from the supplied dictionary, never a corpus.
    let mut legal = HashMap::<String, Vec<char>>::new();
    for (ch, readings) in a.dictionary().legal_readings() {
        for reading in readings {
            if a.is_legal_char(&reading, ch) { legal.entry(reading).or_default().push(ch); }
        }
    }
    for chars in legal.values_mut() { chars.sort_unstable(); chars.dedup(); }
    let mut syllables: Vec<_> = legal.keys().filter(|s| a.is_legal_syllable(s)).cloned().collect();
    syllables.sort();
    if syllables.is_empty() { return Err("no legal syllables".into()); }
    let segments: Vec<Vec<String>> = (0..200).map(|_| {
        let len = 4 + rng.pick(17);
        (0..len).map(|_| syllables[rng.pick(syllables.len())].clone()).collect()
    }).collect();
    let mut words = vec![];
    let mut keys = HashSet::new();
    let mut counts = HashMap::<Vec<String>, usize>::new();
    let mut attempts = 0;
    while words.len() < 1000 {
        attempts += 1;
        if attempts > 1_000_000 { return Err("insufficient distinct synthetic words".into()); }
        let len = 2 + rng.pick(3);
        let reading = if words.len() < 500 {
            let segment = &segments[rng.pick(segments.len())];
            let start = rng.pick(segment.len() - len + 1);
            segment[start..start + len].to_vec()
        } else {
            (0..len).map(|_| syllables[rng.pick(syllables.len())].clone()).collect()
        };
        if counts.get(&reading).copied().unwrap_or(0) >= 32 { continue; }
        let word: String = reading.iter().map(|s| {
            let chars = &legal[s]; chars[rng.pick(chars.len())]
        }).collect();
        if !keys.insert((reading.clone(), word.clone())) { continue; }
        let entry = a.user_word(&word, &reading, 10.0).map_err(|_| "synthetic word validation failed")?;
        *counts.entry(reading).or_default() += 1;
        words.push(entry);
    }
    let lex = Arc::new(a.build_user_lexicon(words));
    if lex.len() != 1000 { return Err("synthetic lexicon size mismatch".into()); }
    let vocab = twime_model::vocab::Vocab::new(a.dictionary());
    let (mut empty, mut user) = (Stats::default(), Stats::default());
    for round in 0..3 {
        for enabled in if round % 2 == 0 { [false, true] } else { [true, false] } {
            a.set_user(enabled.then(|| lex.clone()));
            if enabled { measure(&a, &vocab, &segments, Some(&lex), lambda, &mut user); }
            else { measure(&a, &vocab, &segments, None, lambda, &mut empty); }
        }
    }
    empty.print("empty");
    user.print("user1000");
    Ok(())
}

/// USERLEX 3.5: the same synthetic lexicon under the old rules (O: force off, bonus 10) and the new rules (U: force on,
/// `--bonus2`), interleaved over three rounds; conversion, candidate listing and reselection (rebeam with one lock) timed
/// separately; `--warm` operations per rule set and scenario are not recorded, then `--samples` of each operation.
///   userlat --compare --data-txt F --aliases F --model F [--prior F] [--pn BASE --pn-b2 X --pn-b3 X --pn-h N] [--int8]
///           [--bonus2 X] [--warm 200] [--samples 2000] [--seed N]
fn compare(args: &[String]) -> Result<(), String> {
    let mut m = HashMap::new();
    let mut int8 = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--compare" => {}
            "--int8" => int8 = true,
            f if ["--data-txt", "--aliases", "--model", "--prior", "--pn", "--pn-b2", "--pn-b3", "--pn-h", "--lambda", "--seed", "--bonus2", "--warm", "--samples", "--dump-main1000"].contains(&f) => {
                m.insert(f.to_string(), it.next().ok_or("missing argument value")?.clone());
            }
            _ => return Err("unknown argument".into()),
        }
    }
    let req = |k: &str| m.get(k).map(PathBuf::from).ok_or("missing required asset argument");
    let paths = ModelPaths { data_txt: req("--data-txt")?, aliases: req("--aliases")?, model: req("--model")?, prior: m.get("--prior").map(PathBuf::from) };
    let num = |k: &str, d: f64| -> Result<f64, String> { m.get(k).map_or(Ok(d), |v| v.parse().map_err(|_| "invalid numeric argument".to_string())) };
    let lambda = num("--lambda", 0.5)?;
    let bonus2 = num("--bonus2", twime_model::user::DEFAULT_BONUS)?;
    let warm = num("--warm", 200.0)? as usize;
    let samples = num("--samples", 2000.0)? as usize;
    let mut rng = Rng(num("--seed", 11.0)? as u64);
    let mut a = Assets::load(&paths).map_err(|_| "asset loading failed")?;
    if int8 { a.set_int8().map_err(|_| "int8 failed")?; }
    a.set_norm(twime_model::NormCfg::ALL);
    if let Some(base) = m.get("--pn") {
        let cfg = PnCfg { b2: Some(num("--pn-b2", 10.0)?), b3: Some(num("--pn-b3", 10.0)?), h: m.get("--pn-h").map(|s| s.parse::<usize>().map_err(|_| "invalid PN limit")).transpose()?, cands: true };
        let lex = PnLexicon::open(std::path::Path::new(base), &a).map_err(|_| "PN loading failed")?;
        a.set_pn(Some(Arc::new(lex)), cfg);
    }
    let vocab = twime_model::vocab::Vocab::new(a.dictionary());
    let mut legal = HashMap::<String, Vec<char>>::new();
    for (ch, readings) in a.dictionary().legal_readings() {
        for reading in readings {
            if a.is_legal_char(&reading, ch) { legal.entry(reading).or_default().push(ch); }
        }
    }
    for chars in legal.values_mut() { chars.sort_unstable(); chars.dedup(); }
    let mut syllables: Vec<_> = legal.keys().filter(|s| a.is_legal_syllable(s)).cloned().collect();
    syllables.sort();
    let segments: Vec<Vec<String>> = (0..300).map(|_| { let len = 6 + rng.pick(15); (0..len).map(|_| syllables[rng.pick(syllables.len())].clone()).collect() }).collect();
    let word_at = |rng: &mut Rng, reading: &[String]| -> String { reading.iter().map(|s| { let c = &legal[s]; c[rng.pick(c.len())] }).collect() };
    // Each scenario: its own lexicon (filled until the built lexicon has exactly the stated size) and its own input cycle
    // (the inputs the words were drawn from, so every recorded operation sees the intended hits).
    struct Scenario { name: &'static str, size: usize, inputs: Vec<Vec<String>>, lo: usize, hi: usize }
    let long: Vec<Vec<String>> = segments.iter().filter(|s| s.len() >= 8).cloned().collect();
    let scen = vec![
        Scenario { name: "main1000", size: 1000, inputs: segments.clone(), lo: 2, hi: 6 },
        Scenario { name: "overlap", size: 1000, inputs: segments[..3].to_vec(), lo: 3, hi: 5 },
        Scenario { name: "same32", size: 32, inputs: vec![segments[1][..5].to_vec()], lo: 5, hi: 5 },
        Scenario { name: "len8", size: 1000, inputs: long, lo: 8, hi: 8 },
        Scenario { name: "big50000", size: 50000, inputs: segments.clone(), lo: 2, hi: 8 },
    ];
    for sc in scen {
        let mut entries = vec![];
        let mut keys = HashSet::new();
        let mut per_reading = HashMap::<Vec<String>, usize>::new();
        let mut tries = 0usize;
        // FUZZY F1 gate 5: `--dump-main1000 FILE` writes the main1000 lexicon (same seed, same draws) as a userlex.tsv and
        // stops before any measurement; without the flag nothing changes
        let mut dumped: Vec<(String, String)> = vec![];
        while entries.len() < sc.size {
            tries += 1;
            if tries > 50_000_000 { return Err(format!("{}: cannot reach the lexicon size", sc.name)); }
            let seg = &sc.inputs[rng.pick(sc.inputs.len())];
            let len = sc.lo + rng.pick(sc.hi - sc.lo + 1);
            if len > seg.len() { continue; }
            let st = rng.pick(seg.len() - len + 1);
            let r = seg[st..st + len].to_vec();
            if per_reading.get(&r).copied().unwrap_or(0) >= 32 { continue; }
            let w = word_at(&mut rng, &r);
            if !keys.insert((w.clone(), r.clone())) { continue; }
            if let Ok(e) = a.user_word_opt(&w, &r, None) {
                if sc.name == "main1000" && m.contains_key("--dump-main1000") {
                    dumped.push((w.clone(), r.join(" ")));
                }
                *per_reading.entry(r).or_default() += 1;
                entries.push(e);
            }
        }
        let lex = Arc::new(a.build_user_lexicon(entries));
        if lex.len() != sc.size { return Err(format!("{}: lexicon size {} != {}", sc.name, lex.len(), sc.size)); }
        if let (Some(path), "main1000") = (m.get("--dump-main1000"), sc.name) {
            let mut t = String::from("# 地瓜輸入法 使用者詞庫 v1
");
            for (w, r) in &dumped {
                t.push_str(&format!("{w}	{r}
"));
            }
            std::fs::write(path, t).map_err(|_| "cannot write the lexicon dump")?;
            println!("main1000 dumped words={}", dumped.len());
            return Ok(());
        }
        a.set_user(Some(lex.clone()));
        let mut st: HashMap<(bool, &str), Stats> = HashMap::new();
        let mut hits: HashMap<bool, usize> = HashMap::new();
        let per_round = samples.div_ceil(3);
        for round in 0..3 {
            for force in if round % 2 == 0 { [false, true] } else { [true, false] } {
                a.set_user_rules(force, if force { bonus2 } else { twime_model::user::DEFAULT_BONUS }).map_err(|_| "rules")?;
                let (mut done, mut rec) = (0usize, 0usize);
                'outer: for seg in sc.inputs.iter().cycle() {
                    for end in 1..=seg.len() {
                        let t = Instant::now();
                        let conv = a.convert("", &seg[..end], &[], lambda);
                        let tc = t.elapsed().as_secs_f64() * 1000.0;
                        let t = Instant::now();
                        std::hint::black_box(a.candidates(&conv, end, &CandParams { lambda, ..Default::default() }));
                        let tk = t.elapsed().as_secs_f64() * 1000.0;
                        let mut lock = vec![None; end];
                        lock[0] = a.alternatives(&conv, &seg[..end], 0).get(1).copied();
                        let t = Instant::now();
                        std::hint::black_box(a.rebeam(&conv, &lock, lambda));
                        let tr = t.elapsed().as_secs_f64() * 1000.0;
                        done += 1;
                        if done <= warm { continue; }
                        for (op, ms) in [("convert", tc), ("cands", tk), ("reselect", tr)] {
                            st.entry((force, op)).or_default().ms.push(ms);
                        }
                        let codes: Vec<_> = seg[..end].iter().map(|s| u16::try_from(vocab.syllable_id(s)).ok()).collect();
                        *hits.entry(force).or_default() += lex.hits(&codes, 1).len();
                        rec += 1;
                        if rec >= per_round { break 'outer; }
                    }
                }
            }
        }
        for op in ["convert", "cands", "reselect"] {
            for force in [false, true] {
                let mut x = std::mem::take(st.get_mut(&(force, op)).unwrap());
                x.hits = hits[&force];
                x.print(&format!("{} {} {op} words={} samples={}", sc.name, if force { "U" } else { "O" }, lex.len(), 3 * per_round));
            }
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = if args.iter().any(|x| x == "--compare") { compare(&args) } else { run() };
    if let Err(e) = r { eprintln!("{e}"); std::process::exit(1); }
}
