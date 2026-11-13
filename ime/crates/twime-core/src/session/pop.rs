use super::*;
use twime_model::{autocomp::Completion, pop::CompletionLm};

/// Owned snapshot: safe to compute after releasing the engine's cross-connection gate.
pub struct PopSnapshot {
    lm: Arc<CompletionLm>,
    context: String,
    items: Vec<Completion>,
    q: usize,
}
impl PopSnapshot {
    pub fn compute(self) -> Vec<Completion> {
        let ranked = self.lm.rank(&self.context, self.items).unwrap_or_default();
        distinct_long_prefixes(ranked, self.q)
    }
}

/// Keep LM order, skipping extensions of already retained suffixes of at least two characters.
fn distinct_long_prefixes(ranked: Vec<Completion>, q: usize) -> Vec<Completion> {
    let mut kept: Vec<Completion> = Vec::with_capacity(q);
    for candidate in ranked {
        if kept.len() == q { break; }
        if kept.iter().any(|y| y.extra_chars.chars().count() >= 2
            && candidate.extra_chars.len() > y.extra_chars.len()
            && candidate.extra_chars.starts_with(&y.extra_chars)) {
            continue;
        }
        kept.push(candidate);
    }
    kept
}

#[cfg(test)]
mod dedupe_tests {
    use super::*;

    fn completion(suffix: &str) -> Completion {
        Completion { k: 2, word: format!("二氧{suffix}"), extra_chars: suffix.into(),
            extra: suffix.chars().map(|c| c.to_string()).collect(), user: false, score: -3., share: 0. }
    }
    fn suffixes(input: &[&str], q: usize) -> Vec<String> {
        distinct_long_prefixes(input.iter().map(|s| completion(s)).collect(), q)
            .into_iter().map(|c| c.extra_chars).collect()
    }
    #[test]
    fn skips_long_extensions_but_keeps_single_character_prefix() {
        assert_eq!(suffixes(&["化", "化碳", "化碳氣", "化碳計", "化碳法", "化硫"], 9),
            ["化", "化碳", "化硫"]);
    }
    #[test]
    fn one_character_does_not_exclude_longer_suffixes() {
        assert_eq!(suffixes(&["化", "化碳氣", "化硫"], 9), ["化", "化碳氣", "化硫"]);
    }
    #[test]
    fn fills_vacancies_in_rank_order_before_cap() {
        assert_eq!(suffixes(&["化碳", "化碳氣", "化硫", "化硫氣", "化氮", "化氫"], 3),
            ["化碳", "化硫", "化氮"]);
        assert!(suffixes(&["化碳"], 0).is_empty());
    }
    #[test]
    fn only_retained_true_prefixes_exclude_and_identities_survive() {
        let mut ranked: Vec<_> = ["化碳氣", "化碳", "化碳", "化硫氣"].iter().map(|s| completion(s)).collect();
        ranked[2].k = 3;
        ranked[2].user = true;
        ranked[2].extra = vec!["different-reading".into()];
        let expected = ranked.clone();
        assert_eq!(distinct_long_prefixes(ranked, 9), expected);
    }
}

impl Session {
    /// POP per-k capped pool, before UI filtering.
    pub fn completion_raw_pool(&self) -> Vec<Completion> {
        let Some(conv) = &self.st.conv else { return vec![] };
        let eff = self.eff_syl();
        let excluded: Vec<bool> = (0..self.st.syl.len()).map(|i| !self.usable(i)).collect();
        self.engine.assets.autocomp_pop_merged(conv, &eff, &excluded)
    }
    pub fn set_pop_peer(&mut self, enabled: bool) { self.pop_peer = enabled; }
    /// Cheap typing eligibility only: pool construction and UI filtering belong to prepare_idle.
    pub(super) fn pop_ready(&self) -> bool {
        self.pop_peer && self.engine.cfg.autocomp_pop && self.engine.cfg.autocomp_list
            && self.engine.assets.completion_lm.is_some() && self.engine.cfg.candidates
            && !self.password && self.st.cursor == self.st.syl.len() && self.st.syl.len() >= 2
            && self.st.syl.iter().rev().take(2).all(|s| self.engine.assets.is_legal_syllable(s))
            && self.st.typing.is_empty() && self.st.cand.is_none() && self.st.run.is_empty()
            && !self.st.symbols && self.mode() == Mode::Chinese && !self.has_lit() && self.committable()
    }
    pub(super) fn invalidate_pop(&mut self) {
        self.pop_gen = self.pop_gen.wrapping_add(1).max(1);
        self.pop_eligible = false;
        self.pop_result.clear();
    }
    /// UI rules are applied before LM ranking, with dictionary order defining cross-k text deduplication.
    /// Opening the ordinary list here does not change the visible state or pending host writes.
    pub fn completion_ui_pool(&mut self) -> Vec<Completion> {
        if self.st.cursor != self.st.syl.len() || !self.st.typing.is_empty() || self.st.cand.is_some() { return vec![]; }
        let raw = self.st.raw.clone();
        self.pop_opening = true;
        self.open_candidates();
        self.pop_opening = false;
        let ordinary = self.st.cand.take().map(|c| c.items).unwrap_or_default();
        self.st.raw = raw;
        if ordinary.is_empty() { return vec![]; }
        let Some(conv) = &self.st.conv else { return vec![] };
        let eff = self.eff_syl();
        let excluded: Vec<bool> = (0..self.st.syl.len()).map(|i| !self.usable(i)).collect();
        let mut found: Vec<Completion> = vec![];
        for c in self.engine.assets.autocomp_pop_merged(conv, &eff, &excluded) {
            if self.st.syl.len()+c.extra.len() <= MAX_SYLLABLES
                && !ordinary.iter().any(|i| i.text == c.word) && !found.iter().any(|i| i.word == c.word) {
                found.push(c);
            }
        }
        found
    }
    pub fn prepare_idle(&mut self, seq: u32, ack: Option<CommitResult>, generation: u32) -> Option<PopSnapshot> {
        let settings = self.settings_gen;
        self.sync_user_settings();
        self.apply_ack(ack);
        self.last_seq = seq;
        if settings != self.settings_gen { self.invalidate_pop(); }
        if generation == 0 || generation != self.pop_gen || !self.pop_eligible || !self.pop_ready() || self.lexux_hint().is_some() { return None; }
        let items = self.completion_ui_pool();
        if items.is_empty() { return None; }
        let context = format!("{}{}", self.context(), self.st.conv.as_ref()?.text()?);
        Some(PopSnapshot { lm: self.engine.assets.completion_lm.clone()?, context, items, q: self.engine.cfg.page_size.clamp(1, 9) })
    }
    pub fn finish_idle(&mut self, generation: u32, ranked: Vec<Completion>) -> Update {
        let settings = self.settings_gen;
        self.sync_user_settings();
        if settings != self.settings_gen { self.invalidate_pop(); }
        let mut update = self.update(false, None);
        update.hint = None;
        update.pop_gen = 0;
        if generation != 0 && generation == self.pop_gen && self.pop_eligible && self.pop_ready() && self.lexux_hint().is_none() && !ranked.is_empty() {
            update.completion = Some(twime_proto::CompletionHint { id: generation, options: ranked.iter().map(|c| c.extra_chars.clone()).collect() });
            self.pop_result = ranked;
        }
        update
    }
}
