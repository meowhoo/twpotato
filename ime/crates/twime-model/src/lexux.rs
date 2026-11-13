//! LEXUX v6: deterministic post-selection rules. No inference or beam is run here.
use std::{cmp::Ordering, collections::{BTreeMap, HashMap}};
use crate::{Assets, Conversion, convert::ShortView, pn, prior};

/// Previously unused class bit. Old lexicons (bit absent) are hint-only.
pub const JUMP: u8 = 0x80;
/// Distinguishes the new tier encoding from old PN-style class bytes.
pub const META: u8 = 0x40;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config { pub a: f64, pub b: Option<f64>, pub w: bool }
impl Config {
    pub fn parse(m: &HashMap<String, String>) -> Result<Option<Self>, &'static str> {
        let (Some(a), Some(b)) = (m.get("lexux_a"), m.get("lexux_b")) else { return Ok(None) };
        let a = a.parse::<f64>().map_err(|_| "lexux_a")?;
        let b = if b == "off" { None } else { Some(b.parse::<f64>().map_err(|_| "lexux_b")?) };
        let w = match m.get("lexux_w").map(String::as_str) { None | Some("off") => false, Some("on") => true, _ => return Err("lexux_w") };
        let c = Self { a, b, w };
        if !c.valid() { return Err("lexux thresholds") };
        Ok(Some(c))
    }
    pub fn valid(self) -> bool { self.a.is_finite() && self.b.is_none_or(|b| b.is_finite() && b <= self.a) }
    pub fn grid() -> Vec<Self> {
        let mut out = vec![];
        for b in [-2., 0., 1., 2., 4., 6., 8., 12.] {
            for gap in [0., 1., 2., 4., 6., 8.] {
                for w in [false, true] { out.push(Self { a: b + gap, b: Some(b), w }); }
            }
        }
        for a in [-2., 0., 1., 2., 4., 6., 8., 12.] { out.push(Self { a, b: None, w: false }); }
        out
    }
}
/// A frozen cycle option. Reading overrides are reserved for FUZZY F2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub display: String,
    /// None keeps the span's current effective syllables (all current options).
    /// Some supplies the option's syllables; applying them is future FUZZY F2 work.
    pub reading: Option<Vec<String>>,
    /// AUTOCOMP: syllables appended after the span (empty = none); `display` is then the whole word.
    pub extra: Vec<String>,
}
impl Choice {
    fn current(display: String) -> Self { Self { display, reading: None, extra: vec![] } }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint { pub start: usize, pub len: usize, pub options: Vec<Choice>, pub auto: bool }

#[derive(Clone)]
pub(crate) struct Selection {
    pub ids: Vec<u32>, pub blocked: Vec<bool>, pub display: Vec<Option<char>>,
    pub lambda: f64, pub short: prior::ShortCfg,
}
#[derive(Clone)]
struct Hit { s: usize, ids: Vec<u32>, class: u8 }
impl Hit { fn end(&self) -> usize { self.s + self.ids.len() } }
fn overlap(s: usize, n: usize, t: usize, m: usize) -> bool { s < t + m && t < s + n }
fn order(a: &(Hit, f64), b: &(Hit, f64)) -> Ordering {
    a.1.total_cmp(&b.1).then(b.0.ids.len().cmp(&a.0.ids.len())).then(a.0.s.cmp(&b.0.s)).then(a.0.ids.cmp(&b.0.ids))
}

/// Includes the replacement's three following prior terms and exactly the beam's F3 history.
fn score(y: &[u32], s: usize, x: &[u32], lp: &impl Fn(usize, u32) -> f64,
         weight: &impl Fn(usize) -> f64, pre: &[u32], logp: &impl Fn(&[u32], u32) -> f64) -> f64 {
    let mut seq = y.to_vec();
    seq[s..s + x.len()].copy_from_slice(x);
    let mut v = x.iter().enumerate().map(|(k, &id)| lp(s + k, id)).sum::<f64>();
    for i in s..(s + x.len() + 3).min(y.len()) {
        let w = weight(i);
        if w != 0.0 {
            let (hist, n) = prior::hist3(pre, &seq[..i]);
            v += w * logp(&hist[..n], seq[i]);
        }
    }
    v
}
fn zone(c: Config, h: &Hit, d: f64, word: bool) -> u8 {
    if d > c.a { 0 } else if c.b.is_some_and(|b| h.ids.len() >= 3 && h.class & JUMP != 0 && (d < b || (c.w && !word))) { 2 } else { 1 }
}

fn automatic(hits: &[Hit], initial: &[u32], cfg: Config, delta: &impl Fn(&[u32], &Hit) -> f64,
             is_word: &impl Fn(&[u32], &Hit) -> bool) -> (Vec<u32>, Vec<bool>, Vec<Hit>) {
    let mut y = initial.to_vec();
    let mut deleted = vec![false; hits.len()];
    let mut accepted = vec![];
    loop {
        let best = hits.iter().enumerate().filter(|(i, _)| !deleted[*i]).filter_map(|(_, h)| {
            let d = delta(&y, h);
            (zone(cfg, h, d, is_word(&y, h)) == 2).then(|| (h.clone(), d))
        }).min_by(order);
        let Some((h, _)) = best else { break };
        y[h.s..h.end()].copy_from_slice(&h.ids);
        for (i,x) in hits.iter().enumerate() { deleted[i] |= overlap(h.s,h.ids.len(),x.s,x.ids.len()); }
        accepted.push(h);
    }
    (y,deleted,accepted)
}

impl Assets {
    /// Preserve the legal beam source behind a frozen, normalized display option.
    pub fn lexux_lock_source(&self, conv: &Conversion, start: usize, display: &str) -> String {
        let sel = conv.selection.as_ref().expect("LEXUX selection");
        display.chars().enumerate().map(|(k, c)| {
            if self.is_legal_char(&conv.inf.syllables[start + k], c) { c }
            else {
                assert!(sel.display[start + k] == Some(c), "LEXUX frozen glyph");
                self.vocab.char_of(sel.ids[start + k]).expect("LEXUX source")
            }
        }).collect()
    }

    /// Diagnostics: class tier of each hint's first word (0..3; 255 when absent).
    pub fn lexux_hint_layers(&self, conv: &Conversion) -> Vec<u8> {
        conv.hints.iter().map(|h| {
            let word = &h.options[if h.auto { 0 } else { 1 }].display;
            let codes = self.syl_codes(&conv.inf.syllables[h.start..h.start + h.len]);
            self.genw.as_ref().and_then(|lex| lex.hits(&codes).into_iter().find_map(|(s,c,ids)| {
                let w: String = ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect();
                (s == 0 && &w == word).then_some(if c & META != 0 { c & 3 } else { 255 })
            })).unwrap_or(255)
        }).collect()
    }
    pub fn lexux_clean(&self, syllables: &[String], gold: &[char]) -> bool {
        self.genw.as_ref().is_none_or(|lex| !lex.hits(&self.syl_codes(syllables)).iter().any(|(s,_,ids)| {
            (2..=6).contains(&ids.len()) && ids.iter().enumerate().all(|(k,&id)| gold.get(s+k).copied() == self.vocab.char_of(id))
        }))
    }
    pub fn set_lexux(&mut self, cfg: Option<Config>) -> Result<(), &'static str> {
        if cfg.is_some_and(|c| !c.valid()) { return Err("lexux thresholds") }
        self.lexux = cfg; Ok(())
    }
    /// Apply another grid point to the saved selection; never decodes again.
    /// `recent_output` excludes SHORT-PRIOR B compositions.
    pub fn apply_lexux(&self, conv: &Conversion, cfg: Option<Config>, recent_output: bool) -> Conversion {
        let mut out = conv.clone();
        out.hints.clear(); out.auto.clear();
        let Some(sel) = &conv.selection else { return out };
        out.chars = sel.display.clone();
        let (Some(cfg), Some(lex)) = (cfg, &self.genw) else { return out };
        assert!(cfg.valid());
        if conv.fallback || recent_output || conv.inf.recent_output || conv.inf.sets.iter().any(Vec::is_empty) { return out }
        let codes = self.syl_codes(&conv.inf.syllables);
        let fold = |id| if self.norm.out && id == self.vocab_id(crate::norm::TAI_TRAD) { self.vocab_id(crate::norm::TAI) } else { id };
        let equal = |a: &[u32], b: &[u32]| a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y));
        let all = lex.hits(&codes);
        let pns = self.pn.as_ref().map(|(p, _)| p.hits(&codes)).unwrap_or_default();
        let mut blocked = sel.blocked.clone();
        for (s, _, ids) in &pns {
            if ids.len() >= 2 && equal(ids, &sel.ids[*s..s + ids.len()]) { blocked[*s..s + ids.len()].fill(true); }
        }
        let hits: Vec<Hit> = all.iter().filter(|(s, _, ids)| (2..=6).contains(&ids.len()) && !blocked[*s..s + ids.len()].iter().any(|x| *x)
            && !equal(ids, &sel.ids[*s..s + ids.len()])).map(|(s, class, ids)| {
                assert!(ids.iter().enumerate().all(|(k, id)| conv.inf.sets[s + k].binary_search(id).is_ok()), "lexux legality");
                Hit { s: *s, ids: ids.clone(), class: *class }
            }).collect();
        if hits.is_empty() { return out }
        let lp = pn::log_softmax(&conv.inf.scores);
        let sp: ShortView<'_> = self.short_view_of(sel.short, true, &conv.inf);
        let model_lp = |i: usize, id: u32| lp[i][conv.inf.sets[i].binary_search(&id).expect("lexux legal id")];
        let weight = |i| if self.prior.is_some() { sp.weight(sel.lambda, i) } else { 0.0 };
        let logp = |h: &[u32], id| self.prior.as_ref().map_or(0.0, |p| p.logp(h, id));
        let delta = |y: &[u32], h: &Hit| score(y, h.s, &y[h.s..h.end()], &model_lp, &weight, sp.pre(), &logp)
            - score(y, h.s, &h.ids, &model_lp, &weight, sp.pre(), &logp);
        let user = self.user_snapshot();
        let is_word = |y: &[u32], h: &Hit| {
            let part = &y[h.s..h.end()];
            all.iter().chain(pns.iter()).any(|(s, _, ids)| *s == h.s && equal(ids, part)) ||
            self.dict.lookup(&conv.inf.syllables[h.s..h.end()]).iter().any(|e| equal(&e.word.chars().map(|c| self.vocab_id(c)).collect::<Vec<_>>(), part)) ||
            user.as_ref().is_some_and(|u| u.hits(&codes, 2).iter().any(|(s, w)| *s == h.s && equal(&w.ids, part)))
        };
        let (y, deleted, accepted) = automatic(&hits, &sel.ids, cfg, &delta, &is_word);
        for h in accepted {
            out.auto.push((h.s, h.ids.len()));
            let word: String = h.ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect();
            let old: String = sel.display[h.s..h.end()].iter().map(|c| c.unwrap()).collect();
            for (k, c) in word.chars().enumerate() { out.chars[h.s + k] = Some(c); }
            if word != old { out.hints.push(Hint { start: h.s, len: h.ids.len(), options: vec![Choice::current(word), Choice::current(old)], auto: true }); }
        }
        let mut groups: BTreeMap<(usize, usize), Vec<(Hit, f64)>> = BTreeMap::new();
        for (i, h) in hits.iter().enumerate().filter(|(i, _)| !deleted[*i]) {
            let _ = i;
            let d = delta(&y, h);
            if zone(cfg, h, d, is_word(&y, h)) == 1 { groups.entry((h.s, h.ids.len())).or_default().push((h.clone(), d)); }
        }
        let mut candidates = vec![];
        for ((s, len), mut hs) in groups {
            hs.sort_by(order);
            let zero: String = out.chars[s..s + len].iter().map(|c| c.unwrap()).collect();
            let mut options = vec![Choice::current(zero)]; let mut first = None;
            for h in hs {
                let word: String = h.0.ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect();
                if options.iter().any(|o| o.display == word) { continue }
                if first.is_none() { first = Some(h.clone()); }
                options.push(Choice::current(word));
                if options.len() == 3 { break }
            }
            if let Some(key) = first { candidates.push((key, Hint { start: s, len, options, auto: false })); }
        }
        candidates.sort_by(|a, b| order(&a.0, &b.0));
        for (_, h) in candidates {
            if !out.auto.iter().any(|&(s, n)| overlap(s, n, h.start, h.len)) && !out.hints.iter().any(|x| overlap(x.start, x.len, h.start, h.len)) { out.hints.push(h); }
        }
        out.auto.sort(); out.hints.sort_by_key(|h| (h.start, h.len));
        for i in 0..out.chars.len() {
            assert!(out.auto.iter().any(|&(s, n)| s <= i && i < s + n) || out.chars[i] == sel.display[i], "lexux outside span");
        }
        out
    }
}

#[cfg(test)]
#[path = "lexux_tests.rs"]
mod tests;
