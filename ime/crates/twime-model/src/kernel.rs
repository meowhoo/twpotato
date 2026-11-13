//! Numeric kernels shared by the model forward.
//!
//! Copied from bench/s3_latency/src/model.rs (N3 M1, MIT, same project): `matmul`, `matmul_bt`, `layer_norm`,
//! `softmax`, `gelu`, `allowed`, `attention` and their tests, unchanged. The old candidate-scoring `Model` is not
//! carried over.

/// Row-major matrix multiply: c (m×n) = a (m×k) · b (k×n) with given row
/// strides, overwriting c.
#[allow(clippy::too_many_arguments)]
pub(crate) fn matmul(m: usize, k: usize, n: usize, a: &[f32], rsa: usize, b: &[f32], rsb: usize, c: &mut [f32], rsc: usize) {
    unsafe {
        matrixmultiply::sgemm(
            m, k, n, 1.0,
            a.as_ptr(), rsa as isize, 1,
            b.as_ptr(), rsb as isize, 1,
            0.0,
            c.as_mut_ptr(), rsc as isize, 1,
        );
    }
}

/// c (m×n) = a (m×k) · bᵀ where b is n×k with row stride rsb.
#[allow(clippy::too_many_arguments)]
fn matmul_bt(m: usize, k: usize, n: usize, a: &[f32], rsa: usize, b: &[f32], rsb: usize, c: &mut [f32], rsc: usize) {
    unsafe {
        matrixmultiply::sgemm(
            m, k, n, 1.0,
            a.as_ptr(), rsa as isize, 1,
            b.as_ptr(), 1, rsb as isize,
            0.0,
            c.as_mut_ptr(), rsc as isize, 1,
        );
    }
}

pub(crate) fn layer_norm(x: &mut [f32], gamma: &[f32], beta: &[f32]) {
    let n = x.len() as f32;
    let mean = x.iter().sum::<f32>() / n;
    let var = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
    let inv = 1.0 / (var + 1e-5).sqrt();
    for (i, v) in x.iter_mut().enumerate() {
        *v = (*v - mean) * inv * gamma[i] + beta[i];
    }
}

pub(crate) fn softmax(x: &mut [f32]) {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    for v in x.iter_mut() {
        *v /= sum;
    }
}

pub(crate) fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + (0.797_884_6 * (x + 0.044_715 * x * x * x)).tanh())
}

/// Whether row token `i` may attend to column token `j` in a sequence whose
/// first `prefix_len` tokens are the prefix.
pub(crate) fn allowed(i: usize, j: usize, prefix_len: usize) -> bool {
    j < prefix_len || i >= prefix_len
}

/// Single-head attention over q, k, v (each L×dh, row stride `rs`), writing
/// L×dh into `out` (row stride `ors`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn attention(l: usize, dh: usize, prefix_len: usize, q: &[f32], k: &[f32], v: &[f32], rs: usize, out: &mut [f32], ors: usize, scratch: &mut Vec<f32>) {
    scratch.resize(l * l, 0.0);
    matmul_bt(l, dh, l, q, rs, k, rs, scratch, l);
    let scale = 1.0 / (dh as f32).sqrt();
    for i in 0..l {
        let row = &mut scratch[i * l..(i + 1) * l];
        for (j, s) in row.iter_mut().enumerate() {
            *s = if allowed(i, j, prefix_len) { *s * scale } else { f32::NEG_INFINITY };
        }
        softmax(row);
    }
    matmul(l, l, dh, scratch, l, v, rs, out, ors);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn matmul_small() {
        // [1 2; 3 4] · [5 6; 7 8] = [19 22; 43 50]
        let mut c = vec![0.0; 4];
        matmul(2, 2, 2, &[1., 2., 3., 4.], 2, &[5., 6., 7., 8.], 2, &mut c, 2);
        assert_eq!(c, vec![19., 22., 43., 50.]);
        // a · bᵀ with b = [5 6; 7 8] → [17 23; 39 53]
        matmul_bt(2, 2, 2, &[1., 2., 3., 4.], 2, &[5., 6., 7., 8.], 2, &mut c, 2);
        assert_eq!(c, vec![17., 23., 39., 53.]);
    }

    #[test]
    fn layer_norm_softmax_gelu() {
        let mut x = vec![1.0, 3.0];
        layer_norm(&mut x, &[1.0, 1.0], &[0.0, 0.0]);
        // mean 2, var 1 → ±1 / sqrt(1 + 1e-5)
        assert!(close(x[0], -0.999995) && close(x[1], 0.999995));
        let mut s = vec![0.0, (2.0f32).ln()];
        softmax(&mut s);
        assert!(close(s[0], 1.0 / 3.0) && close(s[1], 2.0 / 3.0));
        assert!(close(gelu(0.0), 0.0));
        assert!((gelu(1.0) - 0.841_192).abs() < 1e-4);
    }

    #[test]
    fn mask_shape() {
        // prefix of 2, candidate of 2
        for i in 0..4 {
            for j in 0..4 {
                let expect = if i < 2 { j < 2 } else { true };
                assert_eq!(allowed(i, j, 2), expect, "i={i} j={j}");
            }
        }
    }

    #[test]
    fn attention_connectivity_by_hand() {
        // L=3, prefix 2, candidate 1, dh=1; q=k=0 → uniform over allowed.
        let q = [0.0f32; 3];
        let k = [0.0f32; 3];
        let v = [1.0f32, 3.0, 100.0];
        let mut out = [0.0f32; 3];
        let mut scratch = Vec::new();
        attention(3, 1, 2, &q, &k, &v, 1, &mut out, 1, &mut scratch);
        // Prefix rows see only the prefix: (1 + 3) / 2 = 2.
        assert!(close(out[0], 2.0) && close(out[1], 2.0));
        // The candidate row sees all three: (1 + 3 + 100) / 3.
        assert!(close(out[2], 104.0 / 3.0));
    }
}

/// Rectangular single-head attention: every query sees every key.
#[allow(clippy::too_many_arguments)]
pub(crate) fn attention_rows(nq: usize, dh: usize, nk: usize, q: &[f32], rsq: usize,
    k: &[f32], rsk: usize, v: &[f32], rsv: usize, out: &mut [f32], ors: usize,
    scratch: &mut Vec<f32>) {
    if nq == 0 { return; }
    scratch.resize(nq * nk, 0.0);
    matmul_bt(nq, dh, nk, q, rsq, k, rsk, scratch, nk);
    let scale = 1.0 / (dh as f32).sqrt();
    for row in scratch.chunks_mut(nk) {
        for s in row.iter_mut() { *s *= scale; }
        softmax(row);
    }
    matmul(nq, nk, dh, scratch, nk, v, rsv, out, ors);
}
