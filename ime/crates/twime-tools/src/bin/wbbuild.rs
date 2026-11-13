//! WORDBONUS builder (WORDBONUS-plan 3): `wbbuild <data.txt> <common-readings.txt> <wordbonus.tsv> <out.bin>`.
//! Builds wordbonus.bin against the vocabulary / legal table of the given dictionary files (the fingerprint of pn.lst's
//! kind). Any illegal word, bad length, duplicate row or non-finite bonus fails the build (exit 1, no output file).
use twime_model::{Assets, ModelPaths, wordbonus};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 4 {
        eprintln!("usage: wbbuild <data.txt> <common-readings.txt> <wordbonus.tsv> <out.bin>");
        std::process::exit(2);
    }
    let paths = ModelPaths { data_txt: a[0].clone().into(), aliases: a[1].clone().into(), model: "".into(), prior: None };
    let assets = Assets::load_dictionary_only(&paths).unwrap_or_else(|e| fail(&format!("dictionary: {e}")));
    let text = std::fs::read_to_string(&a[2]).unwrap_or_else(|e| fail(&format!("{}: {e}", a[2])));
    let rows = wordbonus::parse_tsv(&text).unwrap_or_else(|e| fail(&e.to_string()));
    let bytes = wordbonus::build(&assets, &rows).unwrap_or_else(|e| fail(&e.to_string()));
    // self-check: the bytes must load back against the same assets
    wordbonus::WordBonus::from_bytes(&assets, &bytes).unwrap_or_else(|e| fail(&format!("self-check: {e}")));
    std::fs::write(&a[3], &bytes).unwrap_or_else(|e| fail(&format!("{}: {e}", a[3])));
    println!("wordbonus rows={} bytes={}", rows.len(), bytes.len());
}

fn fail(msg: &str) -> ! {
    eprintln!("wbbuild: {msg}");
    std::process::exit(1);
}
