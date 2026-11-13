//! Optional W8A8: shared scalar quantization and integer-only SIMD.
use std::sync::{
    OnceLock,
    atomic::{AtomicU8, Ordering},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Backend {
    Auto = 0,
    Scalar = 1,
    Avx2 = 2,
    Vnni = 3,
}
static FORCED: AtomicU8 = AtomicU8::new(0);
static AVAILABLE: OnceLock<[bool; 4]> = OnceLock::new();
pub fn backend_available(b: Backend) -> bool {
    AVAILABLE.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            [
                true,
                true,
                std::is_x86_feature_detected!("avx2"),
                std::is_x86_feature_detected!("avx512f")
                    && std::is_x86_feature_detected!("avx512bw")
                    && std::is_x86_feature_detected!("avx512vnni"),
            ]
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            [true, true, false, false]
        }
    })[b as usize]
}
/// Process-global test/benchmark override. Unsupported choices leave the override unchanged.
pub fn force_backend(b: Backend) -> Result<(), &'static str> {
    if !backend_available(b) {
        return Err("unsupported int8 backend");
    }
    FORCED.store(b as u8, Ordering::Relaxed);
    Ok(())
}
fn backend() -> Backend {
    match FORCED.load(Ordering::Relaxed) {
        1 => Backend::Scalar,
        2 => Backend::Avx2,
        3 => Backend::Vnni,
        _ if backend_available(Backend::Vnni) => Backend::Vnni,
        _ if backend_available(Backend::Avx2) => Backend::Avx2,
        _ => Backend::Scalar,
    }
}
pub struct QMat {
    q: Vec<i8>,
    scales: Vec<f32>,
    sums: Vec<i32>,
    k: usize,
    n: usize,
    k4: usize,
}
// Both weights and activations use elementwise division and round-away-from-zero.
#[inline]
fn quantize(x: &[f32], q: &mut [i8], activation: bool) -> Result<f32, &'static str> {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        return unsafe { quantize_avx2(x, q, activation) };
    }
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("sse4.1") {
        // Same scalar source, with hardware rounding available to LLVM.
        return unsafe { quantize_sse41(x, q, activation) };
    }
    quantize_scalar(x, q, activation)
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn quantize_avx2(x: &[f32], q: &mut [i8], activation: bool) -> Result<f32, &'static str> {
    quantize_scalar(x, q, activation)
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn quantize_sse41(x: &[f32], q: &mut [i8], activation: bool) -> Result<f32, &'static str> {
    quantize_scalar(x, q, activation)
}
#[inline(always)]
fn quantize_scalar(x: &[f32], q: &mut [i8], activation: bool) -> Result<f32, &'static str> {
    // Independent maxima avoid a serial max dependency. Max of finite absolute
    // values is exact and order independent; any non-finite value rejects the row.
    let mut maxima = [0.0f32; 4];
    let mut finite = true;
    let mut chunks = x.chunks_exact(4);
    for chunk in &mut chunks {
        for c in 0..4 {
            finite &= chunk[c].is_finite();
            maxima[c] = maxima[c].max(chunk[c].abs());
        }
    }
    let mut max = maxima.into_iter().fold(0.0f32, f32::max);
    for &v in chunks.remainder() {
        finite &= v.is_finite();
        max = max.max(v.abs());
    }
    if !finite {
        return Err("non-finite int8 input");
    }
    if activation && max > 1e30 {
        return Err("int8 activation out of range");
    }
    if max < 1e-30 {
        q.fill(0);
        return Ok(0.0);
    }
    let scale = max / 127.0;
    for (v, dst) in x.iter().zip(q) {
        *dst = (v / scale).round().clamp(-127.0, 127.0) as i8;
    }
    Ok(scale)
}
impl QMat {
    pub fn memory_bytes(&self) -> usize { self.q.capacity() + (self.scales.capacity() + self.sums.capacity()) * 4 }

    pub fn new(k: usize, n: usize, w: &[f32]) -> Result<Self, &'static str> {
        if k == 0 || k > 16384 || n == 0 || k.checked_mul(n) != Some(w.len()) {
            return Err("invalid int8 matrix shape (k must be 1..=16384)");
        }
        let k4 = k.next_multiple_of(4);
        // [32-padded n / 16][k4 / 4][16 columns][4 k bytes].
        let mut mat = Self {
            q: vec![0; k4 * n.next_multiple_of(32)],
            scales: vec![0.0; n.next_multiple_of(32)],
            sums: vec![0; n.next_multiple_of(32)],
            k,
            n,
            k4,
        };
        let mut q = vec![0; k];
        let mut col = vec![0.0; k];
        for j in 0..n {
            for i in 0..k {
                col[i] = w[i * n + j];
            }
            mat.scales[j] = quantize(&col, &mut q, false)?;
            for i in 0..k {
                mat.q[(j / 16) * k4 * 16 + (i / 4) * 64 + (j % 16) * 4 + i % 4] = q[i];
            }
            mat.sums[j] = q.iter().map(|&v| v as i32).sum();
        }
        Ok(mat)
    }
    #[cfg(test)]
    fn at(&self, p: usize, j: usize) -> i8 {
        self.q[(j / 16) * self.k4 * 16 + (p / 4) * 64 + (j % 16) * 4 + p % 4]
    }
}
#[derive(Default)]
pub struct Scratch {
    q: Vec<i8>,
    scales: Vec<f32>,
}
impl Scratch {
    pub fn reserve(&mut self, rows: usize, k: usize) { self.q.reserve(rows*k.next_multiple_of(4)); self.scales.reserve(rows); }
    pub fn memory_bytes(&self) -> usize { self.q.capacity() + self.scales.capacity() * 4 }
}
pub fn selected_backend() -> Backend { backend() }
pub fn qmatmul(
    m: usize,
    x: &[f32],
    rsx: usize,
    w: &QMat,
    out: &mut [f32],
    rso: usize,
    scratch: &mut Scratch,
) {
    let (k, n) = (w.k, w.n);
    assert!(rsx >= k && rso >= n);
    // Checked sizes: the SIMD kernels index raw pointers from these strides.
    let need = |rs: usize, w: usize| (m - 1).checked_mul(rs).and_then(|v| v.checked_add(w));
    assert!(m == 0 || (need(rsx, k).is_some_and(|v| x.len() >= v) && need(rso, n).is_some_and(|v| out.len() >= v)));
    assert!(m.checked_mul(w.k4).is_some(), "int8 scratch size overflow");
    let k4 = w.k4;
    scratch.q.resize(m * k4, 0);
    scratch.scales.resize(m, 0.0);
    for i in 0..m {
        let q = &mut scratch.q[i * k4..(i + 1) * k4];
        q[k..].fill(0);
        scratch.scales[i] =
            quantize(&x[i * rsx..i * rsx + k], &mut q[..k], true).unwrap_or_else(|_| {
                q.fill(0);
                f32::NAN
            });
    }
    let b = backend();
    if b == Backend::Scalar {
        for j in (0..n).step_by(16) {
            for i in (0..m).step_by(2) {
                let a = &scratch.q[i * k4..];
                let weights = &w.q[j / 16 * k4 * 16..][..k4 * 16];
                let mr = (m - i).min(2);
                let acc = if mr == 2 {
                    scalar::<2>(a, weights, k4)
                } else {
                    scalar::<1>(a, weights, k4)
                };
                for r in 0..mr {
                    for c in 0..(n - j).min(16) {
                        out[(i + r) * rso + j + c] =
                            (acc[r][c] as f32) * scratch.scales[i + r] * w.scales[j + c];
                    }
                }
            }
        }
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        let rows = if b == Backend::Vnni { 6 } else { 4 };
        let step = if b == Backend::Vnni { 32 } else { 8 };
        // Keep each packed weight panel hot in L1 while visiting all input rows.
        for j in (0..n).step_by(step) {
            for i in (0..m).step_by(rows) {
                let mr = (m - i).min(rows);
                let a = &scratch.q[i * k4..];
                let weights = &w.q[(j / 16) * k4 * 16 + (j % 16) * 4..];
                macro_rules! run {
                    ($r:literal) => {
                        unsafe {
                            if b == Backend::Vnni {
                                vnni::<$r>(
                                    a,
                                    weights,
                                    &w.sums[j..],
                                    &w.scales[j..],
                                    &scratch.scales[i..],
                                    k4,
                                    &mut out[i * rso + j..],
                                    rso,
                                    (n - j).min(32),
                                )
                            } else {
                                avx2::<$r>(
                                    a,
                                    weights,
                                    &w.scales[j..],
                                    &scratch.scales[i..],
                                    k4,
                                    &mut out[i * rso + j..],
                                    rso,
                                    (n - j).min(8),
                                )
                            }
                        }
                    };
                }
                match mr {
                    6 => unsafe {
                        vnni::<6>(
                            a,
                            weights,
                            &w.sums[j..],
                            &w.scales[j..],
                            &scratch.scales[i..],
                            k4,
                            &mut out[i * rso + j..],
                            rso,
                            (n - j).min(32),
                        )
                    },
                    5 => unsafe {
                        vnni::<5>(
                            a,
                            weights,
                            &w.sums[j..],
                            &w.scales[j..],
                            &scratch.scales[i..],
                            k4,
                            &mut out[i * rso + j..],
                            rso,
                            (n - j).min(32),
                        )
                    },
                    4 => run!(4),
                    3 => run!(3),
                    2 => run!(2),
                    _ => run!(1),
                };
            }
        }
    }
}
fn scalar<const R: usize>(a: &[i8], w: &[i8], k4: usize) -> [[i32; 16]; 2] {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        return unsafe { scalar_autovec::<R>(a, w, k4) };
    }
    // Apple silicon only for now: the one arm64 system this has been measured and compared on.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if std::arch::is_aarch64_feature_detected!("dotprod") {
        return unsafe { neon_dot::<R>(a, w, k4) };
    }
    scalar_portable::<R>(a, w, k4)
}
/// The same integers as `scalar_portable`: signed 8-bit products summed in 32 bits are exact (|sum| <= 16384*128*128),
/// so the order of the additions does not matter.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[target_feature(enable = "neon,dotprod")]
unsafe fn neon_dot<const R: usize>(a: &[i8], w: &[i8], k4: usize) -> [[i32; 16]; 2] {
    use std::arch::aarch64::*;
    let a0 = &a[..k4];
    let a1 = if R == 2 { &a[k4..2 * k4] } else { a0 };
    // Four vectors of four output lanes for each row.
    let mut acc = [vdupq_n_s32(0); 8];
    for ((a0, a1), w) in a0.chunks_exact(4).zip(a1.chunks_exact(4)).zip(w.chunks_exact(64)) {
        // The four activations of this step, repeated for every lane: each lane multiplies them with its own four weights.
        let four = |a: &[i8]| vreinterpretq_s8_s32(vdupq_n_s32(i32::from_ne_bytes([a[0] as u8, a[1] as u8, a[2] as u8, a[3] as u8])));
        let (b0, b1) = (four(a0), four(a1));
        for i in 0..4 {
            // In bounds: `w` is a chunk of exactly 64 bytes.
            let wi = unsafe { vld1q_s8(w.as_ptr().add(i * 16)) };
            acc[i] = vdotq_s32(acc[i], wi, b0);
            if R == 2 {
                acc[4 + i] = vdotq_s32(acc[4 + i], wi, b1);
            }
        }
    }
    let mut out = [[0i32; 16]; 2];
    for i in 0..4 {
        // In bounds: four lanes at 0, 4, 8, 12 of a row of 16.
        unsafe {
            vst1q_s32(out[0].as_mut_ptr().add(i * 4), acc[i]);
            vst1q_s32(out[1].as_mut_ptr().add(i * 4), acc[4 + i]);
        }
    }
    out
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn scalar_autovec<const R: usize>(a: &[i8], w: &[i8], k4: usize) -> [[i32; 16]; 2] {
    scalar_portable::<R>(a, w, k4)
}
#[inline(always)]
fn scalar_portable<const R: usize>(a: &[i8], w: &[i8], k4: usize) -> [[i32; 16]; 2] {
    let mut c0 = [0; 16];
    let mut c1 = [0; 16];
    let a0 = &a[..k4];
    let a1 = if R == 2 { &a[k4..2 * k4] } else { a0 };
    for ((a0, a1), w) in a0
        .chunks_exact(4)
        .zip(a1.chunks_exact(4))
        .zip(w.chunks_exact(64))
    {
        // Independent output lanes, with shared weights across two rows.
        for c in 0..16 {
            let (w0, w1, w2, w3) = (
                w[c * 4] as i32,
                w[c * 4 + 1] as i32,
                w[c * 4 + 2] as i32,
                w[c * 4 + 3] as i32,
            );
            c0[c] += a0[0] as i32 * w0 + a0[1] as i32 * w1 + a0[2] as i32 * w2 + a0[3] as i32 * w3;
            if R == 2 {
                c1[c] +=
                    a1[0] as i32 * w0 + a1[1] as i32 * w1 + a1[2] as i32 * w2 + a1[3] as i32 * w3;
            }
        }
    }
    [c0, c1]
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2<const R: usize>(
    a: &[i8],
    w: &[i8],
    scales: &[f32],
    a_scales: &[f32],
    k4: usize,
    out: &mut [f32],
    rso: usize,
    nc: usize,
) {
    use std::arch::x86_64::*;
    unsafe {
        let mut c00 = _mm256_setzero_si256();
        let mut c10 = _mm256_setzero_si256();
        let mut c20 = _mm256_setzero_si256();
        let mut c30 = _mm256_setzero_si256();
        let ones = _mm256_set1_epi16(1);
        for p in (0..k4).step_by(4) {
            let w0 = _mm256_loadu_si256(w.as_ptr().add(p * 16).cast());
            if R > 0 {
                let bits = a.as_ptr().add(0 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm256_set1_epi32(bits);
                let prod = _mm256_maddubs_epi16(_mm256_abs_epi8(v), _mm256_sign_epi8(w0, v));
                c00 = _mm256_add_epi32(c00, _mm256_madd_epi16(prod, ones));
            }
            if R > 1 {
                let bits = a.as_ptr().add(1 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm256_set1_epi32(bits);
                let prod = _mm256_maddubs_epi16(_mm256_abs_epi8(v), _mm256_sign_epi8(w0, v));
                c10 = _mm256_add_epi32(c10, _mm256_madd_epi16(prod, ones));
            }
            if R > 2 {
                let bits = a.as_ptr().add(2 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm256_set1_epi32(bits);
                let prod = _mm256_maddubs_epi16(_mm256_abs_epi8(v), _mm256_sign_epi8(w0, v));
                c20 = _mm256_add_epi32(c20, _mm256_madd_epi16(prod, ones));
            }
            if R > 3 {
                let bits = a.as_ptr().add(3 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm256_set1_epi32(bits);
                let prod = _mm256_maddubs_epi16(_mm256_abs_epi8(v), _mm256_sign_epi8(w0, v));
                c30 = _mm256_add_epi32(c30, _mm256_madd_epi16(prod, ones));
            }
        }
        let s0 = _mm256_loadu_ps(scales.as_ptr().add(0));
        if R > 0 {
            let scale = _mm256_set1_ps(a_scales[0]);
            let y = _mm256_mul_ps(_mm256_mul_ps(_mm256_cvtepi32_ps(c00), scale), s0);
            let dst = out.as_mut_ptr().add(0 * rso);
            if nc == 8 {
                _mm256_storeu_ps(dst, y);
            } else {
                let mask = _mm256_cmpgt_epi32(
                    _mm256_set1_epi32(nc as i32),
                    _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7),
                );
                _mm256_maskstore_ps(dst, mask, y);
            }
        }
        if R > 1 {
            let scale = _mm256_set1_ps(a_scales[1]);
            let y = _mm256_mul_ps(_mm256_mul_ps(_mm256_cvtepi32_ps(c10), scale), s0);
            let dst = out.as_mut_ptr().add(1 * rso);
            if nc == 8 {
                _mm256_storeu_ps(dst, y);
            } else {
                let mask = _mm256_cmpgt_epi32(
                    _mm256_set1_epi32(nc as i32),
                    _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7),
                );
                _mm256_maskstore_ps(dst, mask, y);
            }
        }
        if R > 2 {
            let scale = _mm256_set1_ps(a_scales[2]);
            let y = _mm256_mul_ps(_mm256_mul_ps(_mm256_cvtepi32_ps(c20), scale), s0);
            let dst = out.as_mut_ptr().add(2 * rso);
            if nc == 8 {
                _mm256_storeu_ps(dst, y);
            } else {
                let mask = _mm256_cmpgt_epi32(
                    _mm256_set1_epi32(nc as i32),
                    _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7),
                );
                _mm256_maskstore_ps(dst, mask, y);
            }
        }
        if R > 3 {
            let scale = _mm256_set1_ps(a_scales[3]);
            let y = _mm256_mul_ps(_mm256_mul_ps(_mm256_cvtepi32_ps(c30), scale), s0);
            let dst = out.as_mut_ptr().add(3 * rso);
            if nc == 8 {
                _mm256_storeu_ps(dst, y);
            } else {
                let mask = _mm256_cmpgt_epi32(
                    _mm256_set1_epi32(nc as i32),
                    _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7),
                );
                _mm256_maskstore_ps(dst, mask, y);
            }
        }
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512bw,avx512vnni")]
unsafe fn vnni<const R: usize>(
    a: &[i8],
    w: &[i8],
    sums: &[i32],
    scales: &[f32],
    a_scales: &[f32],
    k4: usize,
    out: &mut [f32],
    rso: usize,
    nc: usize,
) {
    use std::arch::x86_64::*;
    unsafe {
        let mut c00 = _mm512_setzero_si512();
        let mut c01 = _mm512_setzero_si512();
        let mut c10 = _mm512_setzero_si512();
        let mut c11 = _mm512_setzero_si512();
        let mut c20 = _mm512_setzero_si512();
        let mut c21 = _mm512_setzero_si512();
        let mut c30 = _mm512_setzero_si512();
        let mut c31 = _mm512_setzero_si512();
        let mut c40 = _mm512_setzero_si512();
        let mut c41 = _mm512_setzero_si512();
        let mut c50 = _mm512_setzero_si512();
        let mut c51 = _mm512_setzero_si512();
        for p in (0..k4).step_by(4) {
            let w0 = _mm512_loadu_si512(w.as_ptr().add(p * 16).cast());
            let w1 = _mm512_loadu_si512(w.as_ptr().add(k4 * 16 + p * 16).cast());
            if R > 0 {
                let bits = a.as_ptr().add(0 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm512_set1_epi32(bits ^ (0x80808080u32 as i32));
                c00 = _mm512_dpbusd_epi32(c00, v, w0);
                c01 = _mm512_dpbusd_epi32(c01, v, w1);
            }
            if R > 1 {
                let bits = a.as_ptr().add(1 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm512_set1_epi32(bits ^ (0x80808080u32 as i32));
                c10 = _mm512_dpbusd_epi32(c10, v, w0);
                c11 = _mm512_dpbusd_epi32(c11, v, w1);
            }
            if R > 2 {
                let bits = a.as_ptr().add(2 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm512_set1_epi32(bits ^ (0x80808080u32 as i32));
                c20 = _mm512_dpbusd_epi32(c20, v, w0);
                c21 = _mm512_dpbusd_epi32(c21, v, w1);
            }
            if R > 3 {
                let bits = a.as_ptr().add(3 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm512_set1_epi32(bits ^ (0x80808080u32 as i32));
                c30 = _mm512_dpbusd_epi32(c30, v, w0);
                c31 = _mm512_dpbusd_epi32(c31, v, w1);
            }
            if R > 4 {
                let bits = a.as_ptr().add(4 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm512_set1_epi32(bits ^ (0x80808080u32 as i32));
                c40 = _mm512_dpbusd_epi32(c40, v, w0);
                c41 = _mm512_dpbusd_epi32(c41, v, w1);
            }
            if R > 5 {
                let bits = a.as_ptr().add(5 * k4 + p).cast::<i32>().read_unaligned();
                let v = _mm512_set1_epi32(bits ^ (0x80808080u32 as i32));
                c50 = _mm512_dpbusd_epi32(c50, v, w0);
                c51 = _mm512_dpbusd_epi32(c51, v, w1);
            }
        }
        let s0 = _mm512_loadu_ps(scales.as_ptr().add(0));
        let corr0 = _mm512_slli_epi32(_mm512_loadu_si512(sums.as_ptr().add(0).cast()), 7);
        let s1 = _mm512_loadu_ps(scales.as_ptr().add(16));
        let corr1 = _mm512_slli_epi32(_mm512_loadu_si512(sums.as_ptr().add(16).cast()), 7);
        if R > 0 {
            let scale = _mm512_set1_ps(a_scales[0]);
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c00, corr0)), scale),
                s0,
            );
            let valid = nc.saturating_sub(0).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(0 * rso + 0),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c01, corr1)), scale),
                s1,
            );
            let valid = nc.saturating_sub(16).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(0 * rso + 16),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
        }
        if R > 1 {
            let scale = _mm512_set1_ps(a_scales[1]);
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c10, corr0)), scale),
                s0,
            );
            let valid = nc.saturating_sub(0).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(1 * rso + 0),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c11, corr1)), scale),
                s1,
            );
            let valid = nc.saturating_sub(16).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(1 * rso + 16),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
        }
        if R > 2 {
            let scale = _mm512_set1_ps(a_scales[2]);
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c20, corr0)), scale),
                s0,
            );
            let valid = nc.saturating_sub(0).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(2 * rso + 0),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c21, corr1)), scale),
                s1,
            );
            let valid = nc.saturating_sub(16).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(2 * rso + 16),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
        }
        if R > 3 {
            let scale = _mm512_set1_ps(a_scales[3]);
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c30, corr0)), scale),
                s0,
            );
            let valid = nc.saturating_sub(0).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(3 * rso + 0),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c31, corr1)), scale),
                s1,
            );
            let valid = nc.saturating_sub(16).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(3 * rso + 16),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
        }
        if R > 4 {
            let scale = _mm512_set1_ps(a_scales[4]);
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c40, corr0)), scale),
                s0,
            );
            let valid = nc.saturating_sub(0).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(4 * rso + 0),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c41, corr1)), scale),
                s1,
            );
            let valid = nc.saturating_sub(16).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(4 * rso + 16),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
        }
        if R > 5 {
            let scale = _mm512_set1_ps(a_scales[5]);
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c50, corr0)), scale),
                s0,
            );
            let valid = nc.saturating_sub(0).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(5 * rso + 0),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
            let y = _mm512_mul_ps(
                _mm512_mul_ps(_mm512_cvtepi32_ps(_mm512_sub_epi32(c51, corr1)), scale),
                s1,
            );
            let valid = nc.saturating_sub(16).min(16);
            if valid > 0 {
                _mm512_mask_storeu_ps(
                    out.as_mut_ptr().add(5 * rso + 16),
                    ((1u32 << valid) - 1) as u16,
                    y,
                );
            }
        }
    }
}
#[cfg(test)]
pub(crate) static BACKEND_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct BackendReset;
#[cfg(test)]
impl Drop for BackendReset {
    fn drop(&mut self) {
        force_backend(Backend::Auto).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Independent scalar reference, deliberately not using quantize().
    fn reference(x: &[f32], activation: bool) -> (Vec<i8>, f32) {
        let mut max = 0.0f32;
        for i in 0..x.len() {
            if !x[i].is_finite() {
                return (vec![0; x.len()], f32::NAN);
            }
            if x[i].abs() > max {
                max = x[i].abs();
            }
        }
        if activation && max > 1e30 {
            return (vec![0; x.len()], f32::NAN);
        }
        if max < 1e-30 {
            return (vec![0; x.len()], 0.0);
        }
        let scale = max / 127.0;
        let mut q = Vec::new();
        for &v in x {
            q.push((v / scale).round().clamp(-127.0, 127.0) as i8);
        }
        (q, scale)
    }
    fn check(
        m: usize,
        k: usize,
        n: usize,
        x: &[f32],
        rsx: usize,
        weights: &[f32],
        scratch: &mut Scratch,
    ) {
        let w = QMat::new(k, n, weights).unwrap();
        for j in 0..n.next_multiple_of(32) {
            for p in 0..w.k4 {
                if j >= n || p >= k {
                    assert_eq!(w.at(p, j), 0);
                }
            }
        }
        let rso = n + 5;
        let mut want = vec![987.0f32; m * rso];
        let mut xq = vec![0; m * k];
        let mut scales = vec![0.0f32; m];
        for i in 0..m {
            let (q, s) = reference(&x[i * rsx..i * rsx + k], true);
            xq[i * k..(i + 1) * k].copy_from_slice(&q);
            scales[i] = s;
        }
        for j in 0..n {
            let col: Vec<_> = (0..k).map(|p| weights[p * n + j]).collect();
            let (q, s) = reference(&col, false);
            assert_eq!((0..k).map(|p| w.at(p, j)).collect::<Vec<_>>(), q);
            assert_eq!(w.scales[j].to_bits(), s.to_bits());
            assert_eq!(w.sums[j], q.iter().map(|&v| v as i32).sum::<i32>());
            for i in 0..m {
                let mut acc = 0i64;
                for p in 0..k {
                    acc += xq[i * k + p] as i64 * q[p] as i64;
                }
                want[i * rso + j] = (acc as f32) * scales[i] * s;
            }
        }
        for b in [Backend::Scalar, Backend::Avx2, Backend::Vnni, Backend::Auto] {
            if !backend_available(b) {
                assert!(force_backend(b).is_err());
                continue;
            }
            force_backend(b).unwrap();
            let mut got = vec![987.0; m * rso];
            qmatmul(m, x, rsx, &w, &mut got, rso, scratch);
            if b == Backend::Scalar {
                // Also exercise the baseline portable loop on SIMD-capable hosts.
                for i in 0..m {
                    for j in (0..n).step_by(16) {
                        let acc = scalar_portable::<1>(
                            &scratch.q[i * w.k4..],
                            &w.q[j / 16 * w.k4 * 16..][..w.k4 * 16],
                            w.k4,
                        );
                        for c in 0..(n - j).min(16) {
                            let y = (acc[0][c] as f32) * scratch.scales[i] * w.scales[j + c];
                            let expected = want[i * rso + j + c];
                            if expected.is_nan() {
                                assert!(y.is_nan());
                            } else {
                                assert_eq!(y.to_bits(), expected.to_bits());
                            }
                        }
                    }
                }
            }
            for i in 0..m {
                assert_eq!(&scratch.q[i * w.k4..i * w.k4 + k], &xq[i * k..(i + 1) * k]);
                assert!(
                    scratch.q[i * w.k4 + k..(i + 1) * w.k4]
                        .iter()
                        .all(|&v| v == 0)
                );
            }
            for i in 0..m {
                if scales[i].is_nan() {
                    assert!(scratch.scales[i].is_nan());
                } else {
                    assert_eq!(scratch.scales[i].to_bits(), scales[i].to_bits());
                }
            }
            for (a, b) in got.iter().zip(&want) {
                if b.is_nan() {
                    assert!(a.is_nan());
                } else {
                    assert_eq!(a.to_bits(), b.to_bits());
                }
            }
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn neon_dot_equals_the_portable_kernel() {
        if !std::arch::is_aarch64_feature_detected!("dotprod") {
            return;
        }
        let mut seed = 0x2545f491u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 24) as u8 as i8
        };
        for k4 in [4, 8, 12, 64, 196, 16384] {
            let fills: [Box<dyn FnMut() -> i8>; 6] =
                [Box::new(|| 0), Box::new(|| 127), Box::new(|| -127), Box::new(|| -128), Box::new(&mut next), Box::new(|| 1)];
            for (n, mut fill) in fills.into_iter().enumerate() {
                let a: Vec<i8> = (0..2 * k4).map(|_| fill()).collect();
                // Opposite signs in the constant cases too: the largest sums of both signs.
                let w: Vec<i8> = (0..16 * k4).map(|i| if n == 3 && i % 8 < 4 { 127 } else { fill() }).collect();
                assert_eq!(unsafe { neon_dot::<1>(&a, &w, k4) }, scalar_portable::<1>(&a, &w, k4), "k4={k4} fill={n}");
                assert_eq!(unsafe { neon_dot::<2>(&a, &w, k4) }, scalar_portable::<2>(&a, &w, k4), "k4={k4} fill={n}");
                assert_eq!(unsafe { neon_dot::<1>(&a, &w, k4) }[1], [0; 16]);
            }
        }
    }
    #[test]
    #[should_panic]
    fn overflowing_output_stride_is_rejected() {
        let w = QMat::new(1, 1, &[1.0]).unwrap();
        let mut out = [0.0f32; 1];
        qmatmul(2, &[1.0, 1.0], 1, &w, &mut out, usize::MAX, &mut Scratch::default());
    }

    #[test]
    fn integer_reference_strides_tails_and_reused_scratch() {
        let _lock = BACKEND_TEST_LOCK.lock().unwrap();
        let _reset = BackendReset;
        let mut scratch = Scratch::default();
        for (m, k, n) in [
            (1, 1, 1),
            (7, 79, 11),
            (5, 128, 9),
            (3, 17, 5),
            (1, 16384, 3),
            (6, 63, 7),
            (11, 193, 49),
            (9, 255, 65),
            (5, 4, 33),
        ] {
            let rsx = k + 3;
            let x: Vec<_> = (0..m * rsx)
                .map(|i| ((i * 71 % 257) as f32 - 128.0) / 37.0)
                .collect();
            let w: Vec<_> = (0..k * n)
                .map(|i| ((i * 37 % 251) as f32 - 170.0) / 41.0)
                .collect();
            check(m, k, n, &x, rsx, &w, &mut scratch);
        }
    }
    #[test]
    fn packed_microkernel_edges() {
        let _lock = BACKEND_TEST_LOCK.lock().unwrap();
        let _reset = BackendReset;
        let mut scratch = Scratch::default();
        for (m, k, n) in [
            (0, 3, 17),
            (1, 3, 15),
            (2, 5, 17),
            (3, 191, 31),
            (4, 192, 32),
            (5, 193, 33),
            (6, 255, 63),
            (7, 257, 65),
            (11, 5, 49),
        ] {
            let rsx = k + 7;
            let x: Vec<_> = (0..m * rsx)
                .map(|i| ((i * 71 % 257) as f32 - 128.0) / 37.0)
                .collect();
            let w: Vec<_> = (0..k * n)
                .map(|i| ((i * 37 % 251) as f32 - 170.0) / 41.0)
                .collect();
            check(m, k, n, &x, rsx, &w, &mut scratch);
        }
    }
    #[test]
    fn numerical_boundaries_and_bad_rows() {
        let _lock = BACKEND_TEST_LOCK.lock().unwrap();
        let _reset = BackendReset;
        let mut scratch = Scratch::default();
        // scale=1: exact half integers exercise away-from-zero rounding.
        let row = [127.0, -127.0, 0.5, -0.5, 1.5, -1.5, 0.0];
        let mut q = [0; 7];
        assert_eq!(quantize(&row, &mut q, true).unwrap(), 1.0);
        assert_eq!(q, [127, -127, 1, -1, 2, -2, 0]);
        for max in [
            0.0,
            f32::from_bits(1e-30f32.to_bits() - 1),
            1e-30,
            f32::from_bits(1e-30f32.to_bits() + 1),
            1e30,
            f32::from_bits(1e30f32.to_bits() + 1),
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            let mut x = row.repeat(3);
            x[1] = max;
            for v in &mut x[7..14] {
                *v = 0.0;
            }
            // Separate tiny rows with max exactly at the threshold.
            let tiny = [max, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            x.extend(tiny);
            let w: Vec<_> = row
                .iter()
                .flat_map(|&v| [v, -v, 0.0, 1e-31, 1e-30])
                .collect();
            check(4, 7, 5, &x, 7, &w, &mut scratch);
        }
        // Maximum supported magnitude and k, all-positive and negative column sums.
        check(
            1,
            16384,
            2,
            &vec![1e30; 16384],
            16384,
            &[-127.0, 127.0].repeat(16384),
            &mut scratch,
        );
    }
    #[test]
    fn weight_validation_and_extreme_columns() {
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(QMat::new(2, 1, &[1.0, v]).is_err());
        }
        assert!(QMat::new(16385, 1, &vec![0.0; 16385]).is_err());
        let _lock = BACKEND_TEST_LOCK.lock().unwrap();
        let _reset = BackendReset;
        let lo = f32::from_bits(1e-30f32.to_bits() - 1);
        let hi = f32::from_bits(1e-30f32.to_bits() + 1);
        check(
            1,
            2,
            5,
            &[0.0, 127.0],
            2,
            &[
                lo,
                1e-30,
                hi,
                f32::MAX,
                0.0,
                -lo,
                -1e-30,
                -hi,
                -f32::MAX,
                0.0,
            ],
            &mut Scratch::default(),
        );
    }
    #[test]
    fn normal_data_frobenius_error() {
        let _lock = BACKEND_TEST_LOCK.lock().unwrap();
        let _reset = BackendReset;
        let mut seed = 719u64;
        let mut uniform = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 40) as f32 + 1.0) / ((1u32 << 24) as f32 + 1.0)
        };
        let mut normal =
            || (-2.0 * uniform().ln()).sqrt() * (std::f32::consts::TAU * uniform()).cos();
        let (m, k, n) = (9, 129, 37);
        let x: Vec<_> = (0..m * k).map(|_| normal()).collect();
        let w: Vec<_> = (0..k * n).map(|_| normal()).collect();
        let mut fp = vec![0.0; m * n];
        crate::kernel::matmul(m, k, n, &x, k, &w, n, &mut fp, n);
        let q = QMat::new(k, n, &w).unwrap();
        for b in [Backend::Scalar, Backend::Avx2, Backend::Vnni] {
            if !backend_available(b) {
                continue;
            }
            force_backend(b).unwrap();
            let mut got = vec![0.0; m * n];
            qmatmul(m, &x, k, &q, &mut got, n, &mut Scratch::default());
            let err = got
                .iter()
                .zip(&fp)
                .map(|(a, b)| (*a as f64 - *b as f64).powi(2))
                .sum::<f64>();
            let norm = fp.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
            assert!((err / norm).sqrt() < 0.02);
        }
    }
}
