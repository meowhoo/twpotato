//! Scored dev units with their original-text context (copied from bench/s3_latency/src/cases.rs, N3 M1, MIT).

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
};

use serde::Deserialize;

const CONTEXT_CHARS: usize = 64;

#[derive(Deserialize)]
struct Unit {
    id: String,
    doc_id: String,
    split: String,
    status: String,
    text: String,
    symbols: Vec<String>,
}

#[derive(Deserialize)]
struct Doc {
    doc_id: String,
    text: String,
}

/// A scored unit with its original-text context (last 64 characters before
/// the unit in its document).
pub struct Case {
    pub id: String,
    pub doc_id: String,
    pub text: String,
    pub context: String,
    pub symbols: Vec<String>,
}

pub fn load_cases(units: &str, docs: &str) -> Result<Vec<Case>, Box<dyn std::error::Error>> {
    let docs: HashMap<String, String> = BufReader::new(File::open(docs)?)
        .lines()
        .map(|l| {
            let d: Doc = serde_json::from_str(&l?)?;
            Ok((d.doc_id, d.text))
        })
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    let mut cursor: HashMap<String, usize> = HashMap::new();
    let mut cases = vec![];
    for line in BufReader::new(File::open(units)?).lines() {
        let u: Unit = serde_json::from_str(&line?)?;
        if u.split != "dev" {
            continue;
        }
        // Units appear in document order; find each one after the previous.
        let text = &docs[&u.doc_id];
        let from = *cursor.get(&u.doc_id).unwrap_or(&0);
        let start = from
            + text[from..]
                .find(&u.text)
                .ok_or_else(|| format!("{} not found in its document", u.id))?;
        cursor.insert(u.doc_id.clone(), start + u.text.len());
        if u.status != "ok" {
            continue;
        }
        let before: Vec<char> = text[..start].chars().collect();
        let context: String = before[before.len().saturating_sub(CONTEXT_CHARS)..]
            .iter()
            .collect();
        cases.push(Case {
            id: u.id,
            doc_id: u.doc_id,
            text: u.text,
            context,
            symbols: u.symbols,
        });
    }
    Ok(cases)
}
