//! Word-level candidates (N3b-plan section 2): single characters and dictionary words of 2..=`max_span`
//! syllables that end at the cursor, scored from the same inference that produced the sentence.
//!
//! Score of a replacement `w` for the span `[s, e)` (X is the current sentence, X' the sentence with the span replaced):
//!
//! ```text
//! D(w) = sum_{i in span} (lp_i(w_i) - lp_i(X_i)) + lambda * (P(X') - P(X))     // same objective as the decoder
//! S(w) = D(w) + beta * R(w)                                                     // R <= 0, words only
//! R(w) = ln10 * (log10(w) - max log10 of the words with the same reading)
//! ```
//!
//! `lp_i` is the log-softmax of the model scores over the legal set of position i; `P` is the character n-gram prior over
//! the window `[s, min(n, e + 3))`, used only when the whole conversion is complete (as the decoder does).


use crate::convert::{Assets, Conversion};
use crate::norm::{TAI, TAI_TRAD};

/// A source of extra words for a reading (user dictionary, later). `log10` has the scale of `data.txt`.
pub trait WordSource: Send + Sync {
    fn words(&self, syllables: &[String]) -> Vec<WordHit>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct WordHit {
    pub word: String,
    pub log10: f64,
}

#[derive(Clone, Debug)]
pub struct CandParams {
    /// Prior weight (same as the decoder's lambda).
    pub lambda: f64,
    /// Weight of the same-reading rarity penalty R.
    pub beta: f64,
    /// Longest span in syllables; 1 = single characters only.
    pub max_span: usize,
    /// Most words (span >= 2) shown, the current item included when it is a word (the current item is always shown).
    pub word_cap: usize,
    /// Session style (NORM plan 11.1): true = 臺 is the preferred glyph of a 台/臺 pair (only the order of a pair changes).
    pub style_tai: bool,
}

impl Default for CandParams {
    fn default() -> CandParams {
        CandParams { lambda: 0.5, beta: 0.0, max_span: 6, word_cap: 40, style_tai: false }
    }
}

/// One entry of the candidate list.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub text: String,
    /// Number of syllables (= characters) replaced, ending at the cursor.
    pub span: usize,
    pub score: f64,
    /// Replacing the span with `text` leaves the sentence unchanged.
    pub current: bool,
}

struct Raw {
    chars: Vec<char>,
    ids: Vec<u32>,
    span: usize,
    score: f64,
    /// Differs from X at these (position, character) pairs.
    effect: Vec<(usize, char)>,
    /// Only in the PN lexicon (not in data.txt): shown under its own cap (PN-plan 2).
    pn: bool,
    /// Only in the general-word layer (not in data.txt or PN): its own cap (SRCH-plan 4, mode 0).
    genw: bool,
    user: bool,
}

/// Text with 臺 read as 台: the key of a 台/臺 pair.
fn fold_key(c: &[char]) -> Vec<char> {
    c.iter().map(|&x| if x == TAI_TRAD { TAI } else { x }).collect()
}

/// At most `room` words (a 台/臺 pair counts as one place; the pair of the current word is free).
fn cap<'a>(mut words: Vec<&'a Raw>, room: usize, pair: bool, cur_word: Option<&Raw>) -> Vec<&'a Raw> {
    if pair {
        let mut groups: Vec<(usize, Vec<char>)> = vec![];
        if let Some(r) = cur_word {
            groups.push((r.span, fold_key(&r.chars)));
        }
        let cur_group = groups.len();
        words.retain(|r| {
            let k = (r.span, fold_key(&r.chars));
            if groups.contains(&k) {
                return true;
            }
            if groups.len() - cur_group < room {
                groups.push(k);
                return true;
            }
            false
        });
    } else {
        words.truncate(room);
    }
    words
}

impl Assets {
    /// Sets the extra word source (user dictionary interface). `None` removes it.
    pub fn set_word_source(&mut self, src: Option<std::sync::Arc<dyn WordSource>>) {
        self.extra = src;
    }

    /// Candidate list for the span ending at `end` (syllable index + 1, i.e. the cursor), in display order: the current
    /// item first, then the rest by score. Empty if `end` is out of range or its syllable has no character.
    pub fn candidates(&self, conv: &Conversion, end: usize, p: &CandParams) -> Vec<Candidate> {
        let inf = &conv.inf;
        let n = inf.syllables.len();
        if end == 0 || end > n || conv.chars.len() != n || conv.chars[end - 1].is_none() {
            return vec![];
        }
        let user = self.user_snapshot();
        let mut user_hits = vec![];
        if let Some(lex) = user.as_ref().filter(|l| !l.is_empty()) {
            for l in 1..=end.min(crate::user::USER_MAX_LEN) {
                if conv.chars[end - l].is_none() { break; }
                if let Some(codes) = self.syl_codes(&inf.syllables[end - l..end]).into_iter().collect::<Option<Vec<_>>>() {
                    user_hits.extend(lex.lookup(&codes).iter().map(|w| (l, w)));
                }
            }
        }
        let model = !conv.fallback && inf.scores.len() == n;
        // PN lexicon words (model mode, word candidates on): spans up to 8, own cap.
        let pn = self.pn.as_ref().filter(|(_, c)| c.cands && model && p.max_span >= 2).map(|(l, _)| l);
        // General-word layer (mode 0): spans up to 6, own cap.
        let genw = self.genw.as_ref().filter(|_| model && p.max_span >= 2);
        let max_l = if pn.is_some() { p.max_span.max(crate::pn::MAX_LEN) } else { p.max_span.max(1) };
        let max_l = if genw.is_some() { max_l.max(crate::genw::MAX_LEN) } else { max_l }.min(end);
        // Longest run of positions with a character, ending at `end`.
        let mut l_ok = 0;
        while l_ok < max_l && conv.chars[end - 1 - l_ok].is_some() {
            l_ok += 1;
        }
        let x_ids: Vec<Option<u32>> = conv.chars.iter().map(|c| c.map(|c| self.vocab.char_id(c))).collect();
        let first = end - l_ok.max(user_hits.iter().map(|(l, _)| *l).max().unwrap_or(0));

        // log-softmax of the span positions (f64), looked up by id.
        let lse: Vec<f64> = (first..end)
            .map(|i| {
                if !model {
                    return 0.0;
                }
                let v = &inf.scores[i];
                let max = v.iter().map(|&s| f64::from(s)).fold(f64::NEG_INFINITY, f64::max);
                max + v.iter().map(|&s| (f64::from(s) - max).exp()).sum::<f64>().ln()
            })
            .collect();
        // NORM pairs: the model scores of 台 and 臺 are read as one (the 台 score); the pair shares its best final score below.
        let pair = self.norm.pair;
        let (tai_id, trad_id) = (self.vocab.char_id(TAI), self.vocab.char_id(TAI_TRAD));
        let lp = |i: usize, id: u32| -> f64 {
            let set = &inf.sets[i];
            let id = if pair && id == trad_id && set.binary_search(&tai_id).is_ok() { tai_id } else { id };
            match set.binary_search(&id) {
                Ok(j) => f64::from(inf.scores[i][j]) - lse[i - first],
                // USERLEX 1.6: a custom-reading character (only user words and the current output built from them can
                // carry one): the position's lowest legal log-probability, so every score stays finite.
                Err(_) => inf.scores[i].iter().map(|&x| f64::from(x)).reduce(f64::min).map_or(f64::NEG_INFINITY, |m| m - lse[i - first]),
            }
        };
        let use_prior = model && p.lambda != 0.0 && self.prior.is_some() && conv.complete();
        let prior_window = |ids: &[u32], from: usize, to: usize| -> f64 {
            let pk = self.prior.as_ref().unwrap();
            // The pack decides how 台/臺 are queried (a folded T2 pack maps 臺 to 台, T0/T1 are queried as they are).
            (from..to).map(|i| pk.logp(&ids[i.saturating_sub(3)..i], ids[i])).sum()
        };
        // SHORT-PRIOR (SHORT-PRIOR-plan 1): with lambda(c_i) / F3 on, D(w) weighs each window position by its own
        // lambda(c_i) and conditions the first positions on the context tail; off = the expression above, bit for bit.
        let sp = self.short_view(inf);
        let short_window = |y: &[u32], x: &[u32], from: usize, to: usize| -> f64 {
            let pk = self.prior.as_ref().unwrap();
            (from..to)
                .map(|i| {
                    let w = sp.weight(p.lambda, i);
                    let (hy, ny) = crate::prior::hist3(sp.pre(), &y[..i]);
                    let (hx, nx) = crate::prior::hist3(sp.pre(), &x[..i]);
                    w * (pk.logp(&hy[..ny], y[i]) - pk.logp(&hx[..nx], x[i]))
                })
                .sum()
        };
        let x_all: Vec<u32> = x_ids.iter().map(|o| o.unwrap_or(0)).collect();

        // Raw candidates of every span length.
        let mut raws: Vec<Raw> = vec![];
        // `custom`: bit k = position k may be outside the legal set (a user word's own custom reading, USERLEX 1.6).
        let push = |me: &Assets, l: usize, chars: Vec<char>, raws: &mut Vec<Raw>, custom: u8| {
            let s = end - l;
            let ids: Vec<u32> = chars.iter().map(|&c| me.vocab.char_id(c)).collect();
            if (0..l).any(|k| inf.sets[s + k].binary_search(&ids[k]).is_err() && custom & (1 << k) == 0) {
                return;
            }
            let effect: Vec<(usize, char)> = (0..l).filter(|&k| conv.chars[s + k] != Some(chars[k])).map(|k| (s + k, chars[k])).collect();
            let score = if model {
                let mut d: f64 = (0..l).map(|k| lp(s + k, ids[k]) - lp(s + k, x_ids[s + k].unwrap_or(0))).sum();
                if use_prior {
                    let to = (end + 3).min(n);
                    let mut y = x_all.clone();
                    y[s..end].copy_from_slice(&ids);
                    if sp.prior_plain(p.lambda) {
                        d += p.lambda * (prior_window(&y, s, to) - prior_window(&x_all, s, to));
                    } else {
                        d += short_window(&y, &x_all, s, to);
                    }
                }
                d
            } else {
                0.0
            };
            raws.push(Raw { chars, ids, span: l, score, effect, pn: false, genw: false, user: false });
        };
        // Single characters: the whole legal set (reading order of ids).
        for &id in &inf.sets[end - 1] {
            if let Some(c) = self.vocab.char_of(id) {
                push(self, 1, vec![c], &mut raws, 0);
            }
        }
        // Words.
        for l in 2..=l_ok {
            let syl = &inf.syllables[end - l..end];
            let mut hits: Vec<WordHit> = self.dict.lookup(syl).iter().map(|e| WordHit { word: e.word.clone(), log10: e.log10 }).collect();
            if let Some(src) = &self.extra {
                for h in src.words(syl) {
                    match hits.iter_mut().find(|x| x.word == h.word) {
                        Some(x) => x.log10 = x.log10.max(h.log10),
                        None => hits.push(h),
                    }
                }
            }
            // Beyond max_span only lexicon words are listed; a data.txt word of that length only lends its log10.
            if l > p.max_span.max(1) {
                hits.retain(|_| false);
            }
            let mut pn_only = vec![false; hits.len()];
            if let Some(lex) = pn {
                let codes: Option<Vec<u16>> = self.syl_codes(syl).into_iter().collect();
                for (_, ids) in codes.map(|c| lex.lookup(&c)).unwrap_or_default() {
                    let word: String = ids.iter().filter_map(|&i| self.vocab.char_of(i)).collect();
                    if hits.iter().any(|h| h.word == word) {
                        continue; // same string in data.txt: one item with the data.txt log10
                    }
                    let log10 = match self.dict.lookup(syl).iter().find(|e| e.word == word) {
                        Some(e) => e.log10,
                        None => self.min_log10.get(l).copied().filter(|x| x.is_finite()).unwrap_or(-20.0),
                    };
                    pn_only.push(!self.dict.lookup(syl).iter().any(|e| e.word == word));
                    hits.push(WordHit { word, log10 });
                }
            }
            let mut gen_only = vec![false; hits.len()];
            if let Some(lex) = genw.filter(|_| l <= crate::genw::MAX_LEN) {
                let codes: Option<Vec<u16>> = self.syl_codes(syl).into_iter().collect();
                for (_, ids) in codes.map(|c| lex.lookup(&c)).unwrap_or_default() {
                    let word: String = ids.iter().filter_map(|&i| self.vocab.char_of(i)).collect();
                    if hits.iter().any(|h| h.word == word) {
                        continue; // already listed (data.txt or PN): one item, under that word's cap
                    }
                    let log10 = self.min_log10.get(l).copied().filter(|x| x.is_finite()).unwrap_or(-20.0);
                    pn_only.push(false);
                    gen_only.push(true);
                    hits.push(WordHit { word, log10 });
                }
            }
            gen_only.resize(hits.len(), false);
            let best = hits.iter().map(|h| h.log10).fold(f64::NEG_INFINITY, f64::max);
            for ((h, is_pn), is_gen) in hits.into_iter().zip(pn_only).zip(gen_only) {
                let chars: Vec<char> = h.word.chars().collect();
                if chars.len() != l {
                    continue;
                }
                let before = raws.len();
                push(self, l, chars, &mut raws, 0);
                if raws.len() > before {
                    let r = raws.last_mut().unwrap();
                    r.pn = is_pn;
                    r.genw = is_gen;
                    // Words: add the rarity penalty (model mode) or order by dictionary probability (fallback).
                    r.score += if model { p.beta * std::f64::consts::LN_10 * (h.log10 - best) } else { 1.0e6 + std::f64::consts::LN_10 * h.log10 };
                }
            }
        }
        if !model {
            // Fallback: single characters by the dictionary's single-character order, after the words.
            let order = self.dict.ranked_single_chars(&inf.syllables[end - 1]);
            for r in raws.iter_mut().filter(|r| r.span == 1) {
                let rank = order.iter().position(|&c| c == r.chars[0]).unwrap_or(1 << 20);
                r.score = -(rank as f64);
            }
        }

        for (l, word) in user_hits {
            let chars: Vec<char> = word.ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect();
            if chars.len() != l { continue; }
            let mut fresh = vec![];
            push(self, l, chars, &mut fresh, word.custom);
            if let Some(mut r) = fresh.pop() {
                r.user = true;
                // User words have no rarity penalty; fallback uses the word tier's 1e6 weight.
                if !model { r.score = 1.0e6; }
                if let Some(old) = raws.iter_mut().find(|x| x.span == l && x.ids == r.ids) {
                    old.user = true;
                    old.score = r.score;
                } else {
                    raws.push(r);
                }
            }
        }

        if pair {
            // Pairs: every candidate with 台/臺 at a position where both are legal gets its all-台 and all-臺 forms
            // (synthesized when the dictionary lacks one); the two share the best score of the pair.
            let mut extra: Vec<Raw> = vec![];
            for r in raws.iter() {
                if !r.chars.iter().any(|&c| c == TAI || c == TAI_TRAD) {
                    continue;
                }
                let s = end - r.span;
                for target in [TAI, TAI_TRAD] {
                    let tid = if target == TAI { tai_id } else { trad_id };
                    let chars: Vec<char> = r
                        .chars
                        .iter()
                        .enumerate()
                        .map(|(k, &c)| if (c == TAI || c == TAI_TRAD) && inf.sets[s + k].binary_search(&tid).is_ok() { target } else { c })
                        .collect();
                    if raws.iter().chain(extra.iter()).any(|x| x.span == r.span && x.chars == chars) {
                        continue;
                    }
                    let ids: Vec<u32> = chars.iter().map(|&c| self.vocab.char_id(c)).collect();
                    let effect: Vec<(usize, char)> = (0..r.span).filter(|&k| conv.chars[s + k] != Some(chars[k])).map(|k| (s + k, chars[k])).collect();
                    extra.push(Raw { chars, ids, span: r.span, score: r.score, effect, pn: r.pn, genw: r.genw, user: false });
                }
            }
            raws.extend(extra);
            let mut best: std::collections::HashMap<(usize, Vec<char>), f64> = std::collections::HashMap::new();
            for r in &raws {
                let e = best.entry((r.span, fold_key(&r.chars))).or_insert(f64::NEG_INFINITY);
                *e = e.max(r.score);
            }
            for r in raws.iter_mut() {
                r.score = best[&(r.span, fold_key(&r.chars))];
            }
        }
        // Pair members sort next to each other: the folded ids come before the glyph preference.
        let fkey = |r: &Raw| -> Vec<u32> { if pair { r.ids.iter().map(|&i| if i == trad_id { tai_id } else { i }).collect() } else { vec![] } };
        // Preferred glyph first inside a pair (0 when pairs are off).
        let pref = |r: &Raw| -> usize { if pair { r.chars.iter().filter(|&&c| (c == TAI_TRAD) != p.style_tai && (c == TAI || c == TAI_TRAD)).count() } else { 0 } };

        // The current item: replacement text equals the sentence. Shortest multi-character word, else the single.
        let is_cur = |r: &Raw| r.effect.is_empty();
        // USERLEX 1.2: a forced user-word span ending at the cursor is the current item (first in the list).
        let forced_len = conv.forced.iter().find(|(st, ids)| st + ids.len() == end).map(|(_, ids)| ids.len());
        let cur = forced_len
            .and_then(|fl| raws.iter().find(|r| is_cur(r) && r.span == fl))
            .or_else(|| raws.iter().filter(|r| is_cur(r) && r.span >= 2).min_by_key(|r| r.span))
            .or_else(|| raws.iter().find(|r| is_cur(r) && r.span == 1));
        let cur_text: Option<String> = cur.map(|r| r.chars.iter().collect());
        let cur_key = cur.filter(|_| pair).map(|r| (r.span, fold_key(&r.chars)));
        let is_mate = |r: &Raw| cur_key.as_ref().is_some_and(|k| (r.span, fold_key(&r.chars)) == *k);
        let mut user_items: Vec<&Raw> = raws.iter().filter(|r| r.user && !is_cur(r) && !is_mate(r)).collect();
        user_items.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.span.cmp(&b.span)).then(fkey(a).cmp(&fkey(b))).then(pref(a).cmp(&pref(b))).then(a.ids.cmp(&b.ids)));
        // USERLEX 1.2: the 20-item cap stays, but user words of the forced word's reading are always listed (<= 32).
        let same_reading: Vec<&Raw> = forced_len.map(|fl| user_items.iter().skip(20).filter(|r| r.span == fl).copied().collect()).unwrap_or_default();
        user_items.truncate(20);
        user_items.extend(same_reading);
        let user_effects: Vec<_> = user_items.iter().filter(|r| r.span >= 2).map(|r| &r.effect).collect();
        user_items.retain(|r| r.span >= 2 || !user_effects.contains(&&r.effect));
        user_items.extend(raws.iter().filter(|r| r.user && !is_cur(r) && is_mate(r)));

        // Words: at most `word_cap` in all, the current item counting when it is a word; best by score.
        let mut words: Vec<&Raw> = raws.iter().filter(|r| r.span >= 2 && !is_cur(r) && !r.user).collect();
        words.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.span.cmp(&b.span)).then(fkey(a).cmp(&fkey(b))).then(pref(a).cmp(&pref(b))).then(a.ids.cmp(&b.ids)));
        // PN-only words have their own cap (PN-plan 2); without PN every word is a data.txt word.
        let cur_word = cur.filter(|r| r.span >= 2);
        let room = p.word_cap.saturating_sub(usize::from(cur_word.is_some_and(|r| !r.user && !r.pn && !r.genw)));
        let pn_room = crate::pn::CAND_CAP.saturating_sub(usize::from(cur_word.is_some_and(|r| !r.user && r.pn)));
        let gen_room = crate::genw::CAND_CAP.saturating_sub(usize::from(cur_word.is_some_and(|r| !r.user && r.genw)));
        let (gen_words, words): (Vec<&Raw>, Vec<&Raw>) = words.into_iter().partition(|r| r.genw);
        let (dict_words, pn_words): (Vec<&Raw>, Vec<&Raw>) = words.into_iter().partition(|r| !r.pn);
        let mut words = cap(dict_words, room, pair, cur_word);
        if !pn_words.is_empty() {
            words.extend(cap(pn_words, pn_room, pair, cur_word));
        }
        if !gen_words.is_empty() {
            words.extend(cap(gen_words, gen_room, pair, cur_word));
        }
        // A single character is hidden when a shown word has the same effect on the sentence.
        let shown_effects: Vec<&Vec<(usize, char)>> = words.iter().chain(user_items.iter().filter(|r| r.span >= 2)).map(|r| &r.effect).collect();
        let mut singles: Vec<&Raw> = raws
            .iter()
            .filter(|r| r.span == 1 && !is_cur(r) && !r.user && !shown_effects.contains(&&r.effect))
            .collect();
        let mut rest: Vec<&Raw> = words;
        rest.append(&mut singles);
        rest.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.span.cmp(&b.span)).then(fkey(a).cmp(&fkey(b))).then(pref(a).cmp(&pref(b))).then(a.ids.cmp(&b.ids)));

        let mut out: Vec<Candidate> = Vec::with_capacity(rest.len() + 1);
        if let (Some(r), Some(t)) = (cur, cur_text) {
            out.push(Candidate { text: t, span: r.span, score: r.score, current: true });
        }
        // The partner of the current item (same folded text) follows it directly.
        rest.extend(user_items);
        let (mate, other): (Vec<&Raw>, Vec<&Raw>) = rest.into_iter().partition(|r| is_mate(r));
        let (users, other): (Vec<&Raw>, Vec<&Raw>) = other.into_iter().partition(|r| r.user);
        out.extend(mate.into_iter().chain(users).chain(other).map(|r| Candidate { text: r.chars.iter().collect(), span: r.span, score: r.score, current: false }));
        out
    }
}

#[cfg(test)]
#[path = "cands_tests.rs"]
mod tests;
