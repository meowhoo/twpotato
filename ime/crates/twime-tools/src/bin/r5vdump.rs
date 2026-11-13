//! Read-only fp32 N1/TWG2 candidate-score dump. Never prints unit text.
use std::{collections::HashMap, fs::File, io::{BufRead, BufReader, BufWriter, Write}};
use twime_model::n1::N1;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 6 { return Err("usage: r5vdump model.twg2 vocab.tsv legal.tsv units.tsv out.jsonl".into()); }
    let model = N1::load(std::path::Path::new(&a[1]))?;
    let mut vocab = HashMap::new();
    for line in BufReader::new(File::open(&a[2])?).lines() {
        let line = line?;
        let (id, token) = line.split_once('\t').ok_or("vocab format")?;
        vocab.insert(token.to_owned(), id.parse::<u32>()?);
    }
    let mut legal = HashMap::new();
    for line in BufReader::new(File::open(&a[3])?).lines() {
        let line = line?;
        let (s, ids) = line.split_once('\t').ok_or("legal format")?;
        let ids: Vec<u32> = ids.split(',').filter(|s| !s.is_empty()).map(str::parse).collect::<Result<_,_>>()?;
        legal.insert(s.to_owned(), ids);
    }
    let mut out = BufWriter::new(File::create(&a[5])?);
    let mut n = 0;
    for line in BufReader::new(File::open(&a[4])?).lines() {
        let line = line?;
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 5 { return Err("units format".into()); }
        let ctx: Vec<char> = f[2].chars().collect();
        let mut tokens: Vec<u32> = ctx[ctx.len().saturating_sub(64)..].iter().map(|c| *vocab.get(&c.to_string()).unwrap_or(&0)).collect();
        tokens.push(1);
        let first = tokens.len();
        let syllables: Vec<&str> = f[3].split_whitespace().collect();
        tokens.extend(syllables.iter().map(|s| *vocab.get(*s).unwrap_or(&0)));
        let sets: Vec<&[u32]> = syllables.iter().map(|s| legal.get(*s).map(Vec::as_slice).unwrap_or(&[])).collect();
        let scores = model.scores(&tokens, first, &sets).ok_or("model scores unavailable")?;
        writeln!(out, "{}", serde_json::json!({"id":f[0], "legal":sets, "scores":scores}))?;
        n += 1;
    }
    out.flush()?;
    println!("units={n}");
    Ok(())
}
