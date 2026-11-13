use twime_model::{
    Assets,
    user::{UserWord, UserWordError},
};
use twime_proto::{LexEntry, LexError};
pub const MAX_ENTRIES: usize = 50_000;
pub const MAX_PER_READING: usize = 32;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Comment(String),
    Entry(LexEntry),
    Invalid(String),
}
pub fn normalize_reading(raw: &str) -> Option<Vec<String>> {
    let v: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let s = s.replace('ˉ', "");
            if let Some(rest) = s.strip_prefix('˙') {
                format!("{rest}˙")
            } else {
                s
            }
        })
        .collect();
    if v.is_empty() || v.iter().any(String::is_empty) {
        None
    } else {
        Some(v)
    }
}
pub fn validate(assets: &Assets, entry: &LexEntry) -> Result<(Vec<u16>, UserWord), LexError> {
    let syllables = normalize_reading(&entry.reading).ok_or(LexError::BadReading)?;
    assets
        .user_word_opt(
            &entry.word,
            &syllables,
            // USERLEX 1.3: an omitted weight stays omitted; the model applies the deployment default.
            entry.weight.map(f64::from),
        )
        .map_err(|e| match e {
            UserWordError::Length => LexError::Length,
            UserWordError::UnknownChar(i) => LexError::UnknownChar(i),
            UserWordError::IllegalSyllable(i) => LexError::IllegalSyllable(i),
            UserWordError::IllegalChar(i) => LexError::IllegalChar(i),
            UserWordError::Weight => LexError::Weight,
        })
}
fn entry(raw: &str, assets: &Assets) -> Option<LexEntry> {
    let parts: Vec<&str> = raw.split('\t').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let reading = normalize_reading(parts[1])?.join(" ");
    let weight = if parts.len() == 3 {
        Some(parts[2].parse::<f32>().ok()?)
    } else {
        None
    };
    let entry = LexEntry {
        word: parts[0].into(),
        reading,
        weight,
    };
    validate(assets, &entry).ok()?;
    Some(entry)
}
pub fn parse(bytes: &[u8], assets: &Assets) -> Result<Vec<Line>, LexError> {
    if bytes.len() > MAX_BYTES {
        return Err(LexError::Full);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| LexError::Io)?
        .trim_start_matches('\u{feff}');
    let mut count = 0;
    let mut readings = std::collections::HashMap::<String, std::collections::HashSet<String>>::new();
    let mut out = vec![];
    for raw in text.lines() {
        if raw.starts_with('#') || raw.is_empty() {
            out.push(Line::Comment(raw.into()));
        } else if let Some(e) = entry(raw, assets) {
            let words = readings.entry(e.reading.clone()).or_default();
            if !words.contains(&e.word) && words.len() >= MAX_PER_READING {
                out.push(Line::Invalid(raw.into()));
                continue;
            }
            words.insert(e.word.clone());
            count += 1;
            if count > MAX_ENTRIES {
                return Err(LexError::Full);
            }
            out.push(Line::Entry(e));
        } else {
            out.push(Line::Invalid(raw.into()));
        }
    }
    Ok(out)
}
pub fn serialize(lines: &[Line]) -> Vec<u8> {
    let mut s = String::new();
    if lines.is_empty() {
        s.push_str("# 地瓜輸入法 使用者詞庫 v1\n");
    }
    for line in lines {
        match line {
            Line::Comment(raw) | Line::Invalid(raw) => s.push_str(raw),
            Line::Entry(e) => {
                s.push_str(&e.word);
                s.push('\t');
                s.push_str(&e.reading);
                if let Some(w) = e.weight {
                    s.push('\t');
                    s.push_str(&w.to_string());
                }
            }
        }
        s.push('\n');
    }
    s.into_bytes()
}
/// Upsert preserves the first entry's file position and removes duplicate valid keys.
pub fn upsert(lines: &mut Vec<Line>, entry: LexEntry) -> Result<bool, LexError> {
    let existing = lines.iter().position(
        |line| matches!(line, Line::Entry(e) if e.word == entry.word && e.reading == entry.reading),
    );
    if let Some(i) = existing {
        lines[i] = Line::Entry(entry.clone());
        let mut index = 0;
        lines.retain(|line| { let keep = index <= i || !matches!(line, Line::Entry(e) if e.word == entry.word && e.reading == entry.reading); index += 1; keep });
        Ok(false)
    } else {
        let words: std::collections::HashSet<_> = lines.iter().filter_map(|l| match l {
            Line::Entry(e) if e.reading == entry.reading => Some(&e.word),
            _ => None,
        }).collect();
        if words.len() >= MAX_PER_READING {
            return Err(LexError::Full);
        }
        if lines.iter().filter(|l| matches!(l, Line::Entry(_))).count() >= MAX_ENTRIES {
            return Err(LexError::Full);
        }
        lines.push(Line::Entry(entry));
        Ok(true)
    }
}
