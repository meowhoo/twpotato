//! Character n-gram frequency prior rerank (Python reference train/ent_prior.py).
//!
//! Moved unchanged from bench/s3_latency/src/prior.rs (N3 M1, MIT, same project).
//!
//! `Pack` reads the TWPR1 file written by train/ent_prior_pack.py. `top_k` keeps the 8 best legal characters of a
//! position with their float64 log_softmax over all legal characters; `beam` combines them with the stupid-backoff
//! prior: path score = sum(log_softmax) + lambda * sum(prior), beam width 8, ties -> smaller id sequence.

use std::{cmp::Ordering, io};

pub const KC: usize = 8;
pub const BEAM: usize = 8;
const MULT: u64 = 0x9E37_79B9_7F4A_7C15;

struct Table {
    bits: u32,
    dir: Vec<u32>,
    keys32: Vec<u32>, // order 2
    keys64: Vec<u64>, // orders 3, 4
    cnt: Vec<u32>,
}

impl Table {
    #[inline]
    fn get(&self, key: u64) -> u32 {
        let b = (key.wrapping_mul(MULT) >> (64 - self.bits)) as usize;
        let (lo, hi) = (self.dir[b] as usize, self.dir[b + 1] as usize);
        if self.keys64.is_empty() {
            let k = key as u32;
            for i in lo..hi {
                if self.keys32[i] == k {
                    return self.cnt[i];
                }
            }
        } else {
            for i in lo..hi {
                if self.keys64[i] == key {
                    return self.cnt[i];
                }
            }
        }
        0
    }
    fn len(&self) -> usize {
        self.cnt.len()
    }
}

pub struct Pack {
    pub n_total: u64,
    pub v: u32,
    pub bo: f64,
    pub minc: u32,
    pub cp: Vec<u32>,
    /// (臺 id, 台 id) when the pack was counted with 臺 folded into 台 (NORM T2): queries map 臺 to 台. Detected from
    /// the pack itself (臺 has no unigram, 台 has); a pack counted without folding is queried unchanged.
    fold: Option<(u32, u32)>,
    uni: Vec<u32>,
    tabs: [Table; 3], // orders 2, 3, 4
}

/// Sequential little-endian reader (streams from the file; no whole-file buffer, so loading does not double the memory).
struct Rd<R: io::Read>(R, usize);
impl<R: io::Read> Rd<R> {
    fn take(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0u8; n];
        self.0.read_exact(&mut v)?;
        self.1 += n;
        Ok(v)
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn vec32(&mut self, n: usize) -> io::Result<Vec<u32>> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let k = (n - out.len()).min(1 << 16);
            out.extend(self.take(4 * k)?.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())));
        }
        Ok(out)
    }
    fn vec64(&mut self, n: usize) -> io::Result<Vec<u64>> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let k = (n - out.len()).min(1 << 15);
            out.extend(self.take(8 * k)?.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())));
        }
        Ok(out)
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::other(format!("pack: {msg}"))
}

impl Pack {
    pub fn load(path: &std::path::Path) -> io::Result<Pack> {
        let file = std::fs::File::open(path)?;
        let size = file.metadata()?.len() as usize;
        let mut r = Rd(io::BufReader::with_capacity(1 << 20, file), 0);
        if r.take(8)? != b"TWPR1\0\0\0" {
            return Err(bad("bad magic"));
        }
        if r.u32()? != 1 {
            return Err(bad("version"));
        }
        let id_len = r.u32()? as usize;
        let n_total = r.u64()?;
        let v = r.u32()?;
        let bo = f64::from_le_bytes(r.take(8)?.try_into().unwrap());
        let minc = r.u32()?;
        let cp = r.vec32(id_len)?;
        let uni = r.vec32(id_len)?;
        let mut tabs = vec![];
        for n in 2..=4usize {
            let m = r.u64()? as usize;
            let bits = r.u32()?;
            r.u32()?;
            if !(1..=32).contains(&bits) {
                return Err(bad("bits"));
            }
            let dir = r.vec32((1usize << bits) + 1)?;
            if dir[0] != 0 || dir[dir.len() - 1] as usize != m || dir.windows(2).any(|w| w[0] > w[1]) {
                return Err(bad("directory not monotone / wrong length"));
            }
            let (keys32, keys64) = if n == 2 { (r.vec32(m)?, vec![]) } else { (vec![], r.vec64(m)?) };
            let cnt = r.vec32(m)?;
            tabs.push(Table { bits, dir, keys32, keys64, cnt });
        }
        if r.1 != size {
            return Err(bad("trailing bytes"));
        }
        let sum: u64 = uni.iter().map(|&x| u64::from(x)).sum();
        if sum != n_total || uni.iter().filter(|&&x| x > 0).count() != v as usize {
            return Err(bad("N / V mismatch"));
        }
        let tabs: [Table; 3] = tabs.try_into().ok().unwrap();
        let id_of = |c: char| cp.iter().position(|&x| x == c as u32).map(|i| i as u32);
        let fold = match (id_of('臺'), id_of('台')) {
            (Some(a), Some(b)) if uni[a as usize] == 0 && uni[b as usize] > 0 => Some((a, b)),
            _ => None,
        };
        Ok(Pack { n_total, v, bo, minc, cp, fold, uni, tabs })
    }

    /// Bytes held by the tables (the memory the prior adds).
    pub fn bytes(&self) -> usize {
        4 * (self.cp.len() + self.uni.len())
            + self.tabs.iter().map(|t| 4 * t.dir.len() + 4 * t.keys32.len() + 8 * t.keys64.len() + 4 * t.cnt.len()).sum::<usize>()
    }

    pub fn entries(&self) -> [usize; 3] {
        [self.tabs[0].len(), self.tabs[1].len(), self.tabs[2].len()]
    }

    pub fn unigram(&self, id: u32) -> u32 {
        self.uni.get(id as usize).copied().unwrap_or(0)
    }

    /// Count of the n-gram `ids` (1 to 4 ids), 0 if absent.
    pub fn count(&self, ids: &[u32]) -> u32 {
        match ids.len() {
            1 => self.unigram(ids[0]),
            2..=4 => {
                let key = ids.iter().fold(0u64, |k, &i| (k << 15) | u64::from(i));
                if ids.iter().any(|&i| i >= (1 << 15)) {
                    return 0;
                }
                self.tabs[ids.len() - 2].get(key)
            }
            _ => 0,
        }
    }

    /// Stupid backoff log P(ch | hist), hist = up to 3 previous ids; same expression order as Python `_logp`.
    pub fn logp(&self, hist: &[u32], ch: u32) -> f64 {
        if let Some((from, to)) = self.fold
            && (ch == from || hist.contains(&from))
        {
            let mut h = [0u32; 3];
            for (d, &x) in h.iter_mut().zip(hist) {
                *d = if x == from { to } else { x };
            }
            return self.logp_raw(&h[..hist.len()], if ch == from { to } else { ch });
        }
        self.logp_raw(hist, ch)
    }

    pub fn is_folded(&self) -> bool {
        self.fold.is_some()
    }

    fn logp_raw(&self, hist: &[u32], ch: u32) -> f64 {
        let mut pen = 0.0f64;
        let mut buf = [0u32; 4];
        for o in (1..=hist.len()).rev() {
            buf[..o].copy_from_slice(&hist[hist.len() - o..]);
            buf[o] = ch;
            let a = self.count(&buf[..=o]);
            let b = self.count(&buf[..o]);
            if a > 0 && b > 0 {
                return pen + (f64::from(a) / f64::from(b)).ln();
            }
            pen += self.bo;
        }
        pen + ((f64::from(self.unigram(ch)) + 1.0) / (self.n_total as f64 + f64::from(self.v))).ln()
    }
}

/// Top-`KC` legal characters of one position and their float64 log_softmax over ALL legal characters.
/// `scores[j]` belongs to `ids[j]`. Order: score descending (+0 == -0), ties -> smaller id.
pub fn top_k(scores: &[f32], ids: &[u32]) -> (Vec<u32>, Vec<f64>) {
    let v: Vec<f64> = scores.iter().map(|&s| f64::from(s)).collect();
    let max = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = v.iter().map(|&x| (x - max).exp()).sum();
    let lse = sum.ln();
    let mut order: Vec<usize> = (0..v.len()).collect();
    order.sort_by(|&a, &b| v[b].partial_cmp(&v[a]).unwrap_or(Ordering::Equal).then(ids[a].cmp(&ids[b])));
    order.truncate(KC);
    (order.iter().map(|&j| ids[j]).collect(), order.iter().map(|&j| v[j] - max - lse).collect())
}

/// SHORT-PRIOR stage A (SHORT-PRIOR-plan 1): the prior weight by effective context and the F3 history.
/// Default (no keys) = the fixed lambda and a history inside the composition only, bit for bit the old behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShortCfg {
    /// C (`lambda_short_c`); 0 = no schedule (lambda_L everywhere, no word bonus).
    pub c: usize,
    /// lambda_0 (`lambda_short0`); None = lambda_L.
    pub l0: Option<f64>,
    /// beta (`word_short_bonus`): bonus of a data.txt word jump starting at s = beta * alpha(d + s); 0 = no jumps.
    pub wb: f64,
    /// F3 (`prior_ctx`): the prior history of the first characters continues the committed context.
    pub ctx: bool,
}

impl ShortCfg {
    pub fn is_off(&self) -> bool {
        self.c == 0 && !self.ctx
    }

    /// lambda_0 / beta finite and >= 0; either of them needs C >= 1.
    pub fn valid(&self) -> bool {
        self.l0.is_none_or(|x| x.is_finite() && x >= 0.0)
            && self.wb.is_finite()
            && self.wb >= 0.0
            && (self.c >= 1 || (self.l0.is_none() && self.wb == 0.0))
    }

    /// alpha(c) = max(0, 1 - c / C) (0 without C).
    #[inline]
    pub fn alpha(&self, c: usize) -> f64 {
        if c < self.c { 1.0 - c as f64 / self.c as f64 } else { 0.0 }
    }

    /// Weight of composition position i given d context characters (c_i = d + i). lambda_L = 0 stays 0.
    #[inline]
    pub fn weight(&self, lambda: f64, d: usize, i: usize) -> f64 {
        match self.l0 {
            Some(l0) if lambda != 0.0 && d + i < self.c => lambda + (l0 - lambda) * self.alpha(d + i),
            _ => lambda,
        }
    }

    /// Weights of n positions.
    pub fn weights(&self, lambda: f64, d: usize, n: usize) -> Vec<f64> {
        (0..n).map(|i| self.weight(lambda, d, i)).collect()
    }

    /// Parses the settings keys `lambda_short_c`, `lambda_short0`, `word_short_bonus`, `prior_ctx` (absent = off).
    pub fn parse(get: &dyn Fn(&str) -> Option<String>) -> Result<ShortCfg, String> {
        let num = |k: &str| -> Result<Option<f64>, String> {
            get(k).map(|v| v.trim().parse::<f64>().ok().filter(|x| x.is_finite() && *x >= 0.0).ok_or_else(|| format!("{k} must be a finite number >= 0"))).transpose()
        };
        let c = match get("lambda_short_c") {
            Some(v) => v.trim().parse::<usize>().ok().filter(|&c| c >= 1).ok_or("lambda_short_c must be an integer >= 1")?,
            None => 0,
        };
        let ctx = match get("prior_ctx").as_deref().map(str::trim) {
            None | Some("off") => false,
            Some("on") => true,
            Some(_) => return Err("prior_ctx must be on or off".into()),
        };
        let out = ShortCfg { c, l0: num("lambda_short0")?, wb: num("word_short_bonus")?.unwrap_or(0.0), ctx };
        if c == 0 && (get("lambda_short0").is_some() || get("word_short_bonus").is_some()) || !out.valid() {
            return Err("lambda_short0 / word_short_bonus need lambda_short_c".into());
        }
        Ok(out)
    }

    /// `key=value` lines of the non-default keys (empty when off).
    pub fn describe(&self) -> String {
        let mut v = vec![];
        if self.c > 0 {
            v.push(format!("lambda_short_c={}", self.c));
        }
        if let Some(l0) = self.l0 {
            v.push(format!("lambda_short0={l0}"));
        }
        if self.wb > 0.0 {
            v.push(format!("word_short_bonus={}", self.wb));
        }
        if self.ctx {
            v.push("prior_ctx=on".into());
        }
        v.join(" ")
    }

    /// Word bonus at start s (0 = no jumps there).
    pub fn word_bonus(&self, d: usize, s: usize) -> f64 {
        if self.wb > 0.0 { self.wb * self.alpha(d + s) } else { 0.0 }
    }
}

/// The last <= 3 ids of `pre ++ seq` (the prior history of the next character).
#[inline]
pub fn hist3(pre: &[u32], seq: &[u32]) -> ([u32; 3], usize) {
    let mut h = [0u32; 3];
    let from_seq = seq.len().min(3);
    let from_pre = (3 - from_seq).min(pre.len());
    h[..from_pre].copy_from_slice(&pre[pre.len() - from_pre..]);
    h[from_pre..from_pre + from_seq].copy_from_slice(&seq[seq.len() - from_seq..]);
    (h, from_pre + from_seq)
}

/// F3 context tail: the last <= 3 ids of `ctx` (the context the model uses) that are in the vocabulary, stopping at the
/// first out-of-vocabulary character from the end (the prior counts never cross one).
pub fn ctx_tail(ids_from_end: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut t: Vec<u32> = ids_from_end.take_while(|&x| x != crate::vocab::UNK).take(3).collect();
    t.reverse();
    t
}

/// Beam search; `cands[i]`/`lps[i]` from `top_k`. Returns None if some position has no candidate.
pub fn beam(cands: &[Vec<u32>], lps: &[Vec<f64>], pack: &Pack, lambda: f64) -> Option<Vec<u32>> {
    beam_ex(cands, lps, pack, &|_| lambda, &[])
}

/// `beam` with a per-position weight `w(i)` and the F3 history prefix `pre` (`w = lambda`, `pre = []`: `beam` bit for bit).
pub fn beam_ex(cands: &[Vec<u32>], lps: &[Vec<f64>], pack: &Pack, w: &dyn Fn(usize) -> f64, pre: &[u32]) -> Option<Vec<u32>> {
    if cands.iter().any(Vec::is_empty) {
        return None;
    }
    let mut states: Vec<(f64, Vec<u32>)> = vec![(0.0, vec![])];
    for (i, (cs, ls)) in cands.iter().zip(lps).enumerate() {
        let lambda = w(i);
        let mut next: Vec<(f64, Vec<u32>)> = Vec::with_capacity(states.len() * cs.len());
        for (sc, seq) in &states {
            let (hb, hn) = hist3(pre, seq);
            let hist = &hb[..hn];
            for (&c, &l) in cs.iter().zip(ls) {
                let p = if lambda != 0.0 { pack.logp(hist, c) } else { 0.0 };
                let mut s2 = seq.clone();
                s2.push(c);
                next.push(((sc + l) + lambda * p, s2));
            }
        }
        next.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
        next.truncate(BEAM);
        states = next;
    }
    states.into_iter().next().map(|s| s.1)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn tiny() -> Pack {
        // ids 2..=5; unigram a=2:10 b=3:5 c=4:1; bigram (2,3)=4; trigram (2,3,4)=2; 4-gram none
        let mk = |bits: u32, ents: &[(u64, u32)], wide: bool| {
            let mut dir = vec![0u32; (1 << bits) + 1];
            let mut by: Vec<Vec<(u64, u32)>> = vec![vec![]; 1 << bits];
            for &(k, c) in ents {
                by[(k.wrapping_mul(MULT) >> (64 - bits)) as usize].push((k, c));
            }
            let (mut k32, mut k64, mut cnt) = (vec![], vec![], vec![]);
            for (b, v) in by.iter().enumerate() {
                for &(k, c) in v {
                    k32.push(k as u32);
                    k64.push(k);
                    cnt.push(c);
                }
                dir[b + 1] = cnt.len() as u32;
            }
            Table { bits, dir, keys32: if wide { vec![] } else { k32 }, keys64: if wide { k64 } else { vec![] }, cnt }
        };
        let k = |ids: &[u64]| ids.iter().fold(0u64, |a, &i| (a << 15) | i);
        let mut uni = vec![0u32; 8];
        (uni[2], uni[3], uni[4]) = (10, 5, 1);
        Pack {
            n_total: 16,
            v: 3,
            bo: 0.4f64.ln(),
            minc: 3,
            cp: vec![0; 8],
            fold: None,
            uni,
            tabs: [mk(2, &[(k(&[2, 3]), 4)], false), mk(2, &[(k(&[2, 3, 4]), 2)], true), mk(2, &[], true)],
        }
    }

    #[test]
    fn logp_backoff_orders() {
        let p = tiny();
        let bo = 0.4f64.ln();
        assert_eq!(p.logp(&[2], 3), (4.0f64 / 10.0).ln()); // bigram
        assert_eq!(p.logp(&[2, 3], 4), (2.0f64 / 4.0).ln()); // trigram a/b
        assert_eq!(p.logp(&[3, 2], 4), bo + bo + (2.0f64 / 19.0).ln()); // 3- and 2-gram contexts missing -> unigram
        assert_eq!(p.logp(&[], 5), ((0.0f64 + 1.0) / (16.0 + 3.0)).ln()); // unseen unigram, add-1
        assert_eq!(p.logp(&[4], 3), bo + ((5.0f64 + 1.0) / 19.0).ln()); // numerator missing -> back off one order
    }

    #[test]
    fn folded_pack_maps_the_traditional_id_to_its_simplified_twin() {
        let mut p = tiny();
        let before = (p.logp(&[5], 3), p.logp(&[2], 5), p.logp(&[], 5));
        p.fold = Some((5, 2)); // id 5 folds into id 2
        assert_eq!(p.logp(&[5], 3), p.logp(&[2], 3));
        assert_eq!(p.logp(&[2, 5], 3), p.logp(&[2, 2], 3));
        assert_eq!(p.logp(&[2], 5), p.logp(&[2], 2));
        assert_eq!(p.logp(&[], 5), p.logp(&[], 2));
        assert_ne!(before.2, p.logp(&[], 5));
        assert_eq!(p.logp(&[2], 3), (4.0f64 / 10.0).ln()); // ids not involved are unchanged
    }

    #[test]
    fn top_k_ties_and_zero() {
        let (c, l) = top_k(&[0.0, -0.0, 1.0], &[7, 3, 9]);
        assert_eq!(c, vec![9, 3, 7]); // +0 == -0, tie -> smaller id first
        assert!((l.iter().map(|x| x.exp()).sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn beam_zero_lambda_equals_greedy_with_ties() {
        let p = tiny();
        let cands = vec![vec![2, 3], vec![4]];
        let lps = vec![vec![-0.5, -0.5], vec![0.0]];
        assert_eq!(beam(&cands, &lps, &p, 0.0), Some(vec![2, 4]));
        // positive prior breaks a model tie in favour of the larger id: [2,3] has a bigram, [1,3] backs off
        let (c2, l2) = (vec![vec![1, 2], vec![3]], vec![vec![-0.5, -0.5], vec![0.0]]);
        assert_eq!(beam(&c2, &l2, &p, 0.0), Some(vec![1, 3]));
        assert_eq!(beam(&c2, &l2, &p, 1.0), Some(vec![2, 3]));
        assert_eq!(beam(&[vec![]], &[vec![]], &p, 1.0), None);
    }

    #[test]
    fn short_weights_and_history() {
        let off = ShortCfg::default();
        assert!(off.is_off() && off.valid());
        assert_eq!(off.weights(0.5, 0, 3), vec![0.5; 3]);
        let s = ShortCfg { c: 4, l0: Some(2.0), wb: 0.0, ctx: false };
        assert_eq!(s.weights(0.5, 0, 6), vec![2.0, 1.625, 1.25, 0.875, 0.5, 0.5]);
        assert_eq!(s.weights(0.5, 3, 3), vec![0.875, 0.5, 0.5]);
        assert_eq!(s.weights(0.0, 0, 2), vec![0.0, 0.0]); // lambda_L = 0: no prior at all
        assert_eq!(ShortCfg { c: 4, l0: Some(0.5), wb: 0.0, ctx: false }.weights(0.5, 0, 5), vec![0.5; 5]); // lambda_0 = lambda_L
        assert_eq!(ShortCfg { c: 4, l0: None, wb: 2.0, ctx: false }.weights(0.5, 0, 2), vec![0.5; 2]);
        let w = ShortCfg { c: 4, l0: None, wb: 2.0, ctx: false };
        assert_eq!((w.word_bonus(0, 0), w.word_bonus(0, 2), w.word_bonus(2, 2), w.word_bonus(0, 4)), (2.0, 1.0, 0.0, 0.0));
        assert_eq!(s.word_bonus(0, 0), 0.0);
        assert!(!ShortCfg { c: 0, l0: Some(1.0), wb: 0.0, ctx: false }.valid() && !ShortCfg { c: 2, l0: Some(f64::NAN), wb: 0.0, ctx: false }.valid());
        assert!(!ShortCfg { c: 0, l0: None, wb: 1.0, ctx: false }.valid() && !ShortCfg { c: 2, l0: None, wb: -1.0, ctx: false }.valid());
        assert!(ShortCfg { c: 0, l0: None, wb: 0.0, ctx: true }.valid());
        let m = |kv: &'static [(&'static str, &'static str)]| move |k: &str| kv.iter().find(|x| x.0 == k).map(|x| x.1.to_string());
        assert_eq!(ShortCfg::parse(&m(&[])), Ok(ShortCfg::default()));
        assert_eq!(ShortCfg::parse(&m(&[("lambda_short_c", "8"), ("lambda_short0", "2"), ("word_short_bonus", "1.5"), ("prior_ctx", "on")])),
                   Ok(ShortCfg { c: 8, l0: Some(2.0), wb: 1.5, ctx: true }));
        assert!(ShortCfg::parse(&m(&[("lambda_short0", "2")])).is_err());
        assert!(ShortCfg::parse(&m(&[("word_short_bonus", "0")])).is_err());
        assert!(ShortCfg::parse(&m(&[("lambda_short_c", "0")])).is_err());
        assert!(ShortCfg::parse(&m(&[("lambda_short_c", "4"), ("word_short_bonus", "-1")])).is_err());
        assert!(ShortCfg::parse(&m(&[("prior_ctx", "yes")])).is_err());
        assert_eq!(ShortCfg::parse(&m(&[("prior_ctx", "off")])), Ok(ShortCfg::default()));
        assert_eq!(hist3(&[], &[1, 2, 3, 4]), ([2, 3, 4], 3));
        assert_eq!(hist3(&[7, 8, 9], &[1]), ([8, 9, 1], 3));
        assert_eq!(hist3(&[9], &[]), ([9, 0, 0], 1));
        assert_eq!(hist3(&[], &[]).1, 0);
        assert_eq!(ctx_tail([5, 4, 0, 3, 2].into_iter()), vec![4, 5]); // ids given from the end; stops at UNK
        assert_eq!(ctx_tail([5, 4, 3, 2].into_iter()), vec![3, 4, 5]);
        assert_eq!(ctx_tail([0, 4].into_iter()), Vec::<u32>::new());
    }

    #[test]
    fn beam_ex_uniform_equals_beam_and_prefix_conditions() {
        let p = tiny();
        let cands = vec![vec![3, 4], vec![4, 3]];
        let lps = vec![vec![-0.7, -0.6], vec![-0.5, -0.55]];
        for lam in [0.0, 0.5, 1.0] {
            assert_eq!(beam_ex(&cands, &lps, &p, &|_| lam, &[]), beam(&cands, &lps, &p, lam));
        }
        // prefix [2]: bigram (2,3) makes 3 first; trigram (2,3,4) then 4
        assert_eq!(beam_ex(&cands, &lps, &p, &|_| 3.0, &[2]), Some(vec![3, 4]));
        // without the prefix the model scores alone decide position 0 (3: -0.7 < 4: -0.6 -> 4 first at weight 0)
        assert_eq!(beam_ex(&cands, &lps, &p, &|i| if i == 0 { 0.0 } else { 0.5 }, &[]).map(|v| v[0]), Some(4));
    }

    #[test]
    fn beam_prunes_by_score_then_sequence() {
        // 9 equal-score paths over two positions with 3 candidates: beam keeps 8, order by sequence
        let p = tiny();
        let cands = vec![vec![2, 3, 4], vec![2, 3, 4]];
        let lps = vec![vec![-1.0; 3], vec![-1.0; 3]];
        assert_eq!(beam(&cands, &lps, &p, 0.0), Some(vec![2, 2]));
    }
}
