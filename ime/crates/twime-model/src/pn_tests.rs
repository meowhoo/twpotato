//! Shared cases with train/ent_pn.py (tests/fixtures/pn-cases.json): the same inputs must give the same ids.

use super::*;
use serde_json::Value;

fn ids(v: &Value) -> Vec<u32> {
    v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect()
}

fn opt_f(v: &Value) -> Option<f64> {
    v.as_f64()
}

/// Runs one case through `usable_jumps` + `beam` exactly as `Assets::pn_select` does.
fn run(c: &Value) -> (Vec<u32>, Option<Vec<u32>>) {
    let sets: Vec<Vec<u32>> = c["sets"].as_array().unwrap().iter().map(ids).collect();
    let scores: Vec<Vec<f32>> = c["scores"].as_array().unwrap().iter().map(|r| r.as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect()).collect();
    let locked: Vec<Option<u32>> = c["locked"].as_array().unwrap().iter().map(|x| x.as_u64().map(|v| v as u32)).collect();
    let lambda = c["lambda"].as_f64().unwrap();
    let table: Vec<(Vec<u32>, u32, f64)> = match c["prior"].as_object() {
        Some(p) => p["table"].as_array().unwrap().iter().map(|t| (ids(&t[0]), t[1].as_u64().unwrap() as u32, t[2].as_f64().unwrap())).collect(),
        None => vec![],
    };
    let default = c["prior"].get("default").and_then(Value::as_f64).unwrap_or(0.0);
    let prior = |h: &[u32], ch: u32| table.iter().find(|(th, tc, _)| th == h && *tc == ch).map_or(default, |t| t.2);
    let cfg = PnCfg { b2: opt_f(&c["cfg"]["b2"]), b3: opt_f(&c["cfg"]["b3"]), h: c["cfg"]["h"].as_u64().map(|x| x as usize), cands: true };
    let fold = c["fold"].as_array().map(|f| (f[0].as_u64().unwrap() as u32, f[1].as_u64().unwrap() as u32));
    let hits: Vec<(usize, Vec<u32>)> = c["hits"].as_array().unwrap().iter().map(|h| (h[0].as_u64().unwrap() as usize, ids(&h[1]))).collect();
    let lp = log_softmax(&scores);
    let (cs, ls): (Vec<Vec<u32>>, Vec<Vec<f64>>) = scores
        .iter()
        .zip(&sets)
        .zip(&locked)
        .map(|((sc, s), lk)| match lk {
            Some(id) => (vec![*id], vec![0.0]),
            None => prior::top_k(sc, s),
        })
        .unzip();
    let jumps = usable_jumps(&hits, &sets, &lp, &locked, &cfg, fold);
    let got = beam(&cs, &ls, &sets, &lp, &locked, &jumps, lambda, &prior).unwrap();
    // The plain prior beam on the same inputs (no jumps), for the no-hit case.
    let plain = if lambda != 0.0 { Some(beam(&cs, &ls, &sets, &lp, &locked, &[], lambda, &prior).unwrap()) } else { None };
    (got, plain)
}

#[test]
fn shared_cases() {
    let cases: Value = serde_json::from_str(include_str!("../tests/fixtures/pn-cases.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let (got, plain) = run(c);
        let name = c["name"].as_str().unwrap();
        match &c["expect"] {
            Value::String(s) if s == "prior_beam" => assert_eq!(Some(got), plain, "{name}"),
            e => assert_eq!(got, ids(e), "{name}"),
        }
    }
}

#[test]
fn no_jumps_equals_prior_beam_on_random_inputs() {
    // The PN beam without jumps is the prior beam (same order of float operations, same pruning).
    let mut seed = 99u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..300 {
        let n = 1 + (rnd() % 10) as usize;
        let sets: Vec<Vec<u32>> = (0..n).map(|i| (0..(1 + rnd() % 12) as u32).map(|k| 100 * i as u32 + k).collect()).collect();
        let scores: Vec<Vec<f32>> = sets.iter().map(|s| s.iter().map(|_| ((rnd() % 7) as f32) * 0.5).collect()).collect();
        let locked: Vec<Option<u32>> = sets.iter().map(|s| if rnd() % 5 == 0 { Some(s[(rnd() as usize) % s.len()]) } else { None }).collect();
        let lp = log_softmax(&scores);
        let (cs, ls): (Vec<Vec<u32>>, Vec<Vec<f64>>) = scores
            .iter()
            .zip(&sets)
            .zip(&locked)
            .map(|((sc, s), lk)| match lk {
                Some(id) => (vec![*id], vec![0.0]),
                None => prior::top_k(sc, s),
            })
            .unzip();
        let pf = |h: &[u32], c: u32| -(((h.iter().sum::<u32>() + 3 * c) % 11) as f64) * 0.25;
        let a = beam(&cs, &ls, &sets, &lp, &locked, &[], 0.5, &pf);
        // reference: the same recurrence written as prior::beam does
        let mut states: Vec<(f64, Vec<u32>)> = vec![(0.0, vec![])];
        for (c, l) in cs.iter().zip(&ls) {
            let mut next = vec![];
            for (sc, seq) in &states {
                let hist = &seq[seq.len().saturating_sub(3)..];
                for (&x, &y) in c.iter().zip(l) {
                    let mut s2 = seq.clone();
                    s2.push(x);
                    next.push(((sc + y) + 0.5 * pf(hist, x), s2));
                }
            }
            next.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then_with(|| a.1.cmp(&b.1)));
            next.truncate(BEAM);
            states = next;
        }
        assert_eq!(a, states.into_iter().next().map(|s| s.1));
    }
}

#[test]
fn settings_parse() {
    let mut m = HashMap::new();
    m.insert("b2".to_string(), "off".to_string());
    m.insert("b3".to_string(), "2.5".to_string());
    m.insert("h".to_string(), "8".to_string());
    let c = PnCfg::from_settings(&m).unwrap();
    assert_eq!(c, PnCfg { b2: None, b3: Some(2.5), h: Some(8), cands: true });
    assert_eq!(c.bonus(2), None);
    assert_eq!(c.bonus(7), Some(2.5));
    assert!(parse_bonus("x").is_err());
}
