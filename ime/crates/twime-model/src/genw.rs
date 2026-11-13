//! General-word layer, mode 0: words of the layer's lexicon (same file format as the PN
//! lexicon, `pn::build` / `PnLexicon`) are offered in the candidate list under their own cap; the automatic conversion
//! never reads the layer. A word that is also in data.txt or in the PN lexicon is shown once, as that word.

use std::sync::Arc;

use crate::convert::Assets;
use crate::pn::PnLexicon;

/// Cap of general-layer-only words in one candidate list (not counted in `word_cap` or the PN cap).
pub const CAND_CAP: usize = 20;
/// Longest general-layer word (characters).
pub const MAX_LEN: usize = 6;

impl Assets {
    /// Sets the general-word lexicon; `None` (or an empty lexicon) = the current behavior, bit for bit.
    pub fn set_gen(&mut self, lex: Option<Arc<PnLexicon>>) {
        self.min_log10 = self.dict.min_log10_by_len();
        self.genw = lex.filter(|l| !l.is_empty());
    }
    pub fn genw(&self) -> Option<&Arc<PnLexicon>> {
        self.genw.as_ref()
    }

    /// General-layer words of one exact reading with the log10 the candidate list uses (data.txt's when the word is
    /// there, else the smallest data.txt log10 of that length) and whether the word is only in this layer (not in
    /// data.txt, not in the PN lexicon).
    pub fn gen_words(&self, syllables: &[String]) -> Vec<(String, f64, bool)> {
        let Some(lex) = &self.genw else { return vec![] };
        let Some(codes): Option<Vec<u16>> = self.syl_codes(syllables).into_iter().collect() else { return vec![] };
        let dict = self.dict.lookup(syllables);
        let pn: Vec<String> = self.pn_words(syllables).into_iter().map(|x| x.0).collect();
        lex.lookup(&codes)
            .into_iter()
            .map(|(_, ids)| {
                let w: String = ids.iter().filter_map(|&i| self.vocab.char_of(i)).collect();
                match dict.iter().find(|e| e.word == w) {
                    Some(e) => (w, e.log10, false),
                    None => {
                        let only = !pn.contains(&w);
                        (w, self.min_log10.get(syllables.len()).copied().filter(|x| x.is_finite()).unwrap_or(-20.0), only)
                    }
                }
            })
            .collect()
    }
}
