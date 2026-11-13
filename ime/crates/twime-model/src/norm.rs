//! Glyph normalization of the output layer (Python reference train/ent_norm.py).
//!
//! Table: data/norm/norm-table.v1.tsv (a unit test keeps this file and the table in step).
//! - 臺 -> 台 (user decision 2026-10-12: the default is 台; 臺 stays selectable in the candidate window);
//! - word rules 宣佈/發佈/公佈/分佈/頒佈/散佈 -> ...布;
//! - 佔 is NOT touched (user decision 2026-10-12).
//!
//! Style following (NORM-plan 11.1): with `style_tai` the unlocked 台/臺 characters of a ㄊㄞˊ position are written 臺.
//! Locked characters (the user's own selection) are never changed.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NormCfg {
    /// A1: fold the unlocked output (臺/台 by style, 佈 words).
    pub out: bool,
    /// A3: the model sees the context with 臺 folded into 台.
    pub ctx: bool,
    /// A1: candidate list gets 台/臺 pairs.
    pub pair: bool,
}

impl NormCfg {
    pub const ALL: NormCfg = NormCfg { out: true, ctx: true, pair: true };
}

pub const TAI: char = '台';
pub const TAI_TRAD: char = '臺';

const WORDS: [(&str, &str); 6] = [("宣佈", "宣布"), ("發佈", "發布"), ("公佈", "公布"), ("分佈", "分布"), ("頒佈", "頒布"), ("散佈", "散布")];

/// A3: the context as the model sees it.
pub fn fold_ctx(s: &str) -> String {
    s.chars().map(|c| if c == TAI_TRAD { TAI } else { c }).collect()
}

/// Style of a text: the last 台 or 臺 in it (`Some(true)` = 臺), None if it has neither.
pub fn style_of(s: &str) -> Option<bool> {
    s.chars().rev().find_map(|c| match c {
        TAI => Some(false),
        TAI_TRAD => Some(true),
        _ => None,
    })
}

/// A1: folds the unlocked characters in place. `locked[i]` = the position holds a user selection; `tai_ok[i]` = 臺 is a
/// legal character of the syllable at i (so 台 may be written 臺 there).
pub fn fold_out(chars: &mut [Option<char>], locked: &[bool], tai_ok: &[bool], style_tai: bool) {
    for (i, c) in chars.iter_mut().enumerate() {
        if locked[i] {
            continue;
        }
        match *c {
            Some(TAI_TRAD) if !style_tai => *c = Some(TAI),
            Some(TAI) if style_tai && tai_ok[i] => *c = Some(TAI_TRAD),
            _ => {}
        }
    }
    for (w, r) in WORDS {
        let (w, r): (Vec<char>, Vec<char>) = (w.chars().collect(), r.chars().collect());
        let l = w.len();
        for i in 0..chars.len().saturating_sub(l - 1) {
            if (0..l).all(|k| !locked[i + k] && chars[i + k] == Some(w[k])) {
                for k in 0..l {
                    chars[i + k] = Some(r[k]);
                }
            }
        }
    }
}

/// TA: the default character of the reading ㄊㄚ and its variants.
pub const TA: char = '他';
pub const TA_SYLLABLE: &str = "ㄊㄚ";
pub const TA_VARIANTS: [char; 4] = ['她', '它', '牠', '祂'];
/// Words that are only a ㄊㄚ variant of a 他 word (changed like single characters, never protected as names).
pub const TA_WORDS: [&str; 5] = ["她們", "它們", "牠們", "祂們", "其它"];

/// TA: an unlocked, unprotected position read ㄊㄚ whose character is a variant becomes 他.
pub fn ta_default(chars: &mut [Option<char>], locked: &[bool], syllables: &[String], protect: &[bool]) {
    for i in 0..chars.len() {
        if !locked[i] && !protect[i] && syllables[i] == TA_SYLLABLE && chars[i].is_some_and(|c| TA_VARIANTS.contains(&c)) {
            chars[i] = Some(TA);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ta_default_rule() {
        let syl = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let mut v: Vec<Option<char>> = "她它牠祂她".chars().map(Some).collect();
        let s = syl("ㄊㄚ ㄊㄚ ㄊㄚ ㄊㄚ ㄊㄚ");
        ta_default(&mut v, &[false, true, false, false, false], &s, &[false, false, true, false, false]);
        assert_eq!(v.iter().flatten().collect::<String>(), "他它牠他他", "locked and protected positions stay");
        // other readings are never touched
        let mut v: Vec<Option<char>> = "她".chars().map(Some).collect();
        ta_default(&mut v, &[false], &syl("ㄊㄚˇ"), &[false]);
        assert_eq!(v, vec![Some('她')]);
    }

    fn run(s: &str, locked: &[bool], style: bool) -> String {
        let mut v: Vec<Option<char>> = s.chars().map(Some).collect();
        let ok: Vec<bool> = s.chars().map(|c| c == TAI || c == TAI_TRAD).collect();
        fold_out(&mut v, locked, &ok, style);
        v.into_iter().flatten().collect()
    }

    #[test]
    fn table_matches_tsv() {
        // The table lives in the private data directory (not in the public repository): skipped when it is absent.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/norm/norm-table.v1.tsv");
        if !path.exists() {
            eprintln!("skipped: {} not present", path.display());
            return;
        }
        let t = std::fs::read_to_string(&path).unwrap();
        let mut words = vec![];
        for l in t.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            let f: Vec<&str> = l.split('\t').collect();
            match f[0] {
                "char" => assert_eq!((f[1], f[2]), ("臺", "台")),
                "word" => words.push((f[1], f[2])),
                k => panic!("kind {k}"),
            }
        }
        assert_eq!(words, WORDS.to_vec());
    }

    #[test]
    fn fold_default_and_style() {
        assert_eq!(run("臺灣宣佈佈置佔領", &[false; 8], false), "台灣宣布佈置佔領");
        assert_eq!(run("台灣宣佈", &[false; 4], true), "臺灣宣布"); // style 臺 does not touch 佈 words
        assert_eq!(run("臺灣", &[true, false], false), "臺灣"); // locked stays
        assert_eq!(run("宣佈", &[false, true], false), "宣佈");
        assert_eq!(run("分佈公佈頒佈散佈發佈", &[false; 10], false), "分布公布頒布散布發布");
    }

    #[test]
    fn style_scan() {
        assert_eq!(style_of("abc"), None);
        assert_eq!(style_of("臺灣台北"), Some(false));
        assert_eq!(style_of("台灣臺北"), Some(true));
    }
}
