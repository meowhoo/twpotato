//! WORDBONUS (WORDBONUS-plan): a small per-word bonus table. Each row is (reading syllables, word, bonus); a row
//! whose reading occurs in the input becomes one extra whole-word jump of the PN beam with that row's own bonus. The
//! table is independent of the PN lexicon (no change of `pn.fst/lst`), is off unless `model.cfg` names it, and an empty
//! table is the same as no table (bit-identical output). Any number of rows, each with its own bonus.
//!
//! `wordbonus.tsv`: `reading<TAB>word<TAB>bonus` per line (`#` comments, blank lines skipped), reading = syllables joined by
//! `-` with tones. `wordbonus.bin` (little endian): magic `TWWB1\0\0\0`, u32 version (1), u64 fingerprint of the vocabulary
//! and legal table (as `pn.lst`), u32 row count, then rows sorted strictly by (reading codes, ids): u8 length, f64 bonus,
//! length x u16 syllable code, length x u16 character id. No trailing bytes.

use std::io;

use crate::{convert::Assets, pn::MAX_LEN};

const MAGIC: &[u8; 8] = b"TWWB1\0\0\0";
const HEADER: usize = 8 + 4 + 8 + 4;

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    key: Vec<u16>,
    ids: Vec<u16>,
    bonus: f64,
}

/// The loaded table: rows sorted by (key, ids), looked up by binary search.
#[derive(Debug, Clone, PartialEq)]
pub struct WordBonus {
    rows: Vec<Row>,
    max_len: usize,
}

/// One input row of the builder.
#[derive(Debug, Clone, PartialEq)]
pub struct WbRow {
    pub reading: String,
    pub word: String,
    pub bonus: f64,
}

/// Parses `wordbonus.tsv` text.
pub fn parse_tsv(text: &str) -> io::Result<Vec<WbRow>> {
    let mut out = vec![];
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        let body = line.split('#').next().unwrap_or("").trim();
        if body.is_empty() {
            continue;
        }
        let f: Vec<&str> = body.split('\t').map(str::trim).collect();
        if f.len() != 3 || f.iter().any(|x| x.is_empty()) {
            return Err(bad(format!("wordbonus.tsv line {}: need reading<TAB>word<TAB>bonus", n + 1)));
        }
        let bonus: f64 = f[2].parse().map_err(|_| bad(format!("wordbonus.tsv line {}: bad bonus", n + 1)))?;
        out.push(WbRow { reading: f[0].into(), word: f[1].into(), bonus });
    }
    Ok(out)
}

/// Builds `wordbonus.bin` bytes for `assets`. Errors: a length outside 2..=8 or a reading/word length mismatch, an unknown
/// syllable, a character that is not legal for its syllable, a duplicate row, a bonus that is not finite.
pub fn build(assets: &Assets, rows: &[WbRow]) -> io::Result<Vec<u8>> {
    assert!(assets.vocab.len() < 1 << 16, "vocabulary ids must fit in u16");
    let mut v: Vec<Row> = vec![];
    for r in rows {
        let chars: Vec<char> = r.word.chars().collect();
        let syl: Vec<&str> = r.reading.split('-').collect();
        if chars.len() != syl.len() || !(2..=MAX_LEN).contains(&chars.len()) {
            return Err(bad(format!("wordbonus: bad length of {}", r.word)));
        }
        if !r.bonus.is_finite() {
            return Err(bad(format!("wordbonus: bonus of {} is not finite", r.word)));
        }
        let mut key = vec![];
        for (&c, s) in chars.iter().zip(&syl) {
            let code = assets.vocab.syllable_id(s);
            if code == crate::vocab::UNK || !assets.is_legal_char(s, c) {
                return Err(bad(format!("wordbonus: {} is not legal for reading {}", r.word, r.reading)));
            }
            key.push(code as u16);
        }
        let ids: Vec<u16> = chars.iter().map(|&c| assets.vocab.char_id(c) as u16).collect();
        v.push(Row { key, ids, bonus: r.bonus });
    }
    v.sort_by(|a, b| (&a.key, &a.ids).cmp(&(&b.key, &b.ids)));
    if v.windows(2).any(|w| w[0].key == w[1].key && w[0].ids == w[1].ids) {
        return Err(bad("wordbonus: duplicate row"));
    }
    let mut out = Vec::with_capacity(HEADER + 40 * v.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&crate::pn::fingerprint(assets).to_le_bytes());
    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
    for r in &v {
        out.push(r.key.len() as u8);
        out.extend_from_slice(&r.bonus.to_le_bytes());
        for x in r.key.iter().chain(&r.ids) {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    Ok(out)
}

impl WordBonus {
    /// Parses and validates `wordbonus.bin` bytes against `assets` (version, fingerprint, truncation, trailing bytes, order
    /// and duplicates, finite bonuses, row lengths).
    pub fn from_bytes(assets: &Assets, b: &[u8]) -> io::Result<WordBonus> {
        if b.len() < HEADER || &b[..8] != MAGIC {
            return Err(bad("wordbonus.bin: bad header"));
        }
        if b[8..12] != 1u32.to_le_bytes() {
            return Err(bad("wordbonus.bin: unsupported version"));
        }
        if u64::from_le_bytes(b[12..20].try_into().unwrap()) != crate::pn::fingerprint(assets) {
            return Err(bad("wordbonus.bin was built for another vocabulary / legal table"));
        }
        let n = u32::from_le_bytes(b[20..24].try_into().unwrap()) as usize;
        let mut p = HEADER;
        let mut rows: Vec<Row> = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            let len = *b.get(p).ok_or_else(|| bad("wordbonus.bin: truncated"))? as usize;
            if !(2..=MAX_LEN).contains(&len) {
                return Err(bad("wordbonus.bin: bad row length"));
            }
            let end = p + 1 + 8 + 4 * len;
            let body = b.get(p + 1..end).ok_or_else(|| bad("wordbonus.bin: truncated"))?;
            let bonus = f64::from_le_bytes(body[..8].try_into().unwrap());
            if !bonus.is_finite() {
                return Err(bad("wordbonus.bin: bonus is not finite"));
            }
            let u: Vec<u16> = body[8..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let row = Row { key: u[..len].to_vec(), ids: u[len..].to_vec(), bonus };
            // every syllable code and character id must exist and the character must be legal for its syllable
            for (&code, &id) in row.key.iter().zip(&row.ids) {
                let legal = assets.vocab.syllable_of(u32::from(code)).and_then(|s| assets.legal.get(s)).is_some_and(|v| v.binary_search(&u32::from(id)).is_ok());
                if !legal {
                    return Err(bad("wordbonus.bin: a row has an unknown syllable or an illegal character"));
                }
            }
            if let Some(prev) = rows.last() {
                match (&prev.key, &prev.ids).cmp(&(&row.key, &row.ids)) {
                    std::cmp::Ordering::Less => {}
                    std::cmp::Ordering::Equal => return Err(bad("wordbonus.bin: duplicate row")),
                    std::cmp::Ordering::Greater => return Err(bad("wordbonus.bin: rows out of order")),
                }
            }
            rows.push(row);
            p = end;
        }
        if p != b.len() {
            return Err(bad("wordbonus.bin: trailing bytes"));
        }
        let max_len = rows.iter().map(|r| r.key.len()).max().unwrap_or(0);
        Ok(WordBonus { rows, max_len })
    }

    pub fn open(assets: &Assets, path: &std::path::Path) -> io::Result<WordBonus> {
        Self::from_bytes(assets, &std::fs::read(path)?)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Every row whose reading equals `codes[s..s + l]`: (start, ids, bonus). One binary search per (start, length present
    /// in the table); the caller sorts and merges.
    pub fn hits(&self, codes: &[Option<u16>]) -> Vec<(usize, Vec<u32>, f64)> {
        let mut out = vec![];
        if self.rows.is_empty() {
            return out;
        }
        let mut key: Vec<u16> = Vec::with_capacity(MAX_LEN);
        for s in 0..codes.len() {
            key.clear();
            for l in 1..=self.max_len.min(codes.len() - s) {
                let Some(c) = codes[s + l - 1] else { break };
                key.push(c);
                if l < 2 {
                    continue;
                }
                let lo = self.rows.partition_point(|r| r.key.as_slice() < key.as_slice());
                for r in self.rows[lo..].iter().take_while(|r| r.key == key) {
                    out.push((s, r.ids.iter().map(|&x| u32::from(x)).collect(), r.bonus));
                }
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "wordbonus_tests.rs"]
mod tests;
