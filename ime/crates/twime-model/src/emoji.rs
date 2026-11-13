//! EMOJI (EMOJI-plan 2.4): the reading -> emoji table `emoji.tsv`, read only when `emoji=on`.
//!
//! Format (UTF-8): the first line `#emoji v1 ...`; then `reading<TAB>emoji emoji ...<TAB>source` per line, the reading the
//! syllables joined with `-` (as `Dictionary`'s readings), `source` = `cldr` or `tw` (`tw` rows go before `cldr` ones of
//! the same key, whatever the row order). Empty lines and `#` lines are skipped. A key given twice is merged.
//!
//! The table is kept compact (E5 allows 2 MB for it): one string holding every key followed by its space-separated emoji, and a
//! sorted index into it, searched by binary search. Nothing is allocated per key or per emoji.

/// Most emoji of one key: one syllable, and more.
pub const CAP_SINGLE: usize = 3;
pub const CAP_MULTI: usize = 9;

/// A v1 entry: one non-ASCII code point, or one non-ASCII code point and U+FE0F (so an entry is at most 8 bytes and one
/// shown cell; sequences with skin tones, ZWJ, flags and keycaps are not part of v1).
fn single_emoji(e: &str) -> bool {
    let mut it = e.chars();
    let Some(first) = it.next() else { return false };
    !first.is_ascii() && first != '\u{FE0F}' && it.next().is_none_or(|c| c == '\u{FE0F}' && it.next().is_none())
}

/// Where one key and its emoji list lie in the pool: `pool[at..at + key_len]` is the key, the `list_len` bytes after it the
/// emoji separated by spaces.
#[derive(Debug, Clone, Copy)]
struct Slot {
    at: u32,
    key_len: u16,
    list_len: u16,
}

#[derive(Debug, Default)]
pub struct EmojiTable {
    pool: String,
    /// Sorted by key.
    index: Vec<Slot>,
}

impl EmojiTable {
    pub fn parse(text: &str) -> Result<EmojiTable, String> {
        let mut lines = text.lines().enumerate();
        match lines.next() {
            Some((_, h)) if h == "#emoji v1" || h.starts_with("#emoji v1 ") => {}
            _ => return Err("missing or unknown header (expected `#emoji v1 ...`)".into()),
        }
        // (key, side: 0 = tw, 1 = cldr, row number, emoji list); the strings borrow `text`
        let mut rows: Vec<(&str, u8, usize, &str)> = vec![];
        for (i, line) in lines {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let err = |m: &str| format!("line {}: {m}", i + 1);
            let mut f = line.split('\t');
            let (Some(key), Some(list)) = (f.next(), f.next()) else { return Err(err("expected 2 or 3 tab-separated fields")) };
            let side = match (f.next(), f.next()) {
                (None, _) | (Some("cldr"), None) => 1,
                (Some("tw"), None) => 0,
                (Some(_), None) => return Err(err("source must be cldr or tw")),
                (Some(_), Some(_)) => return Err(err("expected 2 or 3 tab-separated fields")),
            };
            if key.is_empty() || key.split('-').any(str::is_empty) || list.split(' ').all(str::is_empty) {
                return Err(err("empty reading or emoji list"));
            }
            if key.len() > u16::MAX as usize {
                return Err(err("reading too long"));
            }
            if let Some(bad) = list.split(' ').filter(|e| !e.is_empty()).find(|e| !single_emoji(e)) {
                return Err(err(&format!("`{bad}` is not one emoji (v1: one code point, or one code point and U+FE0F)")));
            }
            rows.push((key, side, i, list));
        }
        // a key's rows together: tw before cldr, then in file order
        rows.sort_unstable_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut pool = String::with_capacity(text.len());
        let mut index: Vec<Slot> = vec![];
        let mut k = 0;
        while k < rows.len() {
            let key = rows[k].0;
            let end = k + rows[k..].iter().take_while(|r| r.0 == key).count();
            let cap = if key.contains('-') { CAP_MULTI } else { CAP_SINGLE };
            let at = pool.len();
            pool.push_str(key);
            let list_at = pool.len();
            let mut n = 0;
            for e in rows[k..end].iter().flat_map(|r| r.3.split(' ')).filter(|e| !e.is_empty()) {
                if n == cap {
                    break;
                }
                if !pool[list_at..].split(' ').any(|x| x == e) {
                    if n > 0 {
                        pool.push(' ');
                    }
                    pool.push_str(e);
                    n += 1;
                }
            }
            let at32 = u32::try_from(at).map_err(|_| "table too large".to_string())?;
            index.push(Slot { at: at32, key_len: key.len() as u16, list_len: (pool.len() - list_at) as u16 });
            k = end;
        }
        pool.shrink_to_fit();
        index.shrink_to_fit();
        Ok(EmojiTable { pool, index })
    }

    fn key_of(&self, s: &Slot) -> &str {
        &self.pool[s.at as usize..s.at as usize + s.key_len as usize]
    }

    /// The emoji of a reading (syllables), best first; empty when the table has none.
    pub fn lookup(&self, syllables: &[String]) -> Vec<&str> {
        let key = syllables.join("-");
        match self.index.binary_search_by(|s| self.key_of(s).cmp(key.as_str())) {
            Ok(i) => {
                let s = &self.index[i];
                let from = s.at as usize + s.key_len as usize;
                self.pool[from..from + s.list_len as usize].split(' ').collect()
            }
            Err(_) => vec![],
        }
    }

    /// Number of readings with emoji.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_and_looks_up() {
        let t = EmojiTable::parse("#emoji v1 cldr=49\n\n# note\nㄞˋ-ㄒㄧㄣ\t❤️ 🤎\tcldr\nㄇㄠ\t🐈 🐱 😼 😸\n").unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t.lookup(&s(&["ㄞˋ", "ㄒㄧㄣ"])), ["❤️", "🤎"]);
        // one-syllable key: at most 3 (a bad file is cut), the source column is optional
        assert_eq!(t.lookup(&s(&["ㄇㄠ"])), ["🐈", "🐱", "😼"]);
        assert!(t.lookup(&s(&["ㄇㄠ", "ㄇㄠ"])).is_empty());
        assert!(t.lookup(&[]).is_empty());
        // a key that is only a prefix, or only an extension, of a listed one is not found
        assert!(t.lookup(&s(&["ㄞˋ"])).is_empty());
        assert!(t.lookup(&s(&["ㄇ"])).is_empty());
    }

    #[test]
    fn every_key_is_found_in_a_larger_table() {
        let rows: Vec<String> = (0..500).map(|i| format!("ㄅ{i}-ㄆ\t{}\tcldr\n", char::from_u32(0x1F300 + i).unwrap())).collect();
        let t = EmojiTable::parse(&format!("#emoji v1\n{}", rows.concat())).unwrap();
        assert_eq!(t.len(), 500);
        for i in 0..500u32 {
            assert_eq!(t.lookup(&s(&[&format!("ㄅ{i}"), "ㄆ"])), [char::from_u32(0x1F300 + i).unwrap().to_string()]);
        }
    }

    #[test]
    fn multi_syllable_cap_is_nine() {
        let many: Vec<String> = (0..12).map(|i| char::from_u32(0x1F600 + i).unwrap().to_string()).collect();
        let t = EmojiTable::parse(&format!("#emoji v1\nㄅ-ㄆ\t{}\tcldr\n", many.join(" "))).unwrap();
        assert_eq!(t.lookup(&s(&["ㄅ", "ㄆ"])).len(), 9);
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(EmojiTable::parse("").is_err());
        assert!(EmojiTable::parse("ㄇㄠ\t🐈\tcldr\n").is_err()); // no header
        assert!(EmojiTable::parse("#emoji v2\nㄇㄠ\t🐈\n").is_err());
        assert!(EmojiTable::parse("#emoji v1\nㄇㄠ\n").is_err());
        assert!(EmojiTable::parse("#emoji v1\nㄇㄠ\t🐈\tzz\n").is_err());
        assert!(EmojiTable::parse("#emoji v1\nㄇㄠ\t\tcldr\n").is_err());
        assert!(EmojiTable::parse("#emoji v1\nㄇㄠ-\t🐈\tcldr\n").is_err());
        assert!(EmojiTable::parse("#emoji v1\nㄇㄠ\tabc\tcldr\n").is_err());
        // v1 entries are one code point (+ U+FE0F): no sequences, no long strings, no lone selector
        for bad in ["👨\u{200d}👩", "👍\u{1f3fb}", "6\u{fe0f}\u{20e3}", "漢字", "\u{fe0f}", "❤\u{fe0f}\u{fe0f}", &"😀".repeat(16384)] {
            assert!(EmojiTable::parse(&format!("#emoji v1\nㄇㄠ\t{bad}\tcldr\n")).is_err(), "{bad:?}");
        }
        for ok in ["😀", "❤\u{fe0f}", "👁\u{fe0f}", "貓"] {
            assert!(EmojiTable::parse(&format!("#emoji v1\nㄇㄠ\t{ok}\tcldr\n")).is_ok(), "{ok:?}");
        }
        assert!(EmojiTable::parse("#emoji v1\na\tb\tc\td\n").is_err());
        assert!(EmojiTable::parse(&format!("#emoji v1\n{}\t🐈\tcldr\n", "ㄇ".repeat(30000))).is_err());
    }

    #[test]
    fn duplicate_keys_merge_tw_before_cldr_in_either_row_order() {
        for rows in ["ㄅ-ㄆ\t🐈 🐱\tcldr\nㄅ-ㄆ\t🎉\ttw\n", "ㄅ-ㄆ\t🎉\ttw\nㄅ-ㄆ\t🐈 🐱\tcldr\n"] {
            let t = EmojiTable::parse(&format!("#emoji v1\n{rows}")).unwrap();
            assert_eq!(t.lookup(&s(&["ㄅ", "ㄆ"])), ["🎉", "🐈", "🐱"]);
        }
        // same-source rows keep file order; strings are de-duplicated; the cap cuts after the merge
        let t = EmojiTable::parse("#emoji v1\nㄅ\t🐈\tcldr\nㄅ\t🐱 🐈\tcldr\nㄅ\t😼 😸\tcldr\nㄅ\t🎉\ttw\n").unwrap();
        assert_eq!(t.lookup(&s(&["ㄅ"])), ["🎉", "🐈", "🐱"]);
    }
}
