//! FUZZY F2 (FUZZY-F2-plan 1): word-level fuzzy suggestions in the candidate list. A suggestion is a dictionary /
//! user / PN word ending at the cursor whose reading differs from the effective syllables in one position by a confusion
//! class (the frozen F0 classes, K excluded). It is an ordinary `CandItem` whose `repairs` carry the changed reading, so
//! it shows as `檔案（ㄉㄤˇ）` and choosing it is the F1 choice path (repair syllable, lock, one new inference).
use super::*;
use twime_model::fuzzy_cands::{self, FuzzyParams};

impl Session {
    /// The kind of a legal position (F2): `Chosen` when the user changed its reading by choosing a suggestion, else `Typed`
    /// (also when the choice went back to the typed reading). Only a suggestion or its way back sets `repair` at a legal
    /// position, so without one this is always `Typed`.
    pub(super) fn f2_kind(&self, i: usize) -> RepairKind {
        match self.st.repair.get(i).cloned().flatten() {
            Some(g) if g != self.st.syl[i] && self.usable(i) => RepairKind::Chosen(g),
            _ => RepairKind::Typed,
        }
    }

    /// Plan 1.2-1.4: adds to `items` (the normal list and any repair segment, in their order) the way back of a changed
    /// position before the cursor, then the suggestions at the cursor: at most `STRONG` strong hits at the end of the first
    /// page, the rest after the first page, `CAP` in all.
    pub(super) fn add_fuzzy(&self, kinds: &[RepairKind], items: &mut Vec<CandItem>) {
        let cfg = &self.engine.cfg;
        let Some(ix) = cfg.fuzzy_words.as_ref() else { return };
        let Some(conv) = self.st.conv.as_ref() else { return };
        if items.is_empty() || self.st.cursor == 0 {
            return;
        }
        let a = &self.engine.assets;
        let end = self.st.cursor;
        // plan 1.4: a changed legal position just before the cursor lists the characters of its typed reading (the way
        // back, whatever the dictionary has), after the normal list
        let pos = end - 1;
        if matches!(kinds.get(pos), Some(RepairKind::Chosen(_))) && self.usable(pos) {
            let t = self.st.syl[pos].clone();
            for c in self.repair_chars(&t) {
                let item = CandItem { text: c.to_string(), span: 1, repairs: vec![(pos, t.clone())], extra: vec![], raw: false, lit: false };
                if !items.iter().any(|i| same(i, &item)) {
                    items.push(item);
                }
            }
        }
        if !cfg.word_candidates {
            return;
        }
        let max_len = cfg.max_span.min(fuzzy_cands::MAX_LEN);
        // the run of positions with a character ending at the cursor (as the normal word candidates)
        let mut first = end;
        while first > 0 && end - first < max_len && conv.chars.get(first - 1).copied().flatten().is_some() {
            first -= 1;
        }
        if end - first < 2 {
            return;
        }
        let eff = self.eff_syl_from_kinds(kinds.to_vec());
        let mut alts: Vec<Vec<(String, u8)>> = vec![vec![]; end];
        for j in first..end {
            // an illegal typed syllable is F1's: its reading is not varied here
            if self.usable(j) {
                alts[j] = ix.repairs_all(&eff[j]).iter().filter(|r| r.class != crate::fuzzy::Class::K).map(|r| (r.syl.clone(), r.class.cost())).collect();
            }
        }
        let hits = a.fuzzy_words(&eff, end, first, &alts, &FuzzyParams { penalty: fuzzy_cands::PENALTY, max_len });
        let used = |j: usize| match kinds.get(j) {
            Some(RepairKind::Chosen(g) | RepairKind::Auto(g)) => Some(g.clone()),
            _ => None,
        };
        let mut found: Vec<(CandItem, bool)> = vec![];
        for h in hits {
            let s = end - h.span;
            let repairs = (s..end).filter_map(|k| if k == h.at { Some((k, h.syl.clone())) } else { used(k).map(|g| (k, g)) }).collect();
            let item = CandItem { text: h.text, span: h.span, repairs, extra: vec![], raw: false, lit: false };
            if items.iter().any(|i| same(i, &item)) {
                continue; // already in the list
            }
            found.push((item, h.strong));
        }
        place(items, found, cfg.page_size.max(1));
    }
}

impl Session {
    /// AUTOCOMP-LIST (plan 5.1): completions of the end span when the list is opened at the end: the merged dictionary
    /// order, items with a text already in the list dropped, at most `ACLIST_CAP`; the first min(n, 4, ps - 1) at the end
    /// of the first page (the F2 insertion formula), the rest after the first page.
    pub(super) fn add_autocomp_list(&self, items: &mut Vec<CandItem>) {
        if !self.engine.cfg.autocomp_list || items.is_empty() || self.st.cursor != self.st.syl.len() || self.password || self.has_lit() {
            return;
        }
        let Some(conv) = self.st.conv.as_ref() else { return };
        let a = &self.engine.assets;
        let eff = self.eff_syl();
        let n = eff.len();
        if conv.chars.len() != n {
            return;
        }
        let excluded: Vec<bool> = (0..n).map(|j| !self.usable(j)).collect();
        let mut found: Vec<CandItem> = vec![];
        for c in a.autocomp_list_merged(conv, &eff, &excluded) {
            if n + c.extra.len() > MAX_SYLLABLES {
                continue;
            }
            if items.iter().chain(found.iter()).any(|i| i.text == c.word) {
                continue;
            }
            found.push(CandItem { text: c.word, span: c.k, repairs: vec![], extra: c.extra, raw: false, lit: false });
        }
        found.truncate(ACLIST_CAP);
        let ps = self.engine.cfg.page_size.max(1);
        let first = ACLIST_FIRST.min(found.len()).min(ps - 1);
        let rest = found.split_off(first);
        let i0 = ps.saturating_sub(first).min(items.len()).max(1);
        items.splice(i0..i0, found);
        let j0 = ps.max(i0 + first).min(items.len());
        items.splice(j0..j0, rest);
    }
}

/// AUTOCOMP-LIST: completions on the first page and in all.
const ACLIST_FIRST: usize = 4;
const ACLIST_CAP: usize = 9;

/// The identity of a list item (plan 1.2 dedup): text, span and the readings it sets.
fn same(a: &CandItem, b: &CandItem) -> bool {
    a.text == b.text && a.span == b.span && a.repairs == b.repairs
}

/// Plan 1.3 positions: the first min(`STRONG`, ps - 1) strong hits (A) at the end of the first page, after the current
/// item; the other hits in their order (B) after the first page and after A; `CAP` in all. `found`: the hits in order,
/// each with its strong flag.
fn place(items: &mut Vec<CandItem>, found: Vec<(CandItem, bool)>, ps: usize) {
    let ps = ps.max(1);
    let cap_a = fuzzy_cands::STRONG.min(ps - 1);
    let (mut strong, mut weak) = (vec![], vec![]);
    for (item, is_strong) in found {
        if is_strong && strong.len() < cap_a {
            strong.push(item);
        } else {
            weak.push(item);
        }
    }
    weak.truncate(fuzzy_cands::CAP.saturating_sub(strong.len()));
    let a = strong.len();
    let i0 = ps.saturating_sub(a).min(items.len()).max(1).min(items.len());
    items.splice(i0..i0, strong);
    let j0 = ps.max(i0 + a).min(items.len());
    items.splice(j0..j0, weak);
}

#[cfg(test)]
#[path = "fuzzy_words_tests.rs"]
mod tests;
