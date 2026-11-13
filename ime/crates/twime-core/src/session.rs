//! The per-input-field session: composition, cursor, candidate selection, locking and the commit contract.

mod user;
mod lexux;
mod fuzzy_words;
mod pop;
mod emoji;
mod zyout;
mod neutral;

use std::sync::Arc;

use twime_model::{Assets, CandParams, Conversion};
use twime_proto::{
    Applied, Cands, Comp, CommitResult, Event, Key, KeyEvent, MAX_SYLLABLES, Mode, PeekView, Segment, Update, is_zhuyin_key, peek,
};

use crate::keymap::{self, Slot};

/// Context characters kept for the model (AI.md: 64).
const CONTEXT: usize = 64;

#[derive(Clone, Debug)]
pub struct Config {
    /// Weight of the n-gram prior (0.5 = best on dev and M with G1K, N3 M0 follow-up; 1.0 = PRIOR-RS setting; 0 = off).
    pub lambda: f64,
    /// Whether Space / Down open the candidate list. Off until the shell can show it (M3).
    pub candidates: bool,
    /// Candidates per page.
    pub page_size: usize,
    /// Also list dictionary words that end at the cursor. Off = single characters only (span 1), still
    /// ordered by the new model-based score (N3b decision 2026-10-12), not by the old ranking.
    pub word_candidates: bool,
    /// Longest word span in syllables.
    pub max_span: usize,
    /// Most words shown besides the current item.
    pub word_cap: usize,
    /// Weight of the same-reading rarity penalty (0 = pure model score).
    pub beta: f64,
    /// Version 3 sessions start in English mode.
    pub default_english: bool,
    /// Version 3 sessions start with half-width punctuation style.
    pub punct_half: bool,
    /// The backquote key opens the small symbol table (UX3, plans 6.7); off: it types a half-width backquote.
    pub symbol_table: bool,
    /// Same-document bigram cache (the engine sets it only with PN on). None = off: `DocText` is ignored.
    pub cache: Option<twime_model::cache::CacheCfg>,
    /// FUZZY F1 (FUZZY-plan 3): repair of illegal syllables. None = off, the old behaviour bit for bit.
    pub repair: Option<Repair>,
    /// LEXUX post-selection rules. Missing keys keep the previous behaviour.
    pub lexux: Option<twime_model::lexux::Config>,
    /// FUZZY F2 (FUZZY-F2-plan): word-level fuzzy suggestions in the candidate list, with the confusion-class
    /// index. None = off, the old behaviour bit for bit.
    pub fuzzy_words: Option<Arc<crate::fuzzy::Index>>,
    /// FUZZY F2 step 2 (FUZZY-F2-plan 5): fuzzy options join the LEXUX `\` cycle. Needs `fuzzy_words` and LEXUX;
    /// off = the old behaviour bit for bit.
    pub fuzzy_hints: bool,
    /// AUTOCOMP (AUTOCOMP-plan): word completions in the LEXUX `\` cycle (theta, theta2). Needs LEXUX; None =
    /// off, the old behaviour bit for bit.
    pub autocomp: Option<twime_model::autocomp::AutoParams>,
    /// AUTOCOMP-LIST (AUTOCOMP-LIST-plan 5): word completions inside the candidate list opened at the end;
    /// off = the old behaviour bit for bit.
    pub autocomp_list: bool,
    pub autocomp_pop: bool,
    /// EMOJI (EMOJI-plan): emoji at the end of the candidate list from this reading table. None = off, the old
    /// behaviour bit for bit.
    pub emoji: Option<Arc<twime_model::emoji::EmojiTable>>,
}

/// FUZZY F1 repair mode (plan 3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairMode {
    /// Illegal positions are converted with the first repair syllable of R(t).
    Auto,
    /// Illegal positions stay unconverted until the user picks a repair character from the repair list.
    Suggest,
}

/// FUZZY F1 settings: the mode and the confusion-class index (built from the deployment's data.txt and legal table).
#[derive(Clone)]
pub struct Repair {
    pub mode: RepairMode,
    pub index: Arc<crate::fuzzy::Index>,
}

impl std::fmt::Debug for Repair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repair").field("mode", &self.mode).field("typed", &self.index.rev.len()).finish()
    }
}

/// Most characters of one repair syllable in a list, and most items of a repair segment / list (plan 3.4).
const REPAIR_CHARS: usize = 8;
const REPAIR_ITEMS: usize = 40;

impl Default for Config {
    fn default() -> Config {
        Config {
            lambda: 0.5,
            candidates: true,
            page_size: 9,
            word_candidates: true,
            max_span: 6,
            word_cap: 40,
            beta: 0.0,
            default_english: false,
            punct_half: false,
            symbol_table: true,
            cache: None,
            repair: None,
            lexux: None,
            fuzzy_words: None,
            fuzzy_hints: false,
            autocomp: None,
            autocomp_list: false,
            autocomp_pop: false,
            emoji: None,
        }
    }
}

/// Shared, immutable engine: assets and configuration.
pub struct Engine {
    assets: Arc<Assets>,
    cfg: Config,
    user: Option<crate::UserLink>,
    legacy_toggle: twime_proto::ToggleKey,
}

impl Engine {
    pub fn new(assets: Arc<Assets>, cfg: Config) -> Arc<Engine> {
        Self::with_user(assets, cfg, None, twime_proto::ToggleKey::Shift)
    }
    pub fn assets(&self) -> &Assets {
        &self.assets
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }
    pub fn new_session(self: &Arc<Self>) -> Session {
        Session::new(self.clone())
    }
}

/// Bopomofo symbols typed for the current syllable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Typing {
    initial: Option<char>,
    medial: Option<char>,
    fin: Option<char>,
    tone: Option<char>,
}

impl Typing {
    fn is_empty(&self) -> bool {
        self.initial.is_none() && self.medial.is_none() && self.fin.is_none() && self.tone.is_none()
    }
    fn has_body(&self) -> bool {
        self.initial.is_some() || self.medial.is_some() || self.fin.is_some()
    }
    fn body(&self) -> String {
        [self.initial, self.medial, self.fin].into_iter().flatten().collect()
    }
    /// Dictionary form: body + tone mark (first tone has no mark).
    fn syllable(&self) -> String {
        let mut s = self.body();
        if let Some(t) = self.tone {
            s.push(t);
        }
        s
    }
    fn set(&mut self, c: char) {
        match keymap::slot_of(c) {
            Some(Slot::Initial) => self.initial = Some(c),
            Some(Slot::Medial) => self.medial = Some(c),
            Some(Slot::Final) => self.fin = Some(c),
            Some(Slot::Tone) => self.tone = Some(c),
            None => {}
        }
    }
    /// Removes the last-typed slot in the order tone, final, medial, initial.
    fn backspace(&mut self) {
        if self.tone.take().is_some() || self.fin.take().is_some() || self.medial.take().is_some() {
            return;
        }
        self.initial = None;
    }
}

#[derive(Clone, Debug)]
struct CandItem {
    text: String,
    /// Syllables replaced, ending at `Cand::pos`.
    span: usize,
    /// FUZZY 3.4: the repaired positions this item covers and the syllable it uses there (empty = no repair; then the
    /// shown text is `text` unchanged).
    repairs: Vec<(usize, String)>,
    /// AUTOCOMP-LIST: syllables appended at the end when chosen (empty = an ordinary item); `text` is then the whole word
    /// and `span` the syllables of it already typed.
    extra: Vec<String>,
    /// EMOJI: text committed as it is when chosen (no syllable is replaced: `span` is 0); false for every other item.
    raw: bool,
    /// ZHUYIN-OUT (ZHUYIN-OUT-plan 3.2): the typed bopomofo of the position itself (`text`, span 1); choosing it
    /// makes the position literal (`select_literal`). False for every other item.
    lit: bool,
}

impl CandItem {
    /// The string sent to the shell: the text, plus the readings of its repaired positions.
    fn display(&self) -> String {
        if self.repairs.is_empty() {
            return self.text.clone();
        }
        let r: Vec<&str> = self.repairs.iter().map(|(_, s)| s.as_str()).collect();
        format!("{}（{}）", self.text, r.join("、"))
    }
}

#[derive(Clone, Debug)]
struct Cand {
    /// Index of the last syllable of every item (the cursor - 1).
    pos: usize,
    items: Vec<CandItem>,
    selected: usize,
}

#[derive(Clone)]
struct State {
    prev: Vec<char>,
    syl: Vec<String>,
    locks: Vec<Option<char>>,
    /// Selection that locked each position (0 = none); positions of one group unlock together.
    groups: Vec<u32>,
    /// CACHE-c: groups made by an explicit choice (a number key or Enter in the candidate window, `CandidatePick`), not by
    /// the highlighted item taken when a text key ends the window.
    explicit: Vec<u32>,
    next_group: u32,
    swaps: Vec<lexux::Swap>,
    recent_output: bool,
    typing: Typing,
    cursor: usize,
    cand: Option<Cand>,
    conv: Option<Conversion>,
    /// FUZZY 3.3: the repair syllable the user chose for each position (None = not chosen); kept in step with `syl`.
    repair: Vec<Option<String>>,
    /// ZHUYIN-OUT 3.1: the position stays as the typed bopomofo (`syl[i]`); kept in step with `syl`.
    lit: Vec<bool>,
    /// Input mode (version 3). Survives the clearing of the text state.
    mode: Mode,
    /// English run: ASCII text being composed (version 3).
    run: String,
    /// The small symbol table is open; the composition is its anchor (version 3).
    symbols: bool,
    /// Page of the symbol table (0-based).
    sym_page: usize,
    /// Keys pressed for the current composition, for the `Shift+Enter` rescue; `None` once it is no longer a plain
    /// sequence of typed keys.
    raw: Option<Vec<Key>>,
    /// 台/臺 style of this context (NORM plan 11.1): true writes the unlocked 台/臺 characters as 臺. Survives the
    /// clearing of the text state; reset by a context change (`fresh(false)`), set by `SetContext` and by a selection.
    style_tai: bool,
    /// Bigram set of the document text before the composition, from the last `DocText`; never
    /// built from committed text. Cleared by any commit and by every fresh text state.
    doc: Option<Arc<twime_model::cache::CacheSet>>,
}

impl State {
    fn empty() -> State {
        State {
            prev: vec![],
            syl: vec![],
            locks: vec![],
            groups: vec![],
            explicit: vec![],
            next_group: 1,
            swaps: vec![],
            recent_output: false,
            typing: Typing::default(),
            cursor: 0,
            cand: None,
            conv: None,
            repair: vec![],
            lit: vec![],
            mode: Mode::Chinese,
            run: String::new(),
            symbols: false,
            sym_page: 0,
            raw: Some(vec![]),
            style_tai: false,
            doc: None,
        }
    }
    fn composing(&self) -> bool {
        !self.syl.is_empty() || !self.typing.is_empty() || !self.run.is_empty() || self.symbols
    }
    /// Drops composition and candidates and the document snapshot (CACHE plan 3.3: it serves one composition); keeps
    /// `prev` and `mode`.
    fn clear_composition(&mut self) {
        self.doc = None;
        self.syl.clear();
        self.locks.clear();
        self.groups.clear();
        self.swaps.clear();
        self.explicit.clear();
        self.typing = Typing::default();
        self.cursor = 0;
        self.cand = None;
        self.conv = None;
        self.repair.clear();
        self.lit.clear();
        self.run.clear();
        self.symbols = false;
        self.sym_page = 0;
        self.raw = Some(vec![]);
    }
}

struct Pending {
    seq: u32,
    /// State before the key, kept only when the Update carries a commit.
    before: Option<State>,
    commit: bool,
}

pub struct Session {
    pop_gen: u32,
    pop_eligible: bool,
    pop_peer: bool,
    pop_result: Vec<twime_model::autocomp::Completion>,
    pop_opening: bool,
    engine: Arc<Engine>,
    st: State,
    password: bool,
    pending: Option<Pending>,
    last_seq: u32,
    /// Version 3 behaviour (English mode and runs, punctuation, Caps Lock, `ToggleMode`). Off for connections that did not
    /// negotiate version 3: those sessions behave exactly as before.
    ux: bool,
    lexux_peer: bool,
    hint_mods: twime_proto::Mods,
    /// Half-width punctuation style (from the configuration; constant for the session).
    punct_half: bool,
    symbol_table: bool,
    settings_gen: u64,
    err_gen: Option<u64>,
    notice: Option<String>,
    /// CACHE-c: runs of characters explicitly chosen in the candidate window, in
    /// committed conversions of this field, and the anchor. Outside `State`, so a fresh text state (external edit, ...) and a
    /// failed commit keep it; cleared by focus events, `Close`, `Password`, `SetContext`, and by a `DocText` without the
    /// anchor; a `DocText` also drops the runs it does not contain. Only used with `cache_source=confirmed`.
    confirmed: twime_model::cache::Confirmed,
}

impl Session {
    pub fn new(engine: Arc<Engine>) -> Session {
        let (settings_gen, punct_half, symbol_table) = Self::initial_user_settings(&engine);
        Session { pop_gen: 1, pop_eligible: false, pop_peer: false, pop_result: vec![], pop_opening: false, engine, st: State::empty(), password: false, pending: None, last_seq: 0, ux: false, lexux_peer: false, hint_mods: twime_proto::Mods(0), punct_half, symbol_table, settings_gen, err_gen: None, notice: None, confirmed: Default::default() }
    }

    /// Switches the version 3 behaviour on or off. Only meant right after creation (the initial mode is applied).
    pub fn set_ux(&mut self, ux: bool) {
        self.ux = ux;
        self.st.mode = if ux && self.default_english() { Mode::English } else { Mode::Chinese };
    }

    /// Negotiate LEXUX independently of the version 3 keyboard behaviour. Set at session creation.
    pub fn set_lexux_peer(&mut self, enabled: bool) {
        self.lexux_peer = enabled;
    }

    /// Current input mode (always `Chinese` when the version 3 behaviour is off).
    pub fn mode(&self) -> Mode {
        if self.ux { self.st.mode } else { Mode::Chinese }
    }

    /// The view the shell needs for `peek`.
    pub fn peek_view(&self) -> PeekView {
        PeekView {
            composing: self.st.composing(),
            candidates_open: self.st.cand.is_some(),
            mode: self.mode(),
            run: !self.st.run.is_empty(),
            symbols: self.st.symbols,
            ux: self.ux,
        }
    }

    /// Pairs of the document snapshot and of the bigram set the current conversion uses (CACHE plan 3.3; tests and
    /// diagnostics, never logged).
    pub fn doc_pairs(&self) -> (Option<usize>, Option<usize>) {
        (self.st.doc.as_ref().map(|d| d.len()), self.st.conv.as_ref().and_then(Conversion::cache_pairs))
    }

    /// FUZZY evaluation (tests and diagnostics, never logged): the open candidate list as (text, span, shown string).
    pub fn candidate_items(&self) -> Option<Vec<(String, usize, String)>> {
        self.st.cand.as_ref().map(|c| c.items.iter().map(|i| (i.text.clone(), i.span, i.display())).collect())
    }

    /// Full identity for paired candidate-list evaluation; appended completions have nonempty `extra`.
    pub fn candidate_identities(&self) -> Vec<(String, usize, Vec<(usize, String)>, Vec<String>)> {
        self.st.cand.as_ref().map(|c| c.items.iter().map(|i| (i.text.clone(), i.span, i.repairs.clone(), i.extra.clone())).collect()).unwrap_or_default()
    }

    /// Confirmed runs remembered (CACHE-c; tests and diagnostics, never logged).
    pub fn confirmed_pairs(&self) -> usize {
        self.confirmed.len()
    }

    /// The cache uses only confirmed pairs.
    fn confirm_mode(&self) -> bool {
        self.engine.cfg.cache.as_ref().is_some_and(|c| c.source == twime_model::cache::CacheSource::Confirmed)
    }

    /// Context characters currently remembered (for tests and diagnostics; never logged).
    pub fn context(&self) -> String {
        self.st.prev.iter().collect()
    }

    /// Convenience for tests and drivers without a shell: the previous Update is taken as fully applied.
    pub fn on_key(&mut self, ev: KeyEvent) -> Update {
        let ack = self.pending.as_ref().map(|p| CommitResult { seq: p.seq, commit: Applied::Applied, comp: Applied::Applied });
        let seq = self.last_seq.wrapping_add(1);
        self.handle(seq, ack, Event::Key(ev))
    }

    /// A fresh text state: composition, run, symbols and log gone; the mode stays. `keep_context` keeps `prev`.
    fn fresh(&mut self, keep_context: bool) {
        let prev = if keep_context { std::mem::take(&mut self.st.prev) } else { vec![] };
        let style_tai = keep_context && self.st.style_tai;
        let recent_output = keep_context && self.st.recent_output;
        self.st = State { prev, recent_output, mode: self.st.mode, style_tai, ..State::empty() };
    }

    /// Could this event produce a commit (so that the state before it has to be kept for a failed host write)?
    fn may_commit(&self, ev: &Event) -> bool {
        if self.st.syl.len() >= MAX_SYLLABLES {
            return true;
        }
        match ev {
            Event::Focus(false) | Event::ToggleMode | Event::CandidatePick(_) => true,
            Event::Key(k) => {
                // EMOJI: a number key choosing an emoji commits
                if let Key::Digit(d) = k.key && !k.mods.shift() && self.digit_picks_raw(d) {
                    return true;
                }
                let quiet = matches!(
                    k.key,
                    Key::Letter(_) | Key::Digit(_) | Key::Minus | Key::Comma | Key::Period | Key::Slash | Key::Semicolon
                        | Key::Space | Key::Esc | Key::Backspace | Key::Delete | Key::Tab | Key::Left | Key::Right | Key::Up | Key::Down
                        | Key::Home | Key::End | Key::PageUp | Key::PageDown
                );
                !(quiet && !k.mods.shift() && !k.mods.caps() && self.st.run.is_empty() && !self.st.symbols)
            }
            _ => false,
        }
    }

    /// Handles one request: first the confirmation of the previous Update, then the event.
    pub fn handle(&mut self, seq: u32, ack: Option<CommitResult>, ev: Event) -> Update {
        if let Event::Idle(g) = ev {
            let snapshot = self.prepare_idle(seq, ack, g);
            let ranked = snapshot.map(|s| s.compute()).unwrap_or_default();
            return self.finish_idle(g, ranked);
        }
        let previous_settings = self.settings_gen;
        self.sync_user_settings();
        if previous_settings != self.settings_gen { self.invalidate_pop(); }
        self.apply_ack(ack);
        self.last_seq = seq;
        let old_len = self.st.syl.len();
        let completion = match ev { Event::DownCompletion(id) if id == self.pop_gen => std::mem::take(&mut self.pop_result), _ => vec![] };
        let cancels = !matches!(&ev, Event::Ping | Event::Key(KeyEvent { down: false, .. }));
        if cancels { self.invalidate_pop(); }
        let completed_key = matches!(&ev, Event::Key(KeyEvent { down: true, .. }));
        let may_commit = self.may_commit(&ev);
        let before = may_commit.then(|| self.st.clone());
        let mut commit: Option<String> = None;
        let is_key = matches!(ev, Event::Key(_));
        let handled = match ev {
            Event::Idle(_) => unreachable!(),
            Event::DownCompletion(_) => {
                if completion.is_empty() { self.on_key_event(KeyEvent::press(Key::Down), &mut commit) }
                else {
                    self.pop_opening = true;
                    self.open_candidates();
                    self.pop_opening = false;
                    if let Some(c) = self.st.cand.as_mut() {
                        let found = completion.into_iter().map(|c| CandItem { text: c.word, span: c.k, repairs: vec![], extra: c.extra, raw: false, lit: false });
                        c.items.splice(0..0, found);
                    }
                    true
                }
            }
            Event::Key(k) => self.on_key_event(k, &mut commit),
            Event::Focus(false) => {
                // Leaving the field: commit a fully convertible composition (or an English run), otherwise drop it (plans 4.1 c).
                if self.committable() {
                    commit = self.take_commit();
                }
                self.fresh(false);
                self.confirmed.clear();
                false
            }
            Event::Focus(true) | Event::Close => {
                self.fresh(false);
                self.confirmed.clear();
                false
            }
            Event::Unsynced | Event::ExternalEdit | Event::CompositionAborted => {
                self.fresh(false);
                false
            }
            Event::Password(p) => {
                self.password = p;
                self.fresh(false);
                self.confirmed.clear();
                false
            }
            Event::CandidatePick(i) => {
                if self.st.symbols {
                    if let Some(s) = keymap::SYMBOL_TABLE.get(self.st.sym_page * keymap::SYMBOL_PAGE + i as usize).filter(|_| (i as usize) < keymap::SYMBOL_PAGE) {
                        self.commit_symbol(s, &mut commit);
                    }
                } else if let Some(c) = &self.st.cand {
                    let page = c.selected / self.engine.cfg.page_size.max(1);
                    let idx = page * self.engine.cfg.page_size.max(1) + i as usize;
                    if (i as usize) < self.engine.cfg.page_size.max(1) && idx < c.items.len() {
                        self.select_or_commit(idx, true, &mut commit);
                    }
                }
                false
            }
            Event::ToggleMode => {
                self.toggle_mode(&mut commit);
                false
            }
            Event::AddToLexicon => self.add_to_lexicon(),
            Event::User(_) => false,
            Event::Ping | Event::Status | Event::Shutdown | Event::Hello(_) => false,
            Event::DocText(s) => {
                // CACHE plan 3.3: only before a composition starts, never in password fields, only when the assets use it.
                if !self.password && !self.st.composing() {
                    let a = &self.engine.assets;
                    let set = match &self.engine.cfg.cache {
                        None => None,
                        // CACHE-c 2.2, 2.3: also forgets what is no longer in the text before the cursor
                        Some(c) if self.confirm_mode() => Some(twime_model::cache::CacheSet::from_text_confirmed(a, &s, c, &mut self.confirmed)),
                        Some(c) => Some(twime_model::cache::CacheSet::from_text(a, &s, c)),
                    };
                    self.st.doc = set.map(Arc::new).filter(|d| !d.is_empty());
                }
                false
            }
            Event::SetContext(s) => {
                self.fresh(false);
                self.confirmed.clear();
                self.st.prev = s.chars().rev().take(CONTEXT).collect::<Vec<_>>().into_iter().rev().collect();
                self.st.style_tai = twime_model::norm::style_of(&s).unwrap_or(false);
                false
            }
        };
        if is_key && !self.st.composing() {
            // CACHE plan 3.3: a key that leaves no composition (deleted to empty, passed through, ...) ends the snapshot's use.
            self.st.doc = None;
        }
        if completed_key && self.st.syl.len() > old_len && self.pop_ready() && self.lexux_hint().is_none() {
            // The UI pool is built only by Idle, under the engine gate. Typing merely arms a generation;
            // an empty pool produces an empty Idle hint without changing the composition.
            self.pop_eligible = true;
        }
        let mut u = self.update(handled, commit);
        u.notice = self.notice.take();
        // Keep the pre-key state only if a commit has to be revertible.
        self.pending = Some(Pending { seq, commit: u.commit.is_some(), before: if u.commit.is_some() { before } else { None } });
        u
    }

    fn apply_ack(&mut self, ack: Option<CommitResult>) {
        let Some(p) = self.pending.take() else { return };
        match ack {
            Some(a) if a.seq == p.seq => {
                if a.commit == Applied::Failed || a.comp == Applied::Failed { self.invalidate_pop(); }
                if a.commit == Applied::Failed {
                    // The host did not take the text: back to before the key (the shell sends `Unsynced` next).
                    if let Some(b) = p.before {
                        // The style is a preference, not part of the text: a selection in the same event keeps it.
                        let style_tai = self.st.style_tai;
                        let changed = b.style_tai != style_tai;
                        self.st = b;
                        self.st.style_tai = style_tai;
                        if changed {
                            // The restored composition is written in the kept style (inference reused); the list is closed.
                            self.st.cand = None;
                            if self.st.conv.is_some() {
                                self.reselect();
                            }
                        }
                    }
                } else if a.comp == Applied::Failed {
                    self.st.clear_composition();
                }
            }
            _ => {
                // No usable confirmation of a commit: the remembered context may be wrong.
                if p.commit {
                    self.st.prev.clear();
                    self.st.recent_output = false;
                }
            }
        }
    }

    fn committable(&self) -> bool {
        if !self.st.run.is_empty() {
            return true;
        }
        !self.st.symbols && self.st.typing.is_empty() && !self.st.syl.is_empty() && self.st.conv.as_ref().is_some_and(|c| self.converted(c))
    }

    /// Appends committed text to the remembered context (the last `CONTEXT` characters).
    fn push_prev(&mut self, text: &str) {
        // CACHE plan 3.3: every commit passes here; the document changed, so the snapshot is stale from now on (also for a
        // conversion later in the same event, e.g. the 40th syllable).
        self.st.doc = None;
        self.st.prev.extend(text.chars());
        self.st.recent_output = !self.st.prev.is_empty();
        let n = self.st.prev.len();
        if n > CONTEXT {
            self.st.prev.drain(..n - CONTEXT);
        }
    }

    /// Text of the composition if committable (the converted sentence, or the English run); clears the composition and
    /// extends the context.
    fn take_commit(&mut self) -> Option<String> {
        self.take_commit_with(false)
    }

    /// NEUTRAL: `take_commit`, but a composition whose only obstacle is a position without a character commits what it
    /// shows, that position as its typed bopomofo (like a literal one). Enter and a punctuation / Shift+letter key use it
    /// (the 40th syllable too once the raw-key log is gone), so such a syllable never blocks the composition;
    /// Focus(false), Caps Lock, the mode toggle (raw-key rescue) and AddToLexicon keep the strict rule.
    fn take_commit_shown(&mut self) -> Option<String> {
        self.take_commit_with(true)
    }

    fn take_commit_with(&mut self, shown: bool) -> Option<String> {
        let strict = self.committable();
        let st = &self.st;
        let loose = shown && st.run.is_empty() && !st.symbols && st.typing.is_empty() && !st.syl.is_empty() && st.conv.is_some();
        if !strict && !loose {
            return None;
        }
        let text = if !self.st.run.is_empty() {
            self.st.run.clone()
        } else if strict {
            self.out_text()?
        } else {
            self.shown_text()?
        };
        // a composition with literal (or unconverted) bopomofo teaches nothing (its characters are not those of the positions)
        if strict && self.st.run.is_empty() && self.confirm_mode() && !self.has_lit() {
            // CACHE-c 2.1, 2.2: positions still locked by an explicit choice are the user's confirmed characters.
            let chars: Vec<char> = text.chars().collect();
            let chosen: Vec<bool> =
                self.st.locks.iter().zip(&self.st.groups).map(|(l, g)| l.is_some() && *g != 0 && self.st.explicit.contains(g)).collect();
            if chars.len() == chosen.len() {
                self.confirmed.commit(&self.engine.assets, &chars, &chosen);
            }
        }
        self.push_prev(&text);
        self.st.clear_composition();
        Some(text)
    }

    fn reconvert(&mut self) {
        self.validate_swaps();
        if self.st.syl.is_empty() {
            self.st.conv = None;
            return;
        }
        let ctx: String = self.st.prev.iter().collect();
        let syl = self.eff_syl();
        self.st.conv = Some(self.engine.assets.convert_cached_source(&ctx, self.st.doc.clone(), &syl, &self.st.locks, self.engine.cfg.lambda, self.st.style_tai, self.st.recent_output));
        self.apply_lexux();
    }

    /// FUZZY 3.3 / 3.5: what each position is for the repair.
    fn repair_kinds(&self) -> Vec<RepairKind> {
        // NEUTRAL: with F1 off an illegal neutral-tone syllable is still converted with a toned reading (session/neutral.rs)
        if self.engine.cfg.repair.is_none() { return self.neutral_kinds((0..self.st.syl.len()).map(|i| self.f2_kind(i)).collect()) }
        let cover = self.engine.assets.user_cover(&self.syl_view());
        self.repair_kinds_for_cover(&cover)
    }

    fn repair_kinds_for_cover(&self, cover: &[bool]) -> Vec<RepairKind> {
        let st = &self.st;
        let a = &self.engine.assets;
        let Some(r) = self.engine.cfg.repair.as_ref() else { return (0..st.syl.len()).map(|i| self.f2_kind(i)).collect() };
        let illegal: Vec<bool> = st.syl.iter().enumerate().map(|(i, s)| !st.lit[i] && !a.is_legal_syllable(s)).collect();
        if !illegal.contains(&true) {
            return (0..st.syl.len()).map(|i| self.f2_kind(i)).collect();
        }
        (0..st.syl.len())
            .map(|i| {
                if !illegal[i] {
                    return self.f2_kind(i);
                }
                let rs = r.index.repairs(&st.syl[i]);
                if cover[i] || rs.is_empty() {
                    return RepairKind::Typed;
                }
                match (st.repair.get(i).cloned().flatten(), r.mode) {
                    (Some(g), _) => RepairKind::Chosen(g),
                    (None, RepairMode::Auto) => RepairKind::Auto(rs[0].syl.clone()),
                    (None, RepairMode::Suggest) => RepairKind::Unchosen,
                }
            })
            .collect()
    }

    /// FUZZY 3.3: the effective syllables (exemption > the user's repair choice > the first of R(t) in `auto`); the typed
    /// syllables when the repair is off or every syllable is legal.
    fn eff_syl(&self) -> Vec<String> {
        self.eff_syl_from_kinds(self.repair_kinds())
    }

    fn eff_syl_from_kinds(&self, kinds: Vec<RepairKind>) -> Vec<String> {
        kinds.into_iter()
            .zip(&self.st.syl)
            .enumerate()
            .map(|(i, (k, s))| match k {
                // ZHUYIN-OUT 3.1: a literal position is, for the converter, a syllable without a legal character
                _ if self.st.lit[i] => zyout::mark(s),
                RepairKind::Chosen(g) | RepairKind::Auto(g) => g,
                _ => s.clone(),
            })
            .collect()
    }

    /// FUZZY 3.4 steps 1-4: the characters listed for one repair syllable.
    fn repair_chars(&self, g: &str) -> Vec<char> {
        let a = &self.engine.assets;
        // legal characters in id order; the sorts below are stable, so equal keys keep the id order
        let mut v = a.legal_chars(g);
        if a.has_prior() {
            v.sort_by_key(|&c| std::cmp::Reverse(a.prior_unigram(c).unwrap_or(0)));
        } else {
            let order = a.dictionary().ranked_single_chars(g);
            v.sort_by_key(|c| order.iter().position(|o| o == c).unwrap_or(usize::MAX));
        }
        let (tai, trad) = (twime_model::norm::TAI, twime_model::norm::TAI_TRAD);
        if let (Some(i), Some(j)) = (v.iter().position(|&c| c == tai), v.iter().position(|&c| c == trad)) {
            let at = i.min(j);
            v.retain(|&c| c != tai && c != trad);
            let (first, second) = if self.st.style_tai { (trad, tai) } else { (tai, trad) };
            v.insert(at, first);
            v.insert(at + 1, second);
        }
        v.truncate(REPAIR_CHARS);
        v
    }

    /// FUZZY 3.4 / 3.5: the items of the repair syllables of R(t) at `pos` other than `except`, in R(t) order, at most 40.
    fn repair_items(&self, pos: usize, except: Option<&str>) -> Vec<CandItem> {
        let Some(r) = self.engine.cfg.repair.as_ref() else { return vec![] };
        let mut out = vec![];
        for g in r.index.repairs(&self.st.syl[pos]).iter().map(|x| x.syl.as_str()).filter(|g| Some(*g) != except) {
            for c in self.repair_chars(g) {
                out.push(CandItem { text: c.to_string(), span: 1, repairs: vec![(pos, g.to_string())], extra: vec![], raw: false, lit: false });
            }
        }
        out.truncate(REPAIR_ITEMS);
        out
    }

    fn on_key_event(&mut self, ev: KeyEvent, commit: &mut Option<String>) -> bool {
        self.hint_mods = ev.mods;
        if self.password {
            return false;
        }
        let view = self.peek_view();
        if !peek(&view, ev) {
            return false;
        }
        if ev.key == Key::Backslash && !ev.mods.shift() && !ev.mods.caps() && self.switchable() && self.switch_lexux() {
            return true;
        }
        if self.ux {
            return self.on_key_ux(ev, commit);
        }
        self.plain_key(ev, commit)
    }

    // ---- version 3 key handling ---------------------------------------------------------

    fn on_key_ux(&mut self, ev: KeyEvent, commit: &mut Option<String>) -> bool {
        let (k, shift, caps) = (ev.key, ev.mods.shift(), ev.mods.caps());
        let text = keymap::char_of(k, shift).is_some();
        let cand_digit = !shift && matches!(k, Key::Digit(1..=9));

        if self.st.symbols {
            if cand_digit {
                let d = match k {
                    Key::Digit(d) => d as usize - 1,
                    _ => 0,
                };
                if let Some(s) = keymap::SYMBOL_TABLE.get(self.st.sym_page * keymap::SYMBOL_PAGE + d) {
                    self.commit_symbol(s, commit);
                }
                return true; // a digit beyond the last page's items does nothing
            }
            // Paging, as in the candidate window (wraps around).
            let pages = keymap::SYMBOL_TABLE.len().div_ceil(keymap::SYMBOL_PAGE);
            match k {
                Key::Right | Key::PageDown | Key::Down => {
                    self.st.sym_page = (self.st.sym_page + 1) % pages;
                    return true;
                }
                Key::Left | Key::PageUp | Key::Up => {
                    self.st.sym_page = (self.st.sym_page + pages - 1) % pages;
                    return true;
                }
                _ => {}
            }
            if k == Key::Esc {
                self.st.clear_composition();
                return true;
            }
            // Any other key closes the table (without committing the anchor) and is handled as if it had been typed alone.
            self.st.clear_composition();
            if !peek(&self.peek_view(), ev) {
                return false;
            }
        }
        // Caps Lock overlay: a key that types text ends the composition (commit when possible, else cancel) and passes on.
        if caps && text && self.st.composing() && !(cand_digit && self.st.cand.is_some()) {
            self.st.cand = None;
            if let Some(t) = self.take_commit() {
                *commit = Some(t);
            } else {
                self.st.clear_composition();
            }
            return false;
        }
        if !self.st.run.is_empty() {
            return self.run_key(ev, commit);
        }
        let composing = self.st.composing();
        if composing && k == Key::Enter && shift {
            // Raw-key rescue: the keys that were pressed become letters; not with the candidate window open.
            if self.st.cand.is_none()
                && let Some(t) = self.raw_text()
            {
                self.push_prev(&t);
                self.st.clear_composition();
                *commit = Some(t);
            }
            return true;
        }
        if composing && k == Key::Backquote && !shift {
            // Swallowed whatever else is open: it neither selects the highlighted candidate nor changes the composition.
            return true;
        }
        let bopomofo_key = !shift && is_zhuyin_key(k);
        if text && !bopomofo_key {
            if self.st.cand.is_some() {
                // The highlighted candidate is taken first; then the whole composition is committed with the character.
                let sel = self.st.cand.as_ref().map_or(0, |c| c.selected);
                // EMOJI: a highlighted emoji is committed first, ahead of what the key types
                let mut head = None;
                self.select_or_commit(sel, false, &mut head);
                let handled = self.text_key(ev, commit);
                if let Some(h) = head {
                    *commit = Some(h + commit.as_deref().unwrap_or(""));
                }
                return handled;
            }
            return self.text_key(ev, commit);
        }
        self.plain_key(ev, commit)
    }

    /// The rescue text: the pressed keys as US-position characters. `None` when the log is invalid or empty.
    fn raw_text(&self) -> Option<String> {
        let keys = self.st.raw.as_ref()?;
        let t: String = keys.iter().filter_map(|k| keymap::raw_char(*k)).collect();
        (!t.is_empty()).then_some(t)
    }

    /// A key that types a character and is not a bopomofo key: Shift+letter (English run), punctuation, symbols, number pad.
    fn text_key(&mut self, ev: KeyEvent, commit: &mut Option<String>) -> bool {
        let (k, shift) = (ev.key, ev.mods.shift());
        let converting = self.st.composing();
        if matches!(k, Key::Letter(_)) && shift {
            let Some(ch) = keymap::char_of(k, true) else { return true };
            if converting {
                match self.take_commit_shown() {
                    Some(t) => *commit = Some(t),
                    None => return true, // symbols still pending: nothing is committed
                }
            }
            self.st.run.push(ch);
            return true;
        }
        if k == Key::Backquote && !shift {
            if converting {
                return true;
            }
            if self.symbol_table {
                self.st.symbols = true;
                return true;
            }
        }
        let out = self.punct_text(k, shift);
        let head = if converting {
            match self.take_commit_shown() {
                Some(t) => t,
                None => return true,
            }
        } else {
            String::new()
        };
        self.push_prev(&out);
        *commit = Some(head + &out);
        true
    }

    /// Output of a punctuation / symbol key in the current style.
    fn punct_text(&self, k: Key, shift: bool) -> String {
        if !self.punct_half
            && let Some(f) = keymap::full_of(k, shift)
        {
            return f.to_string();
        }
        keymap::half_of(k, shift).map(String::from).unwrap_or_default()
    }

    /// Keys while an English run is being composed.
    fn run_key(&mut self, ev: KeyEvent, commit: &mut Option<String>) -> bool {
        let (k, shift) = (ev.key, ev.mods.shift());
        if keymap::char_of(k, shift).is_some() {
            if shift && !self.punct_half && keymap::full_of(k, true).is_some() {
                // A full-width mark ends the run: run and mark are committed together.
                let mut t = std::mem::take(&mut self.st.run);
                t.push_str(&self.punct_text(k, true));
                self.push_prev(&t);
                self.st.clear_composition();
                *commit = Some(t);
                return true;
            }
            if let Some(c) = keymap::half_of(k, shift) {
                self.st.run.push(c);
            }
            if self.st.run.chars().count() >= CONTEXT {
                *commit = self.take_commit();
            }
            return true;
        }
        match k {
            Key::Space | Key::Enter => *commit = self.take_commit(),
            Key::Esc => self.st.clear_composition(),
            Key::Backspace => {
                self.st.run.pop();
                if self.st.run.is_empty() {
                    self.st.clear_composition();
                }
            }
            // Tab, Delete, arrows, Home / End, Page keys: the run is committed and the key goes to the host.
            _ => {
                *commit = self.take_commit();
                return false;
            }
        }
        true
    }

    fn commit_symbol(&mut self, s: &str, commit: &mut Option<String>) {
        self.push_prev(s);
        self.st.clear_composition();
        *commit = Some(s.to_string());
    }

    /// `Event::ToggleMode` (plans 6.2): the composition is committed first when it can be.
    fn toggle_mode(&mut self, commit: &mut Option<String>) {
        if !self.ux || self.password {
            return;
        }
        if self.st.symbols {
            self.st.clear_composition();
        }
        if self.st.composing() {
            if let Some(t) = self.take_commit() {
                *commit = Some(t);
            } else if let Some(t) = self.raw_text() {
                // Not convertible but plainly typed: the keys become letters and the input switches to English.
                self.push_prev(&t);
                self.st.clear_composition();
                *commit = Some(t);
                self.st.mode = Mode::English;
                return;
            } else {
                return;
            }
        }
        self.st.mode = if self.st.mode == Mode::English { Mode::Chinese } else { Mode::English };
    }

    /// The bopomofo composition keys (version 1/2 behaviour; version 3 reaches it for bopomofo keys and non-text keys).
    fn plain_key(&mut self, ev: KeyEvent, commit: &mut Option<String>) -> bool {
        let k = ev.key;
        if self.st.cand.is_some() && self.candidate_key(k, ev.mods.shift(), commit) {
            return true;
        }
        if ev.mods.shift() {
            return true; // swallowed, no effect
        }
        if let Some(c) = keymap::symbol(k) {
            if let Some(log) = self.st.raw.as_mut() {
                log.push(k);
            }
            self.type_symbol(c, commit);
            return true;
        }
        match k {
            Key::Space => {
                if self.st.typing.has_body() {
                    if let Some(log) = self.st.raw.as_mut() {
                        log.push(k);
                    }
                    self.complete_syllable(commit, true);
                } else if self.st.typing.is_empty() {
                    self.open_candidates();
                }
            }
            Key::Down => {
                if self.st.typing.is_empty() {
                    self.open_candidates();
                }
            }
            Key::Enter => {
                if let Some(t) = self.take_commit_shown() {
                    *commit = Some(t);
                }
            }
            Key::Esc => self.st.clear_composition(),
            Key::Backspace => {
                self.st.raw = None;
                if !self.st.typing.is_empty() {
                    self.st.typing.backspace();
                } else if self.st.cursor > 0 {
                    let i = self.st.cursor - 1;
                    self.unlock_group_of(i);
                    self.st.syl.remove(i);
                    self.st.locks.remove(i);
                    self.st.groups.remove(i);
                    self.st.repair.remove(i);
                    self.st.lit.remove(i);
                    self.st.cursor = i;
                    self.reconvert();
                }
            }
            Key::Delete => {
                self.st.raw = None;
                if self.st.typing.is_empty() && self.st.cursor < self.st.syl.len() {
                    let i = self.st.cursor;
                    self.unlock_group_of(i);
                    self.st.syl.remove(i);
                    self.st.locks.remove(i);
                    self.st.groups.remove(i);
                    self.st.repair.remove(i);
                    self.st.lit.remove(i);
                    self.reconvert();
                }
            }
            Key::Left => {
                self.st.raw = None;
                if self.st.typing.is_empty() {
                    self.st.cursor = self.st.cursor.saturating_sub(1);
                }
            }
            Key::Right => {
                self.st.raw = None;
                if self.st.typing.is_empty() {
                    self.st.cursor = (self.st.cursor + 1).min(self.st.syl.len());
                }
            }
            Key::Home => {
                self.st.raw = None;
                if self.st.typing.is_empty() {
                    self.st.cursor = 0;
                }
            }
            Key::End => {
                self.st.raw = None;
                if self.st.typing.is_empty() {
                    self.st.cursor = self.st.syl.len();
                }
            }
            _ => {} // Tab, Up, PageUp, PageDown, ...: swallowed
        }
        true
    }

    fn type_symbol(&mut self, c: char, commit: &mut Option<String>) {
        match keymap::slot_of(c) {
            Some(Slot::Tone) => {
                if self.st.typing.has_body() {
                    self.st.typing.set(c);
                    self.complete_syllable(commit, false);
                } else if c == '˙' {
                    // Neutral tone typed first; the syllable completes at the next tone / Space.
                    self.st.typing.set(c);
                }
            }
            Some(_) => self.st.typing.set(c),
            None => {}
        }
    }

    /// Finishes the syllable being typed (tone from the tone slot, else the first tone).
    fn complete_syllable(&mut self, commit: &mut Option<String>, by_space: bool) {
        let single = by_space && self.st.typing.tone.is_none() && self.st.typing.body().chars().count() == 1;
        let s = self.st.typing.syllable();
        self.st.typing = Typing::default();
        if s.is_empty() {
            return;
        }
        if self.st.syl.len() >= MAX_SYLLABLES {
            // Before the 40th syllable the current result is committed. With a position without a character it is refused
            // while the raw-key rescue (Shift+Enter) still has the whole log; otherwise (NEUTRAL) that position is
            // committed as its typed bopomofo, so the new syllable is not dropped.
            let shown = if self.st.raw.is_some() { self.take_commit() } else { self.take_commit_shown() };
            match shown {
                Some(t) => {
                    *commit = Some(t);
                    self.st.raw = None;
                }
                None => return,
            }
        }
        let i = self.st.cursor.min(self.st.syl.len());
        if i == self.st.syl.len() {
            self.settle_completions_at_end();
        }
        self.unlock_groups_around(i);
        self.st.syl.insert(i, s);
        self.st.locks.insert(i, None);
        self.st.groups.insert(i, 0);
        self.st.repair.insert(i, None);
        // ZHUYIN-OUT 3.5: one symbol and Space, no character reads so and F1 has no repair for it: it stays as typed
        let lit = single && self.lone_symbol_stays(&self.st.syl[i]);
        self.st.lit.insert(i, lit);
        self.st.cursor = i + 1;
        self.reconvert();
    }

    fn open_candidates(&mut self) {
        if !self.engine.cfg.candidates || self.st.cursor == 0 {
            return;
        }
        let Some(conv) = &self.st.conv else { return };
        let pos = self.st.cursor - 1;
        if self.st.lit[pos] {
            return; // already the typed bopomofo (Backspace types it again)
        }
        let kinds = self.repair_kinds();
        if conv.chars.get(pos).copied().flatten().is_none() {
            // FUZZY 3.5: an unchosen illegal position in `suggest` opens the repair list; otherwise only the typed bopomofo
            // (ZHUYIN-OUT 3.2). With the typed bopomofo alone the raw-key rescue stays.
            let mut items = if kinds.get(pos) == Some(&RepairKind::Unchosen) { self.repair_items(pos, None) } else { vec![] };
            let rescue = items.is_empty();
            self.add_literal(pos, &mut items);
            self.st.cand = Some(Cand { pos, items, selected: 0 });
            if !rescue {
                self.st.raw = None;
            }
            return;
        }
        let cfg = &self.engine.cfg;
        // Without word candidates the span is 1: single characters, still ordered by the new model-based score.
        let max_span = if cfg.word_candidates { cfg.max_span.max(1) } else { 1 };
        let p = CandParams { lambda: cfg.lambda, beta: cfg.beta, max_span, word_cap: cfg.word_cap, style_tai: self.st.style_tai };
        let used = |j: usize| match kinds.get(j) {
            Some(RepairKind::Chosen(g) | RepairKind::Auto(g)) => Some((j, g.clone())),
            _ => None,
        };
        let cursor = self.st.cursor;
        let mut items: Vec<CandItem> = self
            .engine
            .assets
            .candidates(conv, cursor, &p)
            .into_iter()
            .map(|c| {
                let repairs = (cursor.saturating_sub(c.span)..cursor).filter_map(used).collect();
                CandItem { text: c.text, span: c.span, repairs, extra: vec![], raw: false, lit: false }
            })
            .collect();
        // FUZZY 3.4: a repaired position also lists the characters of its other repair syllables (after the current list)
        // (an illegal typed syllable only: a legal position changed by F2 has its way back in `add_fuzzy`)
        if let Some((_, g)) = used(pos).filter(|_| !self.engine.assets.is_legal_syllable(&self.st.syl[pos])) {
            items.extend(self.repair_items(pos, Some(&g)));
            // NEUTRAL (F1 off): the characters of the other toned readings of a neutral-tone position
            items.extend(self.neutral_items(pos, &g));
        }
        // FUZZY F2: word suggestions with one syllable changed by a confusion class (off = nothing added)
        self.add_fuzzy(&kinds, &mut items);
        // AUTOCOMP-LIST: completions of the end, inserted after the list above is complete (off = nothing added)
        if !self.pop_opening { self.add_autocomp_list(&mut items); }
        // EMOJI: after everything else (off = nothing added)
        if !self.pop_opening { self.add_emoji(&kinds, &mut items); }
        // ZHUYIN-OUT: the typed bopomofo is always the last item
        self.add_literal(pos, &mut items);
        if items.is_empty() {
            return;
        }
        self.st.cand = Some(Cand { pos, items, selected: 0 });
        self.st.raw = None;
    }

    /// Unlocks every position that was locked by the same selection as position `i`.
    fn unlock_group_of(&mut self, i: usize) {
        let g = self.st.groups.get(i).copied().unwrap_or(0);
        if g == 0 {
            return;
        }
        self.st.swaps.retain(|r| r.group != g);
        for j in 0..self.st.groups.len() {
            if self.st.groups[j] == g {
                self.st.groups[j] = 0;
                self.st.locks[j] = None;
            }
        }
    }

    /// A syllable is about to be inserted before index `i`: groups with members on both sides are dissolved.
    fn unlock_groups_around(&mut self, i: usize) {
        let mut dissolve: Vec<u32> = vec![];
        for g in self.st.groups[..i.min(self.st.groups.len())].iter().copied().filter(|g| *g != 0) {
            if self.st.groups[i.min(self.st.groups.len())..].contains(&g) && !dissolve.contains(&g) {
                dissolve.push(g);
            }
        }
        for g in dissolve {
            self.st.swaps.retain(|r| r.group != g);
            for j in 0..self.st.groups.len() {
                if self.st.groups[j] == g {
                    self.st.groups[j] = 0;
                    self.st.locks[j] = None;
                }
            }
        }
    }

    /// Keys while the candidate list is open. Returns true if the key was consumed here.
    fn candidate_key(&mut self, k: Key, shift: bool, commit: &mut Option<String>) -> bool {
        let page_size = self.engine.cfg.page_size.max(1);
        let Some((n, selected)) = self.st.cand.as_ref().map(|c| (c.items.len(), c.selected)) else { return false };
        let mut new_sel = selected;
        match k {
            Key::Digit(d @ 1..=9) if !shift => {
                let idx = selected / page_size * page_size + d as usize - 1;
                if d as usize <= page_size && idx < n {
                    self.select_or_commit(idx, true, commit);
                    return true;
                }
            }
            Key::Space | Key::Down => new_sel = (selected + 1) % n,
            Key::Up => new_sel = (selected + n - 1) % n,
            Key::Right | Key::PageDown => {
                let next = (selected / page_size + 1) * page_size;
                new_sel = if next < n { next } else { 0 };
            }
            Key::Left | Key::PageUp => {
                let page = selected / page_size;
                new_sel = if page > 0 { (page - 1) * page_size } else { (n - 1) / page_size * page_size };
            }
            Key::Enter => {
                self.select_or_commit(selected, true, commit);
                return true;
            }
            Key::Esc | Key::Backspace => {
                self.st.cand = None;
                return true;
            }
            Key::Tab => {
                if shift {
                    let page = selected / page_size;
                    new_sel = if page > 0 { (page - 1) * page_size } else { (n - 1) / page_size * page_size };
                } else {
                    let next = (selected / page_size + 1) * page_size;
                    new_sel = if next < n { next } else { 0 };
                }
            }
            Key::Home => new_sel = 0,
            Key::End => new_sel = n - 1,
            Key::Delete | Key::Digit(_) => {}
            // A bopomofo key (or anything else) ends selection and is typed normally.
            _ => {
                self.st.cand = None;
                return false;
            }
        }
        if let Some(c) = self.st.cand.as_mut() {
            c.selected = new_sel;
        }
        true
    }

    /// `explicit`: the user chose this item (CACHE-c 2.1), rather than the highlighted item being taken by a text key.
    fn select(&mut self, idx: usize, explicit: bool) {
        let Some(mut c) = self.st.cand.take() else { return };
        let Some(item0) = c.items.get(idx).cloned() else { return };
        if item0.lit {
            return self.select_literal(c.pos);
        }
        // AUTOCOMP-LIST 5.1: a completion first appends its syllables at the end; then it is an ordinary item over the
        // whole word (locked as one explicit selection)
        let mut item = item0;
        if !item.extra.is_empty() {
            let n = self.st.syl.len();
            if c.pos + 1 != n || n + item.extra.len() > MAX_SYLLABLES {
                return;
            }
            for syl in &item.extra {
                self.st.syl.push(syl.clone());
                self.st.locks.push(None);
                self.st.groups.push(0);
                self.st.repair.push(None);
                self.st.lit.push(false);
            }
            c.pos += item.extra.len();
            item.span += item.extra.len();
            item.extra.clear();
            self.st.cursor = self.st.syl.len();
        }
        let item = &item;
        self.st.raw = None;
        let chars: Vec<char> = item.text.chars().collect();
        let end = c.pos + 1;
        if item.span == 0 || item.span > end || chars.len() != item.span || end > self.st.locks.len() {
            return;
        }
        // A candidate replacing even part of a swap dissolves that entire swap first.
        for j in end - item.span..end {
            if self.st.swaps.iter().any(|r| r.group == self.st.groups[j]) {
                self.unlock_group_of(j);
            }
        }
        // FUZZY 3.3: choosing an item sets the repair syllable of each repaired position it covers; another reading than
        // the current one first unlocks the old group there
        if !item.repairs.is_empty() {
            let kinds = self.repair_kinds();
            for (j, g) in &item.repairs {
                let cur = match kinds.get(*j) {
                    Some(RepairKind::Chosen(x) | RepairKind::Auto(x)) => Some(x),
                    _ => None,
                };
                if cur != Some(g) {
                    self.unlock_group_of(*j);
                }
                if let Some(r) = self.st.repair.get_mut(*j) {
                    *r = Some(g.clone());
                }
            }
        }
        self.validate_swaps();
        let eff = self.eff_syl();
        // A selection locks exactly the span of the chosen item and starts a new group.
        if self.engine.assets.norm().out {
            // The user's own choice of 臺 (or of 台 where 臺 would also fit) sets the style of this context.
            let mut style = None;
            for (k, &ch) in chars.iter().enumerate() {
                let syl = &eff[end - item.span + k];
                if ch == twime_model::norm::TAI_TRAD {
                    style = Some(true);
                } else if ch == twime_model::norm::TAI && style.is_none() && self.engine.assets.is_legal_char(syl, twime_model::norm::TAI_TRAD) {
                    style = Some(false);
                }
            }
            if let Some(v) = style {
                self.st.style_tai = v;
            }
        }
        let g = self.st.next_group;
        self.st.next_group = self.st.next_group.wrapping_add(1).max(1);
        self.st.explicit.retain(|&x| x != g);
        if explicit {
            self.st.explicit.push(g);
        }
        for (k, ch) in chars.into_iter().enumerate() {
            self.st.locks[end - item.span + k] = Some(ch);
            self.st.groups[end - item.span + k] = g;
        }
        self.st.explicit.retain(|g| self.st.groups.contains(g));
        // Only the locks changed: the model input is the same, so the inference is reused (FUZZY 3.3: only while the
        // effective syllables are those of the inference).
        self.reselect();
    }

    fn update(&self, handled: bool, commit: Option<String>) -> Update {
        let st = &self.st;
        let composing = st.composing();
        let composition = composing.then(|| self.comp());
        let candidates = if st.symbols {
            let n = keymap::SYMBOL_TABLE.len();
            let items = keymap::SYMBOL_TABLE.iter().skip(st.sym_page * keymap::SYMBOL_PAGE).take(keymap::SYMBOL_PAGE).map(|s| s.to_string()).collect();
            Some(Cands { items, selected: 0, page: st.sym_page as u16, total: n as u16, per_page: keymap::SYMBOL_PAGE as u8 })
        } else {
            st.cand.as_ref().map(|c| {
                let ps = self.engine.cfg.page_size.max(1);
                let page = c.selected / ps;
                Cands {
                    items: c.items.iter().skip(page * ps).take(ps).map(CandItem::display).collect(),
                    selected: (c.selected % ps) as u16,
                    page: page as u16,
                    total: c.items.len().min(u16::MAX as usize) as u16,
                    per_page: ps.min(u8::MAX as usize) as u8,
                }
            })
        };
        Update {
            pop_gen: if self.pop_eligible && self.pop_ready() { self.pop_gen } else { 0 },
            completion: None,
            hint: self.lexux_hint(),
            toggle_key: self.user_toggle(), notice: None,
            handled,
            commit,
            composition,
            candidates_open: candidates.is_some(),
            candidates,
            composing,
            committable: self.committable(),
            mode: self.mode(),
            engine_us: 0,
            punct_half: self.ux && self.punct_half,
            run: !st.run.is_empty(),
            symbols: st.symbols,
        }
    }

    fn comp(&self) -> Comp {
        let st = &self.st;
        if st.symbols {
            return Comp { segments: vec![Segment { text: "`".into(), pending: true, ..Default::default() }], cursor: 1 };
        }
        if !st.run.is_empty() {
            let n = st.run.chars().count().min(u16::MAX as usize) as u16;
            return Comp { segments: vec![Segment { text: st.run.clone(), run: true, ..Default::default() }], cursor: n };
        }
        let mut segments: Vec<Segment> = Vec::with_capacity(st.syl.len() + 1);
        let chars = |v: &[Segment]| -> usize { v.iter().map(|g| g.text.chars().count()).sum() };
        let mut cursor = 0usize;
        let spans = self.lexux_spans();
        let active = self.active_lexux(&spans);
        for i in 0..=st.syl.len() {
            if i == st.cursor {
                if !st.typing.is_empty() {
                    segments.push(Segment { text: st.typing.syllable(), pending: true, ..Default::default() });
                }
                cursor = chars(&segments);
            }
            if let Some(s) = st.syl.get(i) {
                if st.lit[i] {
                    segments.push(Segment { text: s.clone(), ..Default::default() });
                    continue;
                }
                let ch = st.conv.as_ref().and_then(|c| c.chars.get(i).copied().flatten());
                segments.push(match ch {
                    Some(c) => Segment { hint: spans.iter().enumerate().find(|(_, h)| h.start <= i && i < h.start + h.len).map_or(0, |(j, _)| if Some(j) == active { 2 } else { 1 }), text: c.to_string(), locked: st.locks.get(i).copied().flatten().is_some(), ..Default::default() },
                    None => Segment { text: s.clone(), unconverted: true, ..Default::default() },
                });
            }
        }
        Comp { segments, cursor: cursor.min(u16::MAX as usize) as u16 }
    }
}

/// FUZZY: one position of the composition for the repair.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RepairKind {
    /// Converted with the typed syllable (legal, exempt by a user word, no repair candidate, or the repair is off).
    Typed,
    /// Illegal, converted with the repair syllable the user chose.
    Chosen(String),
    /// Illegal, converted with the first repair syllable (`auto`).
    Auto(String),
    /// Illegal, not chosen yet (`suggest`): stays unconverted.
    Unchosen,
}

#[cfg(test)]
#[path = "session/emoji_tests.rs"]
mod emoji_tests;
#[cfg(test)]
#[path = "session/zyout_tests.rs"]
mod zyout_tests;
#[cfg(test)]
mod lexux_frozen;
#[cfg(test)]
mod lexux_tests;
#[cfg(test)]
#[path = "session/fuzzy_hints_tests.rs"]
mod fuzzy_hints_tests;
#[cfg(test)]
#[path = "session/autocomp_tests.rs"]
mod autocomp_tests;
#[cfg(test)]
#[path = "session/aclist_tests.rs"]
mod aclist_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod word_tests;
#[cfg(test)]
mod norm_tests;
#[cfg(test)]
mod ux_tests;
#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod repair_tests;
#[cfg(test)]
#[path = "session/neutral_tests.rs"]
mod neutral_tests;
