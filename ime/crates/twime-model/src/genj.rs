//! SRCH-JUMP: independent tier bonuses, one H across all enabled tiers.
use std::{collections::{BTreeMap, HashMap}, io, path::Path, sync::Arc};
use super::{PnCfg, PnLexicon, Jump, usable_jumps, crosses_forced};
use crate::Assets;

#[derive(Clone, Debug, PartialEq)]
pub struct GenjCfg {
    pub b3: [Option<f64>; 4],
    pub h: Option<usize>,
}
impl GenjCfg {
    /// No implicit defaults: incomplete deployment settings are errors.
    pub fn parse(m: &HashMap<String, String>, hkey: &str) -> io::Result<Self> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "genj: missing or invalid settings");
        let mut b3 = [None; 4];
        for (i, k) in ["b3_a", "b3_b", "b3_c", "b3_d"].iter().enumerate() {
            b3[i] = super::parse_bonus(m.get(*k).ok_or_else(bad)?)?;
        }
        let h = super::parse_h(m.get(hkey).ok_or_else(bad)?)?;
        let c = Self { b3, h };
        if !c.valid() { return Err(bad()); }
        Ok(c)
    }
    pub fn valid(&self) -> bool {
        self.b3.iter().flatten().all(|b| b.is_finite() && *b >= 0.0) && self.h != Some(0)
    }
    pub fn describe(&self) -> String {
        let b: Vec<_> = self.b3.iter().map(|v| v.map_or_else(|| "off".into(), |v| v.to_string())).collect();
        format!("b3_a={} b3_b={} b3_c={} b3_d={} h={}", b[0], b[1], b[2], b[3], self.h.map_or_else(|| "inf".into(), |v| v.to_string()))
    }
}

/// All four missing = off. Any partial set, bad tier or incomplete config is fatal.
pub fn open(base: &Path, assets: &Assets) -> io::Result<Option<(Arc<PnLexicon>, GenjCfg)>> {
    let path = |ext: &str| std::path::PathBuf::from(format!("{}.{ext}", base.display()));
    let n = ["fst", "lst", "tier", "cfg"].iter().filter(|e| path(e).exists()).count();
    if n == 0 { return Ok(None); }
    if n != 4 { return Err(io::Error::new(io::ErrorKind::InvalidData, "genj: partial asset set")); }
    let cfg = GenjCfg::parse(&super::read_settings(&path("cfg"))?, "h")?;
    let lex = PnLexicon::open(base, assets)?;
    let tier = std::fs::read(path("tier"))?;
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "genj: invalid list/tier or entry count mismatch");
    if tier.len() != lex.len() { return Err(bad()); }
    // Walk the actual list, checking the header count and each one-byte tier against the stored class.
    let mut p = super::HEADER;
    let (mut row, mut keys) = (0, 0);
    while p < lex.lst.len() {
        let n = lex.lst[p] as usize; p += 1; keys += 1;
        if n == 0 { return Err(bad()); }
        for _ in 0..n {
            let Some(head) = lex.lst.get(p..p+2) else { return Err(bad()) };
            let (len, t) = (head[0] as usize, head[1]);
            if !(3..=super::MAX_LEN).contains(&len) || t > 3 || tier.get(row) != Some(&t) { return Err(bad()); }
            p += 2 + 2 * len; row += 1;
            if p > lex.lst.len() { return Err(bad()); }
        }
    }
    if row != tier.len() || keys != lex.keys() { return Err(bad()); }
    Ok(Some((Arc::new(lex), cfg)))
}

/// The PN legality/lock/fold expression is reused; ranking is performed once across tiers.
pub fn jumps(hits: &[(usize, u8, Vec<u32>)], sets: &[Vec<u32>], lp: &[Vec<f64>], locked: &[Option<u32>], forced: &[bool], cfg: &GenjCfg, fold: Option<(u32,u32)>) -> Vec<Jump> {
    let mut all = vec![];
    for tier in 0..4 {
        let raw: Vec<_> = hits.iter().filter(|(s,t,ids)| *t == tier && ids.len() >= 3 && !crosses_forced(forced,*s,ids.len())).map(|(s,_,ids)| (*s,ids.clone())).collect();
        all.extend(usable_jumps(&raw,sets,lp,locked,&PnCfg { b2: None,b3: cfg.b3[tier as usize],h:None,cands:false },fold));
    }
    let mut groups: BTreeMap<(usize,usize),Vec<(f64,Jump)>> = BTreeMap::new();
    for j in merge(vec![],all) {
        let sum = j.ids.iter().enumerate().map(|(k,id)| if locked[j.start+k].is_some() {0.0} else {lp[j.start+k][sets[j.start+k].binary_search(id).unwrap()]}).sum();
        groups.entry((j.start,j.ids.len())).or_default().push((sum,j));
    }
    let mut out = vec![];
    for (_,mut g) in groups {
        g.sort_by(|a,b| b.0.total_cmp(&a.0).then(a.1.ids.cmp(&b.1.ids)));
        if let Some(h) = cfg.h { g.truncate(h); }
        out.extend(g.into_iter().map(|(_,j)|j));
    }
    merge(vec![],out)
}

/// One (start, ids), largest bonus, canonical PN ordering.
pub fn merge(pn: Vec<Jump>, j: Vec<Jump>) -> Vec<Jump> {
    let mut out: BTreeMap<(usize,Vec<u32>),f64> = BTreeMap::new();
    for j in pn.into_iter().chain(j) {
        out.entry((j.start,j.ids)).and_modify(|b| *b=b.max(j.bonus)).or_insert(j.bonus);
    }
    let mut out: Vec<_> = out.into_iter().map(|((start,ids),bonus)| Jump {start,ids,bonus}).collect();
    out.sort_by(|a,b|a.start.cmp(&b.start).then(b.ids.len().cmp(&a.ids.len())).then(a.ids.cmp(&b.ids)));
    out
}

impl Assets {
    pub fn set_genj(&mut self, j: Option<(Arc<PnLexicon>, GenjCfg)>) {
        assert!(j.as_ref().is_none_or(|(_,c)| c.valid()));
        self.genj = j;
    }
    pub fn genj(&self) -> Option<&(Arc<PnLexicon>,GenjCfg)> { self.genj.as_ref() }
    /// WORDBONUS table; `None` or an empty table = the old behaviour bit for bit.
    pub fn set_wordbonus(&mut self, w: Option<Arc<crate::wordbonus::WordBonus>>) {
        self.wordbonus = w.filter(|w| !w.is_empty());
    }
    pub fn wordbonus(&self) -> Option<&Arc<crate::wordbonus::WordBonus>> { self.wordbonus.as_ref() }
    pub fn genj_meta(&self) -> serde_json::Value {
        let (chars,syllables)=self.dict.vocabulary();
        serde_json::json!({"chars":chars,"syllables":syllables,"legal":self.legal})
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn merged_jumps(&self, syllables: &[String], sets: &[Vec<u32>], lp: &[Vec<f64>], locked: &[Option<u32>], forced: &[bool], pn: Option<&PnCfg>) -> Vec<Jump> {
        let codes = self.syl_codes(syllables);
        let fold = self.norm.out.then(|| (self.vocab_id(crate::norm::TAI_TRAD),self.vocab_id(crate::norm::TAI)));
        let pj = match (&self.pn,pn) {
            (Some((lex,_)),Some(cfg)) => {
                let raw: Vec<_> = lex.hits(&codes).into_iter().filter(|(s,_,ids)| !crosses_forced(forced,*s,ids.len())).map(|(s,_,ids)|(s,ids)).collect();
                usable_jumps(&raw,sets,lp,locked,cfg,fold)
            },
            _ => vec![],
        };
        // Keep the original list and operations when J is absent.
        let out = match &self.genj {
            Some((lex,cfg)) => merge(pj,jumps(&lex.hits(&codes),sets,lp,locked,forced,cfg,fold)),
            None => pj,
        };
        // WORDBONUS: per-row bonus jumps; no hit = the list above untouched (bit for bit).
        let Some(wb) = &self.wordbonus else { return out };
        let mut wj = vec![];
        for (s,ids,bonus) in wb.hits(&codes) {
            if crosses_forced(forced,s,ids.len()) { continue; }
            let one = usable_jumps(&[(s,ids)],sets,lp,locked,&PnCfg { b2: Some(0.0), b3: Some(0.0), h: None, cands: false },fold);
            wj.extend(one.into_iter().map(|j| Jump { bonus, ..j }));
        }
        if wj.is_empty() { out } else { merge(out,wj) }
    }
    /// Numeric diagnostic inputs and the exact merged list used by pn_select, for an independent Python reference.
    pub fn genj_dump(&self, conv: &crate::Conversion, locks: &[Option<char>]) -> serde_json::Value {
        let inf = &conv.inf;
        let n = inf.syllables.len();
        let real: Vec<_> = (0..n).map(|i| self.lock_id(&inf.sets,locks,i).or_else(||self.custom_lock(&inf.syllables,locks,i))).collect();
        let mut locked = real;
        let mut forced = vec![false;n];
        for (s,ids) in self.forced_spans(&inf.syllables,&inf.sets,&locked) {
            for (k,id) in ids.into_iter().enumerate() { locked[s+k]=Some(id); forced[s+k]=true; }
        }
        let lp = super::log_softmax(&inf.scores);
        let fold = self.norm.out.then(||(self.vocab_id(crate::norm::TAI_TRAD),self.vocab_id(crate::norm::TAI)));
        let out = if inf.fallback { vec![] } else { self.merged_jumps(&inf.syllables,&inf.sets,&lp,&locked,&forced,self.pn.as_ref().map(|p|&p.1)) };
        serde_json::json!({"fallback":inf.fallback,"codes":self.syl_codes(&inf.syllables),"sets":inf.sets,"scores":inf.scores,"locked":locked,"forced":forced,"fold":fold,
            "jumps":out.iter().map(|j|serde_json::json!([j.start,j.ids,j.bonus])).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
#[path = "genj_tests.rs"]
mod tests;

