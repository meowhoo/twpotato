//! Single-pass character model.
//!
//! Moved from bench/s3_latency/src/n1.rs (N3 M1, MIT, same project); only the `use` lines changed.
//!
//! Input `[context ≤ 64][SEP][syllables ≤ 39]` uses bidirectional attention
//! or a TWG2 prefix mask with reusable context K/V; each syllable scores only the legal
//! characters of its syllable against the shared character embedding and
//! takes the best one (ties: smallest token id). Layer arithmetic is the
//! same as `model.rs` (pre-LN, no bias on QKV/output, FFN with bias).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

use crate::kernel::{attention, gelu, layer_norm, matmul};
use crate::lexicon::Dictionary;
use crate::vocab::{SEP, Vocab};

pub const POSITIONS: usize = 104;
pub const CONTEXT: usize = 64;
const HEAD_DIM: usize = 32;

struct Layer {
    ln1_g: Vec<f32>,
    ln1_b: Vec<f32>,
    w_qkv: Vec<f32>, // d × 3d
    w_o: Vec<f32>,   // d × d
    ln2_g: Vec<f32>,
    ln2_b: Vec<f32>,
    w_1: Vec<f32>, // d × 4d
    b_1: Vec<f32>,
    w_2: Vec<f32>, // 4d × d
    b_2: Vec<f32>,
}

/// G1 layer: RMSNorm, RoPE attention, SwiGLU, no biases.
struct G1Layer {
    ln1: Vec<f32>,
    w_qkv: Vec<f32>, // d x 3d
    w_o: Vec<f32>,   // d x d
    ln2: Vec<f32>,
    w_gate: Vec<f32>, // d x w
    w_up: Vec<f32>,   // d x w
    w_down: Vec<f32>, // w x d
    int8: Option<[crate::qkernel::QMat; 5]>,
}

struct PrefixEntry {
    tokens: Vec<u32>,
    // Interleaved RoPE K and V rows (stride 2d), one matrix per layer.
    kv: Vec<Vec<f32>>,
}

pub struct N1 {
    prefix: bool,
    prefix_cache_on: AtomicBool,
    prefix_cache: Mutex<Vec<Arc<PrefixEntry>>>,
    /// Some for a TWG1 model (then `pos` and `layers` are empty): layers, FFN width.
    g1: Option<(Vec<G1Layer>, usize)>,
    d: usize,
    tok: Vec<f32>, // vocab × d, also the output embedding
    pos: Vec<f32>, // POSITIONS × d
    layers: Vec<Layer>,
    lnf_g: Vec<f32>,
    lnf_b: Vec<f32>,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.04
    }
    fn vec(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.next()).collect()
    }
}

/// x * (mean(x^2) + 1e-5)^(-1/2) * weight, like nn.RMSNorm(eps=1e-5).
fn rms_norm(x: &mut [f32], weight: &[f32]) {
    let ms = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32;
    let inv = 1.0 / (ms + 1e-5).sqrt();
    for (v, w) in x.iter_mut().zip(weight) {
        *v = *v * inv * w;
    }
}

/// Syllable → legal character ids (sorted), from the loaded dictionary.
pub fn legal_table(dict: &Dictionary, vocab: &Vocab) -> HashMap<String, Vec<u32>> {
    let mut table: HashMap<String, Vec<u32>> = HashMap::new();
    for (c, syllables) in dict.legal_readings() {
        for s in syllables {
            table.entry(s).or_default().push(vocab.char_id(c));
        }
    }
    for ids in table.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    table
}

/// Token ids of `[context tail][SEP][syllables]` and the index of the first
/// syllable.
pub fn encode(vocab: &Vocab, context: &str, syllables: &[String]) -> (Vec<u32>, usize) {
    let chars: Vec<char> = context.chars().collect();
    let mut t: Vec<u32> = chars[chars.len().saturating_sub(CONTEXT)..]
        .iter()
        .map(|&c| vocab.char_id(c))
        .collect();
    t.push(SEP);
    let first = t.len();
    t.extend(syllables.iter().map(|s| vocab.syllable_id(s)));
    (t, first)
}

impl N1 {
    /// Enable or disable prefix K/V reuse (enabled by default).
    pub fn set_prefix_cache(&self, on: bool) {
        self.prefix_cache_on.store(on, Ordering::Relaxed);
    }

    fn prefix_first(&self, tokens: &[u32]) -> usize {
        if self.prefix { tokens.iter().position(|&t| t == SEP).map_or(tokens.len(), |i| i + 1) }
        else { tokens.len() }
    }

    fn scoring_hidden(&self, tokens: &[u32], first: usize) -> Option<(Vec<f32>, usize)> {
        let l = tokens.len();
        if l == 0 || l > POSITIONS || first > l { return None; }
        if !self.prefix { return self.hidden(tokens).map(|h| (h, 0)); }
        if !self.prefix_cache_on.load(Ordering::Relaxed) {
            return Some((self.hidden_g1_at(tokens, None, first, None, None), 0));
        }
        let entry = {
            let mut cache = self.prefix_cache.lock().unwrap();
            cache.iter().position(|e| e.tokens == tokens[..first]).map(|i| {
                let e = cache.remove(i);
                cache.push(Arc::clone(&e));
                e
            })
        };
        if let Some(entry) = entry {
            let h = if first == l { Vec::new() } else {
                self.hidden_g1_at(&tokens[first..], None, first, Some(&entry), None)
            };
            return Some((h, first));
        }
        let mut kv = Vec::new();
        let h = self.hidden_g1_at(tokens, None, first, None, Some(&mut kv));
        let entry = Arc::new(PrefixEntry { tokens: tokens[..first].to_vec(), kv });
        let mut cache = self.prefix_cache.lock().unwrap();
        // Concurrent misses may compute the same prefix. Retain one most recent entry.
        if let Some(i) = cache.iter().position(|e| e.tokens == entry.tokens) { cache.remove(i); }
        if cache.len() == 4 { cache.remove(0); }
        cache.push(entry);
        Some((h, 0))
    }

    /// Fixed-seed, in-memory G1 weights for reproducible benchmarks.
    pub fn random_g1(vocab: usize, d: usize, layers: usize, width: usize, seed: u64) -> N1 {
        assert!(vocab > 0 && d > 0 && d % HEAD_DIM == 0 && width > 0);
        let mut r = Rng(seed | 1);
        let tok = r.vec(vocab*d);
        let ls = (0..layers).map(|_| G1Layer {
            ln1: vec![1.0;d], ln2: vec![1.0;d], w_qkv: r.vec(d*3*d),
            w_o: r.vec(d*d), w_gate: r.vec(d*width), w_up: r.vec(d*width),
            w_down: r.vec(width*d), int8: None,
        }).collect();
        N1 { prefix: false, prefix_cache_on: AtomicBool::new(true), prefix_cache: Mutex::new(Vec::new()), g1: Some((ls,width)), d, tok, pos: vec![], layers: vec![],
            lnf_g: vec![1.0;d], lnf_b: vec![] }
    }

    pub fn random(vocab: usize, d: usize, layers: usize, seed: u64) -> N1 {
        assert!(d % HEAD_DIM == 0, "d must be a multiple of {HEAD_DIM}");
        let mut r = Rng(seed | 1);
        N1 {
            prefix: false, prefix_cache_on: AtomicBool::new(true), prefix_cache: Mutex::new(Vec::new()),
            g1: None,
            d,
            tok: r.vec(vocab * d),
            pos: r.vec(POSITIONS * d),
            layers: (0..layers)
                .map(|_| Layer {
                    ln1_g: vec![1.0; d],
                    ln1_b: vec![0.0; d],
                    w_qkv: r.vec(d * 3 * d),
                    w_o: r.vec(d * d),
                    ln2_g: vec![1.0; d],
                    ln2_b: vec![0.0; d],
                    w_1: r.vec(d * 4 * d),
                    b_1: vec![0.0; 4 * d],
                    w_2: r.vec(4 * d * d),
                    b_2: vec![0.0; d],
                })
                .collect(),
            lnf_g: vec![1.0; d],
            lnf_b: vec![0.0; d],
        }
    }

    /// Loads weights exported by train/n2_train.py: "TWN1", then vocab, d,
    /// layers as little-endian u32, then f32 tensors in field order.
    pub fn load(path: &std::path::Path) -> std::io::Result<N1> {
        let bytes = std::fs::read(path)?;
        if bytes.starts_with(b"TWG2") {
            return Self::load_g2(&bytes);
        }
        let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
        if bytes.starts_with(b"TWG1") {
            return Self::load_g1(&bytes);
        }
        if bytes.len() < 16 || &bytes[..4] != b"TWN1" {
            return Err(bad("not a TWN1 model file"));
        }
        let u = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        let (vocab, d, layers) = (u(4), u(8), u(12));
        if d % HEAD_DIM != 0 {
            return Err(bad("d must be a multiple of the head size"));
        }
        let mut at = 16;
        let mut take = |n: usize| -> std::io::Result<Vec<f32>> {
            let end = at + 4 * n;
            let chunk = bytes.get(at..end).ok_or_else(|| bad("truncated model file"))?;
            at = end;
            Ok(chunk.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
        };
        let tok = take(vocab * d)?;
        let pos = take(POSITIONS * d)?;
        let mut ls = Vec::with_capacity(layers);
        for _ in 0..layers {
            ls.push(Layer {
                ln1_g: take(d)?,
                ln1_b: take(d)?,
                w_qkv: take(d * 3 * d)?,
                w_o: take(d * d)?,
                ln2_g: take(d)?,
                ln2_b: take(d)?,
                w_1: take(d * 4 * d)?,
                b_1: take(4 * d)?,
                w_2: take(4 * d * d)?,
                b_2: take(d)?,
            });
        }
        let m = N1 { prefix: false, prefix_cache_on: AtomicBool::new(true), prefix_cache: Mutex::new(Vec::new()), g1: None, d, tok, pos, layers: ls, lnf_g: take(d)?, lnf_b: take(d)? };
        if at != bytes.len() {
            return Err(bad("trailing bytes in model file"));
        }
        Ok(m)
    }

    /// "TWG1", then vocab, d, layers, width as little-endian u32, then f32
    /// tensors: tok; per layer ln1, w_qkv, w_o, ln2, w_gate, w_up, w_down; lnf.
    fn load_g1(bytes: &[u8]) -> std::io::Result<N1> {
        let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
        if bytes.len() < 20 {
            return Err(bad("truncated model file"));
        }
        let u = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        let (vocab, d, layers, w) = (u(4), u(8), u(12), u(16));
        if d % HEAD_DIM != 0 {
            return Err(bad("d must be a multiple of the head size"));
        }
        let mut at = 20;
        let mut take = |n: usize| -> std::io::Result<Vec<f32>> {
            let end = at + 4 * n;
            let chunk = bytes.get(at..end).ok_or_else(|| bad("truncated model file"))?;
            at = end;
            Ok(chunk.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
        };
        let tok = take(vocab * d)?;
        let mut ls = Vec::with_capacity(layers);
        for _ in 0..layers {
            ls.push(G1Layer {
                ln1: take(d)?,
                w_qkv: take(d * 3 * d)?,
                w_o: take(d * d)?,
                ln2: take(d)?,
                w_gate: take(d * w)?,
                w_up: take(d * w)?,
                w_down: take(w * d)?,
                int8: None,
            });
        }
        let lnf_g = take(d)?;
        if at != bytes.len() {
            return Err(bad("trailing bytes in model file"));
        }
        Ok(N1 { prefix: false, prefix_cache_on: AtomicBool::new(true), prefix_cache: Mutex::new(Vec::new()), g1: Some((ls, w)), d, tok, pos: vec![], layers: vec![], lnf_g, lnf_b: vec![] })
    }

    /// Expands TWG2 tiers into the existing tied G1 embedding table.
    fn load_g2(bytes: &[u8]) -> std::io::Result<N1> {
        use std::io::{Error, ErrorKind, Result};
        fn bad() -> Error { Error::new(ErrorKind::InvalidData, "invalid TWG2 model file") }
        fn mul(a: usize, b: usize) -> Result<usize> { a.checked_mul(b).ok_or_else(bad) }
        struct Reader<'a> { bytes: &'a [u8], at: usize }
        impl Reader<'_> {
            fn chunk(&mut self, n: usize) -> Result<&[u8]> {
                let end = self.at.checked_add(n).ok_or_else(bad)?;
                let chunk = self.bytes.get(self.at..end).ok_or_else(bad)?;
                self.at = end;
                Ok(chunk)
            }
            fn u(&mut self) -> Result<usize> {
                Ok(u32::from_le_bytes(self.chunk(4)?.try_into().unwrap()) as usize)
            }
            fn floats(&mut self, n: usize) -> Result<Vec<f32>> {
                self.chunk(mul(n, 4)?)?.chunks_exact(4).map(|c| {
                    let v = f32::from_le_bytes(c.try_into().unwrap());
                    if v.is_finite() { Ok(v) } else { Err(bad()) }
                }).collect()
            }
        }
        let mut r = Reader { bytes, at: 4 };
        let (vocab, d, layers, w, ntiers) = (r.u()?, r.u()?, r.u()?, r.u()?, r.u()?);
        let flags = r.u()?;
        if flags & !1 != 0 { return Err(bad()); }
        if [vocab, d, layers, w, ntiers].contains(&0) || d % HEAD_DIM != 0 || ntiers > 8 {
            return Err(bad());
        }
        let dd = mul(d, d)?;
        let qkv = mul(dd, 3)?;
        let dw = mul(d, w)?;
        // Check the full body length before allocating the expanded table.
        let per_layer = mul(d, 2)?.checked_add(qkv).and_then(|n| n.checked_add(dd))
            .and_then(|n| n.checked_add(dw.checked_mul(3)?)).ok_or_else(bad)?;
        let tail = mul(mul(per_layer, layers)?.checked_add(d).ok_or_else(bad)?, 4)?;
        if tail > bytes.len().saturating_sub(r.at) || vocab > bytes.len() / 4 { return Err(bad()); }
        let tok_len = mul(vocab, d)?;
        mul(tok_len, std::mem::size_of::<f32>())?;
        let mut tok = Vec::new();
        tok.try_reserve_exact(tok_len).map_err(|_| bad())?;
        tok.resize(tok_len, 0.0f32);
        let mut seen = vec![false; vocab];
        for _ in 0..ntiers {
            let (e, n) = (r.u()?, r.u()?);
            if e == 0 || n == 0 || n > vocab { return Err(bad()); }
            let ids: Vec<usize> = r.chunk(mul(n, 4)?)?.chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()) as usize).collect();
            for (i, &id) in ids.iter().enumerate() {
                if id >= vocab || (i > 0 && ids[i - 1] >= id) || seen[id] { return Err(bad()); }
                seen[id] = true;
            }
            let table = r.floats(mul(n, e)?)?;
            let proj = r.floats(mul(e, d)?)?;
            for (i, &id) in ids.iter().enumerate() {
                for j in 0..d {
                    let mut v = 0.0f32;
                    for k in 0..e { v += table[i * e + k] * proj[k * d + j]; }
                    if !v.is_finite() { return Err(bad()); }
                    tok[id * d + j] = v;
                }
            }
        }
        if seen.iter().any(|&v| !v) || bytes.len().checked_sub(r.at) != Some(tail) { return Err(bad()); }
        let mut ls = Vec::new();
        for _ in 0..layers {
            ls.push(G1Layer {
                ln1: r.floats(d)?, w_qkv: r.floats(qkv)?, w_o: r.floats(dd)?,
                ln2: r.floats(d)?, w_gate: r.floats(dw)?, w_up: r.floats(dw)?,
                w_down: r.floats(dw)?, int8: None,
            });
        }
        let lnf_g = r.floats(d)?;
        Ok(N1 { prefix: flags & 1 != 0, prefix_cache_on: AtomicBool::new(true), prefix_cache: Mutex::new(Vec::new()), g1: Some((ls, w)), d, tok, pos: vec![], layers: vec![], lnf_g, lnf_b: vec![] })
    }

    /// Converts G1 projection matrices; validation errors leave the model unchanged.
    pub fn quantize_int8(&mut self) -> Result<(), &'static str> {
        use crate::qkernel::QMat;
        let (layers, w) = self.g1.as_mut().ok_or("int8 requires a G1 model")?;
        let d = self.d;
        let quantized: Result<Vec<_>, &'static str> = layers.iter().map(|l| {
            if l.int8.is_some() { return Ok(None); }
            Ok(Some([QMat::new(d, 3*d, &l.w_qkv)?, QMat::new(d, d, &l.w_o)?,
                QMat::new(d, *w, &l.w_gate)?, QMat::new(d, *w, &l.w_up)?, QMat::new(*w, d, &l.w_down)?]))
        }).collect();
        for (l, q) in layers.iter_mut().zip(quantized?) {
            if let Some(q) = q {
                l.int8 = Some(q);
                l.w_qkv = Vec::new(); l.w_o = Vec::new();
                l.w_gate = Vec::new(); l.w_up = Vec::new(); l.w_down = Vec::new();
            }
        }
        self.prefix_cache.lock().unwrap().clear();
        Ok(())
    }

    pub fn vocab_len(&self) -> usize {
        self.tok.len() / self.d
    }

    /// Scores of each syllable position's legal characters (in `legal`
    /// order); None if the sequence does not fit.
    pub fn scores(&self, tokens: &[u32], first: usize, legal: &[&[u32]]) -> Option<Vec<Vec<f32>>> {
        let d = self.d;
        #[cfg(feature = "arch-prof")]
        let forward_start = std::time::Instant::now();
        let (h, offset) = self.scoring_hidden(tokens, first)?;
        #[cfg(feature = "arch-prof")]
        crate::prof::add_forward(forward_start.elapsed().as_nanos() as u64, tokens.len());
        #[cfg(feature = "arch-prof")]
        let head_start = std::time::Instant::now();
        let result = Some(
            legal
                .iter()
                .enumerate()
                .map(|(i, ids)| {
                    let hi = &h[(first + i - offset) * d..(first + i + 1 - offset) * d];
                    ids.iter()
                        .map(|&c| hi.iter().zip(&self.tok[c as usize * d..(c as usize + 1) * d]).map(|(a, b)| a * b).sum())
                        .collect()
                })
                .collect(),
        );
        #[cfg(feature = "arch-prof")]
        crate::prof::add_head(head_start.elapsed().as_nanos() as u64, legal.len());
        result
    }

    pub fn param_count(&self) -> usize {
        if let Some((ls, _)) = &self.g1 {
            return self.tok.len()
                + self.lnf_g.len()
                + ls.iter()
                    .map(|l| match l.int8 {
                        // int8: the fp32 projections are freed; count their logical sizes
                        Some(_) => l.ln1.len() + l.ln2.len() + 4 * self.d * self.d + 3 * self.d * self.g1.as_ref().unwrap().1,
                        None => l.ln1.len() + l.w_qkv.len() + l.w_o.len() + l.ln2.len() + l.w_gate.len() + l.w_up.len() + l.w_down.len(),
                    })
                    .sum::<usize>();
        }
        let per = |l: &Layer| {
            l.ln1_g.len() + l.ln1_b.len() + l.w_qkv.len() + l.w_o.len() + l.ln2_g.len()
                + l.ln2_b.len() + l.w_1.len() + l.b_1.len() + l.w_2.len() + l.b_2.len()
        };
        self.tok.len() + self.pos.len() + self.layers.iter().map(per).sum::<usize>()
            + self.lnf_g.len() + self.lnf_b.len()
    }

    /// Final hidden states (L×d); prefix models mask at the first SEP + 1
    /// (fully bidirectional without SEP). None beyond the position limit.
    pub fn hidden(&self, tokens: &[u32]) -> Option<Vec<f32>> {
        let d = self.d;
        let l = tokens.len();
        if l > POSITIONS || l == 0 {
            return None;
        }
        if self.g1.is_some() {
            return Some(self.hidden_g1(tokens));
        }
        let mut x = vec![0.0f32; l * d];
        for (i, &t) in tokens.iter().enumerate() {
            let te = &self.tok[t as usize * d..(t as usize + 1) * d];
            let pe = &self.pos[i * d..(i + 1) * d];
            for c in 0..d {
                x[i * d + c] = te[c] + pe[c];
            }
        }
        let heads = d / HEAD_DIM;
        let mut h = vec![0.0f32; l * d];
        let mut qkv = vec![0.0f32; l * 3 * d];
        let mut att = vec![0.0f32; l * d];
        let mut tmp = vec![0.0f32; l * d];
        let mut ff = vec![0.0f32; l * 4 * d];
        let mut scratch = Vec::new();
        for layer in &self.layers {
            h.copy_from_slice(&x);
            for row in h.chunks_mut(d) {
                layer_norm(row, &layer.ln1_g, &layer.ln1_b);
            }
            matmul(l, d, 3 * d, &h, d, &layer.w_qkv, 3 * d, &mut qkv, 3 * d);
            for head in 0..heads {
                let o = head * HEAD_DIM;
                // prefix_len = l: every row may attend to every column.
                attention(
                    l, HEAD_DIM, l,
                    &qkv[o..], &qkv[d + o..], &qkv[2 * d + o..], 3 * d,
                    &mut att[o..], d, &mut scratch,
                );
            }
            matmul(l, d, d, &att, d, &layer.w_o, d, &mut tmp, d);
            for (xv, tv) in x.iter_mut().zip(&tmp) {
                *xv += tv;
            }
            h.copy_from_slice(&x);
            for row in h.chunks_mut(d) {
                layer_norm(row, &layer.ln2_g, &layer.ln2_b);
            }
            matmul(l, d, 4 * d, &h, d, &layer.w_1, 4 * d, &mut ff, 4 * d);
            for row in ff.chunks_mut(4 * d) {
                for (v, b) in row.iter_mut().zip(&layer.b_1) {
                    *v = gelu(*v + b);
                }
            }
            matmul(l, 4 * d, d, &ff, 4 * d, &layer.w_2, d, &mut tmp, d);
            for row in 0..l {
                for c in 0..d {
                    x[row * d + c] += tmp[row * d + c] + layer.b_2[c];
                }
            }
        }
        for row in x.chunks_mut(d) {
            layer_norm(row, &self.lnf_g, &self.lnf_b);
        }
        Some(x)
    }

    /// CAND forward: a G1 model over `tokens` where token i has RoPE position
    /// `positions[i]` and the additive input vector `extra[i*d..(i+1)*d]` (zero for the context / SEP /
    /// syllable tokens; candidate tokens carry type + slot + category). None if not a G1 model, if the
    /// lengths disagree or if the sequence is empty or longer than `max_len`. Without candidates callers
    /// use `hidden` (the unchanged G1 path).
    pub fn hidden_ext(&self, tokens: &[u32], positions: &[u32], extra: &[f32], max_len: usize) -> Option<Vec<f32>> {
        self.hidden_ext_first(tokens, positions, extra, max_len, self.prefix_first(tokens))
    }

    fn hidden_ext_first(&self, tokens: &[u32], positions: &[u32], extra: &[f32], max_len: usize, first: usize) -> Option<Vec<f32>> {
        let l = tokens.len();
        if first > l || self.g1.is_none() || l == 0 || l > max_len || positions.len() != l || extra.len() != l * self.d {
            return None;
        }
        Some(self.hidden_g1_at(tokens, Some((positions, extra)), first, None, None))
    }

    /// `scores` on the CAND forward (`hidden_ext`): legal-character scores of the syllable positions first..first+n.
    pub fn scores_ext(&self, tokens: &[u32], positions: &[u32], extra: &[f32], max_len: usize, first: usize, legal: &[&[u32]]) -> Option<Vec<Vec<f32>>> {
        let d = self.d;
        #[cfg(feature = "arch-prof")]
        let forward_start = std::time::Instant::now();
        let h = self.hidden_ext_first(tokens, positions, extra, max_len, first)?;
        #[cfg(feature = "arch-prof")]
        crate::prof::add_forward(forward_start.elapsed().as_nanos() as u64, tokens.len());
        #[cfg(feature = "arch-prof")]
        let head_start = std::time::Instant::now();
        let result = Some(
            legal
                .iter()
                .enumerate()
                .map(|(i, ids)| {
                    let hi = &h[(first + i) * d..(first + i + 1) * d];
                    ids.iter()
                        .map(|&c| hi.iter().zip(&self.tok[c as usize * d..(c as usize + 1) * d]).map(|(a, b)| a * b).sum())
                        .collect()
                })
                .collect(),
        );
        #[cfg(feature = "arch-prof")]
        crate::prof::add_head(head_start.elapsed().as_nanos() as u64, legal.len());
        result
    }

    pub fn dim(&self) -> usize {
        self.d
    }

    /// G1 full forward, with the model's attention mask.
    fn hidden_g1(&self, tokens: &[u32]) -> Vec<f32> {
        self.hidden_g1_at(tokens, None, self.prefix_first(tokens), None, None)
    }

    fn hidden_g1_at(&self, tokens: &[u32], ext: Option<(&[u32], &[f32])>, first: usize,
        cached: Option<&PrefixEntry>, mut capture: Option<&mut Vec<Vec<f32>>>) -> Vec<f32> {
        let (layers, w) = self.g1.as_ref().unwrap();
        let offset = if cached.is_some() { first } else { 0 };
        let (d, w, l) = (self.d, *w, tokens.len());
        let mut x = vec![0.0f32; l * d];
        for (i, &t) in tokens.iter().enumerate() {
            x[i * d..(i + 1) * d].copy_from_slice(&self.tok[t as usize * d..(t as usize + 1) * d]);
        }
        if let Some((_, extra)) = ext {
            for (xv, ev) in x.iter_mut().zip(extra) {
                *xv += ev;
            }
        }
        let heads = d / HEAD_DIM;
        let half = HEAD_DIM / 2;
        // theta(p, j) = p * 10000^(-2j/32); rotate-half pairs j with j + 16.
        let mut cs = vec![0.0f32; l * half * 2];
        for i in 0..l {
            let p = match ext {
                Some((pos, _)) => pos[i] as usize,
                None => offset + i,
            };
            for j in 0..half {
                let inv = 1.0f32 / 10000f32.powf(2.0 * j as f32 / HEAD_DIM as f32);
                let ang = p as f32 * inv;
                cs[(i * half + j) * 2] = ang.cos();
                cs[(i * half + j) * 2 + 1] = ang.sin();
            }
        }
        let mut h = vec![0.0f32; l * d];
        let mut qkv = vec![0.0f32; l * 3 * d];
        let mut att = vec![0.0f32; l * d];
        let mut tmp = vec![0.0f32; l * d];
        let mut gate = vec![0.0f32; l * w];
        let mut up = vec![0.0f32; l * w];
        let mut scratch = Vec::new();
        let mut qs = crate::qkernel::Scratch::default();
        for (li, layer) in layers.iter().enumerate() {
            h.copy_from_slice(&x);
            for row in h.chunks_mut(d) {
                rms_norm(row, &layer.ln1);
            }
            if let Some(q) = &layer.int8 {
                crate::qkernel::qmatmul(l, &h, d, &q[0], &mut qkv, 3 * d, &mut qs);
            } else {
                matmul(l, d, 3 * d, &h, d, &layer.w_qkv, 3 * d, &mut qkv, 3 * d);
            }
            for p in 0..l {
                for part in 0..2 {
                    for head in 0..heads {
                        let o = p * 3 * d + part * d + head * HEAD_DIM;
                        for j in 0..half {
                            let (c, s) = (cs[(p * half + j) * 2], cs[(p * half + j) * 2 + 1]);
                            let (x1, x2) = (qkv[o + j], qkv[o + half + j]);
                            qkv[o + j] = x1 * c - x2 * s;
                            qkv[o + half + j] = x2 * c + x1 * s;
                        }
                    }
                }
            }
            if let Some(entry) = cached {
                let nk = first + l;
                let mut kv = Vec::with_capacity(nk * 2 * d);
                kv.extend_from_slice(&entry.kv[li]);
                for row in qkv.chunks(3*d) { kv.extend_from_slice(&row[d..]); }
                for head in 0..heads {
                    let o = head * HEAD_DIM;
                    crate::kernel::attention_rows(l, HEAD_DIM, nk, &qkv[o..], 3*d,
                        &kv[o..], 2*d, &kv[d+o..], 2*d, &mut att[o..], d, &mut scratch);
                }
            } else {
                if let Some(capture) = capture.as_mut() {
                    capture.push(qkv.chunks(3*d).take(first).flat_map(|r| r[d..].iter().copied()).collect());
                }
                for head in 0..heads {
                    let o = head * HEAD_DIM;
                    attention(l, HEAD_DIM, if self.prefix { first } else { l }, &qkv[o..], &qkv[d + o..], &qkv[2 * d + o..], 3 * d, &mut att[o..], d, &mut scratch);
                }
            }
            if let Some(q) = &layer.int8 {
                crate::qkernel::qmatmul(l, &att, d, &q[1], &mut tmp, d, &mut qs);
            } else {
                matmul(l, d, d, &att, d, &layer.w_o, d, &mut tmp, d);
            }
            for (xv, tv) in x.iter_mut().zip(&tmp) {
                *xv += tv;
            }
            h.copy_from_slice(&x);
            for row in h.chunks_mut(d) {
                rms_norm(row, &layer.ln2);
            }
            if let Some(q) = &layer.int8 {
                crate::qkernel::qmatmul(l, &h, d, &q[2], &mut gate, w, &mut qs);
            } else {
                matmul(l, d, w, &h, d, &layer.w_gate, w, &mut gate, w);
            }
            if let Some(q) = &layer.int8 {
                crate::qkernel::qmatmul(l, &h, d, &q[3], &mut up, w, &mut qs);
            } else {
                matmul(l, d, w, &h, d, &layer.w_up, w, &mut up, w);
            }
            for (g, u) in gate.iter_mut().zip(&up) {
                *g = *g / (1.0 + (-*g).exp()) * u;
            }
            if let Some(q) = &layer.int8 {
                crate::qkernel::qmatmul(l, &gate, w, &q[4], &mut tmp, d, &mut qs);
            } else {
                matmul(l, w, d, &gate, w, &layer.w_down, d, &mut tmp, d);
            }
            for (xv, tv) in x.iter_mut().zip(&tmp) {
                *xv += tv;
            }
        }
        for row in x.chunks_mut(d) {
            rms_norm(row, &self.lnf_g);
        }
        x
    }

    /// Best legal character id for each syllable position (`legal[i]` for the
    /// i-th syllable); None for a position with no legal character. Returns
    /// None if the sequence does not fit.
    pub fn predict(&self, tokens: &[u32], first: usize, legal: &[&[u32]]) -> Option<Vec<Option<u32>>> {
        let d = self.d;
        #[cfg(feature = "arch-prof")]
        let forward_start = std::time::Instant::now();
        let (h, offset) = self.scoring_hidden(tokens, first)?;
        #[cfg(feature = "arch-prof")]
        crate::prof::add_forward(forward_start.elapsed().as_nanos() as u64, tokens.len());
        #[cfg(feature = "arch-prof")]
        let head_start = std::time::Instant::now();
        let result = Some(
            legal
                .iter()
                .enumerate()
                .map(|(i, ids)| {
                    let hi = &h[(first + i - offset) * d..(first + i + 1 - offset) * d];
                    let mut best: Option<(f32, u32)> = None;
                    for &c in ids.iter() {
                        let e = &self.tok[c as usize * d..(c as usize + 1) * d];
                        let s: f32 = hi.iter().zip(e).map(|(a, b)| a * b).sum();
                        // ids are sorted, so strict > keeps the smallest id on ties.
                        if best.is_none_or(|(b, _)| s > b) {
                            best = Some((s, c));
                        }
                    }
                    best.map(|(_, c)| c)
                })
                .collect(),
        );
        #[cfg(feature = "arch-prof")]
        crate::prof::add_head(head_start.elapsed().as_nanos() as u64, legal.len());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_g1() -> N1 {
        N1::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/g1-tiny-model.bin")).unwrap()
    }
    fn random_g1(seed: u64) -> N1 {
        let mut r = Rng(seed | 1);
        let (d,w) = (32,67);
        N1 { prefix: false, prefix_cache_on: AtomicBool::new(true), prefix_cache: Mutex::new(Vec::new()), g1: Some(((0..2).map(|_| G1Layer {
            ln1: vec![1.0;d], ln2: vec![1.0;d], w_qkv: r.vec(d*3*d),
            w_o: r.vec(d*d), w_gate: r.vec(d*w), w_up: r.vec(d*w), w_down: r.vec(w*d), int8: None,
        }).collect(),w)), d, tok: r.vec(12*d), pos: vec![], layers: vec![], lnf_g: vec![1.0;d], lnf_b: vec![] }
    }
    fn cosine(a: &[f32], b: &[f32]) -> f64 {
        let dot = a.iter().zip(b).map(|(&x,&y)| x as f64*y as f64).sum::<f64>();
        let norm = |x: &[f32]| x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
        dot/(norm(a)*norm(b)).sqrt()
    }
    #[test]
    fn g1_int8_forward_and_extended_scores() {
        use crate::qkernel::{Backend, backend_available, force_backend, BACKEND_TEST_LOCK, BackendReset};
        let _lock = BACKEND_TEST_LOCK.lock().unwrap();
        let _reset = BackendReset;
        for seed in [0,71,193] {
            let fp = if seed == 0 { tiny_g1() } else { random_g1(seed) };
            let mut q = if seed == 0 { tiny_g1() } else { random_g1(seed) };
            q.quantize_int8().unwrap();
            q.quantize_int8().unwrap();
            for layer in &q.g1.as_ref().unwrap().0 {
                assert!(layer.int8.is_some());
                assert!(layer.w_qkv.is_empty() && layer.w_o.is_empty() && layer.w_gate.is_empty() && layer.w_up.is_empty() && layer.w_down.is_empty());
            }
            let toks: Vec<_> = (0..111).map(|i| (i%5+1) as u32).collect();
            let pos: Vec<_> = (0..111).map(|i| (i*3%103) as u32).collect();
            let extra: Vec<_> = (0..111*fp.d).map(|i| (i as f32*0.7).sin()*0.01).collect();
            let legal: &[&[u32]] = &[&[2,3,4,5],&[],&[1,4]];
            let mut prior = None;
            for backend in [Backend::Scalar,Backend::Avx2,Backend::Vnni] {
                if !backend_available(backend) { continue; }
                force_backend(backend).unwrap();
                let h = q.hidden(&toks[..104]).unwrap();
                let f = fp.hidden(&toks[..104]).unwrap();
                for (a,b) in h.chunks(fp.d).zip(f.chunks(fp.d)) { assert!(cosine(a,b) >= 0.99); }
                let ext = q.hidden_ext(&toks,&pos,&extra,111).unwrap();
                let ext_fp = fp.hidden_ext(&toks,&pos,&extra,111).unwrap();
                for (a,b) in ext.chunks(fp.d).zip(ext_fp.chunks(fp.d)) { assert!(cosine(a,b) >= 0.99); }
                let sc = q.scores_ext(&toks,&pos,&extra,111,65,legal).unwrap();
                let sf = fp.scores_ext(&toks,&pos,&extra,111,65,legal).unwrap();
                for (a,b) in sc.iter().zip(&sf).filter(|(a,_)| !a.is_empty()) { assert!(cosine(a,b) >= 0.99); }
                let scores = q.scores(&toks[..104],65,legal).unwrap();
                let pred = q.predict(&toks[..104],65,legal).unwrap();
                for i in 0..legal.len() {
                    let best = scores[i].iter().enumerate().max_by(|(ia,a),(ib,b)| a.total_cmp(b).then(ib.cmp(ia))).map(|(j,_)| legal[i][j]);
                    assert_eq!(pred[i],best);
                }
                let bits: Vec<_> = ext.iter().map(|v| v.to_bits()).collect();
                if let Some(p) = &prior { assert_eq!(&bits,p); }
                prior = Some(bits);
                assert!(q.hidden(&toks).is_none());
                assert!(q.hidden_ext(&toks,&pos,&extra,104).is_none());
            }
            // Equal legal embeddings retain the smallest legal id; an empty set returns None.
            let row = q.tok[7*q.d..8*q.d].to_vec();
            q.tok[8*q.d..9*q.d].copy_from_slice(&row);
            assert_eq!(q.predict(&[1,2,3],1,&[&[7,8],&[]]).unwrap(),vec![Some(7),None]);
            assert_eq!(q.scores(&[1,2,3],1,&[&[7,8]]).unwrap()[0][0].to_bits(),q.scores(&[1,2,3],1,&[&[7,8]]).unwrap()[0][1].to_bits());
        }
    }
    pub(super) fn g2_fixture(stem: &str) -> (N1, serde_json::Value) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        (N1::load(&dir.join(format!("{stem}-tiny-model.bin"))).unwrap(),
         serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{stem}-tiny-expected.json"))).unwrap()).unwrap())
    }

    #[test]
    fn loads_g2_exports_with_same_scores_and_int8() {
        let _lock = crate::qkernel::BACKEND_TEST_LOCK.lock().unwrap();
        for stem in ["g1f", "g1f1"] {
            let (fp, exp) = g2_fixture(stem);
            let (mut q, _) = g2_fixture(stem);
            let count = q.param_count();
            q.quantize_int8().unwrap();
            assert_eq!(q.param_count(), count);
            let ids = |v: &serde_json::Value| -> Vec<u32> {
                v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect()
            };
            let mut max_error = 0.0f64;
            for (k, seq) in exp["seqs"].as_array().unwrap().iter().enumerate() {
                let tokens = ids(seq);
                let first = exp["firsts"][k].as_u64().unwrap() as usize;
                let legal: Vec<Vec<u32>> = exp["legal"][k].as_array().unwrap().iter().map(ids).collect();
                let sets: Vec<&[u32]> = legal.iter().map(Vec::as_slice).collect();
                let got = fp.scores(&tokens, first, &sets).unwrap();
                let quant = q.scores(&tokens, first, &sets).unwrap();
                for (i, row) in got.iter().enumerate() {
                    for (j, &g) in row.iter().enumerate() {
                        let err = (g as f64 - exp["scores"][k][i][j].as_f64().unwrap()).abs();
                        max_error = max_error.max(err);
                        assert!(err <= 1e-4, "{stem} seq {k} pos {i} cand {j}: {err}");
                    }
                    assert!(cosine(row, &quant[i]) >= 0.99);
                }
                for (a, b) in fp.hidden(&tokens).unwrap().chunks(fp.d).zip(q.hidden(&tokens).unwrap().chunks(fp.d)) {
                    assert!(cosine(a, b) >= 0.99);
                }
            }
            println!("{stem} max_score_abs_error={max_error:.10e}");
        }
    }

    #[test]
    fn g2_rejects_invalid_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let bytes = std::fs::read(dir.join("g1f-tiny-model.bin")).unwrap();
        let u = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (d, e, n) = (u(8), u(28), u(32));
        let table = 36 + n * 4;
        let proj = table + n * e * 4;
        let tier2 = proj + e * d * 4;
        let body = tier2 + 8 + u(tier2 + 4) * 4 + u(tier2 + 4) * u(tier2) * 4 + u(tier2) * d * 4;
        let reject = |b: &[u8]| {
            assert_eq!(N1::load_g2(b).err().expect("must reject").kind(), std::io::ErrorKind::InvalidData);
        };
        for (offset, value) in [
            (4, 0), (8, 0), (12, 0), (16, 0), (20, 0), (24, 2), (24, 0x80000000), (28, 0), (32, 0),
            (8, 33), (20, 9), (40, u(36) as u32), (36, u(4) as u32),
            (tier2 + 8, u(40) as u32), // duplicate across tiers
            (4, u(4) as u32 + 1), // incomplete union
            (8, u32::MAX - 31), (12, u32::MAX), (16, u32::MAX), (28, u32::MAX), (32, u32::MAX),
        ] {
            let mut b = bytes.clone();
            b[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            reject(&b);
        }
        // Remove an id and its table row, leaving a structurally complete file.
        let mut b = bytes.clone();
        b.drain(table + (n - 1) * e * 4..table + n * e * 4);
        b.drain(36 + (n - 1) * 4..36 + n * 4);
        b[32..36].copy_from_slice(&((n - 1) as u32).to_le_bytes());
        reject(&b);
        let mut b = bytes.clone();
        b[36..40].copy_from_slice(&(u(40) as u32 + 1).to_le_bytes());
        reject(&b);
        for len in [0, 4, 23, 27, 31, 35, table - 1, proj - 1, tier2 - 1, body - 1, bytes.len() - 1] {
            reject(&bytes[..len]);
        }
        let mut b = bytes.clone(); b.push(0); reject(&b);
        let dw = d * u(16);
        let mut offset = body;
        let mut offsets = vec![table, proj];
        for _ in 0..u(12) {
            for len in [d, d * 3 * d, d * d, d, dw, dw, dw] {
                offsets.push(offset); offset += len * 4;
            }
        }
        offsets.push(offset);
        for offset in offsets {
            for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut b = bytes.clone(); b[offset..offset + 4].copy_from_slice(&v.to_le_bytes()); reject(&b);
            }
        }
    }

    #[test]
    fn g1_width_1024_int8_forward() {
        let _lock = crate::qkernel::BACKEND_TEST_LOCK.lock().unwrap();
        for seed in [7, 71, 193] {
            let fp = N1::random_g1(12, 64, 2, 1024, seed);
            let mut q = N1::random_g1(12, 64, 2, 1024, seed);
            q.quantize_int8().unwrap();
            let tokens: Vec<u32> = (0..104).map(|i| (i % 12) as u32).collect();
            for (a, b) in fp.hidden(&tokens).unwrap().chunks(fp.d).zip(q.hidden(&tokens).unwrap().chunks(fp.d)) {
                assert!(cosine(a, b) >= 0.99);
            }
            let legal: &[&[u32]] = &[&[2, 3, 4, 5], &[], &[1, 4]];
            for (a, b) in fp.scores(&tokens, 65, legal).unwrap().iter().zip(q.scores(&tokens, 65, legal).unwrap()).filter(|(a, _)| !a.is_empty()) {
                assert!(cosine(a, &b) >= 0.99);
            }
        }
    }

    #[cfg(feature = "arch-prof")]
    #[test]
    fn profiling_covers_all_scoring_paths() {
        let m = tiny_g1();
        let tokens = [1, 2, 3];
        crate::prof::take();
        m.scores(&tokens, 1, &[&[2, 3], &[]]).unwrap();
        m.predict(&tokens, 1, &[&[2, 3]]).unwrap();
        m.scores_ext(&tokens, &[0, 1, 2], &vec![0.0; 3 * m.d], 104, 1, &[&[2, 3]]).unwrap();
        let s = crate::prof::take();
        assert_eq!((s.calls, s.l, s.nsyl), (3, 3, 1));
        assert!(s.forward_ns > 0 && s.head_ns > 0);
        assert_eq!(crate::prof::take(), crate::prof::Snapshot::default());
    }

    #[test]
    fn g1_int8_failure_is_atomic() {
        let mut m = random_g1(7);
        let before = m.param_count();
        m.quantize_int8().unwrap();
        assert_eq!(m.param_count(), before);
        assert!(N1::random(12,32,1,7).quantize_int8().is_err());
        let mut m = random_g1(7);
        m.g1.as_mut().unwrap().0[1].w_down[7] = f32::NAN;
        let before = m.g1.as_ref().unwrap().0[0].w_qkv.clone();
        assert!(m.quantize_int8().is_err());
        assert_eq!(m.g1.as_ref().unwrap().0[0].w_qkv,before);
        assert!(m.g1.as_ref().unwrap().0.iter().all(|l| l.int8.is_none()));
        let mut m = random_g1(7);
        m.g1.as_mut().unwrap().1 = 16385;
        assert!(m.quantize_int8().is_err());
        assert!(m.g1.as_ref().unwrap().0.iter().all(|l| l.int8.is_none()));
    }

    #[test]
    #[cfg_attr(not(windows), ignore = "the hash holds bits of the Windows C runtime (exp/tanh)")]
    fn g1_fp32_bit_hash() {
        // Generated before INT8 forward changes: rustc 1.98.1, release,
        // CARGO_TARGET_DIR=target-int8, -j 6, --test-threads=6;
        // Ryzen 9 7950X, original fp32 matrixmultiply path.
        let m = N1::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/g1-tiny-model.bin")).unwrap();
        let h = m.hidden(&[1, 2, 3, 4, 5]).unwrap();
        let hash = h.iter().flat_map(|v| v.to_bits().to_le_bytes()).fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
        assert_eq!(hash, 7115789696518980396);
    }

    #[test]
    fn attention_is_bidirectional() {
        let m = N1::random(12, 32, 2, 7);
        let a = m.hidden(&[2, 3, SEP, 4, 5]).unwrap();
        let b = m.hidden(&[2, 3, SEP, 4, 6]).unwrap();
        // Changing the last token changes the first position's hidden state.
        assert_ne!(&a[..32], &b[..32]);
    }

    #[test]
    fn only_legal_characters_and_ties() {
        let mut m = N1::random(12, 32, 1, 7);
        let toks = [2, SEP, 9];
        let h = m.hidden(&toks).unwrap();
        let hi = h[2 * 32..3 * 32].to_vec();
        // Make char 7 score highest overall, but it is not legal here.
        for c in 0..32 {
            m.tok[7 * 32 + c] = hi[c] * 100.0;
        }
        let h = m.hidden(&toks).unwrap();
        let hi = &h[2 * 32..3 * 32];
        let score = |m: &N1, c: usize| -> f32 { hi.iter().zip(&m.tok[c * 32..(c + 1) * 32]).map(|(a, b)| a * b).sum() };
        let legal: &[u32] = &[3, 4];
        let want = if score(&m, 4) > score(&m, 3) { 4 } else { 3 };
        assert_eq!(m.predict(&toks, 2, &[legal]).unwrap(), vec![Some(want)]);
        // Equal embeddings tie: the smallest id wins.
        let row: Vec<f32> = m.tok[3 * 32..4 * 32].to_vec();
        m.tok[5 * 32..6 * 32].copy_from_slice(&row);
        assert_eq!(m.predict(&toks, 2, &[&[3, 5]]).unwrap(), vec![Some(3)]);
        // No legal character.
        assert_eq!(m.predict(&toks, 2, &[&[]]).unwrap(), vec![None]);
        // Deterministic.
        assert_eq!(m.predict(&toks, 2, &[legal]), m.predict(&toks, 2, &[legal]));
    }

    #[test]
    fn loads_python_export_with_same_scores() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let m = N1::load(&dir.join("n1-tiny-model.bin")).unwrap();
        let exp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("n1-tiny-expected.json")).unwrap()).unwrap();
        let ids = |v: &serde_json::Value| -> Vec<u32> { v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect() };
        for (k, seq) in exp["seqs"].as_array().unwrap().iter().enumerate() {
            let first = exp["firsts"][k].as_u64().unwrap() as usize;
            let legal: Vec<Vec<u32>> = exp["legal"][k].as_array().unwrap().iter().map(ids).collect();
            let sets: Vec<&[u32]> = legal.iter().map(Vec::as_slice).collect();
            let got = m.scores(&ids(seq), first, &sets).unwrap();
            for (i, row) in got.iter().enumerate() {
                for (j, g) in row.iter().enumerate() {
                    let want = exp["scores"][k][i][j].as_f64().unwrap() as f32;
                    assert!((g - want).abs() < 1e-5, "seq {k} pos {i} cand {j}: {g} vs {want}");
                }
            }
        }
    }

    #[test]
    fn loads_g1_export_with_same_scores() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let m = N1::load(&dir.join("g1-tiny-model.bin")).unwrap();
        assert!(m.g1.is_some());
        let exp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("g1-tiny-expected.json")).unwrap()).unwrap();
        let ids = |v: &serde_json::Value| -> Vec<u32> { v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect() };
        for (k, seq) in exp["seqs"].as_array().unwrap().iter().enumerate() {
            let first = exp["firsts"][k].as_u64().unwrap() as usize;
            let legal: Vec<Vec<u32>> = exp["legal"][k].as_array().unwrap().iter().map(ids).collect();
            let sets: Vec<&[u32]> = legal.iter().map(Vec::as_slice).collect();
            let got = m.scores(&ids(seq), first, &sets).unwrap();
            for (i, row) in got.iter().enumerate() {
                for (j, g) in row.iter().enumerate() {
                    let want = exp["scores"][k][i][j].as_f64().unwrap() as f32;
                    assert!((g - want).abs() < 1e-4, "seq {k} pos {i} cand {j}: {g} vs {want}");
                }
            }
        }
    }

    #[test]
    fn position_bound() {
        let m = N1::random(12, 32, 1, 7);
        assert!(m.hidden(&[2u32; POSITIONS]).is_some());
        assert!(m.hidden(&[2u32; POSITIONS + 1]).is_none());
    }
}

#[cfg(test)]
#[path = "n1_prefix_tests.rs"]
pub(crate) mod n1_prefix_tests;
