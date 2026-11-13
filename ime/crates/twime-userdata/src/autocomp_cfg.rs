//! SETTINGS-AUTOCOMP (SETTINGS-AUTOCOMP-plan): the user-level `autocomp.cfg` that the settings page writes
//! and the engine reads at start-up. Lines are `autocomp_list=on|off` / `autocomp_pop=on|off`; `#` starts a comment.
//! A missing key means "not overridden" (the engine then falls back to the assets model.cfg, then off).
use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    path::Path,
};
use crate::paths::AUTOCOMP_FILE;
use twime_proto::LexError;

pub const MAX_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutocompCfg {
    pub list: Option<bool>,
    pub pop: Option<bool>,
}
/// How the file looked: absent, parsed, or unusable (the whole file is then ignored).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loaded {
    Absent,
    Ok(AutocompCfg),
    Bad,
}
impl Loaded {
    /// The overrides in effect: a bad file overrides nothing.
    pub fn effective(self) -> AutocompCfg {
        match self {
            Loaded::Ok(c) => c,
            _ => AutocompCfg::default(),
        }
    }
}
pub fn parse(text: &str) -> Result<AutocompCfg, usize> {
    let mut out = AutocompCfg::default();
    let mut seen = HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once('=').ok_or(i + 1)?;
        let (k, v) = (k.trim(), v.trim());
        let on = match v {
            "on" => true,
            "off" => false,
            _ => return Err(i + 1),
        };
        if !seen.insert(k.to_string()) {
            return Err(i + 1);
        }
        match k {
            "autocomp_list" => out.list = Some(on),
            "autocomp_pop" => out.pop = Some(on),
            _ => return Err(i + 1),
        }
    }
    Ok(out)
}
/// Both keys are always written; the popup cannot be on without the list.
pub fn serialize(list: bool, pop: bool) -> String {
    let pop = pop && list;
    let f = |b| if b { "on" } else { "off" };
    format!(
        "# 地瓜輸入法 自動完成（由設定程式寫入；刪除本檔即恢復預設）\nautocomp_list={}\nautocomp_pop={}\n",
        f(list),
        f(pop)
    )
}
/// Bounded read (4 KiB); anything that is not valid UTF-8 or valid syntax is `Bad`.
pub fn load_file(path: &Path) -> Loaded {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Absent,
        Err(_) => return Loaded::Bad,
    };
    let mut bytes = vec![];
    if file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes).is_err() || bytes.len() > MAX_BYTES {
        return Loaded::Bad;
    }
    match std::str::from_utf8(&bytes).ok().and_then(|s| parse(s).ok()) {
        Some(c) => Loaded::Ok(c),
        None => Loaded::Bad,
    }
}
pub fn load(dir: &Path) -> Loaded {
    load_file(&dir.join(AUTOCOMP_FILE))
}
/// Creates the directory, takes the user-data lock and writes atomically. An error leaves the old file in place.
pub fn save(dir: &Path, list: bool, pop: bool) -> Result<(), LexError> {
    std::fs::create_dir_all(dir).map_err(|_| LexError::Io)?;
    let _lock = crate::store::lock(dir)?;
    crate::store::write_atomic(&dir.join(AUTOCOMP_FILE), serialize(list, pop).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("twime-accfg-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    #[test]
    fn parse_ok_and_partial() {
        assert_eq!(parse("").unwrap(), AutocompCfg::default());
        assert_eq!(parse("# c\nautocomp_list = on # x\n").unwrap(), AutocompCfg { list: Some(true), pop: None });
        assert_eq!(
            parse("autocomp_pop=off\nautocomp_list=on\n").unwrap(),
            AutocompCfg { list: Some(true), pop: Some(false) }
        );
    }
    #[test]
    fn parse_rejects() {
        for bad in ["autocomp_list=yes", "autocomp_list", "other=on", "autocomp_list=on\nautocomp_list=off", "autocomp_pop=ON", "=on"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn serialize_round_trip_and_pop_needs_list() {
        for (l, p, el, ep) in [(false, false, false, false), (true, false, true, false), (true, true, true, true), (false, true, false, false)] {
            assert_eq!(parse(&serialize(l, p)).unwrap(), AutocompCfg { list: Some(el), pop: Some(ep) });
        }
    }
    #[test]
    fn load_states_and_bounds() {
        let d = tmp("load");
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(load(&d), Loaded::Absent);
        let p = d.join(AUTOCOMP_FILE);
        std::fs::write(&p, serialize(true, true)).unwrap();
        assert_eq!(load(&d), Loaded::Ok(AutocompCfg { list: Some(true), pop: Some(true) }));
        std::fs::write(&p, b"\xff\xfe\x00").unwrap();
        assert_eq!(load(&d), Loaded::Bad);
        assert_eq!(Loaded::Bad.effective(), AutocompCfg::default());
        std::fs::write(&p, "autocomp_list=on\n").unwrap();
        let mut big = "# ".repeat(MAX_BYTES / 2);
        big.push_str("\nautocomp_list=on\n");
        std::fs::write(&p, &big).unwrap();
        assert_eq!(load(&d), Loaded::Bad);
        let mut ok = String::from("#");
        ok.push_str(&"x".repeat(MAX_BYTES - 2));
        ok.push('\n');
        assert_eq!(ok.len(), MAX_BYTES);
        std::fs::write(&p, &ok).unwrap();
        assert_eq!(load(&d), Loaded::Ok(AutocompCfg::default()));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn save_creates_dir_and_overwrites() {
        let d = tmp("save").join("sub");
        save(&d, true, false).unwrap();
        assert_eq!(load(&d), Loaded::Ok(AutocompCfg { list: Some(true), pop: Some(false) }));
        save(&d, false, true).unwrap();
        assert_eq!(load(&d), Loaded::Ok(AutocompCfg { list: Some(false), pop: Some(false) }));
        // a directory in place of the file: the write fails and reports it
        let d2 = tmp("savefail");
        std::fs::create_dir_all(d2.join(AUTOCOMP_FILE)).unwrap();
        assert!(save(&d2, true, true).is_err());
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
        let _ = std::fs::remove_dir_all(&d2);
    }
}
