//! NEUTRAL (R9-targets, NEUTRAL section): a typed neutral-tone syllable without a legal character (`ㄧㄝ˙`,
//! `ㄌㄧㄤ˙`, `ㄦ˙` in r8a/r9: the data has no character with that reading) is converted with one of the toned readings of
//! the same body instead of staying unconverted. It is the F1 `auto` repair restricted to the tone of a neutral syllable,
//! always on while F1 is off; with F1 on, F1 decides (its T class already reaches these syllables).
//!
//! The toned reading is chosen from the left: the longest dictionary word that ends at the position with the effective
//! syllables before it (爺爺 `ㄧㄝˊ ㄧㄝ˙` -> `ㄧㄝˊ-ㄧㄝˊ`, 漂亮 `ㄆㄧㄠˋ ㄌㄧㄤ˙` -> `ㄆㄧㄠˋ-ㄌㄧㄤˋ`), its log10 deciding
//! between readings of the same span; without such a word the reading with the most frequent single character. A user
//! word over the position, a literal position and the user's own choice from the candidate list win.
use super::*;

const NEUTRAL: char = '˙';
const TONES: [&str; 4] = ["", "ˊ", "ˇ", "ˋ"];

impl Session {
    /// `out_text`, but a position without a character gives its typed bopomofo (what `comp()` shows there). None without
    /// a conversion.
    pub(super) fn shown_text(&self) -> Option<String> {
        let conv = self.st.conv.as_ref()?;
        let mut t = String::new();
        for (i, c) in conv.chars.iter().enumerate() {
            match c {
                Some(ch) if self.st.lit.get(i) != Some(&true) => t.push(*ch),
                _ => t.push_str(self.st.syl.get(i)?),
            }
        }
        Some(t)
    }

    /// The legal toned readings of the body of an illegal neutral-tone syllable (first, second, third, fourth tone order);
    /// empty for anything else.
    pub(super) fn neutral_variants(&self, s: &str) -> Vec<String> {
        let a = &self.engine.assets;
        let Some(body) = s.strip_suffix(NEUTRAL) else { return vec![] };
        if body.is_empty() || body.contains(NEUTRAL) || a.is_legal_syllable(s) {
            return vec![];
        }
        TONES.iter().map(|t| format!("{body}{t}")).filter(|g| a.is_legal_syllable(g)).collect()
    }

    /// F1 off: every position that is still `Typed` with an illegal neutral-tone syllable that has a legal toned reading, not
    /// literal and not covered by a user word, becomes `Chosen` (the user's pick of one of those readings) or `Auto`.
    pub(super) fn neutral_kinds(&self, mut kinds: Vec<RepairKind>) -> Vec<RepairKind> {
        let st = &self.st;
        let todo: Vec<usize> = (0..st.syl.len())
            .filter(|&i| kinds.get(i) == Some(&RepairKind::Typed) && !st.lit[i] && st.syl[i].ends_with(NEUTRAL))
            .filter(|&i| !self.engine.assets.is_legal_syllable(&st.syl[i]))
            .collect();
        if todo.is_empty() {
            return kinds;
        }
        let cover = self.engine.assets.user_cover(&self.syl_view());
        for i in todo {
            if cover.get(i) == Some(&true) {
                continue;
            }
            let vars = self.neutral_variants(&st.syl[i]);
            if vars.is_empty() {
                continue;
            }
            if let Some(g) = st.repair.get(i).cloned().flatten().filter(|g| vars.contains(g)) {
                kinds[i] = RepairKind::Chosen(g);
                continue;
            }
            let g = self.neutral_pick(i, &vars, &kinds);
            kinds[i] = RepairKind::Auto(g);
        }
        kinds
    }

    /// The toned reading for position `i` (see the module comment); `vars` is not empty.
    fn neutral_pick(&self, i: usize, vars: &[String], kinds: &[RepairKind]) -> String {
        let (st, a) = (&self.st, &self.engine.assets);
        let dict = a.dictionary();
        // the effective syllable of an earlier position; None where it has no legal reading (literal, unchosen, illegal)
        let eff = |j: usize| -> Option<String> {
            if st.lit[j] {
                return None;
            }
            match &kinds[j] {
                RepairKind::Chosen(g) | RepairKind::Auto(g) => Some(g.clone()),
                RepairKind::Typed if a.is_legal_syllable(&st.syl[j]) => Some(st.syl[j].clone()),
                _ => None,
            }
        };
        let best_of = |prev: &[String]| -> Option<&String> {
            let mut best: Option<(f64, &String)> = None;
            for v in vars {
                let mut r = prev.to_vec();
                r.push(v.clone());
                for e in dict.lookup(&r) {
                    if prev.is_empty() && e.word.chars().count() != 1 {
                        continue;
                    }
                    if best.is_none_or(|(b, _)| e.log10 > b) {
                        best = Some((e.log10, v));
                    }
                }
            }
            best.map(|(_, v)| v)
        };
        let max_span = self.engine.cfg.max_span.max(1).min(i + 1);
        for span in (2..=max_span).rev() {
            let Some(prev) = (i + 1 - span..i).map(eff).collect::<Option<Vec<String>>>() else { continue };
            if let Some(v) = best_of(&prev) {
                return v.clone();
            }
        }
        best_of(&[]).unwrap_or(&vars[0]).clone()
    }

    /// F1 off: the characters of the toned readings of the neutral-tone position `pos` other than `except` (the one in use),
    /// in tone order, at most `REPAIR_ITEMS`; empty with F1 on (its own repair list covers them) or for another position.
    pub(super) fn neutral_items(&self, pos: usize, except: &str) -> Vec<CandItem> {
        if self.engine.cfg.repair.is_some() {
            return vec![];
        }
        let mut out = vec![];
        for g in self.neutral_variants(&self.st.syl[pos]).into_iter().filter(|g| g != except) {
            for c in self.repair_chars(&g) {
                out.push(CandItem { text: c.to_string(), span: 1, repairs: vec![(pos, g.clone())], extra: vec![], raw: false, lit: false });
            }
        }
        out.truncate(REPAIR_ITEMS);
        out
    }
}
