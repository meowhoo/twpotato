//! M1 (a) model-extraction regression: `twime_model::Assets::convert` against the whole-sentence
//! outputs of the existing `s5-eval` (lambda 0 and lambda 1) on the dev units, using the same inputs
//! (original-text context tail, symbols). Prints counts only, never sentence text.
//!
//! Usage: regress <data.txt> <aliases> <units.jsonl> <docs.jsonl> <model.bin> <prior.pack> <s5-eval-l0.jsonl> <s5-eval-l1.jsonl>

use std::{collections::HashMap, fs, io::{BufRead, BufReader}};

use twime_model::{Assets, ModelPaths};
use twime_tools::cases::load_cases;

fn baseline(path: &str) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let mut m = HashMap::new();
    for line in BufReader::new(fs::File::open(path)?).lines() {
        let v: serde_json::Value = serde_json::from_str(&line?)?;
        m.insert(v["id"].as_str().ok_or("id")?.to_string(), v["model"].as_str().ok_or("model")?.to_string());
    }
    Ok(m)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 9 {
        eprintln!("usage: regress <data.txt> <aliases> <units> <docs> <model> <prior> <l0.jsonl> <l1.jsonl>");
        std::process::exit(2);
    }
    let assets = Assets::load(&ModelPaths { data_txt: a[1].clone().into(), aliases: a[2].clone().into(), model: a[5].clone().into(), prior: Some(a[6].clone().into()) })?;
    let cases = load_cases(&a[3], &a[4])?;
    let (b0, b1) = (baseline(&a[7])?, baseline(&a[8])?);
    for (lambda, base) in [(0.0, &b0), (1.0, &b1)] {
        let (mut diff, mut correct) = (0usize, 0usize);
        for c in &cases {
            let conv = assets.convert(&c.context, &c.symbols, &[], lambda);
            // s5-eval counts a unit with a syllable without legal character as an empty output.
            let out = conv.text().unwrap_or_default();
            diff += usize::from(Some(&out) != base.get(&c.id));
            correct += usize::from(out == c.text);
        }
        println!("lambda={lambda}\tunits={}\tdiff_vs_s5_eval={diff}\tcorrect_vs_reference_text={correct}", cases.len());
    }
    Ok(())
}
