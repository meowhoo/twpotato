//! Same-document character-bigram cache (Python reference train/cache_ref.py).
//!
//! The set holds every pair of adjacent characters of the last `window` characters of the document text before the
//! composition (both in the model vocabulary; 臺 read as 台). In the PN beam a generated character whose pair with the
//! previous character of the same conversion is in the set adds `gamma` to its path, at most `cap` times per path.
//! An empty set changes nothing (no floating-point operation is added), so it is B0 bit for bit. Nothing is stored
//! beyond the set built from the text the caller passes; nothing is written or logged.

use std::collections::{HashMap, HashSet};

use crate::{convert::Assets, norm};

/// Largest window (the shell sends at most `twime_proto::DOC_TEXT_UNITS` = 8192 UTF-16 units).
pub const MAX_WINDOW: usize = 8192;

/// Decoder settings of the cache (pn.cfg keys `cache_gamma`, `cache_window`, `cache_cap`).
#[derive(Clone, Debug, PartialEq)]
pub struct CacheCfg {
    /// Bonus of one hit (nat; not multiplied by lambda).
    pub gamma: f64,
    /// Characters of the document text before the composition that are used.
    pub window: usize,
    /// Most hits that score per path; `None` = no limit.
    pub cap: Option<usize>,
    /// Which pairs of the document text count: every pair (`Doc`, the first phase)
    /// or only the pairs the user confirmed in this session (`Confirmed`, CACHE-c).
    pub source: CacheSource,
}

/// Key `cache_source` (`doc` when absent).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CacheSource {
    #[default]
    Doc,
    Confirmed,
}

impl CacheCfg {
    /// From settings keys; `None` when `cache_gamma` is absent or `off`. Checks: gamma finite and >= 0, 2 <= window <=
    /// 8192 (default 64), cap an integer >= 1 or `inf` (default).
    pub fn from_settings(m: &HashMap<String, String>) -> std::io::Result<Option<CacheCfg>> {
        let bad = |k: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bad {k}"));
        let Some(g) = m.get("cache_gamma").filter(|g| g.as_str() != "off") else { return Ok(None) };
        let gamma: f64 = g.parse().map_err(|_| bad("cache_gamma"))?;
        if !gamma.is_finite() || gamma < 0.0 {
            return Err(bad("cache_gamma"));
        }
        let window: usize = m.get("cache_window").map_or(Ok(64), |w| w.parse()).map_err(|_| bad("cache_window"))?;
        if !(2..=MAX_WINDOW).contains(&window) {
            return Err(bad("cache_window"));
        }
        let cap = match m.get("cache_cap").map(String::as_str) {
            None | Some("inf") => None,
            Some(k) => Some(k.parse::<usize>().ok().filter(|&k| k >= 1).ok_or_else(|| bad("cache_cap"))?),
        };
        let source = match m.get("cache_source").map(String::as_str) {
            None | Some("doc") => CacheSource::Doc,
            Some("confirmed") => CacheSource::Confirmed,
            Some(_) => return Err(bad("cache_source")),
        };
        Ok(Some(CacheCfg { gamma, window, cap, source }))
    }

    /// `on:gamma:window:cap:source` for the engine's ready line.
    pub fn describe(&self) -> String {
        let src = match self.source {
            CacheSource::Doc => "doc",
            CacheSource::Confirmed => "confirmed",
        };
        format!("on:{}:{}:{}:{src}", self.gamma, self.window, self.cap.map_or("inf".to_string(), |k| k.to_string()))
    }
}

fn key(a: u32, b: u32) -> u64 {
    (u64::from(a) << 32) | u64::from(b)
}

/// Most confirmed runs a session remembers (CACHE-c 2.2); the oldest go first. Every `DocText` also drops the runs that are
/// not in the window, so in practice the window bounds it.
pub const MAX_RUNS: usize = 1024;

/// Anchor length (CACHE-c 2.2): the last committed conversion of at least this many characters marks the document.
pub const ANCHOR_MIN: usize = 4;

/// CACHE-c: what this session confirmed. `runs` = runs of characters the user
/// explicitly chose in the candidate window (two or more adjacent chosen positions of one committed conversion); `anchor` =
/// the session's last committed conversion of at least `ANCHOR_MIN` characters (the latest one while there is none that
/// long). Vocabulary id sequences (臺 read as 台; None outside the vocabulary), never logged; identical runs kept once.
#[derive(Clone, Debug, Default)]
pub struct Confirmed {
    runs: std::collections::VecDeque<Vec<u32>>,
    /// Last committed conversion of at least `ANCHOR_MIN` characters.
    anchor_long: Option<Vec<Option<u32>>>,
    /// Last committed conversion (the anchor while there is no long one).
    anchor_last: Option<Vec<Option<u32>>>,
    #[cfg(test)]
    test_bound: Option<usize>,
}

fn ids_of(assets: &Assets, text: &[char]) -> Vec<Option<u32>> {
    let tai = (assets.vocab_id(norm::TAI_TRAD), assets.vocab_id(norm::TAI));
    text.iter()
        .map(|&c| {
            let i = assets.vocab_id(c);
            (i != crate::vocab::UNK).then_some(if i == tai.0 { tai.1 } else { i })
        })
        .collect()
}

impl Confirmed {
    /// A committed conversion `text` (one char per syllable) with the positions the user explicitly chose: records its
    /// runs (maximal stretches of chosen in-vocabulary positions, two or more long) and updates the anchor.
    pub fn commit(&mut self, assets: &Assets, text: &[char], chosen: &[bool]) {
        let ids = ids_of(assets, text);
        if !ids.is_empty() {
            if ids.len() >= ANCHOR_MIN {
                self.anchor_long = Some(ids.clone());
            }
            self.anchor_last = Some(ids.clone());
        }
        let mut cur: Vec<u32> = vec![];
        for i in 0..=ids.len() {
            let x = if i < ids.len() && chosen.get(i).copied().unwrap_or(false) { ids[i] } else { None };
            match x {
                Some(v) => cur.push(v),
                None => {
                    if cur.len() >= 2 && !self.runs.contains(&cur) {
                        self.runs.push_back(std::mem::take(&mut cur));
                        if self.runs.len() > self.bound() {
                            self.runs.pop_front();
                        }
                    }
                    cur.clear();
                }
            }
        }
    }
    fn bound(&self) -> usize {
        #[cfg(test)]
        if let Some(b) = self.test_bound {
            return b;
        }
        MAX_RUNS
    }
    /// Forgets everything (runs and anchors).
    pub fn clear(&mut self) {
        self.runs.clear();
        self.anchor_long = None;
        self.anchor_last = None;
    }
    /// The anchor in force: the long one, else the last commit.
    fn anchor(&self) -> Option<&Vec<Option<u32>>> {
        self.anchor_long.as_ref().or(self.anchor_last.as_ref())
    }
    /// Runs remembered.
    pub fn len(&self) -> usize {
        self.runs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

fn occurs(hay: &[Option<u32>], needle: &[Option<u32>]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// The bigram set of the last `window` characters of a text (product and evaluation tools build it the same way), with
/// the bonus settings it was built for (so one `Assets` serves engines with different cache settings).
#[derive(Clone, Debug, Default)]
pub struct CacheSet {
    pairs: HashSet<u64>,
    tai: (u32, u32),
    gamma: f64,
    cap: Option<usize>,
    /// Built from the session's confirmed runs (`from_text_confirmed`): only such a set supports a ㄊㄚ variant (TA plan).
    confirmed: bool,
}

impl CacheSet {
    /// The set of the last `cfg.window` characters of `text`.
    pub fn from_text(assets: &Assets, text: &str, cfg: &CacheCfg) -> CacheSet {
        Self::build(assets, text, cfg)
    }

    /// CACHE-c 2.2, 2.3, for a `DocText`: if the session has runs and its anchor is not in the last `cfg.window` characters
    /// of `text` (another document, or the text was deleted or left the window), everything is forgotten; otherwise the runs
    /// that do not occur whole in the window are dropped. The set = the pairs inside the remaining runs.
    pub fn from_text_confirmed(assets: &Assets, text: &str, cfg: &CacheCfg, confirmed: &mut Confirmed) -> CacheSet {
        let tai = (assets.vocab_id(norm::TAI_TRAD), assets.vocab_id(norm::TAI));
        let empty = CacheSet { pairs: HashSet::new(), tai, gamma: cfg.gamma, cap: cfg.cap, confirmed: true };
        let Some(anchor) = confirmed.anchor() else {
            // nothing committed in this session (or forgotten): no runs either
            confirmed.runs.clear();
            return empty;
        };
        let ids = Self::window_ids(assets, text, cfg.window);
        // the anchor is checked even without runs, so that a stale anchor cannot later drop new confirmations
        if !occurs(&ids, anchor) {
            confirmed.clear();
            return empty;
        }
        if confirmed.runs.is_empty() {
            return empty;
        }
        // start positions of every adjacent pair of the window
        let mut at: std::collections::HashMap<u64, Vec<usize>> = std::collections::HashMap::new();
        for i in 1..ids.len() {
            if let (Some(p), Some(x)) = (ids[i - 1], ids[i]) {
                at.entry(key(p, x)).or_default().push(i - 1);
            }
        }
        let whole = |r: &Vec<u32>| {
            at.get(&key(r[0], r[1])).is_some_and(|st| st.iter().any(|&s| s + r.len() <= ids.len() && ids[s..s + r.len()].iter().zip(r).all(|(a, b)| *a == Some(*b))))
        };
        confirmed.runs.retain(|r| whole(r));
        let pairs = confirmed.runs.iter().flat_map(|r| r.windows(2).map(|w| key(w[0], w[1]))).collect();
        CacheSet { pairs, ..empty }
    }

    /// Ids of the last `window` characters (臺 read as 台; None outside the vocabulary).
    fn window_ids(assets: &Assets, text: &str, window: usize) -> Vec<Option<u32>> {
        let tai = (assets.vocab_id(norm::TAI_TRAD), assets.vocab_id(norm::TAI));
        let n = text.chars().count();
        text.chars()
            .skip(n.saturating_sub(window))
            .map(|c| {
                let id = assets.vocab_id(c);
                (id != crate::vocab::UNK).then_some(if id == tai.0 { tai.1 } else { id })
            })
            .collect()
    }

    fn build(assets: &Assets, text: &str, cfg: &CacheCfg) -> CacheSet {
        let tai = (assets.vocab_id(norm::TAI_TRAD), assets.vocab_id(norm::TAI));
        let ids = Self::window_ids(assets, text, cfg.window);
        let pairs = ids.windows(2).filter_map(|w| Some(key(w[0]?, w[1]?))).collect();
        CacheSet { pairs, tai, gamma: cfg.gamma, cap: cfg.cap, confirmed: false }
    }

    /// Built from confirmed runs (TA plan 1, exception 2).
    pub fn is_confirmed(&self) -> bool {
        self.confirmed
    }

    /// A set of id pairs given directly (shared test cases with train/cache_ref.py); `tai` = (臺 id, 台 id).
    pub fn from_pairs(pairs: &[(u32, u32)], tai: (u32, u32), gamma: f64, cap: Option<usize>) -> CacheSet {
        let f = |id: u32| if id == tai.0 { tai.1 } else { id };
        CacheSet { pairs: pairs.iter().map(|&(a, b)| key(f(a), f(b))).collect(), tai, gamma, cap, confirmed: false }
    }

    /// The beam's view of a non-empty set (None for an empty one: no operation at all).
    pub fn bonus(&self) -> Option<CacheBonus<'_>> {
        (!self.pairs.is_empty()).then_some(CacheBonus { set: self, gamma: self.gamma, cap: self.cap })
    }

    fn fold(&self, id: u32) -> u32 {
        if id == self.tai.0 { self.tai.1 } else { id }
    }

    /// Whether the pair (previous id, id) occurs in the window (臺 read as 台 on both sides).
    pub fn contains(&self, prev: u32, id: u32) -> bool {
        self.pairs.contains(&key(self.fold(prev), self.fold(id)))
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
    /// Distinct pairs.
    pub fn len(&self) -> usize {
        self.pairs.len()
    }
}

/// What the beam needs: the set and the settings.
#[derive(Clone, Copy)]
pub struct CacheBonus<'a> {
    pub set: &'a CacheSet,
    pub gamma: f64,
    pub cap: Option<usize>,
}

impl CacheBonus<'_> {
    /// Hit of `id` after `prev` (None: first character of the conversion, never a hit), if `used` hits still score.
    #[inline]
    pub fn hit(&self, prev: Option<u32>, id: u32, used: usize) -> bool {
        match prev {
            Some(p) => self.cap.is_none_or(|k| used < k) && self.set.contains(p, id),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings() {
        let m = |kv: &[(&str, &str)]| kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
        assert_eq!(CacheCfg::from_settings(&m(&[])).unwrap(), None);
        assert_eq!(CacheCfg::from_settings(&m(&[("cache_gamma", "off")])).unwrap(), None);
        assert_eq!(CacheCfg::from_settings(&m(&[("cache_gamma", "4")])).unwrap(), Some(CacheCfg { gamma: 4.0, window: 64, cap: None, source: CacheSource::Doc }));
        assert_eq!(
            CacheCfg::from_settings(&m(&[("cache_gamma", "3"), ("cache_window", "4000"), ("cache_cap", "2")])).unwrap(),
            Some(CacheCfg { gamma: 3.0, window: 4000, cap: Some(2), source: CacheSource::Doc })
        );
        for bad in [("cache_gamma", "-1"), ("cache_gamma", "nan"), ("cache_gamma", "x")] {
            assert!(CacheCfg::from_settings(&m(&[bad])).is_err());
        }
        for bad in [("cache_window", "1"), ("cache_window", "8193"), ("cache_cap", "0"), ("cache_cap", "-2")] {
            assert!(CacheCfg::from_settings(&m(&[("cache_gamma", "4"), bad])).is_err());
        }
    }

    #[test]
    fn pairs_window_fold() {
        let a = Assets::from_dictionary_text("ㄊㄞˊ 台 -3\nㄊㄞˊ 臺 -4\nㄅㄟˇ 北 -3\nㄕˋ 市 -3\n", "").unwrap();
        let id = |c: char| a.vocab_id(c);
        let cfg = |w| CacheCfg { gamma: 1.0, window: w, cap: Some(1), source: CacheSource::Doc };
        let s = CacheSet::from_text(&a, "臺北，市", &cfg(64));
        assert!(s.contains(id('台'), id('北')) && s.contains(id('臺'), id('北')));
        assert!(!s.contains(id('北'), id('市')), "punctuation breaks the pair");
        assert_eq!(s.len(), 1);
        // window: only the last 2 characters
        let s = CacheSet::from_text(&a, "台北市", &cfg(2));
        assert!(!s.contains(id('台'), id('北')) && s.contains(id('北'), id('市')));
        assert!(CacheSet::from_text(&a, "", &cfg(64)).is_empty() && CacheSet::from_text(&a, "", &cfg(64)).bonus().is_none());
        let b = s.bonus().unwrap();
        assert!(b.hit(Some(id('北')), id('市'), 0) && !b.hit(Some(id('北')), id('市'), 1) && !b.hit(None, id('市'), 0));
    }

    #[test]
    fn source_key() {
        let m = |kv: &[(&str, &str)]| kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
        let c = CacheCfg::from_settings(&m(&[("cache_gamma", "4"), ("cache_source", "confirmed")])).unwrap().unwrap();
        assert_eq!(c.source, CacheSource::Confirmed);
        assert_eq!(c.describe(), "on:4:64:inf:confirmed");
        assert_eq!(CacheCfg::from_settings(&m(&[("cache_gamma", "4"), ("cache_source", "doc")])).unwrap().unwrap().source, CacheSource::Doc);
        assert!(CacheCfg::from_settings(&m(&[("cache_gamma", "4"), ("cache_source", "x")])).is_err());
    }

    #[test]
    fn confirmed_runs() {
        let a = Assets::from_dictionary_text("ㄊㄞˊ 台 -3\nㄊㄞˊ 臺 -4\nㄅㄟˇ 北 -3\nㄕˋ 市 -3\nㄉㄚˋ 大 -3\nㄒㄧㄝˊ 鞋 -3\n", "").unwrap();
        let id = |c: char| a.vocab_id(c);
        let cfg = CacheCfg { gamma: 1.0, window: 64, cap: None, source: CacheSource::Confirmed };
        let set = |t: &str, r: &mut Confirmed| CacheSet::from_text_confirmed(&a, t, &cfg, r);
        let mut r = Confirmed::default();
        // one chosen character alone is no run; 市大 (both chosen) is; the commit 臺北市大 is the anchor
        r.commit(&a, &['臺', '北', '市', '大'], &[true, false, true, true]);
        assert_eq!(r.len(), 1);
        r.commit(&a, &['臺', '北', '市', '大'], &[true, false, true, true]);
        assert_eq!(r.len(), 1, "identical runs kept once");
        let s = set("鞋台北市大", &mut r);
        assert!(s.contains(id('市'), id('大')) && s.len() == 1 && r.len() == 1);
        // the anchor (the last commit of >= 4 characters) is not in the text: everything is forgotten
        let s = set("北市大鞋", &mut r);
        assert!(s.is_empty() && s.bonus().is_none() && r.is_empty());
        // runs not in the window are dropped, the others kept
        let mut r2 = Confirmed::default();
        r2.commit(&a, &['臺', '北'], &[true, true]);
        r2.commit(&a, &['市', '大', '鞋', '北'], &[true, true, false, false]);
        assert_eq!(r2.len(), 2);
        let s = set("市大鞋北", &mut r2);
        assert!(s.contains(id('市'), id('大')) && !s.contains(id('台'), id('北')) && r2.len() == 1);
        // a short commit does not replace a long anchor; the window counts
        r2.commit(&a, &['臺', '北'], &[true, true]);
        assert!(!set("台北市大鞋北", &mut r2).is_empty());
        let small = CacheCfg { window: 3, ..cfg.clone() };
        assert!(CacheSet::from_text_confirmed(&a, "市大鞋北", &small, &mut r2).is_empty() && r2.is_empty());
        // characters outside the vocabulary end a run; nothing chosen records nothing
        let mut r3 = Confirmed::default();
        r3.commit(&a, &['北', 'X', '市'], &[true, true, true]);
        r3.commit(&a, &['北', '市'], &[false, false]);
        assert!(r3.is_empty());
        // first in, first out at the bound
        let mut r4 = Confirmed { test_bound: Some(2), ..Default::default() };
        r4.commit(&a, &['北', '市'], &[true, true]);
        r4.commit(&a, &['市', '大'], &[true, true]);
        r4.commit(&a, &['大', '鞋'], &[true, true]);
        assert_eq!(r4.len(), 2);
        let s = set("北市大鞋", &mut r4);
        assert!(!s.contains(id('北'), id('市')) && s.contains(id('市'), id('大')) && s.contains(id('大'), id('鞋')));
        r4.clear();
        assert!(r4.is_empty());
        // without a long commit the last commit is the anchor (consecutive short commits)
        let mut r5 = Confirmed::default();
        r5.commit(&a, &['鞋', '鞋'], &[false, false]);
        r5.commit(&a, &['北', '市'], &[true, true]);
        assert!(!set("大北市", &mut r5).is_empty() && r5.len() == 1, "the first short commit is not the anchor any more");
        // a stale anchor is checked even without runs: it cannot drop a later confirmation
        let mut r6 = Confirmed::default();
        r6.commit(&a, &['大', '鞋', '大', '鞋'], &[false, false, false, false]);
        assert!(set("北市", &mut r6).is_empty() && r6.anchor().is_none());
        r6.commit(&a, &['北', '市'], &[true, true]);
        assert!(!set("台北市", &mut r6).is_empty());
    }
}
