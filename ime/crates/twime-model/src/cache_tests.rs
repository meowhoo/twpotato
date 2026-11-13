//! CACHE: the cache beam against a frozen copy of the pre-cache `beam` (every surviving
//! state, scores compared bit for bit) and the shared cases with train/cache_ref.py (tests/fixtures/cache-cases.json).

use super::*;
use crate::cache::CacheSet;
use serde_json::Value;

/// The `beam` of pn.rs before CACHE (pn.rs.pre-cache), unchanged except that it returns every surviving final state.
#[allow(clippy::too_many_arguments)]
fn old_beam_states(
    cands: &[Vec<u32>],
    lps: &[Vec<f64>],
    sets: &[Vec<u32>],
    lp: &[Vec<f64>],
    locked: &[Option<u32>],
    jumps: &[Jump],
    lambda: f64,
    prior: &dyn Fn(&[u32], u32) -> f64,
) -> Option<Vec<(f64, Vec<u32>)>> {
    let n = cands.len();
    if cands.iter().any(Vec::is_empty) {
        return None;
    }
    let score_of = |i: usize, id: u32| -> f64 {
        if locked[i].is_some() {
            return 0.0;
        }
        match sets[i].binary_search(&id) {
            Ok(j) => lp[i][j],
            Err(_) => f64::NEG_INFINITY,
        }
    };
    let mut pending: Vec<Vec<(f64, Vec<u32>, usize)>> = vec![vec![]; n + 1];
    pending[0].push((0.0, vec![], 0));
    let mut by_start: Vec<Vec<&Jump>> = vec![vec![]; n];
    for j in jumps {
        by_start[j.start].push(j);
    }
    for p in 0..=n {
        let mut next = std::mem::take(&mut pending[p]);
        next.sort_by(|a, b| a.1.cmp(&b.1).then(b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)).then(a.2.cmp(&b.2)));
        next.dedup_by(|b, a| a.1 == b.1);
        next.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
        next.truncate(BEAM);
        if p == n {
            return Some(next.into_iter().map(|s| (s.0, s.1)).collect());
        }
        for (sc, seq) in next.iter().map(|(s, q, _)| (*s, q)) {
            let hist = &seq[seq.len().saturating_sub(3)..];
            for (&c, &l) in cands[p].iter().zip(&lps[p]) {
                let pr = if lambda != 0.0 { prior(hist, c) } else { 0.0 };
                let mut s2 = seq.clone();
                s2.push(c);
                pending[p + 1].push(((sc + l) + lambda * pr, s2, 0));
            }
            for j in &by_start[p] {
                let mut s2 = seq.clone();
                let mut s = sc;
                for (k, &id) in j.ids.iter().enumerate() {
                    let pr = if lambda != 0.0 { prior(&s2[s2.len().saturating_sub(3)..], id) } else { 0.0 };
                    s = (s + score_of(p + k, id)) + lambda * pr;
                    s2.push(id);
                }
                pending[p + j.ids.len()].push((s + j.bonus, s2, 1 + (MAX_LEN - j.ids.len())));
            }
        }
    }
    None
}

type States = Option<Vec<(u64, Vec<u32>)>>;

fn bits(v: &Option<Vec<(f64, Vec<u32>)>>) -> States {
    v.as_ref().map(|v| v.iter().map(|(s, q)| (s.to_bits(), q.clone())).collect())
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

struct Case {
    cs: Vec<Vec<u32>>,
    ls: Vec<Vec<f64>>,
    sets: Vec<Vec<u32>>,
    lp: Vec<Vec<f64>>,
    locked: Vec<Option<u32>>,
    jumps: Vec<Jump>,
    lambda: f64,
}

/// Random beam inputs with jumps, locks, ties (scores on a 0.5 grid) and lambda 0 / 0.5.
fn random_case(r: &mut Rng) -> Case {
    let n = 1 + (r.next() % 9) as usize;
    let sets: Vec<Vec<u32>> = (0..n).map(|i| (0..(1 + r.next() % 12) as u32).map(|k| 100 * i as u32 + k).collect()).collect();
    let scores: Vec<Vec<f32>> = sets.iter().map(|s| s.iter().map(|_| ((r.next() % 7) as f32) * 0.5).collect()).collect();
    let locked: Vec<Option<u32>> = sets.iter().map(|s| if r.next() % 6 == 0 { Some(s[(r.next() as usize) % s.len()]) } else { None }).collect();
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
    let mut hits = vec![];
    for _ in 0..(r.next() % 6) {
        let s = (r.next() as usize) % n;
        let l = 2 + (r.next() as usize) % 3;
        if s + l <= n {
            hits.push((s, (0..l).map(|k| sets[s + k][(r.next() as usize) % sets[s + k].len()]).collect::<Vec<u32>>()));
        }
    }
    let cfg = PnCfg { b2: Some(0.0), b3: Some(8.0), h: Some(4), cands: true };
    let jumps = usable_jumps(&hits, &sets, &lp, &locked, &cfg, None);
    let lambda = if r.next() % 3 == 0 { 0.0 } else { 0.5 };
    Case { cs, ls, sets, lp, locked, jumps, lambda }
}

fn pf(h: &[u32], c: u32) -> f64 {
    -(((h.iter().sum::<u32>() + 3 * c) % 11) as f64) * 0.25
}

#[test]
fn no_cache_and_no_hit_equal_the_old_beam_bit_for_bit() {
    let mut r = Rng(20261603);
    // a non-empty set whose pairs never occur (ids >= 10^6)
    let far = CacheSet::from_pairs(&[(1_000_000, 1_000_001), (1_000_002, 1_000_003)], (5_000_000, 5_000_001), 4.0, Some(2));
    for _ in 0..3000 {
        let c = random_case(&mut r);
        let old = old_beam_states(&c.cs, &c.ls, &c.sets, &c.lp, &c.locked, &c.jumps, c.lambda, &pf);
        let none = beam_states(&c.cs, &c.ls, &c.sets, &c.lp, &c.locked, &c.jumps, c.lambda, &pf, None);
        let nohit = beam_states(&c.cs, &c.ls, &c.sets, &c.lp, &c.locked, &c.jumps, c.lambda, &pf, far.bonus());
        assert_eq!(bits(&old), bits(&none));
        assert_eq!(bits(&old), bits(&nohit));
        let first = old.and_then(|v| v.into_iter().next().map(|s| s.1));
        assert_eq!(beam(&c.cs, &c.ls, &c.sets, &c.lp, &c.locked, &c.jumps, c.lambda, &pf), first);
    }
}

#[test]
fn hits_add_gamma_once_per_position_and_respect_the_cap() {
    // three positions, single candidates; the pairs (10, 20) and (20, 30) are in the set
    let mk = |cap| CacheSet::from_pairs(&[(10, 20), (20, 30)], (999, 998), 4.0, cap);
    let (s_inf, s_1, s_40) = (mk(None), mk(Some(1)), CacheSet::from_pairs(&[(10, 20), (20, 30)], (999, 998), 40.0, None));
    let sets = vec![vec![10], vec![20], vec![30]];
    let lp = log_softmax(&[vec![0.0], vec![0.0], vec![0.0]]);
    let cs = sets.clone();
    let ls = vec![vec![0.0], vec![0.0], vec![0.0]];
    let locked = vec![None, None, None];
    let z = |_: &[u32], _: u32| 0.0;
    let st = |cap: Option<usize>, jumps: &[Jump]| beam_states(&cs, &ls, &sets, &lp, &locked, jumps, 0.0, &z, if cap.is_some() { s_1.bonus() } else { s_inf.bonus() }).unwrap();
    // single extensions: hits at positions 1 and 2 (position 0 never hits)
    assert_eq!(st(None, &[])[0], (8.0, vec![10, 20, 30]));
    assert_eq!(st(Some(1), &[])[0], (4.0, vec![10, 20, 30]));
    // a jump over all three: the hits inside the jump count, then the jump bonus
    let j = vec![Jump { start: 0, ids: vec![10, 20, 30], bonus: 8.0 }];
    assert_eq!(st(None, &j)[0], (16.0, vec![10, 20, 30]));
    assert_eq!(st(Some(1), &j)[0], (12.0, vec![10, 20, 30]));
    // jump boundary: a jump starting at 1 pairs its first character with the one before it
    let j = vec![Jump { start: 1, ids: vec![20, 30], bonus: 0.0 }];
    assert_eq!(st(None, &j)[0], (8.0, vec![10, 20, 30]));
    // 臺 (999) is read as 台 (998) on both sides
    let set2 = CacheSet::from_pairs(&[(998, 20)], (999, 998), 1.5, None);
    let sets2 = vec![vec![999], vec![20]];
    let r = beam_states(&sets2, &[vec![0.0], vec![0.0]], &sets2, &log_softmax(&[vec![0.0], vec![0.0]]), &[None, None], &[], 0.0, &z, set2.bonus());
    assert_eq!(r.unwrap()[0], (1.5, vec![999, 20]));
    // a locked position keeps its character whatever the cache says
    let sets3 = vec![vec![10], vec![20, 21]];
    let lp3 = log_softmax(&[vec![0.0], vec![0.0, 5.0]]);
    let r = beam_cached(&[vec![10], vec![21]], &[vec![0.0], vec![0.0]], &sets3, &lp3, &[None, Some(21)], &[], 0.0, &z, s_40.bonus());
    assert_eq!(r, Some(vec![10, 21]));
}

fn ids(v: &Value) -> Vec<u32> {
    v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect()
}

/// Shared cases written by `python cache_ref.py cases` (the Python reference's own results): the same ids in Rust.
#[test]
fn shared_cases_with_python() {
    let cases: Value = serde_json::from_str(include_str!("../tests/fixtures/cache-cases.json")).unwrap();
    // Computed with the Windows C runtime. Under glibc one case (a near tie) falls the other way because exp / ln differ
    // in the last place: there, that case may give either the original result or the one glibc was seen to give.
    // Every other case, and any other result of this one, fails on every system.
    const GLIBC_NEAR_TIE: (&str, [u32; 4]) = ("c252", [9001, 100, 203, 310]);
    let mut n = 0;
    for c in cases.as_array().unwrap() {
        let sets: Vec<Vec<u32>> = c["sets"].as_array().unwrap().iter().map(ids).collect();
        let scores: Vec<Vec<f32>> = c["scores"].as_array().unwrap().iter().map(|r| r.as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect()).collect();
        let locked: Vec<Option<u32>> = c["locked"].as_array().unwrap().iter().map(|x| x.as_u64().map(|v| v as u32)).collect();
        let lambda = c["lambda"].as_f64().unwrap();
        let hits: Vec<(usize, Vec<u32>)> = c["hits"].as_array().unwrap().iter().map(|h| (h[0].as_u64().unwrap() as usize, ids(&h[1]))).collect();
        let pairs: Vec<(u32, u32)> = c["pairs"].as_array().unwrap().iter().map(|p| (p[0].as_u64().unwrap() as u32, p[1].as_u64().unwrap() as u32)).collect();
        let tai = (c["tai"][0].as_u64().unwrap() as u32, c["tai"][1].as_u64().unwrap() as u32);
        let gamma = c["gamma"].as_f64().unwrap();
        let cap = c["cap"].as_u64().map(|k| k as usize);
        let fold = c["fold"].as_array().map(|f| (f[0].as_u64().unwrap() as u32, f[1].as_u64().unwrap() as u32));
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
        let cfg = PnCfg { b2: Some(0.0), b3: Some(8.0), h: Some(4), cands: true };
        let jumps = usable_jumps(&hits, &sets, &lp, &locked, &cfg, fold);
        let set = CacheSet::from_pairs(&pairs, tai, gamma, cap);
        let got = beam_cached(&cs, &ls, &sets, &lp, &locked, &jumps, lambda, &pf, set.bonus());
        if cfg!(not(windows)) && c["name"] == GLIBC_NEAR_TIE.0 {
            assert!(got == Some(ids(&c["expect"])) || got == Some(GLIBC_NEAR_TIE.1.to_vec()), "{} {got:?}", c["name"]);
        } else {
            assert_eq!(got, Some(ids(&c["expect"])), "{}", c["name"]);
        }
        n += 1;
    }
    assert!(n >= 100, "too few shared cases");
}
