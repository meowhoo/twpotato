//! FUZZY F2 (FUZZY-F2-plan 1.2, 1.3): word-level fuzzy suggestions. Words of 2..=`MAX_LEN` syllables ending at
//! the cursor whose reading differs from the effective syllables in exactly one position by a confusion variant (the
//! variants and their class costs come from `twime-core`'s frozen F0 classes). Read-only: nothing here changes a
//! conversion or the normal candidate list.

use crate::Assets;

/// Longest span of a fuzzy suggestion.
pub const MAX_LEN: usize = 6;
/// Score penalty per unit of class cost (log10 units): s(h) = log10(h) - PENALTY * cost.
pub const PENALTY: f64 = 0.5;
/// Most strong hits placed at the end of the first page.
pub const STRONG: usize = 3;
/// Most fuzzy suggestions in one list.
pub const CAP: usize = 9;

/// Where a hit comes from (also its tie-break order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    User,
    Dict,
    Pn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FuzzyHit {
    pub text: String,
    pub span: usize,
    /// The changed position (absolute index) and the syllable used there.
    pub at: usize,
    pub syl: String,
    /// Class cost of the change (S 1, P 2, T 3).
    pub cost: u8,
    pub log10: f64,
    pub source: Source,
    /// s(h) = log10 - penalty * cost.
    pub score: f64,
    /// A user word, or a data.txt word with s(h) above the best data.txt log10 of the unchanged reading (plan 1.3).
    pub strong: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FuzzyParams {
    pub penalty: f64,
    /// Longest span (at most `MAX_LEN`).
    pub max_len: usize,
}

impl Default for FuzzyParams {
    fn default() -> FuzzyParams {
        FuzzyParams { penalty: PENALTY, max_len: MAX_LEN }
    }
}

/// Plan 1.3 order: user words first, then the score, then the longer span, then the text; then the source, the changed
/// position and syllable (a total order).
pub fn order(hits: &mut Vec<FuzzyHit>) {
    hits.sort_by(|a, b| {
        (a.source != Source::User)
            .cmp(&(b.source != Source::User))
            .then(b.score.total_cmp(&a.score))
            .then(b.span.cmp(&a.span))
            .then(a.text.cmp(&b.text))
            .then(a.source.cmp(&b.source))
            .then(a.at.cmp(&b.at))
            .then(a.syl.cmp(&b.syl))
    });
    // one item per (text, span, changed position, syllable): the first in that order (the same text with another reading
    // is another item)
    let mut seen: std::collections::HashSet<(String, usize, usize, String)> = std::collections::HashSet::new();
    hits.retain(|h| seen.insert((h.text.clone(), h.span, h.at, h.syl.clone())));
}

impl Assets {
    /// The fuzzy suggestions at the cursor `end`, ordered and deduplicated (not capped).
    /// `syl`: the effective syllables of the composition; positions `first..end` all have a character; `alts[j]`: the
    /// variants (syllable, class cost) of position j (empty = position not varied; `alts.len() >= end`).
    pub fn fuzzy_words(&self, syl: &[String], end: usize, first: usize, alts: &[Vec<(String, u8)>], p: &FuzzyParams) -> Vec<FuzzyHit> {
        let mut hits = vec![];
        if end > syl.len() || first > end || alts.len() < end {
            return hits;
        }
        let user = self.user_snapshot().filter(|l| !l.is_empty());
        let pn = self.pn.as_ref().filter(|(_, c)| c.cands).map(|(l, _)| l.clone());
        let max_l = p.max_len.min(MAX_LEN).min(end - first);
        // min_log10[L] (plan 1.3): the assets' table (built by the PN / SHORT-PRIOR setup); computed here only when that
        // setup did not run (tests, drivers without PN) and a user / PN word needs it
        let table = std::cell::OnceCell::new();
        let min_log10 = |l: usize| -> f64 {
            let v = if self.min_log10.is_empty() { table.get_or_init(|| self.dict.min_log10_by_len()) } else { &self.min_log10 };
            v.get(l).copied().filter(|x| x.is_finite()).unwrap_or(-20.0)
        };
        for l in 2..=max_l {
            let s = end - l;
            // b(L): the best data.txt word of the unchanged (effective) reading
            let base = self.dict.lookup(&syl[s..end]).iter().filter(|e| e.word.chars().count() == l).map(|e| e.log10).fold(f64::NEG_INFINITY, f64::max);
            let mut reading: Vec<String> = syl[s..end].to_vec();
            for j in s..end {
                for (g, cost) in &alts[j] {
                    if *g == syl[j] {
                        continue;
                    }
                    reading[j - s] = g.clone();
                    let pen = p.penalty * f64::from(*cost);
                    let mk = |text: String, log10: f64, source: Source, strong: bool| FuzzyHit {
                        text,
                        span: l,
                        at: j,
                        syl: g.clone(),
                        cost: *cost,
                        log10,
                        source,
                        score: log10 - pen,
                        strong,
                    };
                    // 1. data.txt: every character legal for its (changed) syllable
                    for e in self.dict.lookup(&reading) {
                        let chars: Vec<char> = e.word.chars().collect();
                        if chars.len() == l && chars.iter().zip(&reading).all(|(&c, r)| self.is_legal_char(r, c)) {
                            hits.push(mk(e.word.clone(), e.log10, Source::Dict, e.log10 - pen > base));
                        }
                    }
                    let codes: Option<Vec<u16>> = self.syl_codes(&reading).into_iter().collect();
                    // 2. user words (custom-reading positions allowed by their own bits)
                    if let (Some(lex), Some(codes)) = (user.as_ref(), codes.as_ref()) {
                        for w in lex.lookup(codes) {
                            let chars: Vec<char> = w.ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect();
                            if chars.len() != l || !chars.iter().enumerate().all(|(k, &c)| w.is_custom(k) || self.is_legal_char(&reading[k], c)) {
                                continue;
                            }
                            let text: String = chars.iter().collect();
                            let log10 = self.dict.lookup(&reading).iter().find(|e| e.word == text).map_or_else(|| min_log10(l), |e| e.log10);
                            hits.push(mk(text, log10, Source::User, true));
                        }
                    }
                    // 3. PN words (never strong; a word also in data.txt keeps its data.txt hit through the dedup)
                    if let (Some(lex), Some(codes)) = (pn.as_ref(), codes.as_ref()) {
                        for (_, ids) in lex.lookup(codes) {
                            let chars: Vec<char> = ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect();
                            if chars.len() != l || !chars.iter().zip(&reading).all(|(&c, r)| self.is_legal_char(r, c)) {
                                continue;
                            }
                            let text: String = chars.iter().collect();
                            if self.dict.lookup(&reading).iter().any(|e| e.word == text) {
                                continue;
                            }
                            hits.push(mk(text, min_log10(l), Source::Pn, false));
                        }
                    }
                }
                reading[j - s] = syl[j].clone();
            }
        }
        order(&mut hits);
        hits
    }
}

/// A FUZZY F2 step-2 hint option (FUZZY-F2-plan 5.2): `word` over `start..start + len` read as `reading` (the
/// effective syllables with position `at` changed to `syl`).
#[derive(Clone, Debug, PartialEq)]
pub struct FuzzyHint {
    pub start: usize,
    pub len: usize,
    pub word: String,
    pub reading: Vec<String>,
    pub at: usize,
    pub syl: String,
    pub score: f64,
}

impl Assets {
    /// tau_L of plan 5.2 (v8): the 90th percentile of the data.txt log10 of words of length L.
    pub fn fuzzy_tau(&self) -> &[f64] {
        self.fuzzy_tau.get_or_init(|| self.dict.q90_log10_by_len())
    }

    /// Plan 5.2 rules 1-5 on the output after LEXUX (`conv.chars`, auto replacements included). `eff`: the effective
    /// syllables; `alts[j]`: the variants (syllable, class cost) of position j (empty = not varied: K, illegal typed
    /// syllables); `excluded[j]`: positions no hint may cover (F1 repairs). Returns non-overlapping hints, best first.
    pub fn fuzzy_hints(&self, conv: &crate::Conversion, eff: &[String], alts: &[Vec<(String, u8)>], excluded: &[bool]) -> Vec<FuzzyHint> {
        let Some(sel) = conv.selection.as_ref() else { return vec![] };
        let n = conv.chars.len();
        if conv.fallback || eff.len() != n || alts.len() != n || excluded.len() != n || conv.chars.iter().any(Option::is_none) {
            return vec![];
        }
        let y: Vec<char> = conv.chars.iter().map(|c| c.unwrap()).collect();
        let tau = self.fuzzy_tau();
        let codes = self.syl_codes(eff);
        let fold = |c: char| if self.norm.out && c == crate::norm::TAI_TRAD { crate::norm::TAI } else { c };
        let same = |a: &[char], b: &[char]| a.len() == b.len() && a.iter().zip(b).all(|(&x, &z)| fold(x) == fold(z));
        let text = |ids: &[u32]| -> Vec<char> { ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect() };
        // rule 4: locks, USERLEX spans and used 2-character jumps (the LEXUX selection), PN words of the output, F1 repairs
        let mut blocked: Vec<bool> = (0..n).map(|i| sel.blocked.get(i).copied().unwrap_or(true) || excluded[i]).collect();
        let pns = self.pn.as_ref().map(|(p, _)| p.hits(&codes)).unwrap_or_default();
        for (s, _, ids) in &pns {
            if ids.len() >= 2 && same(&text(ids), &y[*s..s + ids.len()]) {
                blocked[*s..s + ids.len()].fill(true);
            }
        }
        let gens = self.genw.as_ref().map(|g| g.hits(&codes)).unwrap_or_default();
        let user = self.user_snapshot();
        let user_hits = user.as_ref().map(|u| u.hits(&codes, 2)).unwrap_or_default();
        // rule 1: the output span is not a word of its effective reading (the LEXUX "not a word" test)
        let is_word = |s: usize, e: usize| {
            let part = &y[s..e];
            gens.iter().chain(pns.iter()).any(|(t, _, ids)| *t == s && same(&text(ids), part))
                || self.dict.lookup(&eff[s..e]).iter().any(|w| same(&w.word.chars().collect::<Vec<_>>(), part))
                || user_hits.iter().any(|(t, w)| *t == s && same(&text(&w.ids), part))
        };
        // segmentation (user 2026-10-28): the best data.txt split of the output; positions inside a multi-character word of
        // that split, or inside a general / PN / user word of the output, are "cut"; a hint span must be uncut throughout
        let (seg0, cut_dict) = self.fuzzy_seg(&y, eff);
        let mut cut = cut_dict;
        for (s, _, ids) in gens.iter().chain(pns.iter()) {
            if ids.len() >= 2 && same(&text(ids), &y[*s..s + ids.len()]) {
                cut[*s..s + ids.len()].fill(true);
            }
        }
        for (s, w) in &user_hits {
            if w.ids.len() >= 2 && same(&text(&w.ids), &y[*s..s + w.ids.len()]) {
                cut[*s..s + w.ids.len()].fill(true);
            }
        }
        let mut found = vec![];
        for s in 0..n {
            for l in 2..=MAX_LEN.min(n - s) {
                let e = s + l;
                if blocked[s..e].iter().any(|b| *b) || cut[s..e].iter().any(|b| *b) || is_word(s, e) {
                    continue;
                }
                let Some(t) = tau.get(l).copied().filter(|x| x.is_finite()) else { continue };
                // the replaced output must split better by more than the class penalty
                let gain = |w: &str, reading: &[String]| {
                    let mut y2 = y.clone();
                    for (k, c) in w.chars().enumerate() { y2[s + k] = c; }
                    let mut r2 = eff.to_vec();
                    r2[s..e].clone_from_slice(reading);
                    self.fuzzy_seg(&y2, &r2).0 - seg0
                };
                // rule 2: one changed position, a data.txt word legal for the changed reading, common and strong
                let mut cands: Vec<FuzzyHint> = vec![];
                let mut reading: Vec<String> = eff[s..e].to_vec();
                for j in s..e {
                    for (g, cost) in &alts[j] {
                        if *g == eff[j] {
                            continue;
                        }
                        reading[j - s] = g.clone();
                        for w in self.dict.lookup(&reading) {
                            let chars: Vec<char> = w.word.chars().collect();
                            if chars.len() != l || same(&chars, &y[s..e]) || w.log10 < t {
                                continue;
                            }
                            let score = w.log10 - PENALTY * f64::from(*cost);
                            if !chars.iter().zip(&reading).all(|(&c, r)| self.is_legal_char(r, c)) || gain(&w.word, &reading) <= PENALTY * f64::from(*cost) {
                                continue;
                            }
                            cands.push(FuzzyHint { start: s, len: l, word: w.word.clone(), reading: reading.clone(), at: j, syl: g.clone(), score });
                        }
                    }
                    reading[j - s] = eff[j].clone();
                }
                // rule 3: unique best (a tie with the second best gives no hint)
                cands.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.word.cmp(&b.word)).then(a.at.cmp(&b.at)));
                if let Some(best) = cands.first() {
                    if cands.get(1).is_none_or(|second| second.score < best.score) {
                        found.push(best.clone());
                    }
                }
            }
        }
        // rule 5: greedy non-overlapping by (score, longer span, earlier start)
        found.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.len.cmp(&a.len)).then(a.start.cmp(&b.start)));
        let mut out: Vec<FuzzyHint> = vec![];
        for h in found {
            if !out.iter().any(|o| h.start < o.start + o.len && o.start < h.start + h.len) {
                out.push(h);
            }
        }
        out.sort_by_key(|h| h.start);
        out
    }

    /// The best split of `y` (read as `r`) into data.txt words of 1..=`MAX_LEN` characters by summed log10 (a character
    /// with no single-character entry costs `NO_CHAR`), and the positions inside its multi-character words.
    fn fuzzy_seg(&self, y: &[char], r: &[String]) -> (f64, Vec<bool>) {
        const NO_CHAR: f64 = -10.0;
        let n = y.len();
        let mut best = vec![f64::NEG_INFINITY; n + 1];
        let mut from = vec![0usize; n + 1];
        best[0] = 0.0;
        for e in 1..=n {
            for l in 1..=MAX_LEN.min(e) {
                let s = e - l;
                if !best[s].is_finite() {
                    continue;
                }
                let w: String = y[s..e].iter().collect();
                let v = self.dict.lookup(&r[s..e]).iter().filter(|x| x.word == w).map(|x| x.log10).fold(f64::NEG_INFINITY, f64::max);
                let v = if v.is_finite() { v } else if l == 1 { NO_CHAR } else { continue };
                if best[s] + v > best[e] {
                    best[e] = best[s] + v;
                    from[e] = s;
                }
            }
        }
        let mut cut = vec![false; n];
        let mut e = n;
        while e > 0 {
            let s = from[e];
            if e - s >= 2 {
                cut[s..e].fill(true);
            }
            e = s;
        }
        (best[n], cut)
    }
}

#[cfg(test)]
#[path = "fuzzy_cands_tests.rs"]
mod tests;
