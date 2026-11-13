//! Session-local LEXUX choices; they are real locks, never explicit cache confirmations.
use super::*;
use twime_model::lexux::{Choice, Hint};

#[derive(Clone)]
pub(super) struct Swap {
    pub group: u32,
    // Preserve per-option readings in the frozen cycle for future FUZZY F2.
    pub(super) options: Vec<Choice>,
    pub(super) sources: Vec<String>,
    pub(super) index: usize,
    pub(super) auto: bool,
    /// The effective syllables of the current option (validated against the span).
    pub(super) syllables: Vec<String>,
    /// FUZZY F2 step 2 (plan 5.1): each option's full reading of the span, frozen when the record is made.
    pub(super) readings: Vec<Vec<String>>,
    /// The span's `repair` values when the record was made (restored by options that read like option 0).
    pub(super) repair0: Vec<Option<String>>,
    /// The original span length (AUTOCOMP options append syllables after it).
    pub(super) k: usize,
    /// AUTOCOMP 1.3: a completion followed by a new syllable at the group's end; out of the cycle, still a group.
    pub(super) settled: bool,
}

/// The changed syllables of a reading against option 0's, for the hint label (`檔案（ㄉㄤˇ）`).
fn reading_label(display: &str, reading: &[String], zero: &[String]) -> String {
    let diff: Vec<&str> = reading.iter().zip(zero).filter(|(a, b)| a != b).map(|(a, _)| a.as_str()).collect();
    if diff.is_empty() { display.to_string() } else { format!("{display}（{}）", diff.join("、")) }
}

impl Session {
    fn lexux_config(&self) -> Option<twime_model::lexux::Config> {
        if self.lexux_peer {
            self.engine.cfg.lexux
        } else {
            None
        }
    }

    pub(super) fn apply_lexux(&mut self) {
        if let Some(c) = &self.st.conv {
            self.st.conv = Some(self.engine.assets.apply_lexux(
                c,
                self.lexux_config(),
                self.st.recent_output,
            ));
        }
        self.add_fuzzy_hints();
        self.add_autocomp_hints();
        // Locks constrain inference with legal source glyphs. Restore the frozen
        // presentation afterwards, including NORM folds absent from legal sets.
        if let Some(c) = &mut self.st.conv {
            for r in &self.st.swaps {
                if let Some(start) = self.st.groups.iter().position(|&g| g == r.group) {
                    for (k, ch) in r.options[r.index].display.chars().enumerate() {
                        c.chars[start + k] = Some(ch);
                    }
                }
            }
        }
    }

    /// FUZZY F2 step 2 (plan 5.2): fuzzy options on the output after LEXUX. A span identical to a LEXUX hint gets the
    /// fuzzy option appended (at most 3 options); a span crossing a LEXUX hint or replacement is dropped.
    fn add_fuzzy_hints(&mut self) {
        let cfg = &self.engine.cfg;
        if !cfg.fuzzy_hints || self.lexux_config().is_none() || self.st.recent_output {
            return;
        }
        let Some(ix) = cfg.fuzzy_words.clone() else { return };
        let Some(conv) = self.st.conv.as_ref() else { return };
        let a = &self.engine.assets;
        let eff = self.eff_syl();
        let n = eff.len();
        if conv.chars.len() != n {
            return;
        }
        let mut alts: Vec<Vec<(String, u8)>> = vec![vec![]; n];
        let mut excluded = vec![false; n];
        for j in 0..n {
            if self.usable(j) {
                alts[j] = ix.repairs_all(&eff[j]).iter().filter(|r| r.class != crate::fuzzy::Class::K).map(|r| (r.syl.clone(), r.class.cost())).collect();
            } else {
                excluded[j] = true; // F1's position
            }
        }
        let hints = a.fuzzy_hints(conv, &eff, &alts, &excluded);
        if hints.is_empty() {
            return;
        }
        let conv = self.st.conv.as_mut().expect("conversion");
        let cross = |s: usize, n: usize, t: usize, m: usize| s < t + m && t < s + n;
        for h in hints {
            let opt = Choice { display: h.word.clone(), reading: Some(h.reading.clone()), extra: vec![] };
            if let Some(x) = conv.hints.iter_mut().find(|x| x.start == h.start && x.len == h.len) {
                if x.options.len() < 3 && !x.options.iter().any(|o| o.display == h.word) {
                    x.options.push(opt);
                }
                continue;
            }
            if conv.hints.iter().any(|x| cross(x.start, x.len, h.start, h.len)) || conv.auto.iter().any(|&(s, m)| cross(s, m, h.start, h.len)) {
                continue;
            }
            let zero: String = conv.chars[h.start..h.start + h.len].iter().map(|c| c.expect("character")).collect();
            conv.hints.push(Hint { start: h.start, len: h.len, options: vec![Choice { display: zero, reading: None, extra: vec![] }, opt], auto: false });
        }
        conv.hints.sort_by_key(|h| (h.start, h.len));
    }

    /// AUTOCOMP (plan 1.2): completions of the end span. A span identical to a hint gets the completions appended (existing
    /// options first, at most 3); a span crossing a hint or a LEXUX replacement gets none.
    fn add_autocomp_hints(&mut self) {
        let Some(p) = self.engine.cfg.autocomp else { return };
        if self.lexux_config().is_none() || self.st.recent_output || self.password {
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
        let comps: Vec<_> = a.autocomp(conv, &eff, &excluded, &p).into_iter().filter(|c| n + c.extra.len() <= MAX_SYLLABLES).collect();
        let Some(k) = comps.first().map(|c| c.k) else { return };
        let start = n - k;
        let conv = self.st.conv.as_mut().expect("conversion");
        let opts: Vec<Choice> = comps.into_iter().map(|c| Choice { display: c.word, reading: None, extra: c.extra }).collect();
        if let Some(x) = conv.hints.iter_mut().find(|x| x.start == start && x.len == k) {
            for o in opts {
                if x.options.len() < 3 && !x.options.iter().any(|y| y.display == o.display) {
                    x.options.push(o);
                }
            }
            return;
        }
        let cross = |s: usize, m: usize| s < start + k && start < s + m;
        if conv.hints.iter().any(|x| cross(x.start, x.len)) || conv.auto.iter().any(|&(s, m)| cross(s, m)) {
            return;
        }
        let zero: String = conv.chars[start..n].iter().map(|c| c.expect("character")).collect();
        let mut options = vec![Choice { display: zero, reading: None, extra: vec![] }];
        options.extend(opts);
        conv.hints.push(Hint { start, len: k, options, auto: false });
        conv.hints.sort_by_key(|h| (h.start, h.len));
    }

    /// PRED plan 3 (offline tools): every unblocked k's candidate list (rule-5 order, at most `top`), under the same
    /// no recent-output context, not a password field, cursor at the end. Unlike displayed hints, this offline
    /// raw-pool observation is independent of whether LEXUX is enabled.
    pub fn autocomp_lists_now(&self, top: usize) -> Vec<(usize, Vec<twime_model::autocomp::Completion>)> {
        if self.st.recent_output || self.password || self.st.cursor != self.st.syl.len() {
            return vec![];
        }
        let Some(conv) = self.st.conv.as_ref() else { return vec![] };
        let a = &self.engine.assets;
        let eff = self.eff_syl();
        let n = eff.len();
        if conv.chars.len() != n {
            return vec![];
        }
        let excluded: Vec<bool> = (0..n).map(|j| !self.usable(j)).collect();
        a.autocomp_lists(conv, &eff, &excluded).into_iter().map(|(k, l)| {
            (k, l.into_iter().filter(|c| n + c.extra.len() <= MAX_SYLLABLES).take(top).collect::<Vec<_>>())
        }).filter(|(_, l)| !l.is_empty()).collect()
    }

    /// AUTOCOMP 1.3: a new syllable typed at the end settles a completion record ending there.
    pub(super) fn settle_completions_at_end(&mut self) {
        let n = self.st.syl.len();
        let groups = self.st.groups.clone();
        for r in &mut self.st.swaps {
            if r.settled || r.options[r.index].extra.is_empty() {
                continue;
            }
            if groups.iter().rposition(|&g| g == r.group).is_some_and(|e| e + 1 == n) {
                r.settled = true;
            }
        }
    }

    pub(super) fn validate_swaps(&mut self) {
        if self.st.swaps.is_empty() {
            return;
        }
        let eff = self.eff_syl();
        self.validate_swaps_with(&eff);
    }

    pub(super) fn validate_swaps_with(&mut self, eff: &[String]) {
        let invalid: Vec<u32> = self
            .st
            .swaps
            .iter()
            .filter_map(|r| {
                let pos: Vec<usize> = self
                    .st
                    .groups
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &g)| (g == r.group).then_some(i))
                    .collect();
                let valid = pos.len() == r.syllables.len()
                    && !pos.is_empty()
                    && pos.windows(2).all(|w| w[1] == w[0] + 1)
                    && eff[pos[0]..pos[0] + pos.len()] == r.syllables;
                (!valid).then_some(r.group)
            })
            .collect();
        for g in invalid {
            if let Some(i) = self.st.groups.iter().position(|&x| x == g) {
                self.unlock_group_of(i);
            } else {
                self.st.swaps.retain(|r| r.group != g);
            }
        }
    }

    /// Reuse inference only when its effective syllables still match.
    pub(super) fn reselect(&mut self) {
        self.validate_swaps();
        let eff = self.eff_syl();
        match &self.st.conv {
            Some(c) if c.syllables() == eff.as_slice() => {
                self.st.conv = Some(self.engine.assets.rebeam_ex(
                    c,
                    &self.st.locks,
                    self.engine.cfg.lambda,
                    self.st.style_tai,
                ));
                self.apply_lexux();
            }
            _ => self.reconvert(),
        }
    }

    pub(super) fn lexux_spans(&self) -> Vec<Hint> {
        // ZHUYIN-OUT 3.1: no hints while a position is literal bopomofo
        if self.lexux_config().is_none() || self.st.recent_output || self.has_lit() {
            return vec![];
        }
        let mut spans = self
            .st
            .conv
            .as_ref()
            .map_or_else(Vec::new, |c| c.hints.clone());
        // FUZZY F2 v8 (user 2026-10-28): while the cursor is at the end, the last syllable may still join the next one, so
        // fuzzy options of a span ending there are not shown (a span left with one option is dropped)
        let n = self.st.syl.len();
        let at_end = self.st.cursor == n;
        for h in &mut spans {
            if at_end && h.start + h.len == n {
                h.options.retain(|o| o.reading.is_none());
            }
            // AUTOCOMP: completions only while the cursor is at the end
            if !at_end {
                h.options.retain(|o| o.extra.is_empty());
            }
        }
        spans.retain(|h| h.options.len() >= 2);
        for r in self.st.swaps.iter().filter(|r| !r.settled) {
            if let Some(start) = self.st.groups.iter().position(|&g| g == r.group) {
                let mut options = r.options.clone();
                options.rotate_left(r.index);
                spans.push(Hint {
                    start,
                    len: r.syllables.len(),
                    options,
                    auto: r.auto,
                });
            }
        }
        spans
    }

    /// Acceptance tools (FUZZY F2 step 2, plan 5.3 S3): (start, len, index and text of the first option with its own reading) of
    /// every hint span holding a fuzzy option, left to right.
    pub fn fuzzy_hint_spans(&self) -> Vec<(usize, usize, usize, String)> {
        let mut v: Vec<(usize, usize, usize, String)> = self.lexux_spans().iter()
            .filter_map(|h| h.options.iter().position(|o| o.reading.is_some()).map(|i| (h.start, h.len, i, h.options[i].display.clone()))).collect();
        v.sort();
        v
    }

    /// Acceptance tools (AUTOCOMP plan 3): the visible completion group, if any: (start, k, [(cycle index, word, appended
    /// syllables)]).
    pub fn autocomp_hint_now(&self) -> Option<(usize, usize, Vec<(usize, String, Vec<String>)>)> {
        self.lexux_next()?;
        self.lexux_spans().into_iter().find_map(|h| {
            let c: Vec<(usize, String, Vec<String>)> = h.options.iter().enumerate().filter(|(_, o)| !o.extra.is_empty())
                .map(|(i, o)| (i, o.display.clone(), o.extra.clone())).collect();
            (!c.is_empty()).then_some((h.start, h.len, c))
        })
    }

    pub(super) fn active_lexux(&self, spans: &[Hint]) -> Option<usize> {
        let c = self.st.cursor;
        spans
            .iter()
            .enumerate()
            .filter(|(_, h)| h.start < c)
            .max_by_key(|(_, h)| h.start + h.len)
            .map(|(i, _)| i)
            .or_else(|| {
                spans
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| h.start >= c)
                    .min_by_key(|(_, h)| h.start)
                    .map(|(i, _)| i)
            })
    }

    pub(super) fn switchable(&self) -> bool {
        self.ux
            && self.lexux_config().is_some()
            && !self.password
            && self.mode() == Mode::Chinese
            && self.st.composing()
            && self.st.cand.is_none()
            && !self.st.symbols
            && self.st.typing.is_empty()
            && self.st.run.is_empty()
            && !self.has_lit()
            && self.committable()
    }

    pub(super) fn lexux_hint(&self) -> Option<twime_proto::LexuxHint> {
        self.lexux_next()?;
        let spans = self.lexux_spans();
        let h = &spans[self.active_lexux(&spans)?];
        let group = self.st.groups[h.start];
        let eff = self.eff_syl();
        let span = eff[h.start..h.start + h.len].to_vec();
        let (options, readings, current, auto) = if let Some(r) = self.st.swaps.iter().find(|r| r.group == group) {
            (r.options.clone(), r.readings.clone(), r.index, r.auto)
        } else {
            let readings = h.options.iter().map(|o| o.reading.clone().unwrap_or_else(|| span.clone())).collect();
            (h.options.clone(), readings, 0, h.auto)
        };
        // AUTOCOMP 1.3: a group of completions only does not list the original output
        let hide_first = !auto && options.len() >= 2 && options[1..].iter().all(|o| !o.extra.is_empty());
        Some(twime_proto::LexuxHint {
            options: options.iter().zip(&readings).map(|(o, r)| reading_label(&o.display, r, &readings[0])).collect(),
            current, auto, hide_first,
        })
    }

    pub(super) fn lexux_next(&self) -> Option<String> {
        if !self.switchable()
            || self.hint_mods.shift()
            || self.hint_mods.caps()
            || self.hint_mods.command()
        {
            return None;
        }
        let spans = self.lexux_spans();
        let i = self.active_lexux(&spans)?;
        spans[i].options.get(1).map(|o| o.display.clone())
    }

    pub(super) fn switch_lexux(&mut self) -> bool {
        let spans = self.lexux_spans();
        let Some(i) = self.active_lexux(&spans) else {
            return false;
        };
        let h = &spans[i];
        let g = self.st.groups[h.start];
        let start = h.start;
        let mut prev = 0usize;
        let r = if let Some(r) = self.st.swaps.iter_mut().find(|r| r.group == g) {
            prev = r.index;
            r.index = (r.index + 1) % r.options.len();
            r.syllables = r.readings[r.index].clone();
            r.clone()
        } else {
            let group = self.st.next_group;
            self.st.next_group = group.wrapping_add(1).max(1);
            self.st.explicit.retain(|&g| g != group);
            let span = self.eff_syl()[h.start..h.start + h.len].to_vec();
            // a completion reads as the span followed by its appended syllables
            let readings: Vec<Vec<String>> = h.options.iter().map(|o| {
                let mut r = o.reading.clone().unwrap_or_else(|| span.clone());
                r.extend(o.extra.iter().cloned());
                r
            }).collect();
            let r = Swap {
                group,
                // an option with its own reading (FUZZY F2) is legal for that reading as it stands
                sources: h.options.iter().map(|option| if option.reading.is_some() || !option.extra.is_empty() { option.display.clone() } else {
                    self.engine.assets.lexux_lock_source(
                        self.st.conv.as_ref().expect("conversion"), h.start, &option.display)
                }).collect(),
                options: h.options.clone(),
                index: 1,
                auto: h.auto,
                syllables: readings[1].clone(),
                repair0: (h.start..h.start + h.len).map(|k| self.st.repair.get(k).cloned().flatten()).collect(),
                readings,
                k: h.len,
                settled: false,
            };
            self.st.swaps.push(r.clone());
            r
        };
        // AUTOCOMP 1.3: first take back the previous option's appended syllables, then apply the target's
        let base = start + r.k;
        let m_prev = r.options[prev].extra.len();
        if m_prev > 0 {
            let end = (base + m_prev).min(self.st.syl.len());
            self.st.syl.drain(base..end);
            self.st.locks.drain(base..end);
            self.st.groups.drain(base..end);
            self.st.lit.drain(base..end);
            if self.st.repair.len() >= end {
                self.st.repair.drain(base..end);
            }
            if self.st.cursor > end {
                self.st.cursor -= end - base;
            } else if self.st.cursor > base {
                self.st.cursor = base;
            }
        }
        let extra = r.options[r.index].extra.clone();
        if !extra.is_empty() {
            for (j, syl) in extra.iter().enumerate() {
                self.st.syl.insert(base + j, syl.clone());
                self.st.locks.insert(base + j, None);
                self.st.groups.insert(base + j, 0);
                self.st.lit.insert(base + j, false);
                if self.st.repair.len() >= base + j {
                    self.st.repair.insert(base + j, None);
                }
            }
            self.st.cursor = base + extra.len();
        }
        // plan 5.1: the span's `repair` follows the option's reading; options reading like option 0 restore it
        let zero_k = r.readings[0][..r.k].to_vec();
        for k in 0..r.k {
            let i = start + k;
            let v = if r.readings[r.index][..r.k] == zero_k[..] { r.repair0[k].clone() }
                else if r.readings[r.index][k] == self.st.syl[i] { None } else { Some(r.readings[r.index][k].clone()) };
            if let Some(slot) = self.st.repair.get_mut(i) { *slot = v; }
        }
        for (k, ch) in r.sources[r.index].chars().enumerate() {
            self.st.locks[start + k] = Some(ch);
            self.st.groups[start + k] = r.group;
        }
        self.st.raw = None;
        self.reselect();
        true
    }
}
