//! CAND P0 step 1 (a) latency screen: single-thread G1 forward of a full-length
//! input (104 tokens, the unchanged `hidden` path) vs the CAND path with M extra candidate tokens
//! (`hidden_ext`, random additive vectors), interleaved, 2,000 runs each after 100 warm-up runs.
//! Usage: cargo run --release --example cand_lat -- <model.twg1> <M>[,<M>...]
use std::time::Instant;

use twime_model::n1::N1;

fn pct(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 * q).ceil() as usize).saturating_sub(1).min(v.len() - 1)]
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = N1::load(std::path::Path::new(&a[1])).expect("model");
    let d = m.hidden(&[1]).unwrap().len();
    let vocab = m.vocab_len() as u32;
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let base: Vec<u32> = (0..104).map(|_| (rnd() % (vocab as u64 - 1)) as u32 + 1).collect();
    for ms in a[2].split(',') {
        let mm: usize = ms.parse().unwrap();
        let l = 104 + mm;
        let mut toks = base.clone();
        let mut pos: Vec<u32> = (0..104).collect();
        for k in 0..mm {
            toks.push((rnd() % (vocab as u64 - 1)) as u32 + 1);
            pos.push(65 + (k as u32 % 39));
        }
        let extra: Vec<f32> = (0..l * d).map(|i| if i < 104 * d { 0.0 } else { ((rnd() % 1000) as f32 / 1000.0 - 0.5) * 0.04 }).collect();
        let (mut t0, mut t1) = (Vec::new(), Vec::new());
        for it in 0..2100 {
            let s = Instant::now();
            std::hint::black_box(m.hidden(&base).unwrap());
            let e0 = s.elapsed().as_secs_f64() * 1e3;
            let s = Instant::now();
            std::hint::black_box(m.hidden_ext(&toks, &pos, &extra, l).unwrap());
            let e1 = s.elapsed().as_secs_f64() * 1e3;
            if it >= 100 {
                t0.push(e0);
                t1.push(e1);
            }
        }
        let (p50a, p50b) = (pct(&mut t0, 0.5), pct(&mut t1, 0.5));
        let (p99a, p99b) = (pct(&mut t0, 0.99), pct(&mut t1, 0.99));
        println!("M={mm}\tlen104 p50={p50a:.3} p99={p99a:.3}\tlen{l} p50={p50b:.3} p99={p99b:.3}\tdelta_p99={:.3} ms", p99b - p99a);
    }
}
