//! Where the engine and its assets are installed (LINUX-plan 4.3, 6.1).

use std::{ffi::OsString, path::{Path, PathBuf}};

pub const ENGINE_NAME: &str = "twime-engined";

/// (engine executable, assets directory), both from the same root: the user-mode root when its engine exists
/// (`$XDG_DATA_HOME/twpotato/{bin,assets}`, default `~/.local/share`), otherwise the system one. A relative
/// `XDG_DATA_HOME` or `HOME` is invalid and ignored. `None`: no engine is installed.
#[cfg_attr(all(target_os = "macos", not(test)), allow(dead_code))]
pub fn install_root(xdg_data_home: Option<OsString>, home: Option<OsString>, is_file: impl Fn(&Path) -> bool) -> Option<(PathBuf, PathBuf)> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let user = absolute(xdg_data_home).or_else(|| absolute(home).map(|h| h.join(".local/share"))).map(|d| d.join("twpotato"));
    if let Some(root) = user {
        let engine = root.join("bin").join(ENGINE_NAME);
        if is_file(&engine) {
            return Some((engine, root.join("assets")));
        }
    }
    let engine = Path::new("/usr/lib/twpotato").join(ENGINE_NAME);
    is_file(&engine).then(|| (engine, PathBuf::from("/usr/share/twpotato/assets")))
}

/// macOS: (engine executable, assets directory) inside the application bundle this executable belongs to.
/// `exe` is the path of the running executable with symbolic links resolved; it must be `<X>.app/Contents/MacOS/<name>`.
/// Then the engine is its neighbour and the assets are `<X>.app/Contents/Resources/assets` (that directory may be a
/// link). Anything else, or no engine file: `None`, no engine is installed.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn bundle_root(exe: &Path, is_file: impl Fn(&Path) -> bool) -> Option<(PathBuf, PathBuf)> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    if !exe.is_absolute() || macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" || app.extension()? != "app" {
        return None;
    }
    let engine = macos.join(ENGINE_NAME);
    is_file(&engine).then(|| (engine, contents.join("Resources/assets")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(exe: &str, files: &[&str]) -> Option<(String, String)> {
        bundle_root(Path::new(exe), |p| files.contains(&p.to_str().unwrap())).map(|(e, a)| (e.to_str().unwrap().to_owned(), a.to_str().unwrap().to_owned()))
    }

    #[test]
    fn bundle_engine_and_assets_come_from_the_bundle_of_the_executable() {
        let engine = "/L/Input Methods/TwPotato.app/Contents/MacOS/twime-engined";
        assert_eq!(
            bundle("/L/Input Methods/TwPotato.app/Contents/MacOS/TwPotato", &[engine]),
            Some((engine.into(), "/L/Input Methods/TwPotato.app/Contents/Resources/assets".into()))
        );
    }

    #[test]
    fn anything_that_is_not_a_bundle_or_has_no_engine_is_not_installed() {
        assert_eq!(bundle("/a/T.app/Contents/MacOS/t", &[]), None, "no engine file");
        for exe in ["/a/bin/t", "/a/T.app/MacOS/t", "/a/T.app/Contents/Other/t", "/a/T/Contents/MacOS/t", "/a/T.app/Contents/MacOS/x/t", "T.app/Contents/MacOS/t", "/t", "/"] {
            let dir = Path::new(exe).parent().map(|d| d.join(ENGINE_NAME));
            assert_eq!(bundle_root(Path::new(exe), |p| Some(p) == dir.as_deref()), None, "{exe}");
        }
    }

    fn root(xdg: Option<&str>, home: Option<&str>, files: &[&str]) -> Option<(String, String)> {
        install_root(xdg.map(OsString::from), home.map(OsString::from), |p| files.contains(&p.to_str().unwrap()))
            .map(|(e, a)| (e.to_str().unwrap().to_owned(), a.to_str().unwrap().to_owned()))
    }
    const SYS: &str = "/usr/lib/twpotato/twime-engined";

    #[test]
    fn user_root_wins_and_brings_its_own_assets() {
        let user = "/d/twpotato/bin/twime-engined";
        assert_eq!(root(Some("/d"), Some("/h"), &[user, SYS]), Some((user.into(), "/d/twpotato/assets".into())));
    }

    #[test]
    fn default_data_home_is_under_home() {
        let user = "/h/.local/share/twpotato/bin/twime-engined";
        assert_eq!(root(None, Some("/h"), &[user]), Some((user.into(), "/h/.local/share/twpotato/assets".into())));
    }

    #[test]
    fn relative_xdg_value_is_ignored() {
        let user = "/h/.local/share/twpotato/bin/twime-engined";
        assert_eq!(root(Some("rel"), Some("/h"), &["rel/twpotato/bin/twime-engined", user]).unwrap().0, user);
        assert_eq!(root(Some("rel"), Some("also-rel"), &["rel/twpotato/bin/twime-engined"]), None);
    }

    #[test]
    fn system_root_when_the_user_one_has_no_engine() {
        // Assets alone do not make a root: engine and assets are never mixed.
        assert_eq!(root(Some("/d"), Some("/h"), &[SYS, "/d/twpotato/assets"]), Some((SYS.into(), "/usr/share/twpotato/assets".into())));
        assert_eq!(root(None, None, &[SYS]).unwrap().0, SYS);
    }

    #[test]
    fn nothing_installed() {
        assert_eq!(root(Some("/d"), Some("/h"), &[]), None);
    }
}
