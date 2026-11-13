use std::path::PathBuf;
pub const SETTINGS_FILE: &str = "settings.toml";
pub const LEXICON_FILE: &str = "userlex.tsv";
pub const AUTOCOMP_FILE: &str = "autocomp.cfg";
pub const LOCK_FILE: &str = "userdata.lock";
pub fn user_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("TwPotato")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/Application Support/TwPotato")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|v| v.is_absolute()) // a relative value is invalid by the XDG rules
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            })
            .join("twpotato")
    }
}
