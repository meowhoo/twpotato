//! Offline PRED inference. Tree nodes are causal sequences, never sibling-visible.
use crate::{
    kernel,
    qkernel::{self, QMat, Scratch},
};
use std::{
    io::{self, Read},
    path::Path,
    time::Instant,
};

// The C runtime implements erf, not the tanh approximation used by the older N1 kernel.
unsafe extern "C" {
    fn erf(x: f64) -> f64;
}
fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + unsafe { erf((x as f64) / std::f64::consts::SQRT_2) } as f32)
}

struct Linear {
    k: usize,
    n: usize,
    w: Vec<f32>,
    q: Option<QMat>,
    b: Vec<f32>,
}
impl Linear {
    fn new(k: usize, n: usize, w: Vec<f32>, b: Vec<f32>, int8: bool) -> Result<Self, &'static str> {
        let q = if int8 {
            Some(QMat::new(k, n, &w)?)
        } else {
            None
        };
        Ok(Self {
            k,
            n,
            w: if int8 { Vec::new() } else { w },
            q,
            b,
        })
    }
    fn run(&self, m: usize, x: &[f32], y: &mut Vec<f32>, s: &mut Scratch) {
        y.resize(m * self.n, 0.0);
        if m == 0 {
            return;
        }
        if let Some(q) = &self.q {
            qkernel::qmatmul(m, x, self.k, q, y, self.n, s)
        } else {
            kernel::matmul(m, self.k, self.n, x, self.k, &self.w, self.n, y, self.n)
        }
        if !self.b.is_empty() {
            for r in y.chunks_exact_mut(self.n) {
                for (v, b) in r.iter_mut().zip(&self.b) {
                    *v += b;
                }
            }
        }
    }
    fn bytes(&self) -> usize {
        self.w.capacity() * 4
            + self.b.capacity() * 4
            + self.q.as_ref().map_or(0, |q| q.memory_bytes())
    }
}
struct Block {
    ln1: [Vec<f32>; 2],
    ln2: [Vec<f32>; 2],
    qkv: Linear,
    out: Linear,
    up: Linear,
    down: Linear,
}
#[derive(Default, Clone, Copy)]
pub struct Timing {
    pub trunk_ms: f64,
    pub output_ms: f64,
    pub other_ms: f64,
}
#[derive(Default)]
pub struct Workspace {
    x: Vec<f32>,
    norm: Vec<f32>,
    qkv: Vec<f32>,
    att: Vec<f32>,
    tmp: Vec<f32>,
    ff: Vec<f32>,
    selected: Vec<f32>,
    logits: Vec<f32>,
    probs: Vec<f32>,
    ancestors: Vec<usize>,
    tokens: Vec<u32>,
    parents: Vec<usize>,
    positions: Vec<usize>,
    needed: Vec<usize>,
    edges: Vec<Vec<(usize, u32)>>,
    z: Vec<f64>,
    target: Vec<f64>,
    scratch: Scratch,
    pub timing: Timing,
}
impl Workspace {
    pub fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + [
                &self.x,
                &self.norm,
                &self.qkv,
                &self.att,
                &self.tmp,
                &self.ff,
                &self.selected,
                &self.logits,
                &self.probs,
            ]
            .iter()
            .map(|v| v.capacity() * 4)
            .sum::<usize>()
            + [
                &self.ancestors,
                &self.parents,
                &self.positions,
                &self.needed,
            ]
            .iter()
            .map(|v| v.capacity() * std::mem::size_of::<usize>())
            .sum::<usize>()
            + self.tokens.capacity() * 4
            + (self.z.capacity() + self.target.capacity()) * 8
            + self.scratch.memory_bytes()
            + self.edges.capacity() * std::mem::size_of::<Vec<(usize, u32)>>()
            + self
                .edges
                .iter()
                .map(|v| v.capacity() * std::mem::size_of::<(usize, u32)>())
                .sum::<usize>()
    }
    pub fn nodes(&self) -> usize {
        self.tokens.len()
    }
    pub fn output_positions(&self) -> usize {
        self.needed.len()
    }
}
pub struct Lm {
    pub vocab: usize,
    pub width: usize,
    pub positions: usize,
    heads: usize,
    emb: Vec<f32>,
    pos: Vec<f32>,
    output_int8: bool,
    blocks: Vec<Block>,
    ln: [Vec<f32>; 2],
    output: Vec<Linear>,
}
impl Lm {
    pub fn load(path: impl AsRef<Path>, int8: bool) -> io::Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?.read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes, int8).map_err(|s| io::Error::new(io::ErrorKind::InvalidData, s))
    }
    pub fn from_bytes(bytes: &[u8], int8: bool) -> Result<Self, &'static str> {
        if bytes.get(..8) != Some(b"TWLMF32\0") {
            return Err("LM magic");
        }
        let mut r = Reader { b: bytes, off: 8 };
        let v = r.u32()?;
        let d = r.u32()?;
        let layers = r.u32()?;
        let heads = r.u32()?;
        let ff = r.u32()?;
        let positions = r.u32()?;
        if v < 3
            || v > 1_000_000
            || d == 0
            || d > 4096
            || heads == 0
            || d % heads != 0
            || layers > 128
            || ff == 0
            || ff > 16384
            || positions == 0
            || positions > 4096
        {
            return Err("LM dimensions");
        }
        let emb = r.vec(v * d)?;
        let pos = r.vec(positions * d)?;
        let mut blocks = Vec::new();
        for _ in 0..layers {
            let ln1 = [r.vec(d)?, r.vec(d)?];
            let ln2 = [r.vec(d)?, r.vec(d)?];
            let qkv = Linear::new(d, 3 * d, r.vec(d * 3 * d)?, r.vec(3 * d)?, int8)?;
            let out = Linear::new(d, d, r.vec(d * d)?, r.vec(d)?, int8)?;
            let up = Linear::new(d, ff, r.vec(d * ff)?, r.vec(ff)?, int8)?;
            let down = Linear::new(ff, d, r.vec(ff * d)?, r.vec(d)?, int8)?;
            blocks.push(Block {
                ln1,
                ln2,
                qkv,
                out,
                up,
                down,
            });
        }
        let ln = [r.vec(d)?, r.vec(d)?];
        if r.off != bytes.len() {
            return Err("LM trailing data");
        }
        // One logical output GEMM, streamed in vocabulary panels. All needed positions share each panel.
        let mut output = Vec::new();
        for start in (0..v).step_by(256) {
            let n = (v - start).min(256);
            let mut w = vec![0.0; d * n];
            for j in 0..n {
                for k in 0..d {
                    w[k * n + j] = emb[(start + j) * d + k];
                }
            }
            output.push(if int8 {
                Linear::new(d, n, w, Vec::new(), true)?
            } else {
                Linear {
                    k: d,
                    n,
                    w: Vec::new(),
                    q: None,
                    b: Vec::new(),
                }
            });
        }
        Ok(Self {
            vocab: v,
            width: d,
            positions,
            heads,
            emb,
            pos,
            output_int8: int8,
            blocks,
            ln,
            output,
        })
    }
    /// Reserve for the complete context and the worst case of two nonterminal nodes per candidate.
    pub fn workspace(&self, max_candidates: usize) -> Workspace {
        let n = self.positions + 2 * max_candidates;
        let d = self.width;
        let ff = self.blocks.iter().map(|b| b.up.n).max().unwrap_or(d);
        let mut ws = Workspace::default();
        ws.x = Vec::with_capacity(n * d);
        ws.norm = Vec::with_capacity(n * d);
        ws.qkv = Vec::with_capacity(n * 3 * d);
        ws.att = Vec::with_capacity(n * d);
        ws.tmp = Vec::with_capacity(n * d);
        ws.ff = Vec::with_capacity(n * ff);
        ws.selected = Vec::with_capacity(n * d);
        ws.logits = Vec::with_capacity(n * 256);
        ws.probs = Vec::with_capacity(self.positions);
        ws.ancestors = Vec::with_capacity(self.positions);
        ws.tokens = Vec::with_capacity(n);
        ws.parents = Vec::with_capacity(n);
        ws.positions = Vec::with_capacity(n);
        ws.needed = Vec::with_capacity(n);
        ws.edges = (0..max_candidates).map(|_| Vec::with_capacity(3)).collect();
        ws.z = Vec::with_capacity(n);
        ws.target = Vec::with_capacity(3 * max_candidates);
        ws.scratch.reserve(n, ff.max(3 * d));
        ws
    }
    pub fn weight_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.blocks.capacity() * std::mem::size_of::<Block>()
            + self.output.capacity() * std::mem::size_of::<Linear>()
            + (self.emb.capacity()
                + self.pos.capacity()
                + self.ln.iter().map(Vec::capacity).sum::<usize>())
                * 4
            + self.output.iter().map(Linear::bytes).sum::<usize>()
            + self
                .blocks
                .iter()
                .map(|b| {
                    (b.ln1.iter().chain(&b.ln2).map(Vec::capacity).sum::<usize>()) * 4
                        + b.qkv.bytes()
                        + b.out.bytes()
                        + b.up.bytes()
                        + b.down.bytes()
                })
                .sum::<usize>()
    }
    /// Context is already suffix_H. No BOS insertion and no position renumbering.
    /// skip_first_log_z adds the same constant to every nonempty candidate score.
    pub fn score(
        &self,
        context: &[u32],
        candidates: &[Vec<u32>],
        ws: &mut Workspace,
        skip_first_log_z: bool,
    ) -> Result<Vec<f64>, &'static str> {
        let start = Instant::now();
        if context.is_empty()
            || context.len() > self.positions
            || context.iter().any(|&t| t as usize >= self.vocab)
        {
            return Err("invalid context");
        }
        if candidates.iter().any(|c| {
            c.is_empty()
                || c.len() > 3
                || c.iter().any(|&t| t as usize >= self.vocab)
                || context.len() + c.len() > self.positions
        }) {
            return Err("invalid candidate");
        }
        ws.tokens.clear();
        ws.parents.clear();
        ws.positions.clear();
        ws.needed.clear();
        for (i, &t) in context.iter().enumerate() {
            ws.tokens.push(t);
            ws.parents.push(if i == 0 { usize::MAX } else { i - 1 });
            ws.positions.push(i);
        }
        if ws.edges.len() < candidates.len() {
            ws.edges.resize_with(candidates.len(), Vec::new);
        }
        for (ci, c) in candidates.iter().enumerate() {
            ws.edges[ci].clear();
            let mut node = context.len() - 1;
            for (j, &t) in c.iter().enumerate() {
                ws.edges[ci].push((node, t));
                if !ws.needed.contains(&node) {
                    ws.needed.push(node);
                }
                if j + 1 < c.len() {
                    let child = (context.len()..ws.tokens.len())
                        .find(|&n| ws.parents[n] == node && ws.tokens[n] == t);
                    node = if let Some(n) = child {
                        n
                    } else {
                        let n = ws.tokens.len();
                        ws.tokens.push(t);
                        ws.parents.push(node);
                        ws.positions.push(context.len() + j);
                        n
                    };
                }
            }
        }
        let setup = start.elapsed().as_secs_f64() * 1000.0;
        let trunk = Instant::now();
        self.forward(ws);
        let trunk_ms = trunk.elapsed().as_secs_f64() * 1000.0;
        let output = Instant::now();
        let d = self.width;
        let m = ws.needed.len();
        ws.selected.resize(m * d, 0.0);
        for (i, &n) in ws.needed.iter().enumerate() {
            ws.selected[i * d..(i + 1) * d].copy_from_slice(&ws.x[n * d..(n + 1) * d]);
        }
        ws.z.clear();
        ws.z.resize(m, f64::NEG_INFINITY);
        let edges = ws.edges[..candidates.len()].iter().map(Vec::len).sum();
        ws.target.clear();
        ws.target.resize(edges, 0.0);
        let mut base = 0;
        for panel in &self.output {
            if self.output_int8 {
                panel.run(m, &ws.selected, &mut ws.logits, &mut ws.scratch);
            } else {
                ws.logits.resize(m * panel.n, 0.0);
                if m > 0 {
                    unsafe {
                        matrixmultiply::sgemm(
                            m,
                            d,
                            panel.n,
                            1.0,
                            ws.selected.as_ptr(),
                            d as isize,
                            1,
                            self.emb.as_ptr().add(base * d),
                            1,
                            d as isize,
                            0.0,
                            ws.logits.as_mut_ptr(),
                            panel.n as isize,
                            1,
                        );
                    }
                }
            }
            for i in 0..m {
                if skip_first_log_z && ws.needed[i] == context.len() - 1 {
                    continue;
                }
                // Online logsumexp in f64. No vocabulary-sized probability tensor retained.
                let row = &ws.logits[i * panel.n..(i + 1) * panel.n];
                let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
                let sum = row.iter().map(|&l| (l as f64 - max).exp()).sum::<f64>();
                let x = max + sum.ln();
                let z = ws.z[i];
                ws.z[i] = if z == f64::NEG_INFINITY {
                    x
                } else {
                    let a = z.max(x);
                    a + ((z - a).exp() + (x - a).exp()).ln()
                };
            }
            let mut ei = 0;
            for e in &ws.edges[..candidates.len()] {
                for &(node, t) in e {
                    let t = t as usize;
                    if t >= base && t < base + panel.n {
                        let i = ws.needed.iter().position(|&n| n == node).unwrap();
                        ws.target[ei] = ws.logits[i * panel.n + t - base] as f64;
                    }
                    ei += 1;
                }
            }
            base += panel.n;
        }
        let output_ms = output.elapsed().as_secs_f64() * 1000.0;
        let tail = Instant::now();
        let mut result = Vec::with_capacity(candidates.len());
        let mut ei = 0;
        for e in &ws.edges[..candidates.len()] {
            let mut s = 0.0;
            for &(node, _) in e {
                let i = ws.needed.iter().position(|&n| n == node).unwrap();
                s += ws.target[ei]
                    - if skip_first_log_z && node == context.len() - 1 {
                        0.0
                    } else {
                        ws.z[i]
                    };
                ei += 1;
            }
            result.push(s);
        }
        ws.timing = Timing {
            trunk_ms,
            output_ms,
            other_ms: setup + tail.elapsed().as_secs_f64() * 1000.0,
        };
        Ok(result)
    }
    fn forward(&self, ws: &mut Workspace) {
        let d = self.width;
        let n = ws.tokens.len();
        let dh = d / self.heads;
        ws.x.resize(n * d, 0.0);
        ws.att.resize(n * d, 0.0);
        for i in 0..n {
            let t = ws.tokens[i] as usize;
            for j in 0..d {
                ws.x[i * d + j] = self.pos[ws.positions[i] * d + j] + self.emb[t * d + j];
            }
        }
        for b in &self.blocks {
            norm(&ws.x, &mut ws.norm, &b.ln1, d);
            b.qkv.run(n, &ws.norm, &mut ws.qkv, &mut ws.scratch);
            ws.att.fill(0.0);
            for i in 0..n {
                ws.ancestors.clear();
                let mut a = i;
                loop {
                    ws.ancestors.push(a);
                    a = ws.parents[a];
                    if a == usize::MAX {
                        break;
                    }
                }
                ws.ancestors.reverse();
                ws.probs.resize(ws.ancestors.len(), 0.0);
                for h in 0..self.heads {
                    for (ai, &a) in ws.ancestors.iter().enumerate() {
                        let mut dot = 0.0;
                        for j in h * dh..(h + 1) * dh {
                            dot += ws.qkv[i * 3 * d + j] * ws.qkv[a * 3 * d + d + j];
                        }
                        ws.probs[ai] = dot / (dh as f32).sqrt();
                    }
                    kernel::softmax(&mut ws.probs);
                    for (ai, &a) in ws.ancestors.iter().enumerate() {
                        for j in h * dh..(h + 1) * dh {
                            ws.att[i * d + j] += ws.probs[ai] * ws.qkv[a * 3 * d + 2 * d + j];
                        }
                    }
                }
            }
            b.out.run(n, &ws.att, &mut ws.tmp, &mut ws.scratch);
            for (x, y) in ws.x.iter_mut().zip(&ws.tmp) {
                *x += y;
            }
            norm(&ws.x, &mut ws.norm, &b.ln2, d);
            b.up.run(n, &ws.norm, &mut ws.ff, &mut ws.scratch);
            for x in &mut ws.ff {
                *x = gelu(*x);
            }
            b.down.run(n, &ws.ff, &mut ws.tmp, &mut ws.scratch);
            for (x, y) in ws.x.iter_mut().zip(&ws.tmp) {
                *x += y;
            }
        }
        for row in ws.x.chunks_exact_mut(d) {
            kernel::layer_norm(row, &self.ln[0], &self.ln[1]);
        }
    }
    /// Reference algorithm: each candidate step recomputes its entire causal sequence.
    pub fn score_naive(
        &self,
        context: &[u32],
        candidates: &[Vec<u32>],
    ) -> Result<Vec<f64>, &'static str> {
        let mut result = Vec::new();
        let mut ws = Workspace::default();
        for c in candidates {
            let mut left = context.to_vec();
            let mut s = 0.0;
            for &t in c {
                s += self.score(&left, &[vec![t]], &mut ws, false)?[0];
                left.push(t);
            }
            result.push(s);
        }
        Ok(result)
    }
}
fn norm(x: &[f32], y: &mut Vec<f32>, ln: &[Vec<f32>; 2], d: usize) {
    y.clear();
    y.extend_from_slice(x);
    for row in y.chunks_exact_mut(d) {
        kernel::layer_norm(row, &ln[0], &ln[1]);
    }
}
struct Reader<'a> {
    b: &'a [u8],
    off: usize,
}
impl Reader<'_> {
    fn u32(&mut self) -> Result<usize, &'static str> {
        let a = self.b.get(self.off..self.off + 4).ok_or("LM truncated")?;
        self.off += 4;
        Ok(u32::from_le_bytes(a.try_into().unwrap()) as usize)
    }
    fn vec(&mut self, n: usize) -> Result<Vec<f32>, &'static str> {
        let size = n.checked_mul(4).ok_or("LM size")?;
        let end = self.off.checked_add(size).ok_or("LM size")?;
        let a = self.b.get(self.off..end).ok_or("LM truncated")?;
        self.off = end;
        let v: Vec<f32> = a
            .chunks_exact(4)
            .map(|a| f32::from_le_bytes(a.try_into().unwrap()))
            .collect();
        if v.iter().any(|v| !v.is_finite()) {
            return Err("LM nonfinite");
        }
        Ok(v)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn random() -> Lm {
        random_precision(false)
    }
    fn random_precision(int8: bool) -> Lm {
        let (v, d, l, h, f, p) = (19, 8, 2, 2, 16, 12);
        let mut b = b"TWLMF32\0".to_vec();
        for n in [v, d, l, h, f, p] {
            b.extend_from_slice(&(n as u32).to_le_bytes());
        }
        let mut state = 20261029u64;
        let mut add = |n: usize, one: bool| {
            for _ in 0..n {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let x = if one {
                    1.0
                } else {
                    ((state % 10001) as f32 - 5000.0) / 25000.0
                };
                b.extend_from_slice(&x.to_le_bytes());
            }
        };
        add(v * d, false);
        add(p * d, false);
        for _ in 0..l {
            add(d, true);
            add(d, false);
            add(d, true);
            add(d, false);
            for (k, n) in [(d, 3 * d), (d, d), (d, f), (f, d)] {
                add(k * n, false);
                add(n, false);
            }
        }
        add(d, true);
        add(d, false);
        Lm::from_bytes(&b, int8).unwrap()
    }
    #[test]
    fn int8_preserves_input_embeddings_and_quantizes_projections() {
        let mut f = random();
        let mut q = random_precision(true);
        assert_eq!(q.emb, f.emb);
        assert_eq!(q.pos, f.pos);
        assert!(q.output_int8);
        assert!(!f.output_int8);
        for b in &q.blocks {
            for linear in [&b.qkv, &b.out, &b.up, &b.down] {
                assert!(linear.q.is_some());
                assert!(linear.w.is_empty());
            }
        }
        assert!(q.output.iter().all(|p| p.q.is_some() && p.w.is_empty()));
        // Isolate input lookup from quantized projections, including nonzero positions.
        f.blocks.clear();
        q.blocks.clear();
        let mut fw = Workspace::default();
        let mut qw = Workspace::default();
        let candidates = [vec![3, 4], vec![5, 6, 7]];
        let fs = f.score(&[1, 8, 9], &candidates, &mut fw, false).unwrap();
        let qs = q.score(&[1, 8, 9], &candidates, &mut qw, false).unwrap();
        assert_eq!(fw.x, qw.x);
        assert!(qs.iter().all(|s| s.is_finite()));
        assert!(fs.iter().zip(qs).any(|(a, b)| (a - b).abs() > 1e-6));
    }
    #[test]
    fn tree_matches_recomputation_and_deduplicates() {
        let m = random();
        let mut w = Workspace::default();
        for h in [vec![1], vec![2, 3, 4, 5, 6]] {
            let c = vec![
                vec![3, 4, 5],
                vec![3, 4, 6],
                vec![3],
                vec![7, 8],
                vec![7, 4, 9],
                vec![3, 4, 5],
            ];
            let a = m.score(&h, &c, &mut w, false).unwrap();
            let b = m.score_naive(&h, &c).unwrap();
            assert_eq!(w.nodes(), h.len() + 4);
            assert_eq!(w.output_positions(), 5);
            for (a, b) in a.iter().zip(b) {
                assert!((a - b).abs() < 1e-5, "{a} {b}");
            }
            let shifted = m.score(&h, &c, &mut w, true).unwrap();
            let delta = shifted[0] - a[0];
            for (x, y) in shifted.iter().zip(a) {
                assert!((x - y - delta).abs() < 1e-10);
            }
        }
    }
    #[test]
    fn bad_inputs() {
        let m = random();
        let mut w = Workspace::default();
        assert!(m.score(&[], &[vec![3]], &mut w, false).is_err());
        assert!(m.score(&[1], &[vec![99]], &mut w, false).is_err());
        assert!(Lm::from_bytes(b"bad", false).is_err());
    }
}
