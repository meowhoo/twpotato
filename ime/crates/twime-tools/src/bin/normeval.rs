//! NORM dev run through `Assets::convert_ex`, the product path. Prints counts only; writes
//! `{id, text, out}` per unit (the local file is compared with the Python reference `train/ent_norm_eval.py`).
//!
//! Usage: normeval <data.txt> <aliases> <units> <docs> <model> <prior|-> <lambda> <flags> <out.jsonl>
//! flags: comma list of `ctx` (A3), `out` (A1 output fold), `pair` (candidate pairs), `style` (style from the original
//! context: the last of 台/臺), or `none`.

use std::{fs, io::Write, time::Instant};

use twime_model::{Assets, CandParams, ModelPaths, NormCfg, norm::style_of};
use twime_tools::cases::load_cases;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 10 {
        eprintln!("usage: normeval <data.txt> <aliases> <units> <docs> <model> <prior|-> <lambda> <flags> <out.jsonl>");
        std::process::exit(2);
    }
    let lambda: f64 = a[7].parse()?;
    let prior = (a[6] != "-" && lambda != 0.0).then(|| a[6].clone().into());
    let mut assets = Assets::load(&ModelPaths { data_txt: a[1].clone().into(), aliases: a[2].clone().into(), model: a[5].clone().into(), prior })?;
    let has = |f: &str| a[8].split(',').any(|x| x == f);
    assets.set_norm(NormCfg { out: has("out"), ctx: has("ctx"), pair: has("pair") });
    let style = has("style");
    let cases = load_cases(&a[3], &a[4])?;
    let mut w = std::io::BufWriter::new(fs::File::create(&a[9])?);
    let (mut correct, mut trad, mut ms, mut cms) = (0usize, 0usize, vec![], vec![]);
    let time_cands = has("cands");
    for c in &cases {
        let st = style && style_of(&c.context) == Some(true);
        let t = Instant::now();
        let conv = assets.convert_ex(&c.context, &c.symbols, &[], lambda, st);
        ms.push(t.elapsed().as_secs_f64() * 1e3);
        let out = conv.text().unwrap_or_default();
        if time_cands && !c.symbols.is_empty() {
            let t = Instant::now();
            let n = assets.candidates(&conv, c.symbols.len(), &CandParams { lambda, style_tai: st, ..CandParams::default() }).len();
            cms.push(t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(n);
        }
        correct += usize::from(out == c.text);
        trad += out.chars().filter(|&x| x == '臺').count();
        serde_json::to_writer(&mut w, &serde_json::json!({"id": c.id, "text": c.text, "out": out}))?;
        w.write_all(b"\n")?;
    }
    w.flush()?;
    ms.sort_by(f64::total_cmp);
    let q = |p: f64| ms[((ms.len() as f64 - 1.0) * p) as usize];
    if !cms.is_empty() {
        cms.sort_by(f64::total_cmp);
        let cq = |p: f64| cms[((cms.len() as f64 - 1.0) * p) as usize];
        println!("cand_ms_p50={:.3}	cand_ms_p99={:.3}", cq(0.5), cq(0.99));
    }
    println!("units={}\tcorrect_strict={correct}\toutput_trad={trad}\tms_p50={:.3}\tms_p99={:.3}", cases.len(), q(0.5), q(0.99));
    Ok(())
}
