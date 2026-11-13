//! CAND: lexicon matches of the syllable buffer enter the G1 model as extra
//! input tokens. `CandLex` (the `.clex` export of train/cand_export.py) finds the candidates with the same rule as
//! train/cand.py `build` (reading hits fully inside the buffer, fixed order, cap 16 / M tokens); `CandEmb` (`.cemb`)
//! holds the type / slot / category / source / tier vectors added to the candidate tokens.

use std::{collections::HashMap, io, path::Path};

use crate::Assets;

pub const NSLOT: usize = 16;
const MAXL: usize = 8;

fn bad(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}

struct Entry {
    id: u32,
    len: u8,
    srank: u8,
    src: u8,
    tier: u8,
    /// offsets into the pools
    syl: u32,
    cats: u32,
    ncat: u8,
}

pub struct CandLex {
    pub m: usize,
    entries: Vec<Entry>,
    syl_pool: Vec<u16>,
    chr_pool: Vec<u16>,
    cat_pool: Vec<u16>,
    /// hash of a reading (syllable ids) -> contiguous entry range (the export is in (reading, word) order, so the
    /// entries of one reading are adjacent); a reading whose hash collides with an earlier one goes to `collide`
    by: HashMap<u64, (u32, u32)>,
    collide: HashMap<Box<[u16]>, (u32, u32)>,
}

fn key_hash(k: &[u16]) -> u64 {
    // FNV-1a over the little-endian bytes
    let mut h: u64 = 0xcbf29ce484222325;
    for &v in k {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h ^ (k.len() as u64).wrapping_mul(0x9e3779b97f4a7c15)
}

/// One kept candidate: start syllable, entry index into `CandLex`, length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kept {
    pub start: usize,
    pub entry: u32,
    pub len: usize,
}

impl CandLex {
    pub fn open(path: &Path) -> io::Result<CandLex> {
        let b = std::fs::read(path)?;
        if b.len() < 16 || &b[..4] != b"TWCL" {
            return Err(bad("not a TWCL candidate lexicon"));
        }
        let u = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        if u(4) != 1 {
            return Err(bad("unsupported TWCL version"));
        }
        let (m, n) = (u(8) as usize, u(12) as usize);
        let mut at = 16;
        let mut entries = Vec::with_capacity(n);
        let (mut syl_pool, mut chr_pool, mut cat_pool) = (Vec::new(), Vec::new(), Vec::new());
        let u16s = |at: &mut usize, k: usize, out: &mut Vec<u16>| -> io::Result<()> {
            let end = *at + 2 * k;
            let c = b.get(*at..end).ok_or_else(|| bad("truncated TWCL"))?;
            out.extend(c.chunks_exact(2).map(|x| u16::from_le_bytes([x[0], x[1]])));
            *at = end;
            Ok(())
        };
        for i in 0..n {
            let h = b.get(at..at + 9).ok_or_else(|| bad("truncated TWCL"))?;
            let id = u32::from_le_bytes(h[0..4].try_into().unwrap());
            let (len, srank, src, tier, ncat) = (h[4], h[5], h[6], h[7], h[8]);
            at += 9;
            if !(2..=MAXL as u8).contains(&len) || src > 2 || tier > 4 || ncat == 0 {
                return Err(bad("bad TWCL entry"));
            }
            let (so, co) = (syl_pool.len() as u32, cat_pool.len() as u32);
            u16s(&mut at, len as usize, &mut syl_pool)?;
            u16s(&mut at, len as usize, &mut chr_pool)?;
            u16s(&mut at, ncat as usize, &mut cat_pool)?;
            if i > 0 && id <= entries.last().map_or(0, |x: &Entry| x.id) {
                return Err(bad("TWCL entries not in canonical id order"));
            }
            entries.push(Entry { id, len, srank, src, tier, syl: so, cats: co, ncat });
        }
        if at != b.len() {
            return Err(bad("trailing bytes in TWCL"));
        }
        let mut lx = CandLex { m, entries, syl_pool, chr_pool, cat_pool, by: HashMap::new(), collide: HashMap::new() };
        let mut i = 0;
        let n = lx.entries.len();
        while i < n {
            let mut j = i + 1;
            while j < n && lx.key(j as u32) == lx.key(i as u32) {
                j += 1;
            }
            let k = lx.key(i as u32).to_vec();
            let r = (i as u32, (j - i) as u32);
            if lx.find(&k).is_some() {
                return Err(bad("entries of one reading are not contiguous"));
            }
            let h = key_hash(&k);
            if lx.by.contains_key(&h) {
                lx.collide.insert(k.into_boxed_slice(), r);
            } else {
                lx.by.insert(h, r);
            }
            i = j;
        }
        Ok(lx)
    }

    fn key(&self, e: u32) -> &[u16] {
        let x = &self.entries[e as usize];
        &self.syl_pool[x.syl as usize..x.syl as usize + x.len as usize]
    }

    fn find(&self, k: &[u16]) -> Option<(u32, u32)> {
        if let Some(&r) = self.by.get(&key_hash(k))
            && self.key(r.0) == k
        {
            return Some(r);
        }
        self.collide.get(k).copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Canonical lexf-lex id of an entry.
    pub fn id(&self, e: u32) -> u32 {
        self.entries[e as usize].id
    }
    /// Character ids of an entry.
    pub fn chars(&self, e: u32) -> &[u16] {
        let x = &self.entries[e as usize];
        &self.chr_pool[x.syl as usize..x.syl as usize + x.len as usize]
    }
    pub fn file_bytes_estimate(&self) -> usize {
        self.entries.len() * std::mem::size_of::<Entry>() + 2 * (self.syl_pool.len() + self.chr_pool.len() + self.cat_pool.len())
    }

    /// CAND matching steps 1-5 on the syllable token ids of the buffer (UNK / out-of-range ids never match).
    pub fn build(&self, sid: &[u32], m: usize) -> Vec<Kept> {
        let n = sid.len();
        let mut items: Vec<(usize, usize, u8, u32, u32)> = Vec::new(); // (len, start, srank, canonical id, entry index)
        let mut key: Vec<u16> = Vec::with_capacity(MAXL);
        for s in 0..n {
            key.clear();
            for l in 1..=MAXL.min(n - s) {
                let v = sid[s + l - 1];
                if v > u16::MAX as u32 {
                    break;
                }
                key.push(v as u16);
                if l < 2 {
                    continue;
                }
                if let Some((a, c)) = self.find(&key) {
                    for e in a..a + c {
                        let x = &self.entries[e as usize];
                        items.push((l, s, x.srank, x.id, e));
                    }
                }
            }
        }
        items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3)));
        let mut out = Vec::new();
        let mut tok = 0;
        for (l, s, _, _, e) in items {
            if out.len() == NSLOT {
                break;
            }
            if tok + l > m {
                continue;
            }
            out.push(Kept { start: s, entry: e, len: l });
            tok += l;
        }
        out
    }
}

pub struct CandEmb {
    d: usize,
    type_c: Vec<f32>,
    slot: Vec<f32>,
    cat: Vec<f32>,
    src: Vec<f32>,
    tier: Vec<f32>,
    ncat: usize,
}

impl CandEmb {
    pub fn open(path: &Path, d_model: usize) -> io::Result<CandEmb> {
        let b = std::fs::read(path)?;
        if b.len() < 24 || &b[..4] != b"TWCE" {
            return Err(bad("not a TWCE file"));
        }
        let u = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
        let (ver, d, ncat, nslot) = (u(4), u(8), u(12), u(16));
        if ver != 1 || d != d_model || nslot != NSLOT || ncat == 0 {
            return Err(bad("TWCE version / d / slots mismatch"));
        }
        let mut at = 24;
        let mut take = |k: usize| -> io::Result<Vec<f32>> {
            let end = at + 4 * k;
            let c = b.get(at..end).ok_or_else(|| bad("truncated TWCE"))?;
            at = end;
            Ok(c.chunks_exact(4).map(|x| f32::from_le_bytes(x.try_into().unwrap())).collect())
        };
        let e = CandEmb { d, type_c: take(d)?, slot: take(NSLOT * d)?, cat: take(ncat * d)?, src: take(3 * d)?, tier: take(5 * d)?, ncat };
        if at != b.len() {
            return Err(bad("trailing bytes in TWCE"));
        }
        Ok(e)
    }

    /// Input vector added to every token of entry `e` in slot `j`: ((((type + slot[j]) + mean(cat)) + src) + tier), the
    /// operation order of train/cand.py `cand_vectors` (the category mean sums in ascending category id order).
    pub fn vector(&self, lex: &CandLex, e: u32, j: usize, out: &mut [f32]) {
        let d = self.d;
        let x = &lex.entries[e as usize];
        let cats = &lex.cat_pool[x.cats as usize..x.cats as usize + x.ncat as usize];
        let mut cv = vec![0.0f32; d];
        for &c in cats {
            let c = c as usize;
            for (v, w) in cv.iter_mut().zip(&self.cat[c * d..(c + 1) * d]) {
                *v += w;
            }
        }
        let k = cats.len() as f32;
        for i in 0..d {
            out[i] = (((self.type_c[i] + self.slot[j * d + i]) + cv[i] / k) + self.src[x.src as usize * d + i]) + self.tier[x.tier as usize * d + i];
        }
    }
}

/// The CAND part of `Assets`: lexicon, embeddings and the token cap M.
pub struct CandCfg {
    pub lex: CandLex,
    pub emb: CandEmb,
    pub m: usize,
}

impl CandCfg {
    /// Loads `<base>.clex` and `<base>.cemb` against the loaded model and character vocabulary in `assets`;
    /// `m` must not exceed the M the lexicon was exported for.
    pub fn open(base: &Path, assets: &Assets, m: Option<usize>) -> io::Result<CandCfg> {
        let d_model = assets.model_dim().ok_or_else(|| bad("CAND requires a model"))?;
        let lex = CandLex::open(&base.with_extension("clex"))?;
        let emb = CandEmb::open(&base.with_extension("cemb"), d_model)?;
        for (i, x) in lex.entries.iter().enumerate() {
            if lex.chars(i as u32).iter().any(|&c| c < 2) {
                return Err(bad("TWCL character id is UNK / SEP"));
            }
            if lex.chars(i as u32).iter().any(|&c| assets.vocab.char_of(c as u32).is_none()) {
                return Err(bad("TWCL character id is not a vocabulary character token"));
            }
            if lex.cat_pool[x.cats as usize..x.cats as usize + x.ncat as usize].iter().any(|&c| c as usize >= emb.ncat) {
                return Err(bad("TWCL category id out of range"));
            }
        }
        let m = m.unwrap_or(lex.m);
        if m > lex.m || m == 0 {
            return Err(bad("CAND token cap above the exported M"));
        }
        Ok(CandCfg { lex, emb, m })
    }

    /// Token sequence, RoPE positions and additive vectors for the kept candidates appended after `tokens`
    /// (`first` = position of the first syllable token). Returns None when nothing is kept.
    pub fn assemble(&self, tokens: &[u32], first: usize, d: usize) -> Option<(Vec<u32>, Vec<u32>, Vec<f32>, Vec<Kept>)> {
        let kept = self.lex.build(&tokens[first..], self.m);
        if kept.is_empty() {
            return None;
        }
        let n0 = tokens.len();
        let extra_len: usize = kept.iter().map(|k| k.len).sum();
        let mut toks = tokens.to_vec();
        let mut pos: Vec<u32> = (0..n0 as u32).collect();
        let mut extra = vec![0.0f32; (n0 + extra_len) * d];
        let mut at = n0;
        for (j, k) in kept.iter().enumerate() {
            let mut v = vec![0.0f32; d];
            self.emb.vector(&self.lex, k.entry, j, &mut v);
            for (i, &c) in self.lex.chars(k.entry).iter().enumerate() {
                toks.push(c as u32);
                pos.push((first + k.start + i) as u32);
                extra[at * d..(at + 1) * d].copy_from_slice(&v);
                at += 1;
            }
        }
        Some((toks, pos, extra, kept))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};

    fn assets(with_model: bool) -> Assets {
        let mut a = Assets::from_dictionary_text("ㄐㄧㄣ 今 -2\nㄊㄧㄢ 天 -2\n", "").unwrap();
        if with_model {
            a.model = Some(crate::n1::n1_prefix_tests::model(a.vocab.len()));
        }
        a
    }

    struct Fixture(PathBuf, Assets);

    impl Fixture {
        fn new(ncat: u32, cat: u16, chr: u16) -> Self {
            let a = assets(true);
            let d = a.model_dim().unwrap();
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!("twime-cand-{}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&dir).unwrap();
            let base = dir.join("minimal");
            let mut lex = b"TWCL".to_vec();
            for v in [1u32, 2, 1, 0] {
                lex.extend_from_slice(&v.to_le_bytes());
            }
            lex.extend_from_slice(&[2, 0, 0, 0, 1]);
            for v in [a.vocab.syllable_id("ㄐㄧㄣ") as u16, a.vocab.syllable_id("ㄊㄧㄢ") as u16, a.vocab.char_id('今') as u16, chr, cat] {
                lex.extend_from_slice(&v.to_le_bytes());
            }
            fs::write(base.with_extension("clex"), lex).unwrap();
            let mut emb = b"TWCE".to_vec();
            for v in [1u32, d as u32, ncat, NSLOT as u32, 0] {
                emb.extend_from_slice(&v.to_le_bytes());
            }
            for _ in 0..(1 + NSLOT + ncat as usize + 3 + 5) * d {
                emb.extend_from_slice(&1.0f32.to_le_bytes());
            }
            fs::write(base.with_extension("cemb"), emb).unwrap();
            Fixture(base, a)
        }

        fn rejected(&self) {
            assert_eq!(CandCfg::open(&self.0, &self.1, None).err().unwrap().kind(), io::ErrorKind::InvalidData);
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn valid_assets() {
        let f = Fixture::new(2, 1, 3);
        let cfg = CandCfg::open(&f.0, &f.1, None).unwrap();
        assert_eq!(cfg.lex.chars(0), &[2, 3]);
        let mut out = vec![0.0; f.1.model_dim().unwrap()];
        cfg.emb.vector(&cfg.lex, 0, 0, &mut out);
        assert!(out.iter().all(|&v| v == 5.0));
    }

    #[test]
    fn zero_categories() {
        let f = Fixture::new(0, 0, 3);
        assert_eq!(CandEmb::open(&f.0.with_extension("cemb"), f.1.model_dim().unwrap()).err().unwrap().kind(), io::ErrorKind::InvalidData);
        f.rejected();
    }

    #[test]
    fn category_out_of_range() {
        Fixture::new(2, 2, 3).rejected();
    }

    #[test]
    fn unknown_character() {
        Fixture::new(2, 0, 0).rejected();
    }

    #[test]
    fn separator_character() {
        Fixture::new(2, 0, 1).rejected();
    }

    #[test]
    fn syllable_character() {
        let a = assets(false);
        let id = a.vocab.syllable_id("ㄐㄧㄣ");
        assert!(id >= 2 && (id as usize) < a.vocab.len());
        assert!(a.vocab.char_of(id).is_none());
        Fixture::new(2, 0, id.try_into().unwrap()).rejected();
    }

    #[test]
    fn character_out_of_range() {
        let id = assets(false).vocab.len();
        Fixture::new(2, 0, id.try_into().unwrap()).rejected();
    }

    #[test]
    fn missing_model() {
        let mut f = Fixture::new(2, 1, 3);
        f.1.model = None;
        let err = CandCfg::open(&f.0, &f.1, None).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "CAND requires a model");
    }
}
