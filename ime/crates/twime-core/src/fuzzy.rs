//! FUZZY (FUZZY-plan 1, 3.2): confusion classes of a typed syllable, shared by the F0 measurement (error
//! injection) and the F1 repair of illegal syllables. Pure functions over the bopomofo layout and the legal syllable set;
//! nothing here changes the behaviour of a `Session`.
//!
//! - `forward(g, c)`: every syllable class `c` turns the legal syllable `g` into (deduplicated, without `g`, non-empty
//!   body; legal or not).
//! - Class K (adjacent key) is defined by replaying the full key sequence of `g` with one key replaced by a neighbour,
//!   with the composer rules of `Session` (`replay`).
//! - `Index`: the forward relation of every legal syllable and its reverse t -> R(t), ranked (plan 3.2).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use twime_proto::Key;

use crate::keymap::{self, Slot};

/// One confusion class (plan 1). The five S classes are one family for ranking and for the F0 single-error sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    Sa,
    Sb,
    Sc,
    Sd,
    Se,
    Pn,
    Pa,
    Pz,
    Pr,
    Pf,
    T,
    K,
}

/// Every class in definition order.
pub const CLASSES: [Class; 12] =
    [Class::Sa, Class::Sb, Class::Sc, Class::Sd, Class::Se, Class::Pn, Class::Pa, Class::Pz, Class::Pr, Class::Pf, Class::T, Class::K];

/// Families in ranking order (plan 3.2 step 2); also the F0 single-error set names.
pub const FAMILIES: [&str; 8] = ["S", "PN", "PA", "PZ", "PR", "PF", "T", "K"];

impl Class {
    pub fn code(self) -> &'static str {
        match self {
            Class::Sa => "S-a",
            Class::Sb => "S-b",
            Class::Sc => "S-c",
            Class::Sd => "S-d",
            Class::Se => "S-e",
            Class::Pn => "PN",
            Class::Pa => "PA",
            Class::Pz => "PZ",
            Class::Pr => "PR",
            Class::Pf => "PF",
            Class::T => "T",
            Class::K => "K",
        }
    }
    /// Index into `FAMILIES`.
    pub fn family(self) -> usize {
        match self {
            Class::Sa | Class::Sb | Class::Sc | Class::Sd | Class::Se => 0,
            Class::Pn => 1,
            Class::Pa => 2,
            Class::Pz => 3,
            Class::Pr => 4,
            Class::Pf => 5,
            Class::T => 6,
            Class::K => 7,
        }
    }
    /// F1 cost (plan 1 table).
    pub fn cost(self) -> u8 {
        match self.family() {
            0 => 1,
            1..=5 => 2,
            6 => 3,
            _ => 4,
        }
    }
}

/// A syllable as the four composer slots (initial, medial, final, tone; first tone = no mark).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slots {
    pub i: Option<char>,
    pub m: Option<char>,
    pub f: Option<char>,
    pub t: Option<char>,
}

impl Slots {
    /// Parses a dictionary-form syllable (slots in canonical order, each at most once). None for anything else.
    pub fn parse(s: &str) -> Option<Slots> {
        let mut out = Slots::default();
        let mut last = 0u8;
        for c in s.chars() {
            let (rank, slot) = match keymap::slot_of(c)? {
                Slot::Initial => (1, &mut out.i),
                Slot::Medial => (2, &mut out.m),
                Slot::Final => (3, &mut out.f),
                Slot::Tone => (4, &mut out.t),
            };
            if rank <= last {
                return None;
            }
            last = rank;
            *slot = Some(c);
        }
        Some(out)
    }
    pub fn has_body(&self) -> bool {
        self.i.is_some() || self.m.is_some() || self.f.is_some()
    }
    /// Dictionary form (the composer's `Typing::syllable`).
    pub fn text(&self) -> String {
        [self.i, self.m, self.f, self.t].into_iter().flatten().collect()
    }
}

const TONES: [Option<char>; 5] = [None, Some('ˊ'), Some('ˇ'), Some('ˋ'), Some('˙')];

fn swap(x: Option<char>, a: char, b: char) -> Option<Option<char>> {
    match x {
        Some(c) if c == a => Some(Some(b)),
        Some(c) if c == b => Some(Some(a)),
        _ => None,
    }
}

/// The non-K forward operation of one class on `g` (slot form); empty when its condition does not hold.
fn forward_slots(g: Slots, c: Class) -> Vec<Slots> {
    let mut out = vec![];
    let mut push = |s: Slots| out.push(s);
    match c {
        Class::Sa => {
            if g.i.is_some() && g.f == Some('ㄛ') {
                match g.m {
                    None => push(Slots { m: Some('ㄨ'), ..g }),
                    Some('ㄨ') => push(Slots { m: None, ..g }),
                    _ => {}
                }
            }
        }
        Class::Sb => {
            if matches!(g.i, Some('ㄐ' | 'ㄑ' | 'ㄒ'))
                && let Some(m) = swap(g.m, 'ㄩ', 'ㄨ')
            {
                push(Slots { m, ..g });
            }
        }
        Class::Sc => {
            if matches!(g.i, Some('ㄅ' | 'ㄆ' | 'ㄇ' | 'ㄈ')) && g.f == Some('ㄥ') {
                match g.m {
                    None => push(Slots { m: Some('ㄨ'), ..g }),
                    Some('ㄨ') => push(Slots { m: None, ..g }),
                    _ => {}
                }
            }
        }
        Class::Sd => {
            if matches!(g.i, Some('ㄓ' | 'ㄔ' | 'ㄕ' | 'ㄖ' | 'ㄗ' | 'ㄘ' | 'ㄙ')) {
                match g.m {
                    None => push(Slots { m: Some('ㄧ'), ..g }),
                    Some('ㄧ') => push(Slots { m: None, ..g }),
                    _ => {}
                }
            }
        }
        Class::Se => {
            if matches!(g.m, Some('ㄧ' | 'ㄩ'))
                && let Some(f) = swap(g.f, 'ㄝ', 'ㄜ')
            {
                push(Slots { f, ..g });
            }
        }
        Class::Pn => {
            if let Some(f) = swap(g.f, 'ㄣ', 'ㄥ') {
                push(Slots { f, ..g });
            }
        }
        Class::Pa => {
            if let Some(f) = swap(g.f, 'ㄢ', 'ㄤ') {
                push(Slots { f, ..g });
            }
        }
        Class::Pz => {
            if let Some(i) = swap(g.i, 'ㄓ', 'ㄗ').or_else(|| swap(g.i, 'ㄔ', 'ㄘ')).or_else(|| swap(g.i, 'ㄕ', 'ㄙ')) {
                push(Slots { i, ..g });
            }
        }
        Class::Pr => {
            if let Some(i) = swap(g.i, 'ㄖ', 'ㄌ') {
                push(Slots { i, ..g });
            }
        }
        Class::Pf => {
            if let Some(i) = swap(g.i, 'ㄈ', 'ㄏ') {
                push(Slots { i, ..g });
            }
        }
        Class::T => {
            for t in TONES {
                if t != g.t {
                    push(Slots { t, ..g });
                }
            }
        }
        Class::K => {}
    }
    out
}

// ---- class K: key geometry and replay ---------------------------------------------------------------------------

/// Row (0 = number row .. 3 = z row) and 4 x the key centre x (plan 1: `1` = 1.5, `q` = 2.0, `a` = 2.25, `z` = 2.75,
/// +1 per key to the right) of a layout key; None for keys outside the 41 bopomofo keys.
fn geometry(k: Key) -> Option<(i32, i32)> {
    keymap::symbol(k)?;
    const ROWS: [&[u8]; 4] = [b"1234567890-", b"qwertyuiop", b"asdfghjkl;", b"zxcvbnm,./"];
    const X0: [i32; 4] = [6, 8, 9, 11];
    let ch = match k {
        Key::Digit(d) => b'0' + d,
        Key::Letter(c) => c,
        Key::Minus => b'-',
        Key::Comma => b',',
        Key::Period => b'.',
        Key::Slash => b'/',
        Key::Semicolon => b';',
        _ => return None,
    };
    ROWS.iter().enumerate().find_map(|(r, row)| row.iter().position(|&c| c == ch).map(|p| (r as i32, X0[r] + 4 * p as i32)))
}

/// The 41 layout keys in table order.
pub fn layout_keys() -> Vec<Key> {
    let mut v = vec![];
    for d in 0..=9u8 {
        v.push(Key::Digit(d));
    }
    for c in b'a'..=b'z' {
        v.push(Key::Letter(c));
    }
    v.extend([Key::Minus, Key::Comma, Key::Period, Key::Slash, Key::Semicolon]);
    let mut v: Vec<(char, Key)> = v.into_iter().filter_map(|k| keymap::symbol(k).map(|c| (c, k))).collect();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

/// Adjacent layout keys of `k` (plan 1: same row and |dx| = 1, or |dy| = 1 and |dx| <= 0.75), in bopomofo order.
pub fn neighbours(k: Key) -> Vec<Key> {
    let Some((r, x)) = geometry(k) else { return vec![] };
    layout_keys()
        .into_iter()
        .filter(|&o| {
            let Some((r2, x2)) = geometry(o) else { return false };
            (r2 == r && (x2 - x).abs() == 4) || ((r2 - r).abs() == 1 && (x2 - x).abs() <= 3)
        })
        .collect()
}

/// The full key sequence of a syllable: the keys of the initial, medial and final (empty slots skipped), then the
/// completing key (first tone = Space, other tones = their key, neutral tone = the ˙ key last).
pub fn keys_of(g: &Slots) -> Vec<Key> {
    let mut v: Vec<Key> = [g.i, g.m, g.f].into_iter().flatten().filter_map(keymap::key_of).collect();
    v.push(match g.t {
        None => Key::Space,
        Some(t) => keymap::key_of(t).expect("tone key"),
    });
    v
}

/// Outcome of replaying a key sequence on a fresh composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Replay {
    /// Exactly one syllable, completed by the last key.
    One(String),
    /// No syllable completed.
    None,
    /// One syllable, completed before the last key.
    Early,
    /// More than one syllable.
    Many,
}

/// Replays `keys` with the bopomofo composer rules of `Session::plain_key` / `type_symbol` / `complete_syllable` (a fresh
/// session, no candidate window): a body key fills its slot (the same slot again overwrites), a tone key with a body
/// completes, ˙ without a body is kept, other tone keys without a body do nothing, Space with a body completes.
pub fn replay(keys: &[Key]) -> Replay {
    let mut cur = Slots::default();
    let mut done: Vec<(String, usize)> = vec![];
    for (j, &k) in keys.iter().enumerate() {
        let mut complete = false;
        if let Some(c) = keymap::symbol(k) {
            match keymap::slot_of(c) {
                Some(Slot::Tone) => {
                    if cur.has_body() {
                        cur.t = Some(c);
                        complete = true;
                    } else if c == '˙' {
                        cur.t = Some(c);
                    }
                }
                Some(Slot::Initial) => cur.i = Some(c),
                Some(Slot::Medial) => cur.m = Some(c),
                Some(Slot::Final) => cur.f = Some(c),
                None => {}
            }
        } else if k == Key::Space && cur.has_body() {
            complete = true;
        }
        if complete {
            done.push((cur.text(), j));
            cur = Slots::default();
        }
    }
    match done.len() {
        0 => Replay::None,
        1 if done[0].1 + 1 == keys.len() => Replay::One(done.remove(0).0),
        1 => Replay::Early,
        _ => Replay::Many,
    }
}

/// Class K variants of `g` and the counts of excluded replays (none completed, completed early, several completed).
pub fn forward_k(g: &Slots) -> (Vec<String>, [usize; 3]) {
    let keys = keys_of(g);
    let mut out = BTreeSet::new();
    let mut excl = [0usize; 3];
    for j in 0..keys.len() {
        if keys[j] == Key::Space {
            continue;
        }
        for nb in neighbours(keys[j]) {
            let mut v = keys.clone();
            v[j] = nb;
            match replay(&v) {
                Replay::One(t) => {
                    out.insert(t);
                }
                Replay::None => excl[0] += 1,
                Replay::Early => excl[1] += 1,
                Replay::Many => excl[2] += 1,
            }
        }
    }
    let gs = g.text();
    (out.into_iter().filter(|t| *t != gs && Slots::parse(t).is_some_and(|s| s.has_body())).collect(), excl)
}

/// fwd_c(g): deduplicated, sorted, without `g`, non-empty body only. `g` that does not parse gives nothing.
pub fn forward(g: &str, c: Class) -> Vec<String> {
    let Some(s) = Slots::parse(g) else { return vec![] };
    if c == Class::K {
        return forward_k(&s).0;
    }
    let set: BTreeSet<String> = forward_slots(s, c).into_iter().filter(Slots::has_body).map(|x| x.text()).filter(|t| t != g).collect();
    set.into_iter().collect()
}

// ---- index and R(t) -----------------------------------------------------------------------------------------------

/// One repair candidate of a typed syllable: the legal syllable and its cheapest class.
#[derive(Clone, Debug, PartialEq)]
pub struct Repair {
    pub syl: String,
    pub class: Class,
}

/// Most repair candidates kept (plan 3.2).
pub const MAX_REPAIRS: usize = 16;

/// The forward relation of every legal syllable and the ranked reverse index.
#[derive(Debug, Default)]
pub struct Index {
    /// Legal syllable -> per class (in `CLASSES` order) its forward variants.
    pub fwd: BTreeMap<String, Vec<Vec<String>>>,
    /// Typed syllable -> R(t) ranked (plan 3.2), not truncated.
    pub rev: HashMap<String, Vec<Repair>>,
    /// Largest single-character log10 of each legal syllable (None = no single-character entry).
    pub single: HashMap<String, Option<f64>>,
    /// Class K replays excluded: none completed, completed early, several completed.
    pub k_excluded: [usize; 3],
}

impl Index {
    /// Builds the index from the legal syllables and the single-character log10 of each.
    pub fn build(legal: impl IntoIterator<Item = (String, Option<f64>)>) -> Index {
        let mut ix = Index::default();
        for (g, lp) in legal {
            ix.single.insert(g.clone(), lp);
            let Some(s) = Slots::parse(&g) else { continue };
            let mut per = Vec::with_capacity(CLASSES.len());
            for c in CLASSES {
                if c == Class::K {
                    let (v, e) = forward_k(&s);
                    for (a, b) in ix.k_excluded.iter_mut().zip(e) {
                        *a += b;
                    }
                    per.push(v);
                } else {
                    per.push(forward(&g, c));
                }
            }
            ix.fwd.insert(g, per);
        }
        // reverse: t -> {g: cheapest class (cost, then family order, then class order)}
        let mut rev: HashMap<String, BTreeMap<String, Class>> = HashMap::new();
        for (g, per) in &ix.fwd {
            for (ci, ts) in per.iter().enumerate() {
                let c = CLASSES[ci];
                for t in ts {
                    let e = rev.entry(t.clone()).or_default().entry(g.clone()).or_insert(c);
                    if (c.cost(), c.family(), c) < (e.cost(), e.family(), *e) {
                        *e = c;
                    }
                }
            }
        }
        for (t, gs) in rev {
            let mut v: Vec<Repair> = gs.into_iter().map(|(syl, class)| Repair { syl, class }).collect();
            let single = &ix.single;
            let lp = |s: &str| single.get(s).copied().flatten().unwrap_or(f64::NEG_INFINITY);
            v.sort_by(|a, b| {
                (a.class.cost(), a.class.family())
                    .cmp(&(b.class.cost(), b.class.family()))
                    .then(lp(&b.syl).total_cmp(&lp(&a.syl)))
                    .then(a.syl.cmp(&b.syl))
            });
            ix.rev.insert(t, v);
        }
        ix
    }

    /// Builds the index from the assets' legal table (every dictionary syllable with a legal character); the
    /// single-character log10 comes from `raw`, the dictionary parsed from data.txt WITHOUT the alias table (plan 3.2:
    /// "data.txt single-character entries").
    pub fn from_assets(a: &twime_model::Assets, raw: &twime_model::lexicon::Dictionary) -> Index {
        let (_, syls) = a.dictionary().vocabulary();
        Index::build(syls.into_iter().filter(|s| a.is_legal_syllable(s)).map(|s| {
            let lp = raw.lookup(std::slice::from_ref(&s)).iter().filter(|e| e.word.chars().count() == 1).map(|e| e.log10).reduce(f64::max);
            (s, lp)
        }))
    }

    /// `from_assets` with the raw data.txt text.
    pub fn from_data_txt(a: &twime_model::Assets, data_txt: &str) -> Index {
        Index::from_assets(a, &twime_model::lexicon::Dictionary::parse(data_txt).0)
    }

    /// R(t) ranked, not truncated (empty when no legal syllable reaches `t`).
    pub fn repairs_all(&self, t: &str) -> &[Repair] {
        self.rev.get(t).map_or(&[], Vec::as_slice)
    }

    /// R(t) ranked and truncated to `MAX_REPAIRS`.
    pub fn repairs(&self, t: &str) -> &[Repair] {
        let v = self.repairs_all(t);
        &v[..v.len().min(MAX_REPAIRS)]
    }
}

#[cfg(test)]
#[path = "fuzzy_tests.rs"]
mod tests;
