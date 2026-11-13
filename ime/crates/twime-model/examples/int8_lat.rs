//! int8_lat <model.twg1|random:vocab,d,layers,width> [len=104] [runs=2000] [backend=auto|scalar|avx2|vnni|all]
//! Numeric TSV columns: backend (0 auto, 1 scalar, 2 avx2, 3 vnni), length, runs,
//! fp32 p50/p99 ms, int8 p50/p99 ms, max absolute difference, min position cosine.
use std::{path::Path, time::Instant};
use twime_model::{
    n1::N1,
    qkernel::{Backend, backend_available, force_backend},
};

fn pct(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * q).ceil() as usize)
        .saturating_sub(1)
        .min(v.len() - 1)]
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 || a.len() > 5 {
        return Err("usage: int8_lat <model.twg1|random:vocab,d,layers,width> [len=104] [runs=2000] [backend=auto|scalar|avx2|vnni|all]".into());
    }
    let l: usize = a
        .get(2)
        .map_or("104", String::as_str)
        .trim_start_matches("len=")
        .parse()?;
    let runs: usize = a
        .get(3)
        .map_or("2000", String::as_str)
        .trim_start_matches("runs=")
        .parse()?;
    if l == 0 || l > 104 || runs == 0 {
        return Err("len must be 1..=104 and runs must be positive".into());
    }
    let bs = match a
        .get(4)
        .map_or("auto", String::as_str)
        .trim_start_matches("backend=")
    {
        "auto" => vec![Backend::Auto],
        "scalar" => vec![Backend::Scalar],
        "avx2" => vec![Backend::Avx2],
        "vnni" => vec![Backend::Vnni],
        "all" => [Backend::Scalar, Backend::Avx2, Backend::Vnni]
            .into_iter()
            .filter(|&b| backend_available(b))
            .collect(),
        _ => return Err("unknown backend".into()),
    };
    let load = || -> Result<N1, Box<dyn std::error::Error>> {
        if let Some(shape) = a[1].strip_prefix("random:") {
            let v: Vec<usize> = shape.split(',').map(str::parse).collect::<Result<_, _>>()?;
            if v.len() != 4 || v[0] == 0 || v[1] == 0 || v[1] % 32 != 0 || v[3] == 0 {
                return Err("random model requires vocab,d,layers,width; d must be positive and divisible by 32".into());
            }
            Ok(N1::random_g1(v[0], v[1], v[2], v[3], 719))
        } else {
            Ok(N1::load(Path::new(&a[1]))?)
        }
    };
    let fp = load()?;
    let mut q = load()?;
    q.quantize_int8()?;
    let mut seed = 0x9e3779b97f4a7c15u64;
    let tokens: Vec<_> = (0..l)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % fp.vocab_len() as u64) as u32
        })
        .collect();
    for b in bs {
        force_backend(b)?;
        let (mut ft, mut qt) = (Vec::with_capacity(runs), Vec::with_capacity(runs));
        let (mut last_fp, mut last_q) = (Vec::new(), Vec::new());
        for i in 0..runs.checked_add(100).ok_or("runs overflow")? {
            let (f, h, ef, eq);
            // Alternate order as well as interleaving the two models.
            if i % 2 == 0 {
                let t = Instant::now();
                f = std::hint::black_box(fp.hidden(&tokens).unwrap());
                ef = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                h = std::hint::black_box(q.hidden(&tokens).unwrap());
                eq = t.elapsed().as_secs_f64() * 1e3;
            } else {
                let t = Instant::now();
                h = std::hint::black_box(q.hidden(&tokens).unwrap());
                eq = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                f = std::hint::black_box(fp.hidden(&tokens).unwrap());
                ef = t.elapsed().as_secs_f64() * 1e3;
            }
            if i >= 100 {
                ft.push(ef);
                qt.push(eq);
            }
            if i == runs + 99 {
                last_fp = f;
                last_q = h;
            }
        }
        let max_abs = last_fp
            .iter()
            .zip(&last_q)
            .map(|(&a, &b)| (a as f64 - b as f64).abs())
            .fold(0.0f64, f64::max);
        let mut min_cos = 1.0f64;
        for (f, h) in last_fp.chunks(fp.dim()).zip(last_q.chunks(fp.dim())) {
            let dot: f64 = f.iter().zip(h).map(|(&a, &b)| a as f64 * b as f64).sum();
            let norm = |v: &[f32]| v.iter().map(|&x| (x as f64).powi(2)).sum::<f64>();
            let (nf, nh) = (norm(f), norm(h));
            let cos = if nf == 0.0 && nh == 0.0 {
                1.0
            } else if nf == 0.0 || nh == 0.0 {
                0.0
            } else {
                dot / (nf * nh).sqrt()
            };
            if !cos.is_finite() {
                return Err("non-finite cosine".into());
            }
            min_cos = min_cos.min(cos);
        }
        println!(
            "{}\t{}\t{}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.9}\t{:.9}",
            b as u8,
            l,
            runs,
            pct(&mut ft, 0.5),
            pct(&mut ft, 0.99),
            pct(&mut qt, 0.5),
            pct(&mut qt, 0.99),
            max_abs,
            min_cos
        );
    }
    force_backend(Backend::Auto)?;
    Ok(())
}
