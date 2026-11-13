//! User lexicon data, queries and decoding integration.
use crate::{Assets, vocab::UNK};
use std::{collections::HashMap, sync::Arc};
pub const USER_MAX_LEN: usize = 8;
pub const DEFAULT_BONUS: f64 = 10.0;
pub const FORCE_MIN_LEN: usize = 3;
/// Whether user word `w` may place its k-th character at position `i`: the legal set, or its own custom reading there
/// (USERLEX 1.6; only reached for a hit, i.e. the word's full reading matches).
pub(crate) fn user_char_ok(sets: &[Vec<u32>], i: usize, w: &UserWord, k: usize) -> bool {
    sets[i].binary_search(&w.ids[k]).is_ok() || w.is_custom(k)
}
#[derive(Clone, Debug, PartialEq)]
pub struct UserWord {
    pub ids: Vec<u32>,
    /// The entry's weight; `None` = omitted in the lexicon file (the deployment default applies, USERLEX 1.3).
    pub bonus: Option<f64>,
    /// USERLEX 1.6: bit k set = position k pairs its character with a syllable outside the legal table (a custom reading,
    /// allowed only inside this word's matched span).
    pub custom: u8,
}
impl UserWord {
    pub fn is_custom(&self, k: usize) -> bool {
        k < 8 && self.custom & (1 << k) != 0
    }
}
#[derive(Default, Debug)]
pub struct UserLexicon {
    words: HashMap<Vec<u16>, Vec<UserWord>>,
}
impl UserLexicon {
    pub fn len(&self) -> usize {
        self.words.values().map(Vec::len).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
    pub fn lookup(&self, codes: &[u16]) -> &[UserWord] {
        self.words.get(codes).map(Vec::as_slice).unwrap_or(&[])
    }
    /// AUTOCOMP: user words whose reading starts with `prefix` and has 1..=`max_extra` more syllables: (extra codes, word),
    /// in key order (a scan: user lexicons are small).
    pub fn prefix_words(&self, prefix: &[u16], max_extra: usize) -> Vec<(Vec<u16>, &UserWord)> {
        let mut out: Vec<(Vec<u16>, &UserWord)> = self.words.iter()
            .filter(|(k, _)| k.len() > prefix.len() && k.len() - prefix.len() <= max_extra && k.starts_with(prefix))
            .flat_map(|(k, ws)| ws.iter().map(move |w| (k[prefix.len()..].to_vec(), w)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.ids.cmp(&b.1.ids)));
        out
    }
    pub fn hits(&self, codes: &[Option<u16>], min_len: usize) -> Vec<(usize, &UserWord)> {
        let mut out = vec![];
        for start in 0..codes.len() {
            let key: Vec<u16> = codes[start..]
                .iter()
                .take(USER_MAX_LEN)
                .map_while(|x| *x)
                .collect();
            for len in (min_len.max(1)..=key.len()).rev() {
                out.extend(self.lookup(&key[..len]).iter().map(|w| (start, w)));
            }
        }
        out
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserWordError {
    Length,
    UnknownChar(u8),
    IllegalSyllable(u8),
    IllegalChar(u8),
    Weight,
}
impl Assets {
    /// Build in-memory assets with the same vocabulary check as the file loader.
    pub fn from_dictionary_model(data: &str, aliases: &str, model: crate::n1::N1) -> Result<Self, String> {
        let mut assets = Self::from_dictionary_text(data, aliases)?;
        if model.vocab_len() != assets.vocab.len() {
            return Err("model and dictionary vocabulary sizes differ".into());
        }
        assets.model = Some(model);
        Ok(assets)
    }
    pub fn user_word(
        &self,
        word: &str,
        syllables: &[String],
        bonus: f64,
    ) -> Result<(Vec<u16>, UserWord), UserWordError> {
        self.user_word_opt(word, syllables, Some(bonus))
    }
    /// `user_word` with an omittable weight (USERLEX 1.3: `None` keeps "omitted" so the deployment default applies).
    pub fn user_word_opt(
        &self,
        word: &str,
        syllables: &[String],
        bonus: Option<f64>,
    ) -> Result<(Vec<u16>, UserWord), UserWordError> {
        let chars: Vec<char> = word.chars().collect();
        if !(1..=USER_MAX_LEN).contains(&chars.len()) || chars.len() != syllables.len() {
            return Err(UserWordError::Length);
        }
        if bonus.is_some_and(|b| !b.is_finite() || !(0.0..=30.0).contains(&b)) {
            return Err(UserWordError::Weight);
        }
        let mut codes = vec![];
        let mut ids = vec![];
        let mut custom = 0u8;
        for (i, (&c, s)) in chars.iter().zip(syllables).enumerate() {
            // USERLEX 1.6 (new rules only): the syllable must be in the model's syllable table and the pair need not be
            // legal. With the rules off the old validation applies unchanged.
            let sid = self.vocab.syllable_id(s);
            if !(if self.user_force { sid != UNK } else { self.is_legal_syllable(s) }) {
                return Err(UserWordError::IllegalSyllable(i as u8));
            }
            let id = self.vocab.char_id(c);
            if id == UNK {
                return Err(UserWordError::UnknownChar(i as u8));
            }
            if !self.is_legal_char(s, c) {
                if !self.user_force {
                    return Err(UserWordError::IllegalChar(i as u8));
                }
                custom |= 1 << i;
            }
            codes.push(u16::try_from(sid).map_err(|_| UserWordError::IllegalSyllable(i as u8))?);
            ids.push(id);
        }
        Ok((codes, UserWord { ids, bonus, custom }))
    }
    pub fn build_user_lexicon(
        &self,
        words: impl IntoIterator<Item = (Vec<u16>, UserWord)>,
    ) -> UserLexicon {
        let mut lex = UserLexicon::default();
        for (key, word) in words {
            let v = lex.words.entry(key).or_default();
            if let Some(old) = v.iter_mut().find(|x| x.ids == word.ids) {
                old.bonus = word.bonus;
            } else if v.len() < 32 {
                v.push(word);
            }
        }
        lex
    }
    /// USERLEX deployment rules: `force` = user words of >= 3 characters win when their full reading is typed (1.1);
    /// `bonus2` = bonus of an entry whose weight was omitted (1.3, 0..=30). `(false, 10)` = the old behaviour bit for bit.
    pub fn set_user_rules(&mut self, force: bool, bonus2: f64) -> Result<(), UserWordError> {
        if !bonus2.is_finite() || !(0.0..=30.0).contains(&bonus2) {
            return Err(UserWordError::Weight);
        }
        self.user_force = force;
        self.user_bonus2 = bonus2;
        Ok(())
    }
    pub fn user_rules(&self) -> (bool, f64) {
        (self.user_force, self.user_bonus2)
    }
    pub(crate) fn user_bonus(&self, w: &UserWord) -> f64 {
        w.bonus.unwrap_or(self.user_bonus2)
    }
    /// USERLEX 1.1: the forced spans for these real locks. Hits of >= 3 characters whose every character is legal at its
    /// position and fits the real lock there (台/臺 fold as for jumps; the locked id is kept); chosen longest first, then
    /// earliest start, then lexicon order (first added first), never overlapping. Empty when the rules are off.
    pub(crate) fn forced_spans(&self, syllables: &[String], sets: &[Vec<u32>], real: &[Option<u32>]) -> Vec<(usize, Vec<u32>)> {
        if !self.user_force {
            return vec![];
        }
        let Some(lex) = self.user_snapshot().filter(|l| !l.is_empty()) else { return vec![] };
        let codes = self.syl_codes(syllables);
        let fold = self.norm.out.then(|| (self.vocab.char_id(crate::norm::TAI_TRAD), self.vocab.char_id(crate::norm::TAI)));
        let folded = |id| match fold { Some((trad, tai)) if id == trad => tai, _ => id };
        // (length, start, order within the reading, ids written)
        let mut cands: Vec<(usize, usize, usize, Vec<u32>)> = vec![];
        let mut order_at: HashMap<(usize, usize), usize> = HashMap::new();
        for (start, word) in lex.hits(&codes, FORCE_MIN_LEN) {
            let len = word.ids.len();
            let order = order_at.entry((start, len)).or_insert(0);
            let rank = *order;
            *order += 1;
            if start + len > sets.len() {
                continue;
            }
            let mut ids = Vec::with_capacity(len);
            let ok = word.ids.iter().enumerate().all(|(k, &id)| {
                let i = start + k;
                if !user_char_ok(sets, i, word, k) {
                    return false;
                }
                match real[i] {
                    Some(lk) if folded(lk) == folded(id) => { ids.push(lk); true }
                    Some(_) => false,
                    None => { ids.push(id); true }
                }
            });
            if ok {
                cands.push((len, start, rank, ids));
            }
        }
        cands.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut taken = vec![false; sets.len()];
        let mut out = vec![];
        for (len, start, _, ids) in cands {
            if taken[start..start + len].iter().any(|&t| t) {
                continue;
            }
            taken[start..start + len].fill(true);
            out.push((start, ids));
        }
        out.sort_by_key(|x| x.0);
        out
    }
    /// USERLEX 1.6: the id of the lock at `i` when it is a custom-reading character of a user word whose full reading
    /// matches a span covering `i` (with that character at that offset); None otherwise.
    pub(crate) fn custom_lock(&self, syllables: &[String], locks: &[Option<char>], i: usize) -> Option<u32> {
        if !self.user_force {
            return None;
        }
        let c = locks.get(i).copied().flatten()?;
        let lex = self.user_snapshot().filter(|l| !l.is_empty())?;
        let id = self.vocab.char_id(c);
        if id == UNK {
            return None;
        }
        let lo = i.saturating_sub(USER_MAX_LEN - 1);
        let codes = self.syl_codes(&syllables[lo..(i + USER_MAX_LEN).min(syllables.len())]);
        lex.hits(&codes, 1).into_iter().any(|(st, w)| {
            let st = lo + st;
            st <= i && i < st + w.ids.len() && w.ids[i - st] == id && w.is_custom(i - st)
        }).then_some(id)
    }
    pub fn set_user(&self, lex: Option<Arc<UserLexicon>>) {
        *self.user.write().unwrap() = lex.filter(|x| !x.is_empty());
    }
    pub(crate) fn user_snapshot(&self) -> Option<Arc<UserLexicon>> {
        self.user.read().unwrap().clone()
    }
    pub fn char_readings(&self, c: char) -> Vec<String> {
        let id = self.vocab.char_id(c);
        if id == UNK {
            return vec![];
        }
        let mut readings: Vec<(String, f64)> = self
            .legal
            .iter()
            .filter(|(_, ids)| ids.binary_search(&id).is_ok())
            .map(|(s, _)| {
                let score = self
                    .dict
                    .lookup(&[s.clone()])
                    .iter()
                    .find(|e| e.word == c.to_string())
                    .map(|e| e.log10)
                    .unwrap_or(f64::NEG_INFINITY);
                (s.clone(), score)
            })
            .collect();
        readings.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        readings.into_iter().map(|x| x.0).collect()
    }
    pub fn dict_readings(&self, word: &str) -> Vec<String> {
        if word.is_empty() || word.chars().count() > USER_MAX_LEN {
            return vec![];
        }
        self.dict.readings_for_word(word)
    }
}
impl Assets {
    /// Only enter the jump beam when at least one user word survives legality and locks.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn user_select(&self, syllables: &[String], sets: &[Vec<u32>], scores: &[Vec<f32>], locked: &[Option<u32>], lambda: f64, pn: Option<&crate::pn::PnCfg>, cache: Option<crate::cache::CacheBonus<'_>>, forced: &[bool], sp: &crate::convert::ShortView<'_>) -> Option<(Vec<u32>, Vec<bool>)> {
        let lex = self.user_snapshot()?;
        if lex.is_empty() || sets.iter().zip(locked).any(|(s, lk)| lk.is_none() && s.is_empty()) {
            return None;
        }
        let codes = self.syl_codes(syllables);
        let fold = self.norm.out.then(|| (self.vocab.char_id(crate::norm::TAI_TRAD), self.vocab.char_id(crate::norm::TAI)));
        let folded = |id| match fold { Some((trad, tai)) if id == trad => tai, _ => id };
        let mut user = vec![];
        for (start, word) in lex.hits(&codes, 2) {
            let mut ids = word.ids.clone();
            let ok = ids.iter_mut().enumerate().all(|(k, id)| {
                let i = start + k;
                match locked[i] {
                    Some(lk) if folded(lk) == folded(*id) => { *id = lk; true }
                    Some(_) => false,
                    None => user_char_ok(sets, i, word, k),
                }
            });
            if ok {
                user.push(crate::pn::Jump { start, ids, bonus: self.user_bonus(word) });
            }
        }
        if user.is_empty() {
            return None;
        }
        let lp = crate::pn::log_softmax(scores);
        let (cs, ls): (Vec<Vec<u32>>, Vec<Vec<f64>>) = scores.iter().zip(sets).zip(locked)
            .map(|((sc, ids), lk)| match lk {
                Some(id) => (vec![*id], vec![0.0]),
                None => crate::prior::top_k(sc, ids),
            }).unzip();
        let mut jumps = self.merged_jumps(syllables, sets, &lp, locked, forced, pn);
        jumps.extend(user.iter().cloned());
        if sp.has_words() {
            jumps.extend(self.short_word_jumps(syllables, sets, &lp, locked, forced, sp));
        }
        jumps.sort_by(|a, b| a.start.cmp(&b.start).then(b.ids.len().cmp(&a.ids.len())).then_with(|| a.ids.cmp(&b.ids)));
        let best = match &self.prior {
            Some(pk) if lambda != 0.0 => crate::pn::beam_cached_ex(&cs, &ls, sets, &lp, locked, &jumps, &|i| sp.weight(lambda, i), sp.pre(), &|h, c| pk.logp(h, c), cache),
            _ => crate::pn::beam_cached(&cs, &ls, sets, &lp, locked, &jumps, 0.0, &|_, _| 0.0, cache),
        }?;
        let mut mask = vec![false; syllables.len()];
        for jump in user {
            let end = jump.start + jump.ids.len();
            if best[jump.start..end] == jump.ids {
                mask[jump.start..end].fill(true);
            }
        }
        Some((best, mask))
    }
}

#[cfg(test)]
#[path = "user_tests.rs"]
mod decoding_tests;

#[cfg(test)]
mod user_tests {
    use super::*;
    fn assets() -> Assets {
        Assets::from_dictionary_text("ㄓˋ 志 -1\nㄓ 知 -2\nㄓˋ 知 -3\nㄇㄧㄥˊ 明 -1\nㄇㄧㄥˊ 名 -2\nㄓˋ-ㄇㄧㄥˊ 志明 -1\nㄓ-ㄇㄧㄥˊ 知名 -2\n", "").unwrap()
    }
    fn syl(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }
    #[test]
    fn validation_positions() {
        let a = assets();
        for (w, s, e) in [
            ("", "", UserWordError::Length),
            ("志明", "ㄓˋ", UserWordError::Length),
            ("志龘", "ㄓˋ ㄇㄧㄥˊ", UserWordError::UnknownChar(1)),
            ("志明", "ㄓˋ bad", UserWordError::IllegalSyllable(1)),
        ] {
            assert_eq!(a.user_word(w, &syl(s), 10.0), Err(e));
        }
        // Rules off (default): an illegal pair is still an error; USERLEX 1.6 rules on: a custom-reading position.
        assert_eq!(a.user_word("明志", &syl("ㄓˋ ㄇㄧㄥˊ"), 10.0), Err(UserWordError::IllegalChar(0)));
        let mut b = assets();
        b.set_user_rules(true, DEFAULT_BONUS).unwrap();
        assert_eq!(b.user_word("明志", &syl("ㄓˋ ㄇㄧㄥˊ"), 10.0).unwrap().1.custom, 0b11);
        for bonus in [f64::NAN, f64::INFINITY, -1.0, 31.0] {
            assert_eq!(
                a.user_word("志", &syl("ㄓˋ"), bonus),
                Err(UserWordError::Weight)
            );
        }
        assert!(a.user_word("志明", &syl("ㄓˋ ㄇㄧㄥˊ"), 30.0).is_ok());
    }
    #[test]
    fn hits_are_ordered_and_stop_at_none() {
        let a = assets();
        let x = a.user_word("志", &syl("ㄓˋ"), 1.0).unwrap();
        let y = a.user_word("志明", &syl("ㄓˋ ㄇㄧㄥˊ"), 2.0).unwrap();
        let z = a.user_word("知明", &syl("ㄓˋ ㄇㄧㄥˊ"), 3.0).unwrap();
        let (c, d) = (x.0[0], y.0[1]);
        let lex = a.build_user_lexicon([x, y, z]);
        let hits = lex.hits(&[Some(c), Some(d), None, Some(c), Some(d)], 1);
        assert_eq!(
            hits.iter().map(|(i, w)| (*i, w.bonus.unwrap())).collect::<Vec<_>>(),
            vec![(0, 2.0), (0, 3.0), (0, 1.0), (3, 2.0), (3, 3.0), (3, 1.0)]
        );
        assert_eq!(lex.hits(&[Some(c), None, Some(d)], 2).len(), 0);
        assert!(lex.hits(&[Some(c)], 9).is_empty());
    }
    #[test]
    fn dedup_limit_and_snapshots() {
        let a = assets();
        let key = vec![7];
        let mut words: Vec<_> = (0..34)
            .map(|i| {
                (
                    key.clone(),
                    UserWord {
                        ids: vec![i],
                        bonus: Some(1.0),
                        custom: 0,
                    },
                )
            })
            .collect();
        words.push((
            key.clone(),
            UserWord {
                ids: vec![0],
                bonus: Some(4.0),
                custom: 0,
            },
        ));
        let lex = a.build_user_lexicon(words);
        assert_eq!(lex.len(), 32);
        assert_eq!(lex.lookup(&key)[0].bonus, Some(4.0));
        a.set_user(Some(Arc::new(lex)));
        let snapshot = a.user_snapshot().unwrap();
        a.set_user(Some(Arc::new(UserLexicon::default())));
        assert!(a.user_snapshot().is_none());
        assert_eq!(snapshot.len(), 32);
    }
    #[test]
    fn readings_sort_and_dictionary_match() {
        let a = assets();
        assert_eq!(a.char_readings('知'), syl("ㄓ ㄓˋ"));
        assert_eq!(a.dict_readings("志明"), vec!["ㄓˋ ㄇㄧㄥˊ"]);
        assert!(a.dict_readings("明志").is_empty());
        assert!(a.char_readings('龘').is_empty());
        let a =
            Assets::from_dictionary_text("ㄓ 知 -2\nㄔ 知 -2\nㄓ-ㄓ 知知 -3\nㄔ-ㄔ 知知 -1\n", "")
                .unwrap();
        assert_eq!(a.char_readings('知'), vec!["ㄓ", "ㄔ"]);
        assert_eq!(a.dict_readings("知知"), vec!["ㄔ ㄔ", "ㄓ ㄓ"]);
    }
    #[test]
    fn eight_polyphonic_chars_find_the_256th_combination() {
        let word = "知知知知知知知知";
        let reading = vec!["ㄔ"; 8].join("-");
        let dict = format!("ㄓ 知 -1\nㄔ 知 -2\n{reading} {word} -3\n");
        let a = Assets::from_dictionary_text(&dict, "").unwrap();
        assert_eq!(a.char_readings('知'), vec!["ㄓ", "ㄔ"]);
        assert_eq!(a.dict_readings(word), vec![vec!["ㄔ"; 8].join(" ")]);
    }
}

#[cfg(test)]
#[path = "userlex_tests.rs"]
mod userlex_tests;
