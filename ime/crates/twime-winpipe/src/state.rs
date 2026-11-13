//! Per-user state files shared by the engine and the setup tool: the data directory,
//! `running.lock` content and `unconfirmed.json` (a list of logon session ids; only ever grows until the tool clears
//! entries whose session no longer exists). No typed text is ever stored here.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("TwPotato")
    }
    // The same directory as the user data (twime-userdata), as on Windows; the file names do not collide.
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/TwPotato")
    }
    // XDG state directory (running.lock, unconfirmed.json are state, not configuration).
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|v| v.is_absolute()) // a relative value is invalid by the XDG rules
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"))
            .join("twpotato")
    }
}

fn is_id(s: &str) -> bool {
    s.len() == 17 && s.as_bytes()[8] == b':' && s.bytes().enumerate().all(|(i, b)| i == 8 || b.is_ascii_hexdigit())
}

/// Parses a `{"logon_sessions":["…","…"]}` document. Anything else is an error (the caller then cannot confirm).
pub fn parse_ids(text: &str) -> io::Result<Vec<String>> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "unconfirmed.json is damaged");
    let t = text.trim();
    if !(t.starts_with('{') && t.ends_with('}') && t.contains("\"logon_sessions\"")) {
        return Err(bad());
    }
    let open = t.find('[').ok_or_else(bad)?;
    let close = t.rfind(']').ok_or_else(bad)?;
    let mut ids = vec![];
    for part in t[open + 1..close].split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let p = p.strip_prefix('"').and_then(|p| p.strip_suffix('"')).ok_or_else(bad)?;
        if !is_id(p) {
            return Err(bad());
        }
        ids.push(p.to_string());
    }
    Ok(ids)
}

pub fn format_ids(ids: &[String]) -> String {
    let list: Vec<String> = ids.iter().map(|i| format!("\"{i}\"")).collect();
    format!("{{\"logon_sessions\":[{}]}}\n", list.join(","))
}

/// `Ok(vec![])` if the file does not exist (clean state); any other failure is an error.
pub fn read_unconfirmed(dir: &Path) -> io::Result<Vec<String>> {
    match fs::read_to_string(dir.join("unconfirmed.json")) {
        Ok(t) => parse_ids(&t),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e),
    }
}

/// Adds `id` (keeping existing entries) and writes through a temporary file; fails if the old file is unreadable.
pub fn append_unconfirmed(dir: &Path, id: &str) -> io::Result<()> {
    let mut ids = read_unconfirmed(dir)?;
    if !ids.iter().any(|i| i == id) {
        ids.push(id.to_string());
    }
    write_unconfirmed(dir, &ids)
}

pub fn write_unconfirmed(dir: &Path, ids: &[String]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join("unconfirmed.json.tmp");
    fs::write(&tmp, format_ids(ids))?;
    fs::rename(&tmp, dir.join("unconfirmed.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_damage() {
        let ids = vec!["00000000:0003e7ab".to_string(), "00000000:00ab12cd".to_string()];
        assert_eq!(parse_ids(&format_ids(&ids)).unwrap(), ids);
        assert_eq!(parse_ids(&format_ids(&[])).unwrap(), Vec::<String>::new());
        for bad in ["", "{", "{\"x\":[]}", "{\"logon_sessions\":[\"zz\"]}", "{\"logon_sessions\":[1]}", "garbage"] {
            assert!(parse_ids(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn append_keeps_old_entries_and_refuses_damaged_files() {
        let d = std::env::temp_dir().join(format!("twime-state-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        assert!(read_unconfirmed(&d).unwrap().is_empty());
        append_unconfirmed(&d, "00000000:00000001").unwrap();
        append_unconfirmed(&d, "00000000:00000002").unwrap();
        append_unconfirmed(&d, "00000000:00000001").unwrap();
        assert_eq!(read_unconfirmed(&d).unwrap().len(), 2);
        fs::write(d.join("unconfirmed.json"), "oops").unwrap();
        assert!(append_unconfirmed(&d, "00000000:00000003").is_err());
        assert_eq!(fs::read_to_string(d.join("unconfirmed.json")).unwrap(), "oops");
        let _ = fs::remove_dir_all(&d);
    }
}
