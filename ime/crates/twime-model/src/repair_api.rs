//! FUZZY F1 (FUZZY-plan 3.6): the three read-only queries the illegal-syllable repair in `twime-core` needs.
//! Nothing here changes a conversion or a candidate list.

use crate::{Assets, user::USER_MAX_LEN};

impl Assets {
    /// The legal characters of a syllable, in character id order (empty for an illegal syllable).
    pub fn legal_chars(&self, syllable: &str) -> Vec<char> {
        self.legal.get(syllable).map_or(vec![], |ids| ids.iter().filter_map(|&id| self.vocab.char_of(id)).collect())
    }

    /// The prior's single-character count of `c` (None without a prior).
    pub fn prior_unigram(&self, c: char) -> Option<u32> {
        self.prior.as_ref().map(|p| p.unigram(self.vocab.char_id(c)))
    }

    /// Per position: is it covered by a user word whose full reading matches the typed syllables there (any length, custom
    /// readings included; the same lexicon snapshot, syllable codes and `UserLexicon::hits` as decoding)? All false
    /// without a user lexicon.
    pub fn user_cover(&self, syllables: &[String]) -> Vec<bool> {
        let mut cover = vec![false; syllables.len()];
        let Some(lex) = self.user_snapshot().filter(|l| !l.is_empty()) else { return cover };
        for (start, w) in lex.hits(&self.syl_codes(syllables), 1) {
            let end = (start + w.ids.len().min(USER_MAX_LEN)).min(cover.len());
            cover[start..end].fill(true);
        }
        cover
    }
}
