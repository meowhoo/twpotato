//! AUTOCOMP (AUTOCOMP-plan 1.2): word completions at the end of the composition. Read-only: the session puts
//! the completions into the LEXUX `\` cycle.

use std::collections::HashMap;

use crate::{Assets, Conversion};

/// Longest word (syllables).
pub const MAX_LEN: usize = 6;
/// Most appended syllables.
pub const MAX_EXTRA: usize = 3;
/// Prefix lengths tried (largest first).
pub const MIN_K: usize = 2;
pub const MAX_K: usize = 5;
/// AUTOCOMP-LIST (plan 3): candidates kept per k before merging.
const LIST_TOP: usize = 20;
/// AUTOCOMP-POP only; LIST retains its accepted per-k cap.
const POP_TOP: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoParams {
    /// Share of the first completion among the prefix-consistent words.
    pub theta: f64,
    /// Share of the second completion.
    pub theta2: f64,
}

impl AutoParams {
    /// The deployed thresholds (chosen on the tuning half, AUTOCOMP-plan 3; provisional until then).
    pub const DEPLOYED: AutoParams = AutoParams { theta: 0.7, theta2: 0.3 };
}

/// One completion: the last `k` syllables keep their characters, `extra` syllables and `extra_chars` are appended.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub k: usize,
    pub word: String,
    pub extra: Vec<String>,
    pub extra_chars: String,
    pub user: bool,
    pub score: f64,
    pub share: f64,
}

impl Assets {
    /// Builds the dictionary prefix index now (the engine calls it at start when AUTOCOMP-LIST is on, so the first
    /// opened list does not pay for it).
    pub fn warm_autocomp(&self) {
        self.autocomp_index();
    }

    fn autocomp_index(&self) -> &HashMap<String, Vec<(String, String, f64)>> {
        self.autocomp_index.get_or_init(|| self.dict.prefix_index(MAX_LEN, MAX_EXTRA))
    }

    /// Plan 1.2 rules 1-3, 5 and 7 (PRED plan 3: no share / tau filter): for every unblocked k (largest first) the full
    /// prefix-consistent candidate list in the rule-5 order, each with its dictionary share within that list. `eff`:
    /// effective syllables; `excluded[i]`: positions no completion may cover (F1 repairs and the session's own exclusions;
    /// locks, USERLEX spans, used jumps and LEXUX automatic spans are taken from the conversion here).
    pub fn autocomp_lists(&self, conv: &Conversion, eff: &[String], excluded: &[bool]) -> Vec<(usize, Vec<Completion>)> {
        self.autocomp_lists_with(conv, eff, excluded, None)
    }

    /// `top = Some(t)`: only each list's first `t` items are exact (their order and scores; `share` is then not
    /// meaningful): the generated / place-name lexicons are skipped for a k whose user words decide the list, or whose
    /// dictionary words already give `t` candidates above the floor score that any word outside the dictionary gets.
    fn autocomp_lists_with(&self, conv: &Conversion, eff: &[String], excluded: &[bool], top: Option<usize>) -> Vec<(usize, Vec<Completion>)> {
        let Some(sel) = conv.selection.as_ref() else { return vec![] };
        let n = conv.chars.len();
        if conv.fallback || eff.len() != n || excluded.len() != n || conv.chars.iter().any(Option::is_none) {
            return vec![];
        }
        let y: Vec<char> = conv.chars.iter().map(|c| c.unwrap()).collect();
        let fold = |c: char| if self.norm.out && c == crate::norm::TAI_TRAD { crate::norm::TAI } else { c };
        let same = |a: &[char], b: &[char]| a.len() == b.len() && a.iter().zip(b).all(|(&x, &z)| fold(x) == fold(z));
        let mut lists = vec![];
        let floors = if self.min_log10.is_empty() { self.dict.min_log10_by_len() } else { self.min_log10.clone() };
        let min_log10 = |l: usize| floors.get(l).copied().filter(|x| x.is_finite()).unwrap_or(-20.0);
        let user = self.user_snapshot();
        for k in (MIN_K..=MAX_K.min(n)).rev() {
            let s = n - k;
            let blocked = (s..n).any(|i| excluded[i] || sel.blocked.get(i).copied().unwrap_or(true))
                || conv.auto.iter().any(|&(a, m)| a < n && s < a + m);
            if blocked {
                continue;
            }
            let prefix = &eff[s..n];
            // (word, full reading) -> (score, user)
            let mut cands: HashMap<(String, Vec<String>), (f64, bool)> = HashMap::new();
            let add = |cands: &mut HashMap<(String, Vec<String>), (f64, bool)>, word: String, reading: Vec<String>, score: f64, is_user: bool| {
                let chars: Vec<char> = word.chars().collect();
                if chars.len() != reading.len() || chars.len() > MAX_LEN || !same(&chars[..k], &y[s..n])
                    || !chars.iter().zip(&reading).all(|(&c, r)| self.is_legal_char(r, c)) {
                    return;
                }
                let e = cands.entry((word, reading)).or_insert((f64::NEG_INFINITY, false));
                e.0 = e.0.max(score);
                e.1 |= is_user;
            };
            if let Some(v) = self.autocomp_index().get(&prefix.join("-")) {
                for (reading, word, log10) in v {
                    // the prefix characters first (most words sharing the readings do not share the characters)
                    let mut w = word.chars();
                    if !y[s..n].iter().all(|&c| w.next().is_some_and(|x| fold(x) == fold(c))) {
                        continue;
                    }
                    add(&mut cands, word.clone(), reading.split('-').map(String::from).collect(), *log10, false);
                }
            }
            let codes: Option<Vec<u16>> = self.syl_codes(prefix).into_iter().collect();
            let Some(codes) = codes else { continue };
            let dict_score = |word: &str, reading: &[String]| {
                self.dict.lookup(reading).iter().find(|e| e.word == word).map_or_else(|| min_log10(reading.len()), |e| e.log10)
            };
            let extend = |extra: &[u16]| -> Option<Vec<String>> {
                let mut r = prefix.to_vec();
                for &c in extra {
                    r.push(self.vocab.syllable_of(u32::from(c))?.to_string());
                }
                Some(r)
            };
            if let Some(u) = user.as_ref() {
                for (extra, w) in u.prefix_words(&codes, MAX_EXTRA) {
                    let Some(reading) = extend(&extra) else { continue };
                    let word: String = w.ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect();
                    let sc = dict_score(&word, &reading);
                    add(&mut cands, word, reading, sc, true);
                }
            }
            let skip = top.is_some_and(|t| {
                let floor = (k + 1..=(k + MAX_EXTRA).min(MAX_LEN)).map(min_log10).fold(f64::NEG_INFINITY, f64::max);
                cands.values().any(|v| v.1) || cands.values().filter(|v| v.0 > floor).count() >= t
            });
            for lex in [self.genw.as_ref(), self.pn.as_ref().map(|(l, _)| l)].into_iter().flatten().filter(|_| !skip) {
                for (extra, _, ids) in lex.prefix_words(&codes, MAX_EXTRA) {
                    // the prefix characters first (most words sharing the readings do not share the characters)
                    if ids.len() < k || !ids[..k].iter().zip(&y[s..n]).all(|(&id, &c)| self.vocab.char_of(id).is_some_and(|x| fold(x) == fold(c))) {
                        continue;
                    }
                    let Some(reading) = extend(&extra) else { continue };
                    let word: String = ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect();
                    let sc = dict_score(&word, &reading);
                    add(&mut cands, word, reading, sc, false);
                }
            }
            // rule 4: shares within the prefix-consistent words; with user words present, among the user words only
            let has_user = cands.values().any(|v| v.1);
            let mut list: Vec<((String, Vec<String>), (f64, bool))> = cands.into_iter().filter(|(_, v)| !has_user || v.1).collect();
            if list.is_empty() {
                continue;
            }
            // rule 5 order: user words first, score, shorter, word, reading
            list.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(b.1.0.total_cmp(&a.1.0)).then(a.0.1.len().cmp(&b.0.1.len()))
                .then(a.0.0.cmp(&b.0.0)).then(a.0.1.cmp(&b.0.1)));
            let best = list[0].1.0;
            let total: f64 = list.iter().map(|(_, v)| 10f64.powf(v.0 - best)).sum();
            lists.push((k, list.iter().take(top.unwrap_or(usize::MAX)).map(|e| Completion {
                k,
                word: e.0.0.clone(),
                extra: e.0.1[k..].to_vec(),
                extra_chars: e.0.0.chars().skip(k).collect(),
                user: e.1.1,
                score: e.1.0,
                share: 10f64.powf(e.1.0 - best) / total,
            }).collect()));
        }
        lists
    }

    /// AUTOCOMP-LIST (plan 2, 5.1): every unblocked k's first 20 candidates merged into one list, ordered by user words
    /// first, dictionary log10 descending, total length ascending, word, full reading, k descending.
    pub fn autocomp_list_merged(&self, conv: &Conversion, eff: &[String], excluded: &[bool]) -> Vec<Completion> {
        self.autocomp_merged_with_top(conv, eff, excluded, LIST_TOP)
    }

    /// POP has its own cap; ordinary LIST behavior is independent.
    pub fn autocomp_pop_merged(&self, conv: &Conversion, eff: &[String], excluded: &[bool]) -> Vec<Completion> {
        self.autocomp_merged_with_top(conv, eff, excluded, POP_TOP)
    }

    fn autocomp_merged_with_top(&self, conv: &Conversion, eff: &[String], excluded: &[bool], top: usize) -> Vec<Completion> {
        let mut all: Vec<Completion> = self.autocomp_lists_with(conv, eff, excluded, Some(top)).into_iter().flat_map(|(_, l)| l.into_iter().take(top)).collect();
        #[cfg(debug_assertions)]
        {
            let full: Vec<Completion> = self.autocomp_lists(conv, eff, excluded).into_iter().flat_map(|(_, l)| l.into_iter().take(top)).collect();
            let key = |c: &Completion| (c.k, c.word.clone(), c.extra.clone(), c.user, c.score.to_bits());
            assert!(all.iter().map(key).eq(full.iter().map(key)), "AUTOCOMP-LIST fast path differs");
        }
        all.sort_by(|a, b| b.user.cmp(&a.user).then(b.score.total_cmp(&a.score)).then((a.k + a.extra.len()).cmp(&(b.k + b.extra.len())))
            .then(a.word.cmp(&b.word)).then(a.extra.cmp(&b.extra)).then(b.k.cmp(&a.k)));
        all
    }

    /// Plan 1.2 rules 1-7: the completions of the largest k whose first candidate passes the share and tau rules (empty =
    /// no hint): the first, and the second when its share reaches theta2.
    pub fn autocomp(&self, conv: &Conversion, eff: &[String], excluded: &[bool], p: &AutoParams) -> Vec<Completion> {
        let tau = self.fuzzy_tau();
        for (_, list) in self.autocomp_lists(conv, eff, excluded) {
            let first = &list[0];
            let common = first.user || tau.get(first.k + first.extra.len()).copied().is_some_and(|t| t.is_finite() && first.score >= t);
            if first.share < p.theta || !common {
                continue;
            }
            let mut out = vec![first.clone()];
            if let Some(second) = list.get(1).filter(|e| e.share >= p.theta2) {
                out.push(second.clone());
            }
            return out;
        }
        vec![]
    }
}
