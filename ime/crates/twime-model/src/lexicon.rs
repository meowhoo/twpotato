//! McBopomofo dictionary (data.txt) loading, alias table and legal-reading queries.
//!
//! Moved from engine/src/lib.rs `Dictionary` (N3 M1, MIT, same project): `parse`, `apply_aliases`, `legal_readings`,
//! `lookup`, `vocabulary` keep their behavior; the S2 k-best `candidates`, `annotate` and alias bookkeeping used only
//! by them are not carried over. `best_single_char` is new (fallback when the model is unavailable).

use std::collections::HashMap;

use unicode_script::{Script, UnicodeScript};

/// One dictionary word for a reading.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub word: String,
    pub log10: f64,
    /// 1-based line number in the source file; breaks ties between paths.
    pub line: u32,
}

#[derive(Debug, Default, PartialEq)]
pub struct LoadStats {
    pub kept: usize,
    pub special: usize,
    pub non_han: usize,
    pub length_mismatch: usize,
    pub duplicate: usize,
}

/// Counts from `Dictionary::apply_aliases`.
#[derive(Debug, Default, PartialEq)]
pub struct AliasStats {
    pub char_rules: usize,
    pub word_rules: usize,
    /// New (reading, word) entries.
    pub added: usize,
    /// Alias readings that already existed; the higher probability was kept.
    pub merged: usize,
}

/// Reading (syllables joined with `-`) to its words.
#[derive(Debug, Default)]
pub struct Dictionary {
    by_reading: HashMap<String, Vec<Entry>>,
    max_syllables: usize,
    /// FUZZY F2 step 2: median log10 per word length of the parsed data.txt, before any alias expansion.
    raw_median: Vec<f64>,
    /// The 90th percentile of the same values (FUZZY F2 step 2 tau_L).
    raw_q90: Vec<f64>,
}

/// Unicode `Script=Han`.
pub fn is_han(c: char) -> bool {
    c.script() == Script::Han
}

impl Dictionary {
    /// All dictionary readings for a word, including aliases, without a product cutoff.
    pub fn readings_for_word(&self, word: &str) -> Vec<String> {
        let mut found: Vec<_> = self.by_reading.iter().filter_map(|(reading, entries)| {
            entries.iter().find(|e| e.word == word).map(|e| (reading, e.log10))
        }).collect();
        found.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
        found.into_iter().map(|(r, _)| r.replace('-', " ")).collect()
    }
    /// Parses `reading word log10` lines; keeps only words made of Han
    /// characters whose count equals the number of syllables.
    pub fn parse(text: &str) -> (Dictionary, LoadStats) {
        let mut dict = Dictionary::default();
        let mut stats = LoadStats::default();
        for (i, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut parts = line.split(' ');
            let (Some(reading), Some(word), Some(score)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if reading.starts_with('_') {
                stats.special += 1;
                continue;
            }
            if !word.chars().all(is_han) {
                stats.non_han += 1;
                continue;
            }
            let syllables = reading.split('-').count();
            if syllables != word.chars().count() {
                stats.length_mismatch += 1;
                continue;
            }
            let Ok(log10) = score.parse::<f64>() else {
                continue;
            };
            let entries = dict.by_reading.entry(reading.to_string()).or_default();
            let entry = Entry {
                word: word.to_string(),
                log10,
                line: i as u32 + 1,
            };
            // Same reading and word twice: keep the higher probability.
            if let Some(existing) = entries.iter_mut().find(|e| e.word == entry.word) {
                stats.duplicate += 1;
                if entry.log10 > existing.log10 {
                    *existing = entry;
                }
                continue;
            }
            entries.push(entry);
            stats.kept += 1;
            dict.max_syllables = dict.max_syllables.max(syllables);
        }
        dict.raw_median = dict.median_now();
        dict.raw_q90 = dict.by_len_now(|v| v[((v.len() - 1) as f64 * 0.9).round() as usize]);
        (dict, stats)
    }

    /// Adds common (non-standard) readings from the alias
    /// table. Lines: `字 X std alt` applies to every
    /// entry where character X reads `std`; `詞 W std alt` applies to that
    /// entry only. Each original entry gets at most one char-rule alias, with
    /// all its char rules applied at once. Aliases copy the probability and
    /// line of their entry; an existing (reading, word) keeps the higher one.
    pub fn apply_aliases(&mut self, table: &str) -> Result<AliasStats, String> {
        let mut stats = AliasStats::default();
        let mut char_rules: HashMap<(char, String), String> = HashMap::new();
        let mut word_rules: Vec<(String, String, String)> = vec![];
        for (i, raw) in table.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split_whitespace().collect();
            let err = |m: &str| format!("alias line {}: {m}: {raw}", i + 1);
            let [kind, word, std, alt] = f[..] else {
                return Err(err("expected 4 fields"));
            };
            let n = word.chars().count();
            if std.split('-').count() != n || alt.split('-').count() != n || std == alt {
                return Err(err("syllable count mismatch or identical readings"));
            }
            if !self.lookup_reading(std).iter().any(|e| e.word == word) {
                return Err(err("standard reading not in dictionary"));
            }
            match kind {
                "字" if n == 1 => {
                    let c = word.chars().next().unwrap();
                    if char_rules.insert((c, std.into()), alt.into()).is_some() {
                        return Err(err("duplicate rule"));
                    }
                    stats.char_rules += 1;
                }
                "詞" => {
                    word_rules.push((word.into(), std.into(), alt.into()));
                    stats.word_rules += 1;
                }
                _ => return Err(err("kind must be 字 (one character) or 詞")),
            }
        }
        let mut new: Vec<(String, String, Entry)> = vec![];
        for (reading, entries) in &self.by_reading {
            for e in entries {
                let syl: Vec<&str> = reading.split('-').collect();
                let mut changed = false;
                let alias: Vec<&str> = e
                    .word
                    .chars()
                    .zip(&syl)
                    .map(|(c, s)| match char_rules.get(&(c, s.to_string())) {
                        Some(a) => {
                            changed = true;
                            a.as_str()
                        }
                        None => s,
                    })
                    .collect();
                if changed {
                    new.push((reading.clone(), alias.join("-"), e.clone()));
                }
            }
        }
        for (word, std, alt) in word_rules {
            let e = self.lookup_reading(&std).iter().find(|e| e.word == word).unwrap();
            new.push((std, alt, e.clone()));
        }
        // Deterministic insertion order (HashMap iteration is not).
        new.sort_by(|a, b| (a.2.line, &a.1).cmp(&(b.2.line, &b.1)));
        for (_std, alt, e) in new {
            let entries = self.by_reading.entry(alt).or_default();
            if let Some(existing) = entries.iter_mut().find(|x| x.word == e.word) {
                stats.merged += 1;
                if e.log10 > existing.log10 {
                    *existing = e;
                }
            } else {
                entries.push(e);
                stats.added += 1;
            }
        }
        Ok(stats)
    }

    fn lookup_reading(&self, reading: &str) -> &[Entry] {
        self.by_reading.get(reading).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Each character's syllables across the loaded entries (including
    /// aliases), for checking that a reading is legal.
    pub fn legal_readings(&self) -> HashMap<char, Vec<String>> {
        let mut m: HashMap<char, Vec<String>> = HashMap::new();
        for (reading, entries) in &self.by_reading {
            for e in entries {
                for (c, s) in e.word.chars().zip(reading.split('-')) {
                    let v = m.entry(c).or_default();
                    if !v.iter().any(|x| x == s) {
                        v.push(s.to_string());
                    }
                }
            }
        }
        m
    }

    pub fn lookup(&self, syllables: &[String]) -> &[Entry] {
        self.by_reading
            .get(&syllables.join("-"))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Han characters and syllables appearing in the loaded entries.
    pub fn vocabulary(&self) -> (Vec<char>, Vec<String>) {
        let mut chars: Vec<char> = self
            .by_reading
            .values()
            .flatten()
            .flat_map(|e| e.word.chars())
            .collect();
        chars.sort_unstable();
        chars.dedup();
        let mut syllables: Vec<String> = self
            .by_reading
            .keys()
            .flat_map(|r| r.split('-').map(str::to_string))
            .collect();
        syllables.sort_unstable();
        syllables.dedup();
        (chars, syllables)
    }

    /// Smallest log10 of the words of each length (index = characters; NaN-free, +inf where no word has that length).
    pub fn min_log10_by_len(&self) -> Vec<f64> {
        let mut m = vec![f64::INFINITY; self.max_syllables + 1];
        for e in self.by_reading.values().flatten() {
            let n = e.word.chars().count();
            if n < m.len() && e.log10 < m[n] {
                m[n] = e.log10;
            }
        }
        m
    }

    /// Median log10 of the data.txt words of each length (index = characters; NaN where there is none), as parsed: alias
    /// copies and alias overrides of existing entries do not change it.
    pub fn median_log10_by_len(&self) -> Vec<f64> {
        self.raw_median.clone()
    }

    /// The 90th percentile of the same values, sorted ascending, at the zero-based index round((n - 1) * 0.9) (a rounded
    /// linear index, not nearest rank): FUZZY F2 step 2's tau_L (plan v8,
    /// chosen on the development half).
    pub fn q90_log10_by_len(&self) -> Vec<f64> {
        self.raw_q90.clone()
    }

    fn median_now(&self) -> Vec<f64> {
        self.by_len_now(|v| {
            let m = v.len() / 2;
            if v.len() % 2 == 1 { v[m] } else { (v[m - 1] + v[m]) / 2.0 }
        })
    }

    fn by_len_now(&self, f: impl Fn(&[f64]) -> f64) -> Vec<f64> {
        let mut by: Vec<Vec<f64>> = vec![vec![]; self.max_syllables + 1];
        let mut seen = std::collections::HashSet::new();
        let mut all: Vec<&Entry> = self.by_reading.values().flatten().collect();
        all.sort_by_key(|e| e.line);
        for e in all {
            if !seen.insert(e.line) {
                continue;
            }
            let n = e.word.chars().count();
            if n < by.len() {
                by[n].push(e.log10);
            }
        }
        by.into_iter().map(|mut v| {
            if v.is_empty() {
                return f64::NAN;
            }
            v.sort_by(f64::total_cmp);
            f(&v)
        }).collect()
    }

    /// AUTOCOMP prefix index: for every word of 3..=`max_len` syllables and every prefix of 2..=`max_len` - 1 syllables
    /// leaving 1..=`max_extra` more, prefix reading ("a-b") -> (full reading, word, log10), in reading / line order.
    pub fn prefix_index(&self, max_len: usize, max_extra: usize) -> HashMap<String, Vec<(String, String, f64)>> {
        let mut m: HashMap<String, Vec<(String, String, f64, u32)>> = HashMap::new();
        for (reading, entries) in &self.by_reading {
            let syl: Vec<&str> = reading.split('-').collect();
            let n = syl.len();
            if !(3..=max_len).contains(&n) {
                continue;
            }
            for e in entries {
                for k in n.saturating_sub(max_extra).max(2)..n {
                    m.entry(syl[..k].join("-")).or_default().push((reading.clone(), e.word.clone(), e.log10, e.line));
                }
            }
        }
        m.into_iter().map(|(k, mut v)| {
            v.sort_by(|a, b| a.0.cmp(&b.0).then(a.3.cmp(&b.3)));
            (k, v.into_iter().map(|(r, w, l, _)| (r, w, l)).collect())
        }).collect()
    }

    /// Single characters of a one-syllable reading, best first (probability, then smaller line number).
    pub fn ranked_single_chars(&self, syllable: &str) -> Vec<char> {
        let mut v: Vec<&Entry> = self.lookup_reading(syllable).iter().filter(|e| e.word.chars().count() == 1).collect();
        v.sort_by(|a, b| b.log10.total_cmp(&a.log10).then(a.line.cmp(&b.line)));
        v.into_iter().filter_map(|e| e.word.chars().next()).collect()
    }

    /// Highest-probability single character of a one-syllable reading, used when the model is unavailable
    /// (AI.md fallback rule).
    pub fn best_single_char(&self, syllable: &str) -> Option<char> {
        self.ranked_single_chars(syllable).first().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syl(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn load_rules() {
        let text = "\
# header
_ctrl_punctuation_! ！ 0.0
ㄅㄚ 8️⃣ -8
ㄅㄚ 八 -3
ㄅㄚ 八 -2
ㄅㄚ-ㄅㄚ 八 -5
ㄐㄧㄣ-ㄊㄧㄢ 今天 -3.19
ㄐㄧㄣ-ㄊㄧㄢ MACRO@DATE -8
";
        let (dict, stats) = Dictionary::parse(text);
        assert_eq!(stats, LoadStats { kept: 2, special: 1, non_han: 2, length_mismatch: 1, duplicate: 1 });
        assert_eq!(dict.lookup(&syl("ㄅㄚ"))[0], Entry { word: "八".into(), log10: -2.0, line: 5 });
        assert_eq!(dict.lookup(&syl("ㄐㄧㄣ ㄊㄧㄢ"))[0].word, "今天");
    }

    #[test]
    fn aliases_and_single_char() {
        let mut d = Dictionary::parse(
            "ㄨㄟˊ 危 -4\nㄨㄟ 威 -3\nㄨㄟˊ-ㄒㄧㄢˇ 危險 -4.1\nㄑㄧˊ 期 -3.3\nㄩㄥˇ 擁 -4.23\nㄩㄥ 擁 -6.8\nㄌㄜˋ-ㄙㄜˋ 垃圾 -4\n",
        )
        .0;
        let s = d.apply_aliases("字 危 ㄨㄟˊ ㄨㄟ\n字 擁 ㄩㄥˇ ㄩㄥ\n詞 垃圾 ㄌㄜˋ-ㄙㄜˋ ㄌㄚ-ㄐㄧ\n").unwrap();
        assert_eq!(s, AliasStats { char_rules: 2, word_rules: 1, added: 3, merged: 1 });
        let words = |r: &str| d.lookup(&syl(r)).iter().map(|e| e.word.clone()).collect::<Vec<_>>();
        assert_eq!(words("ㄨㄟ"), vec!["威", "危"]);
        assert_eq!(words("ㄨㄟ ㄒㄧㄢˇ"), vec!["危險"]);
        assert_eq!(words("ㄌㄚ ㄐㄧ"), vec!["垃圾"]);
        assert_eq!(d.lookup(&syl("ㄩㄥ"))[0].log10, -4.23);
        assert_eq!(d.best_single_char("ㄨㄟ"), Some('威'));
        assert_eq!(d.best_single_char("ㄒㄧㄢˇ"), None);
        assert!(d.apply_aliases("字 危 ㄨㄟˇ ㄨㄟ").is_err());
        let legal = d.legal_readings();
        assert!(legal[&'危'].contains(&"ㄨㄟ".to_string()) && legal[&'危'].contains(&"ㄨㄟˊ".to_string()));
    }

    #[test]
    fn median_by_len_ignores_alias_copies() {
        // 丙丙 reads ㄨ-ㄨ (-2) and ㄨˊ-ㄨ (-8); 乙乙 -6
        let text = "ㄨ-ㄨ 丙丙 -2\nㄨˊ-ㄨ 丙丙 -8\nㄧ-ㄧ 乙乙 -6\nㄨ 丙 -1\n";
        let mut d = Dictionary::parse(text).0;
        assert_eq!(d.median_log10_by_len()[2], -6.0);
        // the alias overrides the existing (ㄨˊ-ㄨ, 丙丙) entry with -2; another alias adds a new reading
        d.apply_aliases("詞 丙丙 ㄨ-ㄨ ㄨˊ-ㄨ\n詞 乙乙 ㄧ-ㄧ ㄧˊ-ㄧ\n").unwrap();
        assert_eq!(d.lookup(&syl("ㄨˊ ㄨ"))[0].log10, -2.0);
        assert_eq!(d.lookup(&syl("ㄧˊ ㄧ"))[0].log10, -6.0);
        assert_eq!(d.median_log10_by_len()[2], -6.0);
        assert!(d.median_log10_by_len()[1] == -1.0);
        // q90 of -8, -6, -2: index round(2 * 0.9) = 2
        assert_eq!(d.q90_log10_by_len()[2], -2.0);
        // seven values: index round(6 * 0.9) = 5 (nearest rank would take 6)
        let seven: String = ["甲甲甲", "乙乙乙", "丙丙丙", "丁丁丁", "戊戊戊", "己己己", "庚庚庚"].iter().enumerate()
            .map(|(k, w)| format!("ㄚ-ㄚ-ㄚ {w} -{}\n", k + 1)).collect();
        assert_eq!(Dictionary::parse(&seven).0.q90_log10_by_len()[3], -2.0);
    }
}
