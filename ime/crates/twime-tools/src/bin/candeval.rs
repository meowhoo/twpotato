//! N3b evaluation of word-level candidates. Prints counts and my own homophone examples
//! only; never any dev sentence text.
//!
//! Usage:
//!   candeval eval  <tune|report|all> <data.txt> <aliases> <units> <docs> <model> <prior> <variant>...
//!   candeval side  <tune|report|all> <assets as above>                      (effect of selecting the current item)
//!   candeval lat   <assets as above> [n] [cpu]                              (core latency, paired)
//!   candeval homo  <homophone-cases.tsv> <data.txt> <aliases> <model> <prior> <variant>...
//! PN: env CANDEVAL_PN=<settings> applies the settings' NORM (norm=all) and, unless `pn=off`, the
//! lexicon and its decoder / candidate settings (b2, b3, h, cands) to every Assets this tool loads; the rarer-word check of
//! `homo` then also counts lexicon words (lexicon-only words with the smallest data.txt log10 of their length).
//! Variants: B0 (single characters ordered by model score, today's behavior), B1 (new scoring, singles only),
//! C:<beta> (singles + words), V2:<beta> (words and singles grouped by span, longest first).

use std::{fs, time::Instant};

use twime_model::{Assets, CandParams, Candidate, Conversion, ModelPaths};
use twime_tools::cases::{Case, load_cases};

const LAMBDA: f64 = 0.5;
const NS: [usize; 4] = [1, 3, 5, 9];

#[derive(Clone, Debug)]
enum Variant {
    B0,
    B1,
    C(f64),
    V2(f64),
}

fn parse_variant(s: &str) -> Variant {
    let beta = |x: &str| x.parse::<f64>().unwrap_or_else(|_| panic!("bad beta in {s}"));
    match s.split_once(':') {
        None if s == "B0" => Variant::B0,
        None if s == "B1" => Variant::B1,
        Some(("C", b)) => Variant::C(beta(b)),
        Some(("V2", b)) => Variant::V2(beta(b)),
        _ => panic!("bad variant {s}"),
    }
}

fn name(v: &Variant) -> String {
    match v {
        Variant::B0 => "B0".into(),
        Variant::B1 => "B1".into(),
        Variant::C(b) => format!("C:{b}"),
        Variant::V2(b) => format!("V2:{b}"),
    }
}

/// The list the window would show, as (text chars, span) in order.
fn list(assets: &Assets, conv: &Conversion, end: usize, v: &Variant) -> Vec<(Vec<char>, usize)> {
    let params = |max_span, beta| CandParams { lambda: LAMBDA, beta, max_span, word_cap: 40, style_tai: false };
    let from = |c: Vec<Candidate>| c.into_iter().map(|c| (c.text.chars().collect::<Vec<_>>(), c.span)).collect::<Vec<_>>();
    match v {
        Variant::B0 => assets.alternatives(conv, conv.syllables(), end - 1).into_iter().map(|c| (vec![c], 1)).collect(),
        Variant::B1 => from(assets.candidates(conv, end, &params(1, 0.0))),
        Variant::C(b) => from(assets.candidates(conv, end, &params(6, *b))),
        Variant::V2(b) => {
            let mut c = from(assets.candidates(conv, end, &params(6, *b)));
            if c.len() > 1 {
                // The current item stays first; the rest are grouped by span, longest first (stable inside a group).
                let first = c.remove(0);
                c.sort_by(|a, b| b.1.cmp(&a.1));
                c.insert(0, first);
            }
            c
        }
    }
}

/// Positions (leftmost run) where the output differs from the reference: (a, b) inclusive.
fn first_run(x: &[Option<char>], t: &[char]) -> Option<(usize, usize)> {
    let a = (0..t.len()).find(|&i| x[i] != Some(t[i]))?;
    let mut b = a;
    while b + 1 < t.len() && x[b + 1] != Some(t[b + 1]) {
        b += 1;
    }
    Some((a, b))
}

fn fits(item: &(Vec<char>, usize), t: &[char], end: usize) -> bool {
    item.1 <= end && item.0[..] == t[end - item.1..end]
}

fn fnv_even(doc_id: &str) -> bool {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in doc_id.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h % 2 == 0
}

fn load(a: &[String], first: usize) -> Assets {
    let mut x = Assets::load(&ModelPaths { data_txt: a[first].clone().into(), aliases: a[first + 1].clone().into(), model: a[first + 4].clone().into(), prior: Some(a[first + 5].clone().into()) }).unwrap();
    apply_pn(&mut x);
    x
}

/// CANDEVAL_PN (see the header).
fn apply_pn(x: &mut Assets) {
    let Ok(sp) = std::env::var("CANDEVAL_PN") else { return apply_gen(x) };
    let m = twime_model::pn::read_settings(std::path::Path::new(&sp)).expect("pn settings");
    if m.get("norm").map_or("all", String::as_str) == "all" {
        x.set_norm(twime_model::NormCfg::ALL);
    }
    if m.get("pn").map_or("on", String::as_str) == "on" {
        let lex = twime_model::pn::PnLexicon::open(std::path::Path::new(&m["lexicon"]), x).expect("pn lexicon");
        x.set_pn(Some(std::sync::Arc::new(lex)), twime_model::pn::PnCfg::from_settings(&m).expect("pn cfg"));
    }
    eprintln!("CANDEVAL_PN {sp}: norm {:?} pn {:?}", x.norm(), x.pn().map(|p| &p.1));
    apply_gen(x);
}

/// CANDEVAL_GEN=<lexicon base>: the general-word layer in the candidate list; the
/// rarer-word check of `homo` also counts its words. Applied after CANDEVAL_PN (also when CANDEVAL_PN is unset).
fn apply_gen(x: &mut Assets) {
    let Ok(base) = std::env::var("CANDEVAL_GEN") else { return };
    if base.is_empty() || base == "off" {
        return;
    }
    let lex = twime_model::pn::PnLexicon::open(std::path::Path::new(&base), x).expect("gen lexicon");
    eprintln!("CANDEVAL_GEN {base}: {} words", lex.len());
    x.set_gen(Some(std::sync::Arc::new(lex)));
}

/// A same-length word ranked before the expected one has a smaller log10 than the expected word's own (`entries`: the
/// words of the span's reading with their log10, data.txt and the PN lexicon).
fn rarer_ahead(list: &[(Vec<char>, usize)], rank: usize, len: usize, entries: &[(String, f64)], own: f64) -> bool {
    list[..rank - 1].iter().filter(|it| it.1 == len).any(|it| entries.iter().any(|(w, l)| w.chars().eq(it.0.iter().copied()) && *l < own))
}

fn select_cases(split: &str, cases: Vec<Case>) -> Vec<Case> {
    cases.into_iter().filter(|c| match split { "tune" => fnv_even(&c.doc_id), "report" => !fnv_even(&c.doc_id), _ => true }).collect()
}

struct Unit {
    case: Case,
    refs: Vec<char>,
    conv: Conversion,
}

fn prepare(assets: &Assets, cases: Vec<Case>) -> (Vec<Unit>, usize, usize) {
    let (mut skipped_len, mut incomplete) = (0, 0);
    let mut out = vec![];
    for c in cases {
        let refs: Vec<char> = c.text.chars().collect();
        if refs.len() != c.symbols.len() {
            skipped_len += 1;
            continue;
        }
        let conv = assets.convert(&c.context, &c.symbols, &[], LAMBDA);
        if !conv.complete() {
            incomplete += 1;
            continue;
        }
        out.push(Unit { case: c, refs, conv });
    }
    (out, skipped_len, incomplete)
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 { "n/a".into() } else { format!("{:.2}% ({n}/{d})", 100.0 * n as f64 / d as f64) }
}

fn eval(a: &[String]) {
    let split = a[2].as_str();
    let assets = load(a, 3);
    let variants: Vec<Variant> = a[9..].iter().map(|s| parse_variant(s)).collect();
    let cases = select_cases(split, load_cases(&a[5], &a[6]).unwrap());
    let total_cases = cases.len();
    let (units, skipped, incomplete) = prepare(&assets, cases);
    let errors: Vec<&Unit> = units.iter().filter(|u| u.conv.chars.iter().zip(&u.refs).any(|(x, t)| *x != Some(*t))).collect();
    let multi: Vec<&&Unit> = errors.iter().filter(|u| { let x = &u.conv.chars; let (a, b) = first_run(x, &u.refs).unwrap(); b > a }).collect();
    println!("split={split} cases={total_cases} convertible={} skipped_len={skipped} incomplete={incomplete} error_units={} first_run_len_ge2={}", units.len(), errors.len(), multi.len());
    println!("lambda={LAMBDA} correct_units={}", units.len() - errors.len());
    for v in &variants {
        // 1. end-of-run hit @N, 2. whole run in one selection, 3. word hits.
        let mut hit = [0usize; 4];
        let mut run_hit = [0usize; 4];
        let mut word_hit = [0usize; 4];
        let (mut ref_word_cases, mut ref_word_hit) = (0usize, [0usize; 4]);
        for u in &errors {
            let (a0, b) = first_run(&u.conv.chars, &u.refs).unwrap();
            let end = b + 1;
            let l = list(&assets, &u.conv, end, v);
            // Reference word ending at b that the dictionary has.
            let ref_lens: Vec<usize> = (2..=6.min(end)).filter(|&len| assets.dictionary().lookup(&u.case.symbols[end - len..end]).iter().any(|e| e.word.chars().eq(u.refs[end - len..end].iter().copied()))).collect();
            if !ref_lens.is_empty() {
                ref_word_cases += 1;
            }
            for (ni, &n) in NS.iter().enumerate() {
                let top = &l[..l.len().min(n)];
                if top.iter().any(|it| fits(it, &u.refs, end)) {
                    hit[ni] += 1;
                }
                if b > a0 && top.iter().any(|it| it.1 >= b - a0 + 1 && fits(it, &u.refs, end)) {
                    run_hit[ni] += 1;
                }
                if top.iter().any(|it| it.1 >= 2 && fits(it, &u.refs, end)) {
                    word_hit[ni] += 1;
                }
                if top.iter().any(|it| ref_lens.contains(&it.1) && fits(it, &u.refs, end)) {
                    ref_word_hit[ni] += 1;
                }
            }
        }
        println!("== {}", name(v));
        for (ni, &n) in NS.iter().enumerate() {
            println!("  @{n}: end-of-run fix {} | whole run in one pick {} | fix by a word {} | dictionary reference word shown {}", pct(hit[ni], errors.len()), pct(run_hit[ni], multi.len()), pct(word_hit[ni], errors.len()), pct(ref_word_hit[ni], ref_word_cases));
        }
        // 4. Repair simulation.
        let (mut ok, mut fail, mut steps_sum) = (0usize, 0usize, 0usize);
        let mut within = [0usize; 3];
        for u in &errors {
            let mut locks: Vec<Option<char>> = vec![None; u.refs.len()];
            let mut conv = u.conv.clone();
            let mut steps = 0;
            let done = loop {
                let Some((_a, b)) = first_run(&conv.chars, &u.refs) else { break true };
                if steps >= 6 {
                    break false;
                }
                let end = b + 1;
                let l = list(&assets, &conv, end, v);
                let Some(it) = l.iter().take(9).find(|it| fits(it, &u.refs, end)) else { break false };
                for k in 0..it.1 {
                    locks[end - it.1 + k] = Some(it.0[k]);
                }
                conv = assets.rebeam(&conv, &locks, LAMBDA);
                steps += 1;
            };
            if done {
                ok += 1;
                steps_sum += steps;
                for (i, w) in within.iter_mut().enumerate() {
                    *w += usize::from(steps <= i + 1);
                }
            } else {
                fail += 1;
            }
        }
        println!("  repair (first page-and-a-half = top 9, <=6 picks, oracle): fixed {} failed {} | <=1 pick {} <=2 picks {} <=3 picks {} | mean picks of fixed {:.3}", pct(ok, errors.len()), pct(fail, errors.len()), pct(within[0], errors.len()), pct(within[1], errors.len()), pct(within[2], errors.len()), steps_sum as f64 / ok.max(1) as f64);
    }
}

/// Effect of choosing the current item at every cursor position: does any character outside the span change?
fn side(a: &[String]) {
    let split = a[2].as_str();
    let assets = load(a, 3);
    let cases = select_cases(split, load_cases(&a[5], &a[6]).unwrap());
    let (units, _, _) = prepare(&assets, cases);
    let (mut positions, mut changed_pos, mut changed_units, mut changed_inside) = (0usize, 0usize, 0usize, 0usize);
    for u in &units {
        let n = u.conv.chars.len();
        let mut any = false;
        for end in 1..=n {
            let l = assets.candidates(&u.conv, end, &CandParams { lambda: LAMBDA, ..CandParams::default() });
            let Some(cur) = l.first().filter(|c| c.current) else { continue };
            let mut locks: Vec<Option<char>> = vec![None; n];
            for (k, ch) in cur.text.chars().enumerate() {
                locks[end - cur.span + k] = Some(ch);
            }
            let re = assets.rebeam(&u.conv, &locks, LAMBDA);
            positions += 1;
            let outside = (0..n).any(|i| (i < end - cur.span || i >= end) && re.chars[i] != u.conv.chars[i]);
            let inside = (end - cur.span..end).any(|i| re.chars[i] != u.conv.chars[i]);
            changed_pos += usize::from(outside);
            changed_inside += usize::from(inside);
            any |= outside;
        }
        changed_units += usize::from(any);
    }
    println!("split={split} units={} positions={positions} changed_outside_span: positions {} units {} | changed_inside_span positions {}", units.len(), pct(changed_pos, positions), pct(changed_units, units.len()), changed_inside);
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn lat(a: &[String]) {
    let assets = load(a, 2);
    let n: usize = a.get(8).and_then(|s| s.parse().ok()).unwrap_or(5000);
    if let Some(cpu) = a.get(9).and_then(|s| s.parse::<u32>().ok()) {
        unsafe {
            use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessAffinityMask};
            let _ = SetProcessAffinityMask(GetCurrentProcess(), 1usize << cpu);
        }
    }
    let cases = load_cases(&a[4], &a[5]).unwrap();
    let (units, _, incomplete) = prepare(&assets, cases);
    assert!(!units.is_empty());
    println!("units available {} (incomplete {incomplete}); measuring {n} samples (units drawn at random with replacement)", units.len());
    let mut seed = 12345u64;
    let mut rnd = |m: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % m as u64) as usize
    };
    let c = CandParams { lambda: LAMBDA, ..CandParams::default() };
    let s1 = CandParams { max_span: 1, ..c.clone() };
    let (mut old, mut single, mut full, mut diff, mut diff_old, mut reb, mut inf, mut sel_diff) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    for _ in 0..n {
        let u = &units[rnd(units.len())];
        let nsyl = u.conv.chars.len();
        let end = 1 + rnd(nsyl);
        // Untimed warm-up of both paths (the prior tables are then in cache, as they are right after a conversion), then
        // the two timed calls in random order so that neither always runs second.
        std::hint::black_box((assets.candidates(&u.conv, end, &s1), assets.candidates(&u.conv, end, &c)));
        let t = Instant::now();
        let x = assets.alternatives(&u.conv, &u.case.symbols, end - 1);
        let to = t.elapsed().as_secs_f64() * 1e6;
        old.push(to);
        let words_first = rnd(2) == 0;
        let (mut ts, mut tf) = (0.0, 0.0);
        for pass in 0..2 {
            if (pass == 0) == words_first {
                let t = Instant::now();
                let z = assets.candidates(&u.conv, end, &c);
                tf = t.elapsed().as_secs_f64() * 1e6;
                std::hint::black_box(z);
            } else {
                let t = Instant::now();
                let y = assets.candidates(&u.conv, end, &s1);
                ts = t.elapsed().as_secs_f64() * 1e6;
                std::hint::black_box(y);
            }
        }
        single.push(ts);
        full.push(tf);
        diff.push(tf - ts);
        diff_old.push(tf - to);
        std::hint::black_box(x);
        let mut locks = vec![None; nsyl];
        locks[end - 1] = u.conv.chars[end - 1];
        let t = Instant::now();
        let r = assets.rebeam(&u.conv, &locks, LAMBDA);
        let tr = t.elapsed().as_secs_f64() * 1e6;
        reb.push(tr);
        let t = Instant::now();
        let r2 = assets.convert(&u.case.context, &u.case.symbols, &locks, LAMBDA);
        let ti = t.elapsed().as_secs_f64() * 1e6;
        inf.push(ti);
        sel_diff.push(tr - ti);
        std::hint::black_box((r, r2));
    }
    let row = |name: &str, v: &mut Vec<f64>| println!("{name:<44} n={} p50 {:9.1} us  p95 {:9.1} us  p99 {:9.1} us  max {:9.1} us", v.len(), percentile(v, 0.5), percentile(v, 0.95), percentile(v, 0.99), percentile(v, 1.0));
    row("open: old single-character list (alternatives)", &mut old);
    row("open: new scoring, singles only (max_span 1)", &mut single);
    row("open: singles + words (max_span 6)", &mut full);
    row("open: paired difference (words - singles)", &mut diff);
    row("open: paired difference (words - old list)", &mut diff_old);
    row("select: rebeam (locks only)", &mut reb);
    row("select: old path = full convert (inference)", &mut inf);
    row("select: paired difference (rebeam - full convert)", &mut sel_diff);
}

fn homo(a: &[String]) {
    let mut assets = Assets::load(&ModelPaths { data_txt: a[3].clone().into(), aliases: a[4].clone().into(), model: a[5].clone().into(), prior: Some(a[6].clone().into()) }).unwrap();
    apply_pn(&mut assets);
    let variants: Vec<Variant> = a[7..].iter().map(|s| parse_variant(s)).collect();
    let text = fs::read_to_string(&a[2]).unwrap();
    for v in &variants {
        println!("== {}", name(v));
        // neutral: expected word first / a rarer same-reading word ahead of it / present in the list at all
        let (mut n_cases, mut n_first, mut n_rarer, mut n_present) = (0, 0, 0, 0);
        // domain: expected word first / in the top 3 / present; diagnostic rank (current item not pinned) first
        let (mut d_cases, mut d_first, mut d_top3, mut d_present, mut d_diag_first) = (0, 0, 0, 0, 0);
        let (mut cases, mut start_ok) = (0, 0);
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let f: Vec<&str> = line.split('\t').collect();
            let (id, kind, word, sentence, symbols) = (f[0], f[1], f[2], f[3], f[4]);
            let syl: Vec<String> = symbols.split(' ').map(str::to_string).collect();
            let chars: Vec<char> = sentence.chars().collect();
            assert_eq!(syl.len(), chars.len(), "{id}: symbol count");
            let w: Vec<char> = word.chars().collect();
            let start = chars.windows(w.len()).position(|x| x == &w[..]).unwrap_or_else(|| panic!("{id}: word not in sentence"));
            let end = start + w.len();
            let conv = assets.convert("", &syl, &[], LAMBDA);
            let got: String = conv.chars.iter().map(|c| c.unwrap_or('?')).collect();
            let right = got == sentence;
            let l = list(&assets, &conv, end, v);
            let rank = l.iter().position(|it| it.1 == w.len() && it.0 == w).map(|i| i + 1);
            // Same-reading words by dictionary probability.
            // Same-reading words: data.txt, plus the PN lexicon (lexicon-only words with the substitute log10).
            let mut entries: Vec<(String, f64)> = assets.dictionary().lookup(&syl[start..end]).iter().map(|e| (e.word.clone(), e.log10)).collect();
            for (pw, pl, _) in assets.pn_words(&syl[start..end]).into_iter().chain(assets.gen_words(&syl[start..end])) {
                if !entries.iter().any(|(x, _)| *x == pw) {
                    entries.push((pw, pl));
                }
            }
            let own = entries.iter().find(|e| e.0 == word).map(|e| e.1);
            let rarer_ahead = match (rank, own) {
                (Some(r), Some(own)) => rarer_ahead(&l, r, w.len(), &entries, own),
                _ => false,
            };
            // Diagnostic (not an input behavior): rank among the words of this span by score alone, the current item not pinned.
            let diag = match v {
                Variant::C(b) | Variant::V2(b) => {
                    let c = assets.candidates(&conv, end, &CandParams { lambda: LAMBDA, beta: *b, max_span: 6, word_cap: 40, style_tai: false });
                    let mut words: Vec<&Candidate> = c.iter().filter(|x| x.span == w.len()).collect();
                    words.sort_by(|x, y| y.score.total_cmp(&x.score));
                    words.iter().position(|x| x.text == word).map(|i| i + 1)
                }
                _ => None,
            };
            let top: Vec<String> = l.iter().take(9).map(|it| it.0.iter().collect::<String>()).collect();
            println!("  {id:<12} {kind:<7} expected {word} start_ok={right} rank_in_list={} diag_rank_without_pin={} dict_has_word={} rarer_word_ahead={rarer_ahead} top9=[{}]", rank.map_or("-".into(), |r| r.to_string()), diag.map_or("-".into(), |r| r.to_string()), own.is_some(), top.join(" "));
            cases += 1;
            start_ok += usize::from(right);
            if kind == "neutral" {
                n_cases += 1;
                n_first += usize::from(rank == Some(1));
                n_rarer += usize::from(rarer_ahead);
                n_present += usize::from(rank.is_some());
            } else {
                d_cases += 1;
                d_first += usize::from(rank == Some(1));
                d_top3 += usize::from(rank.is_some_and(|r| r <= 3));
                d_present += usize::from(rank.is_some());
                d_diag_first += usize::from(diag == Some(1));
            }
        }
        println!("  summary: cases {cases}, initial conversion right {start_ok}");
        println!("  neutral (common word expected): expected first {n_first}/{n_cases} | a rarer same-reading word ahead of it {n_rarer}/{n_cases} | expected word in the list {n_present}/{n_cases}");
        println!("  domain (rarer word expected): expected first {d_first}/{d_cases} | in top 3 {d_top3}/{d_cases} | in the list {d_present}/{d_cases} | diagnostic first by score alone {d_diag_first}/{d_cases}");
    }
}

/// rebeam against a full conversion after chains of random selections and unlocks on dev units (real weights, full prior).
fn chain(a: &[String]) {
    let assets = load(a, 2);
    let n: usize = a.get(8).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let cases = load_cases(&a[4], &a[5]).unwrap();
    let (units, _, _) = prepare(&assets, cases);
    let mut seed = 777u64;
    let mut rnd = |m: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % m as u64) as usize
    };
    let (mut steps, mut mismatches, mut selections) = (0usize, 0usize, 0usize);
    for _ in 0..n {
        let u = &units[rnd(units.len())];
        let len = u.conv.chars.len();
        let mut locks: Vec<Option<char>> = vec![None; len];
        let mut conv = u.conv.clone();
        for _ in 0..6 {
            if rnd(10) < 7 {
                let end = 1 + rnd(len);
                let l = assets.candidates(&conv, end, &CandParams { lambda: LAMBDA, ..CandParams::default() });
                if l.is_empty() {
                    continue;
                }
                let it = &l[rnd(l.len())];
                for (k, ch) in it.text.chars().enumerate() {
                    locks[end - it.span + k] = Some(ch);
                }
                selections += 1;
            } else {
                locks[rnd(len)] = None;
            }
            conv = assets.rebeam(&conv, &locks, LAMBDA);
            let direct = assets.convert(&u.case.context, &u.case.symbols, &locks, LAMBDA);
            steps += 1;
            mismatches += usize::from(direct.chars != conv.chars);
        }
    }
    println!("units {n} steps {steps} selections {selections} rebeam_vs_full_convert_mismatches {mismatches}");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("chain") if a.len() >= 8 => chain(&a),
        Some("eval") if a.len() >= 10 => eval(&a),
        Some("side") if a.len() >= 9 => side(&a),
        Some("lat") if a.len() >= 8 => lat(&a),
        Some("homo") if a.len() >= 8 => homo(&a),
        _ => {
            eprintln!("usage: see the header of candeval.rs");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rarer_ahead;

    #[test]
    fn a_lexicon_only_word_ahead_of_the_common_word_counts_as_rarer() {
        // the list shows a lexicon-only word (substitute log10 = the smallest data.txt log10) before the expected word
        let list = vec![("薏人".chars().collect::<Vec<char>>(), 2), ("藝人".chars().collect(), 2), ("藝".chars().collect(), 1)];
        let entries = vec![("藝人".to_string(), -4.0), ("薏人".to_string(), -8.5)];
        assert!(rarer_ahead(&list, 2, 2, &entries, -4.0));
        // not counted when it is behind, or when the word ahead is the more common one
        assert!(!rarer_ahead(&list, 1, 2, &entries, -4.0));
        let entries2 = vec![("藝人".to_string(), -4.0), ("薏人".to_string(), -3.0)];
        assert!(!rarer_ahead(&list, 2, 2, &entries2, -4.0));
    }
}
