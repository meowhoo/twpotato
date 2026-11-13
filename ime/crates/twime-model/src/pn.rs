//! Proper-name lexicon bonus (PN-plan section 2; Python reference train/ent_pn.py).
//!
//! The beam of `prior::beam` is extended with whole-word jumps: at a start position every lexicon word whose reading
//! equals the syllables there (2..=8 syllables, tones included) is appended in one step, with the same per-character
//! score as a single-character extension (float64 log_softmax over all legal characters, 0 at a locked position, plus
//! lambda * prior) and then a bonus b (b2 for two-character words, b3 for longer ones; `None` = no jumps of that length).
//! States are kept per end position, deduplicated by their full id prefix (best score; ties: single extension before
//! jumps, longer jumps first), sorted by (score desc, id sequence asc) and cut to `BEAM`.
//!
//! Storage (ENT-plan E6): `<base>.fst` maps the reading (each syllable as its 2-byte big-endian vocabulary id) to an
//! offset in `<base>.lst`, which holds every word of that reading (u8 count, then per word u8 length, u8 class, u16 ids).
//! Both files are memory-mapped. The `.lst` header carries a fingerprint of the vocabulary and legal table; a lexicon
//! built for other assets is refused. Legality is checked when building, not at start-up.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

use crate::{
    convert::Assets,
    prior::{self, BEAM},
};

const MAGIC: &[u8; 8] = b"TWPN1\0\0\0";
const HEADER: usize = 8 + 4 + 8 + 4 + 4;
/// Longest lexicon word in syllables.
pub const MAX_LEN: usize = 8;
/// Most lexicon words shown in the candidate list (on top of `word_cap`).
pub const CAND_CAP: usize = 20;

/// Decoder settings of the bonus.
#[derive(Clone, Debug, PartialEq)]
pub struct PnCfg {
    /// Bonus of two-character jumps; `None` = no two-character jumps.
    pub b2: Option<f64>,
    /// Bonus of jumps of three or more characters; `None` = none.
    pub b3: Option<f64>,
    /// Hits kept per (start, length), best by the sum of their log_softmax; `None` = all.
    pub h: Option<usize>,
    /// Lexicon words in the candidate list.
    pub cands: bool,
}

impl Default for PnCfg {
    fn default() -> PnCfg {
        PnCfg { b2: None, b3: None, h: None, cands: true }
    }
}

impl PnCfg {
    pub fn bonus(&self, len: usize) -> Option<f64> {
        if len == 2 { self.b2 } else { self.b3 }
    }
}

/// The shared settings file of the Rust and Python runs: `key=value` lines (`#` comments). Keys: `lexicon` (base path,
/// relative to `root`), `b2`, `b3` (`off` or a number), `h` (`inf` or a count), `cands` (0/1); other keys (model, prior,
/// norm, lambda) are read by the callers.
pub fn read_settings(path: &Path) -> io::Result<HashMap<String, String>> {
    let mut m = HashMap::new();
    for line in std::fs::read_to_string(path)?.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    Ok(m)
}

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

pub fn parse_bonus(s: &str) -> io::Result<Option<f64>> {
    if s == "off" { Ok(None) } else { s.parse::<f64>().map(Some).map_err(|_| bad(format!("bad bonus {s}"))) }
}

pub fn parse_h(s: &str) -> io::Result<Option<usize>> {
    if s == "inf" { Ok(None) } else { s.parse::<usize>().map(Some).map_err(|_| bad(format!("bad h {s}"))) }
}

impl PnCfg {
    pub fn from_settings(m: &HashMap<String, String>) -> io::Result<PnCfg> {
        let get = |k: &str, d: &str| m.get(k).cloned().unwrap_or_else(|| d.to_string());
        Ok(PnCfg { b2: parse_bonus(&get("b2", "off"))?, b3: parse_bonus(&get("b3", "off"))?, h: parse_h(&get("h", "inf"))?, cands: get("cands", "1") == "1" })
    }
}

/// A memory-mapped lexicon.
pub struct PnLexicon {
    map: fst::Map<memmap2::Mmap>,
    lst: memmap2::Mmap,
    entries: usize,
    keys: usize,
}

/// Fingerprint of the vocabulary and the legal table (FNV-1a 64 over the sorted syllables and their legal ids).
pub fn fingerprint(a: &Assets) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h ^= u64::from(x);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&(a.vocab.len() as u64).to_le_bytes());
    let mut syl: Vec<&String> = a.legal.keys().collect();
    syl.sort();
    for s in syl {
        eat(s.as_bytes());
        eat(&a.vocab.syllable_id(s).to_le_bytes());
        eat(&[0xff]);
        for id in &a.legal[s] {
            eat(&id.to_le_bytes());
        }
    }
    h
}

fn paths(base: &Path) -> (PathBuf, PathBuf) {
    let s = base.as_os_str().to_string_lossy();
    (PathBuf::from(format!("{s}.fst")), PathBuf::from(format!("{s}.lst")))
}

/// Counts of a build.
#[derive(Debug, Default, PartialEq)]
pub struct BuildStats {
    pub kept: usize,
    pub keys: usize,
    pub illegal: usize,
    pub bad_length: usize,
    pub duplicate: usize,
}

/// One input word: text, reading (syllables joined with `-`), class code.
pub struct BuildWord {
    pub word: String,
    pub reading: String,
    pub class: u8,
}

/// Writes `<base>.fst` and `<base>.lst` for `assets`. Words with a character that is not legal for its syllable, or a
/// length outside 2..=8, are dropped and counted.
pub fn build(assets: &Assets, words: &[BuildWord], base: &Path) -> io::Result<BuildStats> {
    build_impl(assets, words, base, false)
}

/// J retains the complete jump=1 source set. Readings outside the deployment table remain stored: unknown syllables
/// use UNK (syl_codes returns None, so these never match); other illegal pairs are rejected by usable_jumps.
pub fn build_j(assets: &Assets, words: &[BuildWord], base: &Path) -> io::Result<BuildStats> {
    if words.iter().any(|w| !(3..=MAX_LEN).contains(&w.word.chars().count()) || w.class > 3) {
        return Err(bad("genj: invalid length or tier".into()));
    }
    build_impl(assets, words, base, true)
}

fn build_impl(assets: &Assets, words: &[BuildWord], base: &Path, retain_known: bool) -> io::Result<BuildStats> {
    assert!(assets.vocab.len() < 1 << 16, "vocabulary ids must fit in u16");
    let mut st = BuildStats::default();
    let mut by_key: std::collections::BTreeMap<Vec<u8>, Vec<(u8, Vec<u16>)>> = Default::default();
    for w in words {
        let chars: Vec<char> = w.word.chars().collect();
        let syl: Vec<&str> = w.reading.split('-').collect();
        if chars.len() != syl.len() || !(2..=MAX_LEN).contains(&chars.len()) {
            st.bad_length += 1;
            continue;
        }
        let known = chars.iter().all(|&c| assets.vocab.char_id(c) != crate::vocab::UNK);
        if !(chars.iter().zip(&syl).all(|(&c, s)| assets.is_legal_char(s, c)) || retain_known && known) {
            st.illegal += 1;
            continue;
        }
        let key: Vec<u8> = syl.iter().flat_map(|s| (assets.vocab.syllable_id(s) as u16).to_be_bytes()).collect();
        let ids: Vec<u16> = chars.iter().map(|&c| assets.vocab.char_id(c) as u16).collect();
        let list = by_key.entry(key).or_default();
        if list.iter().any(|(_, x)| *x == ids) {
            st.duplicate += 1;
            continue;
        }
        assert!(list.len() < 255, "too many words for one reading");
        list.push((w.class, ids));
        st.kept += 1;
    }
    st.keys = by_key.len();
    let mut lst: Vec<u8> = Vec::with_capacity(HEADER + 8 * st.kept);
    lst.extend_from_slice(MAGIC);
    lst.extend_from_slice(&1u32.to_le_bytes());
    lst.extend_from_slice(&fingerprint(assets).to_le_bytes());
    lst.extend_from_slice(&(st.kept as u32).to_le_bytes());
    lst.extend_from_slice(&(st.keys as u32).to_le_bytes());
    let (fp, lp) = paths(base);
    let mut mb = fst::MapBuilder::new(io::BufWriter::new(std::fs::File::create(&fp)?)).map_err(io::Error::other)?;
    for (key, list) in &by_key {
        mb.insert(key, lst.len() as u64).map_err(io::Error::other)?;
        lst.push(list.len() as u8);
        for (class, ids) in list {
            lst.push(ids.len() as u8);
            lst.push(*class);
            for id in ids {
                lst.extend_from_slice(&id.to_le_bytes());
            }
        }
    }
    mb.finish().map_err(io::Error::other)?;
    std::fs::write(lp, lst)?;
    Ok(st)
}

impl PnLexicon {
    /// Maps the two files; refuses a lexicon built for other assets.
    pub fn open(base: &Path, assets: &Assets) -> io::Result<PnLexicon> {
        let (fp, lp) = paths(base);
        let map = fst::Map::new(unsafe { memmap2::Mmap::map(&std::fs::File::open(fp)?)? }).map_err(|e| bad(format!("pn fst: {e}")))?;
        let lst = unsafe { memmap2::Mmap::map(&std::fs::File::open(lp)?)? };
        if lst.len() < HEADER || &lst[..8] != MAGIC || lst[8..12] != 1u32.to_le_bytes() {
            return Err(bad("pn lst: bad header".into()));
        }
        let fpr = u64::from_le_bytes(lst[12..20].try_into().unwrap());
        if fpr != fingerprint(assets) {
            return Err(bad("pn lexicon was built for another vocabulary / legal table".into()));
        }
        let entries = u32::from_le_bytes(lst[20..24].try_into().unwrap()) as usize;
        let keys = u32::from_le_bytes(lst[24..28].try_into().unwrap()) as usize;
        Ok(PnLexicon { map, lst, entries, keys })
    }

    pub fn len(&self) -> usize {
        self.entries
    }
    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }
    pub fn keys(&self) -> usize {
        self.keys
    }
    /// Bytes of the two mapped files.
    pub fn file_bytes(&self) -> usize {
        self.map.as_fst().as_bytes().len() + self.lst.len()
    }

// Explicit measurement helper; callers opt in. Touches the actual mapped pages.
    pub fn measure_touch_pages(&self) -> (usize, usize, u8) {
        let mut pages = 0usize;
        let mut checksum = 0u8;
        for bytes in [self.map.as_fst().as_bytes(), &self.lst[..]] {
            for i in (0..bytes.len()).step_by(4096) {
                checksum ^= unsafe { std::ptr::read_volatile(bytes.as_ptr().add(i)) };
                pages += 1;
            }
        }
        (pages, self.file_bytes(), std::hint::black_box(checksum))
    }

    fn words_at(&self, off: usize, out: &mut Vec<(u8, Vec<u32>)>) {
        let b = &self.lst[..];
        let n = b[off] as usize;
        let mut p = off + 1;
        for _ in 0..n {
            let (len, class) = (b[p] as usize, b[p + 1]);
            p += 2;
            out.push((class, (0..len).map(|k| u32::from(u16::from_le_bytes([b[p + 2 * k], b[p + 2 * k + 1]]))).collect()));
            p += 2 * len;
        }
    }

    /// Every word whose reading equals `codes[s..s + l]` (2 <= l <= 8): (start, class, ids), by start, then length, then
    /// the stored order. `codes[i]` = vocabulary id of syllable i (None: not a syllable, no word covers it).
    pub fn hits(&self, codes: &[Option<u16>]) -> Vec<(usize, u8, Vec<u32>)> {
        let fst = self.map.as_fst();
        let mut out = vec![];
        let mut words = vec![];
        for s in 0..codes.len() {
            let mut node = fst.root();
            let mut acc = fst::raw::Output::zero();
            for (l, code) in codes[s..codes.len().min(s + MAX_LEN)].iter().enumerate() {
                let Some(code) = code else { break };
                let mut ok = true;
                for byte in code.to_be_bytes() {
                    match node.find_input(byte) {
                        Some(i) => {
                            let t = node.transition(i);
                            acc = acc.cat(t.out);
                            node = fst.node(t.addr);
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    break;
                }
                if l + 1 >= 2 && node.is_final() {
                    words.clear();
                    self.words_at(acc.cat(node.final_output()).value() as usize, &mut words);
                    for (c, ids) in words.drain(..) {
                        out.push((s, c, ids));
                    }
                }
            }
        }
        out
    }

    /// AUTOCOMP: every word whose reading starts with `prefix` and has 1..=`max_extra` more syllables: (the extra syllable
    /// codes, class, ids), in key order.
    pub fn prefix_words(&self, prefix: &[u16], max_extra: usize) -> Vec<(Vec<u16>, u8, Vec<u32>)> {
        let fst = self.map.as_fst();
        let mut node = fst.root();
        let mut acc = fst::raw::Output::zero();
        for byte in prefix.iter().flat_map(|c| c.to_be_bytes()) {
            let Some(i) = node.find_input(byte) else { return vec![] };
            let t = node.transition(i);
            acc = acc.cat(t.out);
            node = fst.node(t.addr);
        }
        let mut out = vec![];
        let mut words = vec![];
        // depth-first over whole syllables (two bytes each)
        let mut stack: Vec<(fst::raw::Node<'_>, fst::raw::Output, Vec<u16>)> = vec![(node, acc, vec![])];
        while let Some((n, a, extra)) = stack.pop() {
            if !extra.is_empty() && n.is_final() {
                words.clear();
                self.words_at(a.cat(n.final_output()).value() as usize, &mut words);
                for (c, ids) in words.drain(..) {
                    out.push((extra.clone(), c, ids));
                }
            }
            if extra.len() == max_extra {
                continue;
            }
            for t1 in n.transitions() {
                let n1 = fst.node(t1.addr);
                for t2 in n1.transitions() {
                    let mut e = extra.clone();
                    e.push(u16::from_be_bytes([t1.inp, t2.inp]));
                    stack.push((fst.node(t2.addr), a.cat(t1.out).cat(t2.out), e));
                }
            }
        }
        out.sort_by(|x, y| x.0.cmp(&y.0));
        out
    }

    /// Words of one exact reading (candidate list).
    pub fn lookup(&self, codes: &[u16]) -> Vec<(u8, Vec<u32>)> {
        let key: Vec<u8> = codes.iter().flat_map(|c| c.to_be_bytes()).collect();
        let mut out = vec![];
        if let Some(off) = self.map.get(&key) {
            self.words_at(off as usize, &mut out);
        }
        out
    }
}

/// USERLEX 1.1 step 5: a PN jump that intersects a forced user-word span (starting outside, entering or spanning it) is
/// never used.
pub(crate) fn crosses_forced(forced: &[bool], start: usize, len: usize) -> bool {
    forced.iter().skip(start).take(len).any(|&f| f)
}

/// One usable jump: start, the ids written (a locked position carries the locked id), its bonus.
#[derive(Clone, Debug, PartialEq)]
pub struct Jump {
    pub start: usize,
    pub ids: Vec<u32>,
    pub bonus: f64,
}

/// Per-position float64 log_softmax over the legal set: `lp[i][j]` for `sets[i][j]` (same expression as `top_k`).
pub fn log_softmax(scores: &[Vec<f32>]) -> Vec<Vec<f64>> {
    scores
        .iter()
        .map(|sc| {
            let v: Vec<f64> = sc.iter().map(|&s| f64::from(s)).collect();
            let max = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let lse = v.iter().map(|&x| (x - max).exp()).sum::<f64>().ln();
            v.iter().map(|&x| x - max - lse).collect()
        })
        .collect()
}

/// Usable jumps from raw hits `(start, ids)`: drops hits of a length without bonus, hits with a character outside the
/// legal set, and hits that do not fit a lock (with `fold = Some((臺, 台))` a lock matches a word character with the
/// same folded id and the locked id is written; without it the ids must be equal). Then keeps `h` per (start, length),
/// best by the sum of the per-character model scores (0 at locks), ties -> smaller id sequence. Sorted by start, then
/// length (longest first), then id sequence.
pub fn usable_jumps(hits: &[(usize, Vec<u32>)], sets: &[Vec<u32>], lp: &[Vec<f64>], locked: &[Option<u32>], cfg: &PnCfg, fold: Option<(u32, u32)>) -> Vec<Jump> {
    let f = |x: u32| match fold {
        Some((trad, tai)) if x == trad => tai,
        _ => x,
    };
    let mut groups: std::collections::BTreeMap<(usize, usize), Vec<(f64, Vec<u32>)>> = Default::default();
    for (s, ids) in hits {
        let (s, l) = (*s, ids.len());
        let Some(_) = cfg.bonus(l) else { continue };
        if s + l > sets.len() || l < 2 {
            continue;
        }
        let mut out = Vec::with_capacity(l);
        let mut sum = 0.0f64;
        let mut ok = true;
        for (k, &id) in ids.iter().enumerate() {
            let i = s + k;
            match locked[i] {
                Some(lk) => {
                    if lk != id && (fold.is_none() || f(lk) != f(id)) {
                        ok = false;
                        break;
                    }
                    out.push(lk);
                }
                None => match sets[i].binary_search(&id) {
                    Ok(j) => {
                        sum += lp[i][j];
                        out.push(id);
                    }
                    Err(_) => {
                        ok = false;
                        break;
                    }
                },
            }
        }
        if !ok {
            continue;
        }
        let g = groups.entry((s, l)).or_default();
        if !g.iter().any(|(_, x)| *x == out) {
            g.push((sum, out));
        }
    }
    let mut jumps = vec![];
    for ((s, l), mut g) in groups {
        g.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        if let Some(h) = cfg.h {
            g.truncate(h);
        }
        let bonus = cfg.bonus(l).unwrap();
        jumps.extend(g.into_iter().map(|(_, ids)| Jump { start: s, ids, bonus }));
    }
    jumps.sort_by(|a, b| a.start.cmp(&b.start).then(b.ids.len().cmp(&a.ids.len())).then_with(|| a.ids.cmp(&b.ids)));
    jumps
}

/// The beam with jumps. `cands`/`lps` as for `prior::beam` (a locked position has its single id with 0); `lp` and `sets`
/// give the per-character model score of a jump character, `locked` marks positions scored 0. `prior(hist, id)` is the
/// prior log-probability (callers pass 0 for lambda 0 / no prior; it is multiplied by `lambda`).
#[allow(clippy::too_many_arguments)]
pub fn beam(
    cands: &[Vec<u32>],
    lps: &[Vec<f64>],
    sets: &[Vec<u32>],
    lp: &[Vec<f64>],
    locked: &[Option<u32>],
    jumps: &[Jump],
    lambda: f64,
    prior: &dyn Fn(&[u32], u32) -> f64,
) -> Option<Vec<u32>> {
    beam_cached(cands, lps, sets, lp, locked, jumps, lambda, prior, None)
}

/// `beam` with the same-document bigram cache (CACHE-plan 2.2): a generated character whose pair with the previous
/// character of this conversion is in the set adds `gamma` after its own score (single extension and every character of a
/// jump), at most `cap` times per path. `None` (or an empty set: no hit) performs exactly the operations of `beam`.
#[allow(clippy::too_many_arguments)]
pub fn beam_cached(
    cands: &[Vec<u32>],
    lps: &[Vec<f64>],
    sets: &[Vec<u32>],
    lp: &[Vec<f64>],
    locked: &[Option<u32>],
    jumps: &[Jump],
    lambda: f64,
    prior: &dyn Fn(&[u32], u32) -> f64,
    cache: Option<crate::cache::CacheBonus<'_>>,
) -> Option<Vec<u32>> {
    beam_states(cands, lps, sets, lp, locked, jumps, lambda, prior, cache).and_then(|v| v.into_iter().next().map(|s| s.1))
}

/// The surviving states at the last position (score, sequence), sorted and cut as the beam keeps them; `beam_cached`
/// returns the first. Separate so that tests can compare every score bit for bit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn beam_states(
    cands: &[Vec<u32>],
    lps: &[Vec<f64>],
    sets: &[Vec<u32>],
    lp: &[Vec<f64>],
    locked: &[Option<u32>],
    jumps: &[Jump],
    lambda: f64,
    prior: &dyn Fn(&[u32], u32) -> f64,
    cache: Option<crate::cache::CacheBonus<'_>>,
) -> Option<Vec<(f64, Vec<u32>)>> {
    beam_states_ex(cands, lps, sets, lp, locked, jumps, &|_| lambda, &[], prior, cache)
}

/// `beam_cached` with the SHORT-PRIOR per-position weight `w(i)` (every character of a jump uses its own position) and the
/// F3 history prefix `pre`; `w = lambda`, `pre = []` is `beam_cached` bit for bit.
#[allow(clippy::too_many_arguments)]
pub fn beam_cached_ex(
    cands: &[Vec<u32>],
    lps: &[Vec<f64>],
    sets: &[Vec<u32>],
    lp: &[Vec<f64>],
    locked: &[Option<u32>],
    jumps: &[Jump],
    w: &dyn Fn(usize) -> f64,
    pre: &[u32],
    prior: &dyn Fn(&[u32], u32) -> f64,
    cache: Option<crate::cache::CacheBonus<'_>>,
) -> Option<Vec<u32>> {
    beam_states_ex(cands, lps, sets, lp, locked, jumps, w, pre, prior, cache).and_then(|v| v.into_iter().next().map(|s| s.1))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn beam_states_ex(
    cands: &[Vec<u32>],
    lps: &[Vec<f64>],
    sets: &[Vec<u32>],
    lp: &[Vec<f64>],
    locked: &[Option<u32>],
    jumps: &[Jump],
    w: &dyn Fn(usize) -> f64,
    pre: &[u32],
    prior: &dyn Fn(&[u32], u32) -> f64,
    cache: Option<crate::cache::CacheBonus<'_>>,
) -> Option<Vec<(f64, Vec<u32>)>> {
    let n = cands.len();
    if cands.iter().any(Vec::is_empty) {
        return None;
    }
    let score_of = |i: usize, id: u32| -> f64 {
        if locked[i].is_some() {
            return 0.0;
        }
        match sets[i].binary_search(&id) {
            Ok(j) => lp[i][j],
            // USERLEX 1.6: only a user word's own custom-reading position reaches here (PN jumps and single extensions
            // are inside the legal set): the position's lowest legal log-probability (finite; -inf for an empty set).
            Err(_) => lp[i].iter().copied().reduce(f64::min).unwrap_or(f64::NEG_INFINITY),
        }
    };
    // pending[q]: (score, sequence, generation key: 0 = single extension, 1 + (8 - length) = jump, cache hits so far).
    // The hit count is a function of the sequence, so the dedup by full prefix is unchanged.
    let mut pending: Vec<Vec<(f64, Vec<u32>, usize, usize)>> = vec![vec![]; n + 1];
    pending[0].push((0.0, vec![], 0, 0));
    let mut by_start: Vec<Vec<&Jump>> = vec![vec![]; n];
    for j in jumps {
        by_start[j.start].push(j);
    }
    for p in 0..=n {
        let mut next = std::mem::take(&mut pending[p]);
        // Dedup by full prefix: best score, ties -> smaller generation key (stable: first generated among equals).
        next.sort_by(|a, b| a.1.cmp(&b.1).then(b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)).then(a.2.cmp(&b.2)));
        next.dedup_by(|b, a| a.1 == b.1);
        next.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
        next.truncate(BEAM);
        if p == n {
            return Some(next.into_iter().map(|s| (s.0, s.1)).collect());
        }
        let lambda = w(p);
        for (sc, seq, used) in next.iter().map(|(s, q, _, u)| (*s, q, *u)) {
            let (hb, hn) = prior::hist3(pre, seq);
            let hist = &hb[..hn];
            for (&c, &l) in cands[p].iter().zip(&lps[p]) {
                let pr = if lambda != 0.0 { prior(hist, c) } else { 0.0 };
                let mut s2 = seq.clone();
                s2.push(c);
                let mut v = (sc + l) + lambda * pr;
                let mut u = used;
                if let Some(cb) = &cache
                    && cb.hit(seq.last().copied(), c, u)
                {
                    v += cb.gamma;
                    u += 1;
                }
                pending[p + 1].push((v, s2, 0, u));
            }
            for j in &by_start[p] {
                let mut s2 = seq.clone();
                let mut s = sc;
                let mut u = used;
                for (k, &id) in j.ids.iter().enumerate() {
                    let wk = w(p + k);
                    let pr = if wk != 0.0 {
                        let (hb, hn) = prior::hist3(pre, &s2);
                        prior(&hb[..hn], id)
                    } else {
                        0.0
                    };
                    s = (s + score_of(p + k, id)) + wk * pr;
                    if let Some(cb) = &cache
                        && cb.hit(s2.last().copied(), id, u)
                    {
                        s += cb.gamma;
                        u += 1;
                    }
                    s2.push(id);
                }
                pending[p + j.ids.len()].push((s + j.bonus, s2, 1 + (MAX_LEN - j.ids.len()), u));
            }
        }
    }
    None
}

impl Assets {
    /// Sets the lexicon and its settings; `None` (or an empty lexicon) = the current behavior, bit for bit.
    pub fn set_pn(&mut self, lex: Option<std::sync::Arc<PnLexicon>>, cfg: PnCfg) {
        self.min_log10 = self.dict.min_log10_by_len();
        self.pn = lex.filter(|l| !l.is_empty()).map(|l| (l, cfg));
    }
    pub fn pn(&self) -> Option<&(std::sync::Arc<PnLexicon>, PnCfg)> {
        self.pn.as_ref()
    }

    /// Syllable codes for lexicon lookups (None: not a syllable of the vocabulary).
    pub(crate) fn syl_codes(&self, syllables: &[String]) -> Vec<Option<u16>> {
        syllables.iter().map(|s| Some(self.vocab.syllable_id(s)).filter(|&x| x != crate::vocab::UNK).map(|x| x as u16)).collect()
    }

    /// The PN selection for one inference (model mode, every set non-empty): ids per position.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn pn_select(&self, pn: Option<&PnCfg>, syllables: &[String], sets: &[Vec<u32>], scores: &[Vec<f32>], locked: &[Option<u32>], lambda: f64, cache: Option<crate::cache::CacheBonus<'_>>, forced: &[bool], sp: &crate::convert::ShortView<'_>) -> Option<Vec<u32>> {
        let lp = log_softmax(scores);
        let (cs, ls): (Vec<Vec<u32>>, Vec<Vec<f64>>) = scores
            .iter()
            .zip(sets)
            .zip(locked)
            .map(|((sc, ids), lk)| match lk {
                Some(id) => (vec![*id], vec![0.0]),
                None => prior::top_k(sc, ids),
            })
            .unzip();
        let mut jumps = self.merged_jumps(syllables, sets, &lp, locked, forced, pn);
        if sp.has_words() {
            jumps.extend(self.short_word_jumps(syllables, sets, &lp, locked, forced, sp));
            jumps.sort_by(|a, b| a.start.cmp(&b.start).then(b.ids.len().cmp(&a.ids.len())).then_with(|| a.ids.cmp(&b.ids)));
        }
        match &self.prior {
            Some(pk) if lambda != 0.0 => beam_cached_ex(&cs, &ls, sets, &lp, locked, &jumps, &|i| sp.weight(lambda, i), sp.pre(), &|h, c| pk.logp(h, c), cache),
            _ => beam_cached(&cs, &ls, sets, &lp, locked, &jumps, 0.0, &|_, _| 0.0, cache),
        }
    }
}

#[cfg(test)]
#[path = "pn_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cache_tests.rs"]
mod cache_tests;

/// A lexicon hit whose characters equal the reference text at its span (diagnostics of PN-plan 5).
#[derive(Clone, Debug)]
pub struct GoldHit {
    pub start: usize,
    pub len: usize,
    /// Stored class code (layer << 4 | class).
    pub class: u8,
    /// Rank among the usable hits of the same (start, length) by the H order (0 = best).
    pub rank: usize,
    pub group: usize,
    /// Positions of the span whose reference character is outside the top 8.
    pub out8: usize,
    /// Local score of the PN-off output over the span minus that of the reference word (same history outside).
    pub gap: f64,
}

impl Assets {
    /// PRED: the vocabulary's characters in id order.
    pub fn vocab_chars(&self) -> &[char] {
        self.vocab.chars()
    }
    pub fn vocab_id(&self, c: char) -> u32 {
        self.vocab.char_id(c)
    }
    /// The model's legal-set scores of a conversion (empty in fallback mode).
    pub fn legal_scores(&self, conv: &crate::convert::Conversion) -> Vec<Vec<f32>> {
        conv.inf.scores.clone()
    }

    /// Hits equal to `gold` (compared with 臺 read as 台) for an unlocked conversion; None without model scores.
    pub fn pn_diag(&self, conv: &crate::convert::Conversion, gold: &[char], lambda: f64) -> Option<Vec<GoldHit>> {
        let inf = &conv.inf;
        let lex = &self.pn.as_ref()?.0;
        let n = inf.syllables.len();
        if conv.fallback || inf.scores.len() != n || gold.len() != n || inf.sets.iter().any(Vec::is_empty) {
            return None;
        }
        let fold = |c: char| if c == crate::norm::TAI_TRAD { crate::norm::TAI } else { c };
        let lp = log_softmax(&inf.scores);
        let top: Vec<Vec<u32>> = inf.scores.iter().zip(&inf.sets).map(|(sc, ids)| prior::top_k(sc, ids).0).collect();
        let off = self.rebeam_with(conv, &[], lambda, false, None, false);
        let off_ids: Vec<u32> = off.chars.iter().map(|c| self.vocab.char_id(c.unwrap_or('\u{FFFD}'))).collect();
        let pr = |h: &[u32], c: u32| match &self.prior {
            Some(pk) if lambda != 0.0 => lambda * pk.logp(h, c),
            _ => 0.0,
        };
        let lpi = |i: usize, id: u32| inf.sets[i].binary_search(&id).map_or(f64::NEG_INFINITY, |j| lp[i][j]);
        let mut groups: std::collections::BTreeMap<(usize, usize), Vec<(f64, Vec<u32>, u8)>> = Default::default();
        for (s, class, ids) in lex.hits(&self.syl_codes(&inf.syllables)) {
            if ids.iter().enumerate().any(|(k, id)| inf.sets[s + k].binary_search(id).is_err()) {
                continue;
            }
            let sum: f64 = ids.iter().enumerate().map(|(k, &id)| lpi(s + k, id)).sum();
            groups.entry((s, ids.len())).or_default().push((sum, ids, class));
        }
        let mut out = vec![];
        for ((s, l), mut g) in groups {
            g.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            for (rank, (_, ids, class)) in g.iter().enumerate() {
                let ok = ids.iter().enumerate().all(|(k, &id)| self.vocab.char_of(id).map(fold) == Some(fold(gold[s + k])));
                if !ok {
                    continue;
                }
                let out8 = ids.iter().enumerate().filter(|&(k, id)| !top[s + k].contains(id)).count();
                let (mut so, mut sg) = (0.0f64, 0.0f64);
                let mut hist_o: Vec<u32> = off_ids[s.saturating_sub(3)..s].to_vec();
                let mut hist_g = hist_o.clone();
                for k in 0..l {
                    let (o, gch) = (off_ids[s + k], ids[k]);
                    so += lpi(s + k, o) + pr(&hist_o[hist_o.len().saturating_sub(3)..], o);
                    sg += lpi(s + k, gch) + pr(&hist_g[hist_g.len().saturating_sub(3)..], gch);
                    hist_o.push(o);
                    hist_g.push(gch);
                }
                out.push(GoldHit { start: s, len: l, class: *class, rank, group: g.len(), out8, gap: so - sg });
            }
        }
        Some(out)
    }
}

#[cfg(test)]
#[path = "pn_assets_tests.rs"]
mod assets_tests;

impl Assets {
    /// Lexicon words of one exact reading with the log10 the candidate list uses (data.txt's when the word is there,
    /// else the smallest data.txt log10 of that length) and whether the word is only in the lexicon.
    pub fn pn_words(&self, syllables: &[String]) -> Vec<(String, f64, bool)> {
        let Some((lex, _)) = &self.pn else { return vec![] };
        let Some(codes): Option<Vec<u16>> = self.syl_codes(syllables).into_iter().collect() else { return vec![] };
        let dict = self.dict.lookup(syllables);
        lex.lookup(&codes)
            .into_iter()
            .map(|(_, ids)| {
                let w: String = ids.iter().filter_map(|&i| self.vocab.char_of(i)).collect();
                match dict.iter().find(|e| e.word == w) {
                    Some(e) => (w, e.log10, false),
                    None => (w, self.min_log10.get(syllables.len()).copied().filter(|x| x.is_finite()).unwrap_or(-20.0), true),
                }
            })
            .collect()
    }
}

impl Assets {
    /// Lexicon hits whose ids equal `ids` over their span: (start, length, class code). Diagnostics (which words an
    /// output contains).
    pub fn pn_matches(&self, syllables: &[String], ids: &[u32]) -> Vec<(usize, usize, u8)> {
        let Some((lex, _)) = &self.pn else { return vec![] };
        lex.hits(&self.syl_codes(syllables)).into_iter().filter(|(s, _, w)| ids.get(*s..*s + w.len()) == Some(&w[..])).map(|(s, c, w)| (s, w.len(), c)).collect()
    }
}

#[path = "genj.rs"]
pub mod genj;
