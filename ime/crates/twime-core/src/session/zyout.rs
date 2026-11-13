//! ZHUYIN-OUT (ZHUYIN-OUT-plan 3): bopomofo typed as it is, like the Microsoft New Phonetic IME. A position can be
//! *literal*: it shows and commits its typed syllable (`syl[i]`, tone after the symbols, no mark for the first tone) and
//! the converter sees it as a syllable without a legal character, exactly like an illegal syllable. A position becomes
//! literal by choosing the last item of the candidate list (`select_literal`) or, with one symbol and Space, by itself.
use super::*;

/// Private-use code point put before a literal syllable for the converter: no table has such a syllable, so it is unknown
/// to the model and has no legal character. It never leaves the engine (not in any `Update`).
const LIT_MARK: char = '\u{E000}';

/// The syllable a literal position is converted as.
pub(super) fn mark(s: &str) -> String {
    format!("{LIT_MARK}{s}")
}

impl Session {
    /// Is any position literal?
    pub(super) fn has_lit(&self) -> bool {
        self.st.lit.iter().any(|&l| l)
    }

    /// Position `i` is a legal syllable that is typed as it is (not literal, not repaired away): what the legality checks of
    /// the repair, F2, completion and emoji code ask.
    pub(super) fn usable(&self, i: usize) -> bool {
        !self.st.lit[i] && self.engine.assets.is_legal_syllable(&self.st.syl[i])
    }

    /// The typed syllables with every literal one replaced by its converter form (for the user-word cover).
    pub(super) fn syl_view(&self) -> Vec<String> {
        self.st.syl.iter().zip(&self.st.lit).map(|(s, &l)| if l { mark(s) } else { s.clone() }).collect()
    }

    /// Every position has a character or is literal.
    pub(super) fn converted(&self, c: &Conversion) -> bool {
        c.chars.iter().enumerate().all(|(i, ch)| ch.is_some() || self.st.lit.get(i) == Some(&true))
    }

    /// The text of the composition: the character of each position, the typed syllable at a literal one. None while a
    /// position has neither. The one source of the commit and of the shown text.
    pub(super) fn out_text(&self) -> Option<String> {
        let conv = self.st.conv.as_ref()?;
        let mut t = String::new();
        for (i, c) in conv.chars.iter().enumerate() {
            if self.st.lit.get(i) == Some(&true) {
                t.push_str(&self.st.syl[i]);
            } else {
                t.push((*c)?);
            }
        }
        Some(t)
    }

    /// 3.5: a lone symbol completed by Space stays as typed when no character reads so and F1 has no repair for it.
    pub(super) fn lone_symbol_stays(&self, s: &str) -> bool {
        !self.engine.assets.is_legal_syllable(s) && self.engine.cfg.repair.as_ref().is_none_or(|r| r.index.repairs(s).is_empty())
    }

    /// 3.2: the typed bopomofo of `pos` as the last item of the list.
    pub(super) fn add_literal(&self, pos: usize, items: &mut Vec<CandItem>) {
        items.push(CandItem { text: self.st.syl[pos].clone(), span: 1, repairs: vec![], extra: vec![], raw: false, lit: true });
    }

    /// 3.3: choosing the typed bopomofo of `pos`: the position leaves its selection group (and swap) and becomes literal;
    /// one new inference. It teaches nothing (no explicit group, no style).
    pub(super) fn select_literal(&mut self, pos: usize) {
        if pos >= self.st.lit.len() {
            return;
        }
        self.st.raw = None;
        self.unlock_group_of(pos);
        self.st.locks[pos] = None;
        self.st.groups[pos] = 0;
        self.st.lit[pos] = true;
        self.st.explicit.retain(|g| self.st.groups.contains(g));
        self.reconvert();
    }

    /// ZHUYIN-OUT evaluation (tests and diagnostics, never logged): whether each item of the open candidate list is the
    /// typed bopomofo.
    pub fn candidate_lit_flags(&self) -> Vec<bool> {
        self.st.cand.as_ref().map(|c| c.items.iter().map(|i| i.lit).collect()).unwrap_or_default()
    }

    /// The vectors parallel to the syllables have one entry each.
    #[cfg(test)]
    pub(super) fn assert_parallel(&self) {
        let (st, n) = (&self.st, self.st.syl.len());
        assert!(st.locks.len() == n && st.groups.len() == n && st.repair.len() == n && st.lit.len() == n, "parallel vectors out of step");
        assert!(st.cursor <= n);
    }
}
