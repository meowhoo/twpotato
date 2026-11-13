//! EMOJI (EMOJI-plan 3): emoji at the very end of the candidate list when the whole composition is the reading of
//! an emoji keyword (`愛心` -> ❤️). An emoji item is `raw`: choosing it commits its text and clears the composition (no
//! syllable stays, so no cursor inside the text is possible). The model, the other items and the protocol are untouched.
use super::*;
use twime_model::emoji::{CAP_MULTI, CAP_SINGLE};

/// Most syllables of a keyword (the table has none longer).
const EMOJI_MAX_SYLLABLES: usize = 6;

impl Session {
    /// Adds the emoji of the composition's reading to the end of `items` (when the list is opened at the end of a composition
    /// of 1-6 legal syllables typed as they are: no F1 repair and no F2 suggestion chosen, nothing half typed, no password
    /// field). An emoji whose text is already in the list is skipped.
    pub(super) fn add_emoji(&self, kinds: &[RepairKind], items: &mut Vec<CandItem>) {
        let Some(table) = self.engine.cfg.emoji.as_ref() else { return };
        let st = &self.st;
        let n = st.syl.len();
        if items.is_empty() || self.password || n == 0 || n > EMOJI_MAX_SYLLABLES || st.cursor != n || !st.typing.is_empty() {
            return;
        }
        if !(0..n).all(|i| self.usable(i)) || !kinds.iter().all(|k| *k == RepairKind::Typed) {
            return;
        }
        let cap = if n == 1 { CAP_SINGLE } else { CAP_MULTI };
        for e in table.lookup(&st.syl).into_iter().take(cap) {
            if !items.iter().any(|i| i.text == e) {
                items.push(CandItem { text: e.to_string(), span: 0, repairs: vec![], extra: vec![], raw: true, lit: false });
            }
        }
    }

    /// Chooses item `idx`: an emoji (`raw`) is committed as it is, any other item is selected (`select`).
    pub(super) fn select_or_commit(&mut self, idx: usize, explicit: bool, commit: &mut Option<String>) {
        let raw = self.st.cand.as_ref().and_then(|c| c.items.get(idx)).filter(|i| i.raw).map(|i| i.text.clone());
        match raw {
            Some(t) => self.commit_symbol(&t, commit),
            None => self.select(idx, explicit),
        }
    }

    /// Would number key `d` choose an emoji (so that the state before it has to be kept for a failed host write)?
    pub(super) fn digit_picks_raw(&self, d: u8) -> bool {
        let ps = self.engine.cfg.page_size.max(1);
        let Some(c) = self.st.cand.as_ref() else { return false };
        (1..=9).contains(&d) && d as usize <= ps && c.items.get(c.selected / ps * ps + d as usize - 1).is_some_and(|i| i.raw)
    }

    /// EMOJI evaluation (tests and diagnostics, never logged): whether each item of the open candidate list is an emoji.
    pub fn candidate_raw_flags(&self) -> Vec<bool> {
        self.st.cand.as_ref().map(|c| c.items.iter().map(|i| i.raw).collect()).unwrap_or_default()
    }
}
