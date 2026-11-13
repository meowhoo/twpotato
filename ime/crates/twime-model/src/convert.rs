//! `Assets`: dictionary + vocabulary + legal table + weights (+ prior), and the conversion function.
//!
//! The model part replicates the `eval_n1` path of bench/s3_latency/src/bin/s5-eval.rs exactly (same encode,
//! `scores`, first-maximum greedy pick, `top_k` + `beam` for lambda != 0), so outputs are bit-identical to it when no
//! position is locked. New here: per-position locks (a locked position keeps its character; the beam sees only that
//! character there) and the single-character fallback of AI.md.

use std::{collections::HashMap, io, path::PathBuf, sync::Arc};

use crate::{
    lexicon::Dictionary,
    n1::{self, N1},
    norm::{self, NormCfg},
    prior::{self, Pack},
    vocab::Vocab,
};

/// Files the shell hands to the engine (N3-plan rule 3).
#[derive(Clone, Debug)]
pub struct ModelPaths {
    pub data_txt: PathBuf,
    pub aliases: PathBuf,
    pub model: PathBuf,
    pub prior: Option<PathBuf>,
}

pub struct Assets {
    pub completion_lm: Option<Arc<crate::pop::CompletionLm>>,
    pub(crate) lexux: Option<crate::lexux::Config>,
    pub(crate) user: std::sync::RwLock<Option<std::sync::Arc<crate::user::UserLexicon>>>,
    pub(crate) dict: Dictionary,
    pub(crate) vocab: Vocab,
    pub(crate) legal: HashMap<String, Vec<u32>>,
    pub(crate) model: Option<N1>,
    pub(crate) prior: Option<Pack>,
    pub(crate) extra: Option<Arc<dyn crate::cands::WordSource>>,
    /// Glyph normalization (NORM plan); all off by default = the old behaviour bit for bit.
    pub(crate) norm: NormCfg,
    /// Proper-name lexicon bonus (PN-plan); None = the old behaviour bit for bit.
    pub(crate) pn: Option<(Arc<crate::pn::PnLexicon>, crate::pn::PnCfg)>,
    /// Per-word bonus table (WORDBONUS-plan); None (or an empty table) = the old behaviour bit for bit.
    pub(crate) wordbonus: Option<Arc<crate::wordbonus::WordBonus>>,
    /// NORM-MO (NORM-MO-plan): the model sees the light-tone syllable ㄇㄛ˙ as ㄇㄜ˙ (the only one it was trained on); false = the old behaviour bit for bit.
    pub(crate) mo_norm: bool,
    /// General-word layer (SRCH-plan 4, mode 0: candidate list only); None = the old behaviour bit for bit.
    pub(crate) genj: Option<(Arc<crate::pn::PnLexicon>, crate::pn::genj::GenjCfg)>,
    pub(crate) genw: Option<Arc<crate::pn::PnLexicon>>,
    /// Smallest data.txt log10 per word length (index = characters), for lexicon-only candidate words.
    pub(crate) min_log10: Vec<f64>,
    /// CAND (CAND-plan): lexicon matches as model input; None = the old behaviour bit for bit.
    pub(crate) cand: Option<Arc<crate::cand::CandCfg>>,
    /// TA (TA-plan): ㄊㄚ variants of the automatic conversion become 他; off = the old behaviour bit for bit.
    pub(crate) ta_default: bool,
    /// USERLEX (USERLEX-plan 1): user words of >= 3 characters whose full reading is typed are forced; off = the
    /// old behaviour bit for bit.
    pub(crate) user_force: bool,
    /// USERLEX 1.3: bonus of a user word whose weight was omitted (the old fixed default unless the deployment sets it).
    pub(crate) user_bonus2: f64,
    /// SHORT-PRIOR stage A (SHORT-PRIOR-plan 1): lambda(c_i) and the F3 history; default = the old behaviour bit for bit.
    pub(crate) short: prior::ShortCfg,
    /// SHORT-PRIOR WB word weights (T2, `word_short_table`): key "reading|word" (syllables joined by '-'); None = T1 only.
    pub(crate) short_table: Option<Arc<HashMap<String, f64>>>,
    /// FUZZY F2 step 2 (FUZZY-F2-plan 5.2): median data.txt log10 per word length, computed on first use.
    pub(crate) fuzzy_tau: std::sync::OnceLock<Vec<f64>>,
    /// AUTOCOMP: data.txt syllable-prefix index, built on first use.
    pub(crate) autocomp_index: std::sync::OnceLock<HashMap<String, Vec<(String, String, f64)>>>,
}

/// What the beams need of SHORT-PRIOR for one inference: the settings, the context length d the model used and the F3
/// history prefix (empty with F3 off).
pub(crate) struct ShortView<'a> {
    cfg: prior::ShortCfg,
    d: usize,
    pre: &'a [u32],
    table: Option<&'a HashMap<String, f64>>,
}

impl ShortView<'_> {
    #[cfg(test)]
    pub(crate) const OFF: ShortView<'static> = ShortView { cfg: prior::ShortCfg { c: 0, l0: None, wb: 0.0, ctx: false }, d: 0, pre: &[], table: None };
    #[inline]
    pub(crate) fn weight(&self, lambda: f64, i: usize) -> f64 {
        self.cfg.weight(lambda, self.d, i)
    }
    pub(crate) fn pre(&self) -> &[u32] {
        self.pre
    }
    /// The prior term is the old one (no lambda schedule or lambda_0 = lambda_L, no F3): `cands.rs` keeps its old expression.
    pub(crate) fn prior_plain(&self, lambda: f64) -> bool {
        self.cfg.l0.is_none_or(|x| x == lambda) && !self.cfg.ctx
    }
    pub(crate) fn has_words(&self) -> bool {
        self.cfg.wb > 0.0 && self.d < self.cfg.c
    }
}

/// What one inference produced; shared (cheap to clone) by every `Conversion` derived from it with different locks.
pub(crate) struct Inference {
    pub(crate) recent_output: bool,
    pub(crate) syllables: Vec<String>,
    /// Per position: the full legal id set and, with the model, the score of each (same order).
    pub(crate) sets: Vec<Vec<u32>>,
    pub(crate) scores: Vec<Vec<f32>>,
    /// Fallback mode: the unlocked best single character of each syllable.
    pub(crate) base_fallback: Vec<Option<char>>,
    pub(crate) fallback: bool,
    /// The document bigram set this inference was made with (CACHE plan 2.3; `rebeam` keeps using it).
    pub(crate) cache: Option<Arc<crate::cache::CacheSet>>,
    /// SHORT-PRIOR: characters of context the model used (d, at most n1::CONTEXT; 0 in fallback mode) and the last <= 3
    /// in-vocabulary ids of that context (the F3 history; used only when the setting is on).
    pub(crate) ctx_len: usize,
    pub(crate) ctx_tail: Vec<u32>,
}

/// Result of one conversion. The inference part is kept so that a change of locks only needs `Assets::rebeam`.
#[derive(Clone)]
pub struct Conversion {
    pub hints: Vec<crate::lexux::Hint>,
    pub auto: Vec<(usize, usize)>,
    pub(crate) selection: Option<Arc<crate::lexux::Selection>>,
    /// Chosen character of each syllable; None where the syllable has no legal character.
    pub chars: Vec<Option<char>>,
    /// True if the single-character fallback was used instead of the model.
    pub fallback: bool,
    pub(crate) inf: Arc<Inference>,
    /// USERLEX 1.1: the forced user-word spans of this selection (start, ids), in position order.
    pub(crate) forced: Vec<(usize, Vec<u32>)>,
}

impl std::fmt::Debug for Conversion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conversion").field("chars", &self.chars).field("fallback", &self.fallback).finish()
    }
}

impl Conversion {
    /// All syllables have a character.
    pub fn complete(&self) -> bool {
        self.chars.iter().all(Option::is_some)
    }
    /// The text if every syllable has a character.
    pub fn text(&self) -> Option<String> {
        self.chars.iter().copied().collect()
    }
    /// The syllables this conversion was made for.
    pub fn syllables(&self) -> &[String] {
        &self.inf.syllables
    }
    /// Pairs of the document bigram set this conversion uses (None: no cache). Diagnostics and tests.
    pub fn cache_pairs(&self) -> Option<usize> {
        self.inf.cache.as_ref().map(|c| c.len())
    }
}

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl Assets {
    /// Enables W8A8 inference; missing or unsupported models are errors.
    pub fn set_int8(&mut self) -> io::Result<()> {
        self.model.as_mut().ok_or_else(|| bad("int8 requires a model".into()))?
            .quantize_int8().map_err(|e| bad(e.into()))
    }

    /// Loads dictionary, alias table, weights and (if given) the prior. A missing or mismatching weights file is an
    /// error; callers that want the AI.md fallback use `load_dictionary_only`.
    pub fn load(p: &ModelPaths) -> io::Result<Assets> {
        let mut a = Self::load_dictionary_only(p)?;
        let m = N1::load(&p.model)?;
        if m.vocab_len() != a.vocab.len() {
            return Err(bad(format!("model vocab {} != dictionary vocab {}", m.vocab_len(), a.vocab.len())));
        }
        a.model = Some(m);
        if let Some(pp) = &p.prior {
            a.prior = Some(Pack::load(pp)?);
        }
        Ok(a)
    }

    /// Dictionary and legal table only: conversions use the single-character fallback.
    pub fn load_dictionary_only(p: &ModelPaths) -> io::Result<Assets> {
        let (mut dict, _) = Dictionary::parse(&std::fs::read_to_string(&p.data_txt)?);
        dict.apply_aliases(&std::fs::read_to_string(&p.aliases)?).map_err(bad)?;
        let vocab = Vocab::new(&dict);
        let legal = n1::legal_table(&dict, &vocab);
        Ok(Assets { completion_lm: None, lexux: None, user: Default::default(), dict, vocab, legal, model: None, prior: None, extra: None, norm: NormCfg::default(), pn: None, genj: None, wordbonus: None, mo_norm: false, genw: None, min_log10: vec![], cand: None, ta_default: false, user_force: false, user_bonus2: crate::user::DEFAULT_BONUS, short: prior::ShortCfg::default(), short_table: None, fuzzy_tau: Default::default(), autocomp_index: Default::default() })
    }

    /// Dictionary text and alias table given directly (no files, no model): for tests and tiny setups.
    pub fn from_dictionary_text(data: &str, aliases: &str) -> Result<Assets, String> {
        let (mut dict, _) = Dictionary::parse(data);
        dict.apply_aliases(aliases)?;
        let vocab = Vocab::new(&dict);
        let legal = n1::legal_table(&dict, &vocab);
        Ok(Assets { completion_lm: None, lexux: None, user: Default::default(), dict, vocab, legal, model: None, prior: None, extra: None, norm: NormCfg::default(), pn: None, genj: None, wordbonus: None, mo_norm: false, genw: None, min_log10: vec![], cand: None, ta_default: false, user_force: false, user_bonus2: crate::user::DEFAULT_BONUS, short: prior::ShortCfg::default(), short_table: None, fuzzy_tau: Default::default(), autocomp_index: Default::default() })
    }

    /// `convert_ex` with the document bigram set `doc` (None or empty = `convert_ex` bit for bit). The set is kept in the
    /// conversion, so `rebeam*` with other locks uses it too.
    pub fn convert_cached(&self, context: &str, doc: Option<Arc<crate::cache::CacheSet>>, syllables: &[String], locks: &[Option<char>], lambda: f64, style_tai: bool) -> Conversion {
        let mut base = self.infer(context, syllables);
        if doc.is_some() {
            let inf = Arc::get_mut(&mut base.inf).expect("fresh inference");
            inf.cache = doc;
        }
        self.rebeam_ex(&base, locks, lambda, style_tai)
    }

    /// Select context source at composition start; recent-output compositions exclude LEXUX.
    pub fn convert_cached_source(&self, context: &str, doc: Option<Arc<crate::cache::CacheSet>>, syllables: &[String], locks: &[Option<char>], lambda: f64, style_tai: bool, recent_output: bool) -> Conversion {
        let mut base = self.infer(context, syllables);
        let inf = Arc::get_mut(&mut base.inf).expect("fresh inference");
        inf.cache = doc;
        inf.recent_output = recent_output;
        self.rebeam_ex(&base, locks, lambda, style_tai)
    }

    /// SHORT-PRIOR stage A settings (`ShortCfg::default()` = off). Invalid values are refused.
    pub fn set_short(&mut self, c: prior::ShortCfg) -> Result<(), String> {
        if !c.valid() {
            return Err("lambda_short0 / word_short_bonus must be finite numbers >= 0 and need lambda_short_c >= 1".into());
        }
        self.short = c;
        if self.min_log10.is_empty() {
            self.min_log10 = self.dict.min_log10_by_len();
        }
        Ok(())
    }

    /// SHORT-PRIOR WB (SHORT-PRIOR-plan 1): data.txt words of 2..=6 characters whose log10 is above the floor of
    /// their length, as jumps starting where alpha(d + s) > 0, bonus beta * alpha(d + s); legality and locks as PN jumps
    /// (`usable_jumps`, no H cap), never crossing a forced user-word span.
    pub(crate) fn short_word_jumps(&self, syllables: &[String], sets: &[Vec<u32>], lp: &[Vec<f64>], locked: &[Option<u32>], forced: &[bool], sp: &ShortView<'_>) -> Vec<crate::pn::Jump> {
        let n = syllables.len();
        let mut out = vec![];
        if !sp.has_words() {
            return out;
        }
        let fold = self.norm.out.then(|| (self.vocab.char_id(crate::norm::TAI_TRAD), self.vocab.char_id(crate::norm::TAI)));
        for st in 0..n {
            let bonus = sp.cfg.word_bonus(sp.d, st);
            if bonus <= 0.0 {
                break;
            }
            // weight: T1 = 1 above the floor of the length; T2 = the table's weight where it has the word (else T1)
            let mut hits: Vec<(f64, usize, Vec<u32>)> = vec![];
            for l in 2..=6usize.min(n - st) {
                if crate::pn::crosses_forced(forced, st, l) {
                    continue;
                }
                let floor = self.min_log10.get(l).copied().unwrap_or(f64::INFINITY);
                let reading = sp.table.map(|_| syllables[st..st + l].join("-"));
                for e in self.dict.lookup(&syllables[st..st + l]) {
                    if e.word.chars().count() != l {
                        continue;
                    }
                    let t1 = if e.log10 > floor { 1.0 } else { 0.0 };
                    let w = match (sp.table, &reading) {
                        (Some(t), Some(r)) => t.get(&format!("{r}|{}", e.word)).copied().unwrap_or(t1),
                        _ => t1,
                    };
                    if w > 0.0 {
                        hits.push((w, st, e.word.chars().map(|c| self.vocab.char_id(c)).collect()));
                    }
                }
            }
            let mut ws: Vec<f64> = hits.iter().map(|h| h.0).collect();
            ws.sort_by(f64::total_cmp);
            ws.dedup();
            for w in ws {
                let hw: Vec<(usize, Vec<u32>)> = hits.iter().filter(|h| h.0 == w).map(|h| (h.1, h.2.clone())).collect();
                let cfg = crate::pn::PnCfg { b2: Some(bonus * w), b3: Some(bonus * w), h: None, cands: false };
                out.extend(crate::pn::usable_jumps(&hw, sets, lp, locked, &cfg, fold));
            }
        }
        // one jump per (start, ids), the largest bonus; ordered as usable_jumps orders
        out.sort_by(|a, b| a.start.cmp(&b.start).then(b.ids.len().cmp(&a.ids.len())).then_with(|| a.ids.cmp(&b.ids)).then(b.bonus.total_cmp(&a.bonus)));
        out.dedup_by(|b, a| a.start == b.start && a.ids == b.ids);
        out
    }
    pub fn short(&self) -> prior::ShortCfg {
        self.short
    }

    pub(crate) fn short_view<'a>(&'a self, inf: &'a Inference) -> ShortView<'a> {
        self.short_view_of(self.short, true, inf)
    }

    pub(crate) fn short_view_of<'a>(&'a self, cfg: prior::ShortCfg, table: bool, inf: &'a Inference) -> ShortView<'a> {
        ShortView { cfg, d: inf.ctx_len, pre: if cfg.ctx { &inf.ctx_tail } else { &[] }, table: self.short_table.as_deref().filter(|_| table) }
    }

    /// SHORT-PRIOR T2 word weights (`word_short_table`); None removes them.
    pub fn set_short_table(&mut self, t: Option<HashMap<String, f64>>) {
        self.short_table = t.map(Arc::new);
    }

    /// Lines `reading<TAB>word<TAB>weight` (reading = syllables joined by '-', weight finite in 0..=1), `#` comments.
    pub fn load_short_table(path: &std::path::Path) -> io::Result<HashMap<String, f64>> {
        let mut m = HashMap::new();
        for (k, l) in std::fs::read_to_string(path)?.lines().enumerate() {
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = l.split('\t').collect();
            let w = f.get(2).and_then(|x| x.parse::<f64>().ok()).filter(|w| w.is_finite() && (0.0..=1.0).contains(w));
            match (f.len(), w) {
                (3, Some(w)) if !f[0].is_empty() && !f[1].is_empty() => {
                    if m.insert(format!("{}|{}", f[0], f[1]), w).is_some() {
                        return Err(bad(format!("{}: line {} repeats a word", path.display(), k + 1)));
                    }
                }
                _ => return Err(bad(format!("{}: bad line {}", path.display(), k + 1))),
            }
        }
        Ok(m)
    }

    /// Evaluation tools (SHORT-PRIOR grid): `rebeam_ex` with explicit SHORT-PRIOR settings instead of the assets' own
    /// (call `set_short` once before, so the word floors are known). The product path always uses the assets' settings.
    pub fn rebeam_short(&self, conv: &Conversion, locks: &[Option<char>], lambda: f64, style_tai: bool, short: prior::ShortCfg, table: bool) -> Conversion {
        self.rebeam_core(conv, locks, lambda, style_tai, self.pn.as_ref().map(|(_, c)| c), true, conv.inf.cache.as_deref(), short, table)
    }

    /// Switches CAND on (`Some`) or off.
    pub fn set_cand(&mut self, c: Option<Arc<crate::cand::CandCfg>>) {
        self.cand = c;
    }
    pub fn cand(&self) -> Option<&crate::cand::CandCfg> {
        self.cand.as_deref()
    }

    /// Switches the glyph normalization (output fold, context fold, candidate pairs).
    pub fn set_norm(&mut self, n: NormCfg) {
        self.norm = n;
    }
    pub fn norm(&self) -> NormCfg {
        self.norm
    }

    /// Hidden size of the loaded model.
    pub fn model_dim(&self) -> Option<usize> {
        self.model.as_ref().map(N1::dim)
    }

    pub fn has_model(&self) -> bool {
        self.model.is_some()
    }
    pub fn has_prior(&self) -> bool {
        self.prior.is_some()
    }
    pub fn prior_bytes(&self) -> usize {
        self.prior.as_ref().map_or(0, Pack::bytes)
    }
    pub fn dictionary(&self) -> &Dictionary {
        &self.dict
    }

    /// Whether the syllable has at least one legal character.
    pub fn is_legal_syllable(&self, s: &str) -> bool {
        self.legal.get(s).is_some_and(|v| !v.is_empty())
    }

    /// Whether `c` is a legal character for `syllable`.
    pub fn is_legal_char(&self, syllable: &str, c: char) -> bool {
        let id = self.vocab.char_id(c);
        self.legal.get(syllable).is_some_and(|v| v.binary_search(&id).is_ok())
    }

    fn full_sets(&self, syllables: &[String]) -> Vec<Vec<u32>> {
        syllables.iter().map(|s| self.legal.get(s).cloned().unwrap_or_default()).collect()
    }

    pub(crate) fn lock_id(&self, sets: &[Vec<u32>], locks: &[Option<char>], i: usize) -> Option<u32> {
        let c = locks.get(i).copied().flatten()?;
        let id = self.vocab.char_id(c);
        sets[i].binary_search(&id).is_ok().then_some(id)
    }

    /// Converts `syllables` given the `context` tail (at most 64 characters are used) and optional locked characters
    /// (a lock that is not legal for its syllable is ignored). `lambda == 0` or no prior: greedy; otherwise the prior
    /// beam (s5-eval semantics). Uses the fallback if the model is absent or the sequence does not fit.
    pub fn convert(&self, context: &str, syllables: &[String], locks: &[Option<char>], lambda: f64) -> Conversion {
        self.convert_ex(context, syllables, locks, lambda, false)
    }

    /// `convert` with the 台/臺 style of the session (NORM plan 11.1): true writes unlocked 台/臺 as 臺.
    pub fn convert_ex(&self, context: &str, syllables: &[String], locks: &[Option<char>], lambda: f64, style_tai: bool) -> Conversion {
        let base = self.infer(context, syllables);
        self.rebeam_ex(&base, locks, lambda, style_tai)
    }

    /// The inference step alone (no locks, `chars` empty until `rebeam`): the legal sets and the model scores, or the
    /// fallback bookkeeping when the model is absent or the sequence does not fit.
    fn infer(&self, context: &str, syllables: &[String]) -> Conversion {
        self.infer_with(context, syllables, true)
    }

    /// NORM-MO: switch on/off (independent of `set_norm`); off = the old behaviour bit for bit.
    pub fn set_mo_norm(&mut self, on: bool) {
        self.mo_norm = on;
    }
    pub fn mo_norm(&self) -> bool {
        self.mo_norm
    }

    /// NORM-MO: the model-input copy of `tokens` with every syllable token ㄇㄛ˙ (after `first`) replaced by ㄇㄜ˙; None when off,
    /// when either syllable is not in the vocabulary or when there is nothing to change (the context part is never touched).
    fn mo_mapped(&self, tokens: &[u32], first: usize) -> Option<Vec<u32>> {
        if !self.mo_norm {
            return None;
        }
        let (mo, me) = (self.vocab.syllable_id("ㄇㄛ˙"), self.vocab.syllable_id("ㄇㄜ˙"));
        if mo == crate::vocab::UNK || me == crate::vocab::UNK || !tokens[first..].contains(&mo) {
            return None;
        }
        let mut v = tokens.to_vec();
        for t in &mut v[first..] {
            if *t == mo {
                *t = me;
            }
        }
        Some(v)
    }

    fn infer_with(&self, context: &str, syllables: &[String], use_model: bool) -> Conversion {
        let sets = self.full_sets(syllables);
        if let (true, Some(m)) = (use_model, &self.model) {
            // A3: the model sees 臺 as 台 in the context.
            let folded;
            let context = if self.norm.ctx {
                folded = norm::fold_ctx(context);
                folded.as_str()
            } else {
                context
            };
            let (tokens, first) = n1::encode(&self.vocab, context, syllables);
            let mapped = self.mo_mapped(&tokens, first);
            let model_tokens: &[u32] = mapped.as_deref().unwrap_or(&tokens);
            let refs: Vec<&[u32]> = sets.iter().map(Vec::as_slice).collect();
            // CAND: with kept candidates the extended forward, otherwise the unchanged path
            // (the base sequence must fit the old limit, so a too-long buffer falls back exactly as without CAND)
            let scores = match self.cand.as_ref().filter(|_| tokens.len() <= n1::POSITIONS).and_then(|c| c.assemble(&tokens, first, m.dim())) {
                Some((mut toks, pos, extra, _)) => {
                    if mapped.is_some() { toks[..tokens.len()].copy_from_slice(model_tokens); }
                    m.scores_ext(&toks, &pos, &extra, n1::POSITIONS + self.cand.as_ref().unwrap().m, first, &refs)
                }
                None => m.scores(model_tokens, first, &refs),
            };
            if let Some(scores) = scores {
                let ctx_len = tokens.len().min(first.saturating_sub(1));
                let ctx_tail = prior::ctx_tail(tokens[..ctx_len].iter().rev().copied());
                let inf = Inference { recent_output: false, syllables: syllables.to_vec(), sets, scores, base_fallback: vec![], fallback: false, cache: None, ctx_len, ctx_tail };
                return Conversion { hints: vec![], auto: vec![], selection: None, chars: vec![], fallback: false, inf: Arc::new(inf), forced: vec![] };
            }
        }
        let base_fallback = syllables
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if sets[i].is_empty() {
                    return None;
                }
                // Normally the dictionary has a single-character entry; otherwise the smallest id.
                self.dict
                    .best_single_char(s)
                    .filter(|c| sets[i].binary_search(&self.vocab.char_id(*c)).is_ok())
                    .or_else(|| self.vocab.char_of(sets[i][0]))
            })
            .collect();
        let inf = Inference { recent_output: false, syllables: syllables.to_vec(), sets, scores: vec![], base_fallback, fallback: true, cache: None, ctx_len: 0, ctx_tail: vec![] };
        Conversion { hints: vec![], auto: vec![], selection: None, chars: vec![], fallback: true, inf: Arc::new(inf), forced: vec![] }
    }

    /// The selection step: the same inference with other `locks`, without running the model again. Bit-identical to
    /// `convert(context, syllables, locks, lambda)` for the context and syllables the inference was made for.
    pub fn rebeam(&self, conv: &Conversion, locks: &[Option<char>], lambda: f64) -> Conversion {
        self.rebeam_ex(conv, locks, lambda, false)
    }

    /// `rebeam` with the 台/臺 style (see `convert_ex`).
    pub fn rebeam_ex(&self, conv: &Conversion, locks: &[Option<char>], lambda: f64, style_tai: bool) -> Conversion {
        self.rebeam_with(conv, locks, lambda, style_tai, self.pn.as_ref().map(|(_, c)| c), true)
    }

    /// `rebeam_ex` with explicit PN settings (`None` = PN off; ignored without a lexicon) and output fold switch
    /// (`fold = false` returns the raw selection before the NORM output fold, for equivalence checks).
    pub fn rebeam_with(&self, conv: &Conversion, locks: &[Option<char>], lambda: f64, style_tai: bool, pn: Option<&crate::pn::PnCfg>, fold: bool) -> Conversion {
        self.rebeam_with_cache(conv, locks, lambda, style_tai, pn, fold, conv.inf.cache.as_deref())
    }

    /// `rebeam_with` with an explicit document bigram set instead of the one kept in the conversion (evaluation tools with a
    /// sliding window). The set carries its bonus settings; it is used on the PN path only (ignored otherwise).
    #[allow(clippy::too_many_arguments)]
    pub fn rebeam_with_cache(&self, conv: &Conversion, locks: &[Option<char>], lambda: f64, style_tai: bool, pn: Option<&crate::pn::PnCfg>, fold: bool, doc: Option<&crate::cache::CacheSet>) -> Conversion {
        self.rebeam_core(conv, locks, lambda, style_tai, pn, fold, doc, self.short, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn rebeam_core(&self, conv: &Conversion, locks: &[Option<char>], lambda: f64, style_tai: bool, pn: Option<&crate::pn::PnCfg>, fold: bool, doc: Option<&crate::cache::CacheSet>, short: prior::ShortCfg, table: bool) -> Conversion {
        let inf = &conv.inf;
        let cache = doc.and_then(crate::cache::CacheSet::bonus);
        let sets = &inf.sets;
        let n = inf.syllables.len();
        // USERLEX 1.6: a lock outside the legal set is accepted only where a user word that pairs exactly that character
        // with this syllable as a custom reading still matches its full reading; otherwise ignored as before.
        let real: Vec<Option<u32>> = (0..n).map(|i| self.lock_id(sets, locks, i).or_else(|| self.custom_lock(&inf.syllables, locks, i))).collect();
        // USERLEX 1.1: forced user-word spans become temporary per-character constraints (never written as real locks).
        let forced = self.forced_spans(&inf.syllables, sets, &real);
        let mut locked = real.clone();
        let mut forced_mask = vec![false; n];
        for (st, ids) in &forced {
            for (k, &id) in ids.iter().enumerate() {
                locked[st + k] = Some(id);
                forced_mask[st + k] = true;
            }
        }
        let mut user_mask = forced_mask.clone();
        let sp = self.short_view_of(short, table, inf);
        let chars: Vec<Option<char>> = if inf.fallback {
            (0..n)
                .map(|i| match locked[i] {
                    Some(id) => self.vocab.char_of(id),
                    None => inf.base_fallback[i],
                })
                .collect()
        } else {
            let scores = &inf.scores;
            let mut ids: Vec<Option<u32>> = scores
                .iter()
                .zip(sets)
                .zip(&locked)
                .map(|((sc, ids), lk)| {
                    if lk.is_some() {
                        return *lk;
                    }
                    let mut best: Option<usize> = None;
                    for (j, s) in sc.iter().enumerate() {
                        if best.is_none_or(|b| *s > sc[b]) {
                            best = Some(j);
                        }
                    }
                    best.map(|j| ids[j])
                })
                .collect();
            if let Some((best, mask)) = ids.iter().all(Option::is_some).then(|| self.user_select(&inf.syllables, sets, scores, &locked, lambda, pn, cache.clone(), &forced_mask, &sp)).flatten() {
                ids = best.into_iter().map(Some).collect();
                for (u, m) in user_mask.iter_mut().zip(mask) {
                    *u |= m;
                }
            } else if ((self.pn.is_some() && pn.is_some()) || self.genj.is_some() || self.wordbonus.is_some())
                && ids.iter().all(Option::is_some)
            {
                // PN (PN-plan 2): the beam with whole-word jumps, also for lambda 0 / no prior (prior term 0).
                if let Some(best) = self.pn_select(pn, &inf.syllables, sets, scores, &locked, lambda, cache, &forced_mask, &sp) {
                    ids = best.into_iter().map(Some).collect();
                }
            } else if sp.has_words()
                && ids.iter().all(Option::is_some)
                && let Some(best) = self.short_select(sets, scores, &locked, lambda, cache.clone(), &forced_mask, &inf.syllables, &sp)
            {
                // SHORT-PRIOR WB without PN / user words: the jump beam with the word jumps only.
                ids = best.into_iter().map(Some).collect();
            } else if let (Some(pk), true) = (&self.prior, lambda != 0.0)
                && ids.iter().all(Option::is_some)
            {
                let (cs, ls): (Vec<Vec<u32>>, Vec<Vec<f64>>) = scores
                    .iter()
                    .zip(sets)
                    .zip(&locked)
                    .map(|((sc, ids), lk)| match lk {
                        Some(id) => (vec![*id], vec![0.0]),
                        None => prior::top_k(sc, ids),
                    })
                    .unzip();
                if let Some(best) = prior::beam_ex(&cs, &ls, pk, &|i| sp.weight(lambda, i), sp.pre()) {
                    ids = best.into_iter().map(Some).collect();
                }
            }
            ids.iter().map(|p| p.map(|p| self.vocab.char_of(p).unwrap_or('\u{FFFD}'))).collect()
        };
        let raw = chars.clone();
        let mut chars = chars;
        if self.norm.out && fold {
            let lk: Vec<bool> = locked.iter().enumerate().map(|(i, lk)| lk.is_some() || user_mask[i]).collect();
            let trad = self.vocab.char_id(norm::TAI_TRAD);
            let tai_ok: Vec<bool> = sets.iter().map(|s| s.binary_search(&trad).is_ok()).collect();
            norm::fold_out(&mut chars, &lk, &tai_ok, style_tai);
        }
        if self.ta_default && fold {
            let lk: Vec<bool> = locked.iter().enumerate().map(|(i, lk)| lk.is_some() || user_mask[i]).collect();
            let protect = self.ta_protect(&inf.syllables, &chars, doc);
            norm::ta_default(&mut chars, &lk, &inf.syllables, &protect);
        }
        let selection = if !inf.fallback && raw.iter().all(Option::is_some) && sets.iter().all(|s| !s.is_empty()) {
            Some(Arc::new(crate::lexux::Selection {
                ids: raw.iter().map(|c| self.vocab_id(c.unwrap())).collect(),
                blocked: real.iter().enumerate().map(|(i, r)| r.is_some() || user_mask[i]).collect(),
                display: chars.clone(), lambda, short,
            }))
        } else { None };
        let result = Conversion { chars, fallback: inf.fallback, inf: conv.inf.clone(), forced, hints: vec![], auto: vec![], selection };
        self.apply_lexux(&result, self.lexux, inf.recent_output)
    }

    /// SHORT-PRIOR WB on the plain path (no PN, no user words): the jump beam with the word jumps (None: no usable jump).
    #[allow(clippy::too_many_arguments)]
    fn short_select(&self, sets: &[Vec<u32>], scores: &[Vec<f32>], locked: &[Option<u32>], lambda: f64, cache: Option<crate::cache::CacheBonus<'_>>, forced: &[bool], syllables: &[String], sp: &ShortView<'_>) -> Option<Vec<u32>> {
        let lp = crate::pn::log_softmax(scores);
        let jumps = self.short_word_jumps(syllables, sets, &lp, locked, forced, sp);
        if jumps.is_empty() {
            return None;
        }
        let (cs, ls): (Vec<Vec<u32>>, Vec<Vec<f64>>) = scores
            .iter()
            .zip(sets)
            .zip(locked)
            .map(|((sc, ids), lk)| match lk {
                Some(id) => (vec![*id], vec![0.0]),
                None => prior::top_k(sc, ids),
            })
            .unzip();
        match &self.prior {
            Some(pk) if lambda != 0.0 => crate::pn::beam_cached_ex(&cs, &ls, sets, &lp, locked, &jumps, &|i| sp.weight(lambda, i), sp.pre(), &|h, c| pk.logp(h, c), cache),
            _ => crate::pn::beam_cached(&cs, &ls, sets, &lp, locked, &jumps, 0.0, &|_, _| 0.0, cache),
        }
    }

    /// AI.md fallback: the highest-probability single character of each syllable (never uses the model).
    pub fn fallback(&self, syllables: &[String], locks: &[Option<char>]) -> Conversion {
        let base = self.infer_with("", syllables, false);
        self.rebeam(&base, locks, 0.0)
    }

    /// Candidates for one position: the chosen character first, then the rest by model score (ties: smaller id);
    /// in fallback mode by dictionary single-character probability. Always the full legal set of the syllable.
    pub fn alternatives(&self, conv: &Conversion, syllables: &[String], pos: usize) -> Vec<char> {
        let (Some(sy), Some(ids)) = (syllables.get(pos), conv.inf.sets.get(pos)) else { return vec![] };
        let mut ranked: Vec<(f32, u32)> = match conv.inf.scores.get(pos) {
            Some(sc) if !conv.fallback && sc.len() == ids.len() => ids.iter().copied().zip(sc.iter().copied()).map(|(id, s)| (s, id)).collect(),
            _ => {
                let order = self.dict.ranked_single_chars(sy);
                ids.iter()
                    .map(|&id| {
                        let rank = self.vocab.char_of(id).and_then(|c| order.iter().position(|&o| o == c)).unwrap_or(1 << 20);
                        (-(rank as f32), id)
                    })
                    .collect()
            }
        };
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut out: Vec<char> = ranked.into_iter().filter_map(|(_, id)| self.vocab.char_of(id)).collect();
        if let Some(Some(top)) = conv.chars.get(pos)
            && let Some(i) = out.iter().position(|c| c == top)
        {
            out.remove(i);
            out.insert(0, *top);
        }
        out
    }
}

impl Assets {
    /// TA (TA-plan): switch the ㄊㄚ default on (the engine) or off (default; evaluation tools unless asked).
    pub fn set_ta_default(&mut self, on: bool) {
        self.ta_default = on;
    }

    /// TA plan 1, exceptions 2-4: positions whose ㄊㄚ variant stays (confirmed cache support, inside a proper name of the
    /// PN lexicon that the output spells, or 他 not legal for the syllable). Empty work when no position is a candidate.
    fn ta_protect(&self, syllables: &[String], chars: &[Option<char>], doc: Option<&crate::cache::CacheSet>) -> Vec<bool> {
        let n = chars.len();
        let mut protect = vec![false; n];
        let cand: Vec<usize> = (0..n).filter(|&i| syllables[i] == norm::TA_SYLLABLE && chars[i].is_some_and(|c| norm::TA_VARIANTS.contains(&c))).collect();
        if cand.is_empty() {
            return protect;
        }
        let id = |c: Option<char>| c.map(|c| self.vocab_id(c));
        for &i in &cand {
            if !self.is_legal_char(&syllables[i], norm::TA) {
                protect[i] = true;
            }
            if let Some(s) = doc.filter(|s| s.is_confirmed()) {
                let x = id(chars[i]).unwrap();
                let left = i > 0 && id(chars[i - 1]).is_some_and(|p| s.contains(p, x));
                let right = i + 1 < n && id(chars[i + 1]).is_some_and(|q| s.contains(x, q));
                if left || right {
                    protect[i] = true;
                }
            }
        }
        if let Some((lex, _)) = &self.pn {
            let tai = (self.vocab_id(norm::TAI_TRAD), self.vocab_id(norm::TAI));
            let fold = |x: u32| if x == tai.0 { tai.1 } else { x };
            let out: Vec<Option<u32>> = chars.iter().map(|&c| id(c).map(fold)).collect();
            let codes = self.syl_codes(syllables);
            for (st, _, ids) in lex.hits(&codes) {
                let l = ids.len();
                if l < 2 || st + l > n || !cand.iter().any(|&i| i >= st && i < st + l) {
                    continue;
                }
                if (0..l).all(|k| out[st + k] == Some(fold(ids[k]))) {
                    let word: String = ids.iter().filter_map(|&x| self.vocab.char_of(x)).collect();
                    if !norm::TA_WORDS.contains(&word.as_str()) {
                        for p in protect.iter_mut().skip(st).take(l) {
                            *p = true;
                        }
                    }
                }
            }
        }
        protect
    }
}

#[cfg(test)]
#[path = "ta_tests.rs"]
mod ta_tests;

#[cfg(test)]
mod prefix_tests {
    use super::*;

    #[test]
    fn normalized_context_reuses_prefix() {
        use crate::n1::n1_prefix_tests::{model, entries};
        let mut a = Assets::from_dictionary_text("ㄊㄞˊ 台 -3\nㄊㄞˊ 臺 -4\nㄅㄟˇ 北 -3\n", "").unwrap();
        a.set_norm(NormCfg::ALL);
        a.model = Some(model(a.vocab.len()));
        let syllables = vec!["ㄊㄞˊ".to_string(), "ㄅㄟˇ".to_string()];
        let first = a.convert("臺", &syllables, &[], 0.0);
        assert!(!first.fallback);
        let m = a.model.as_ref().unwrap();
        let before = entries(m);
        assert_eq!(before.len(),1);
        let second = a.convert("台", &syllables, &[], 0.0);
        assert_eq!(entries(m),before, "same Arc means a hit, not a rebuilt entry");
        assert_eq!(first.text(),second.text());
        m.set_prefix_cache(false);
        for context in ["臺", "台"] {
            let full = a.convert(context, &syllables, &[], 0.0);
            assert_eq!(full.text(),second.text());
            for (x,y) in full.inf.scores.iter().flatten().zip(second.inf.scores.iter().flatten()) {
                assert!((x-y).abs() <= 1e-5);
            }
        }
        assert_eq!(entries(m),before);
    }
}

#[cfg(test)]
#[path = "short_tests.rs"]
mod short_tests;

#[cfg(test)]
#[path = "normmo_tests.rs"]
mod normmo_tests;
