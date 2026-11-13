//! The engine process: owns the model and all input sessions; serves the TSF shells over a named pipe.
//!
//! One connection per shell instance; a connection multiplexes sessions by id. Inference is serialized by default
//! (N3-plan section 4). Panics are caught per connection and never take the process down.

pub mod runlock;

use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use twime_core::Engine;
use twime_proto::{Event, Reply, Request, Update, UserReq, UserReply, LexError, CAP_DOC_TEXT, MAX_FRAME};
use twime_winpipe::{PipeServer, ServerConn};

use crate::runlock::RunLock;

/// Resolve int8 from the last explicit flag, otherwise the optional assets model.cfg.
pub fn int8_setting(flag: Option<bool>, dir: Option<&Path>) -> Result<(bool, Option<PathBuf>), String> {
    if let Some(on) = flag {
        return Ok((on, None));
    }
    let Some(file) = dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) else {
        return Ok((false, None));
    };
    Ok((model_cfg_with(&file, true)?.int8, Some(file)))
}

/// USERLEX (USERLEX-plan 1.3): user-lexicon rules from the assets model.cfg, independent of the int8 flags
/// (each USERLEX key is overridden only by its own flag, in main). Absent keys = the old behaviour (force off, bonus 10).
/// With an int8 flag the file's int8 line and any other key are skipped as before (only the USERLEX keys are read and
/// validated).
pub fn user_rules_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<(Option<bool>, Option<f64>), String> {
    match dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) {
        Some(file) => {
            let c = model_cfg_with(&file, int8_flag.is_none())?;
            Ok((c.user_force, c.user_bonus2))
        }
        None => Ok((None, None)),
    }
}

/// FUZZY F1 (FUZZY-plan 3.1): the illegal-syllable repair from the assets model.cfg (`repair=off|auto|suggest`;
/// absent = off), read like the USERLEX keys (also with an int8 flag).
pub fn repair_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<Option<twime_core::RepairMode>, String> {
    match dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) {
        Some(file) => Ok(model_cfg_with(&file, int8_flag.is_none())?.repair),
        None => Ok(None),
    }
}

/// FUZZY F2 (FUZZY-F2-plan 1.1): word-level fuzzy suggestions from the assets model.cfg (`fuzzy_words=on|off`;
/// absent = off), read like the repair key.
pub fn fuzzy_words_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<bool, String> {
    match dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) {
        Some(file) => Ok(model_cfg_with(&file, int8_flag.is_none())?.fuzzy_words.unwrap_or(false)),
        None => Ok(false),
    }
}

/// AUTOCOMP-LIST (AUTOCOMP-LIST-plan 5.1): completions in the candidate list (`autocomp_list=on|off`; absent =
/// off).
pub fn autocomp_list_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<bool, String> {
    match dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) {
        Some(file) => Ok(model_cfg_with(&file, int8_flag.is_none())?.autocomp_list.unwrap_or(false)),
        None => Ok(false),
    }
}

/// EMOJI (EMOJI-plan 3.1): emoji candidates at the end of the candidate list (`emoji=on|off`; absent = off).
pub fn emoji_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<bool, String> {
    match dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) {
        Some(file) => Ok(model_cfg_with(&file, int8_flag.is_none())?.emoji.unwrap_or(false)),
        None => Ok(false),
    }
}

/// EMOJI: reads `emoji.tsv` of the assets directory when `emoji=on`. No directory or no file: the feature stays off with
/// one warning (like POP without its LM weights); an unreadable (other than missing) or malformed file is an error that stops
/// startup.
pub fn load_emoji(dir: Option<&Path>) -> Result<Option<std::sync::Arc<twime_model::emoji::EmojiTable>>, String> {
    let Some(file) = dir.map(|d| d.join("emoji.tsv")) else {
        eprintln!("twime-engined: emoji=on but emoji.tsv is not installed; emoji candidates stay off");
        return Ok(None);
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("twime-engined: emoji=on but emoji.tsv is not installed; emoji candidates stay off");
            return Ok(None);
        }
        Err(e) => return Err(format!("cannot read {}: {e}", file.display())),
    };
    twime_model::emoji::EmojiTable::parse(&text).map(|t| Some(std::sync::Arc::new(t))).map_err(|e| format!("bad {}: {e}", file.display()))
}

/// SETTINGS-AUTOCOMP (SETTINGS-AUTOCOMP-plan 2): flag > user-level autocomp.cfg > assets model.cfg > off, per key.
pub fn autocomp_list_effective(flag: Option<bool>, user: Option<bool>, int8_flag: Option<bool>, dir: Option<&Path>) -> Result<bool, String> {
    match flag.or(user) { Some(v) => Ok(v), None => autocomp_list_setting(int8_flag, dir) }
}
/// Same for the popup key.
pub fn autocomp_pop_effective(flag: Option<bool>, user: Option<bool>, int8_flag: Option<bool>, dir: Option<&Path>) -> Result<bool, String> {
    match flag.or(user) { Some(v) => Ok(v), None => autocomp_pop_setting(int8_flag, dir) }
}

/// The user-level autocomp.cfg of `user_dir` (only when user data is enabled); a bad file is ignored as a whole.
pub fn autocomp_user_cfg(user_data: bool, user_dir: &Path) -> twime_userdata::autocomp_cfg::Loaded {
    if user_data { twime_userdata::autocomp_cfg::load(user_dir) } else { twime_userdata::autocomp_cfg::Loaded::Absent }
}

/// Start-up summary fragment: `autocomp_user=absent|bad|list:on|off|-,pop:on|off|-` (`-` = not overridden).
pub fn autocomp_user_summary(l: twime_userdata::autocomp_cfg::Loaded) -> String {
    use twime_userdata::autocomp_cfg::Loaded;
    let f = |v: Option<bool>| match v { Some(true) => "on", Some(false) => "off", None => "-" };
    match l {
        Loaded::Absent => "autocomp_user=absent".into(),
        Loaded::Bad => "autocomp_user=bad".into(),
        Loaded::Ok(c) => format!("autocomp_user=list:{},pop:{}", f(c.list), f(c.pop)),
    }
}

pub fn autocomp_pop_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<bool, String> {
    match dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) {
        Some(file) => Ok(model_cfg_with(&file, int8_flag.is_none())?.autocomp_pop.unwrap_or(false)),
        None => Ok(false),
    }
}

/// AUTOCOMP (AUTOCOMP-plan 1.1): word completions from the assets model.cfg (`autocomp=on|off`; absent = off).
pub fn autocomp_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<bool, String> {
    match dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) {
        Some(file) => Ok(model_cfg_with(&file, int8_flag.is_none())?.autocomp.unwrap_or(false)),
        None => Ok(false),
    }
}

/// FUZZY F2 step 2 (FUZZY-F2-plan 5.1): fuzzy options in the LEXUX `\` cycle from the assets model.cfg
/// (`fuzzy_hints=on|off`; absent = off), read like `fuzzy_words`.
pub fn fuzzy_hints_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<bool, String> {
    match dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) {
        Some(file) => Ok(model_cfg_with(&file, int8_flag.is_none())?.fuzzy_hints.unwrap_or(false)),
        None => Ok(false),
    }
}

/// SHORT-PRIOR stage A (SHORT-PRIOR-plan 1): `lambda_short_c`, `lambda_short0`, `word_short_bonus`, `prior_ctx`
/// from the assets model.cfg, read like the USERLEX keys (also with an int8 flag); absent = off (the old behaviour).
/// Also returns `word_short_table` (T2 word weights, a file name inside the assets directory; None = T1 only).
pub fn short_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<(twime_model::prior::ShortCfg, Option<PathBuf>), String> {
    match (dir, dir.map(|d| d.join("model.cfg")).filter(|p| p.exists())) {
        (Some(d), Some(file)) => {
            let c = model_cfg_with(&file, int8_flag.is_none())?;
            let cfg = twime_model::prior::ShortCfg::parse(&|k| c.short.get(k).cloned()).map_err(|e| format!("bad {}: {e}", file.display()))?;
            let table = match c.short.get("word_short_table") {
                Some(n) if n.is_empty() || n.contains(['/', '\\', ':']) || n.starts_with('.') => {
                    return Err(format!("bad {}: word_short_table must be a plain file name in the assets directory", file.display()));
                }
                Some(_) if cfg.wb == 0.0 => return Err(format!("bad {}: word_short_table needs word_short_bonus", file.display())),
                Some(n) => Some(d.join(n)),
                None => None,
            };
            Ok((cfg, table))
        }
        _ => Ok((Default::default(), None)),
    }
}

/// WORDBONUS (WORDBONUS-plan 3): `wordbonus=<file name>` of the assets model.cfg names the table (a plain file name
/// inside the assets directory). Unset or empty = off, whatever files lie in the directory. Set but the file missing = error.
pub fn wordbonus_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let (Some(d), Some(file)) = (dir, dir.map(|d| d.join("model.cfg")).filter(|p| p.exists())) else { return Ok(None) };
    match model_cfg_with(&file, int8_flag.is_none())?.wordbonus {
        None => Ok(None),
        Some(n) if n.is_empty() => Ok(None),
        Some(n) if n.contains(['/', '\\', ':']) || n.starts_with('.') => Err(format!("bad {}: wordbonus must be a plain file name in the assets directory", file.display())),
        Some(n) => {
            let p = d.join(n);
            if p.is_file() { Ok(Some(p)) } else { Err(format!("{}: wordbonus names {} which is missing", file.display(), p.display())) }
        }
    }
}

/// LEXUX keys are independent of the int8 override, just like USERLEX and SHORT-PRIOR.
pub fn lexux_setting(int8_flag: Option<bool>, dir: Option<&Path>) -> Result<Option<twime_model::lexux::Config>, String> {
    let Some(file) = dir.map(|d| d.join("model.cfg")).filter(|p| p.exists()) else { return Ok(None) };
    let c = model_cfg_with(&file, int8_flag.is_none())?;
    twime_model::lexux::Config::parse(&c.lexux).map_err(|e| format!("bad {}: {e}", file.display()))
}

const LEXUX_KEYS: [&str; 3] = ["lexux_a", "lexux_b", "lexux_w"];

const SHORT_KEYS: [&str; 5] = ["lambda_short_c", "lambda_short0", "word_short_bonus", "prior_ctx", "word_short_table"];

/// `off` / `auto` / `suggest` (the model.cfg key and the `--repair` flag).
pub fn parse_repair(v: &str) -> Option<Option<twime_core::RepairMode>> {
    match v {
        "off" => Some(None),
        "auto" => Some(Some(twime_core::RepairMode::Auto)),
        "suggest" => Some(Some(twime_core::RepairMode::Suggest)),
        _ => None,
    }
}

struct ModelCfg {
    int8: bool,
    user_force: Option<bool>,
    user_bonus2: Option<f64>,
    repair: Option<twime_core::RepairMode>,
    fuzzy_words: Option<bool>,
    /// FUZZY F2 step 2: `fuzzy_hints=on|off`.
    fuzzy_hints: Option<bool>,
    /// AUTOCOMP: `autocomp=on|off`.
    autocomp: Option<bool>,
    /// AUTOCOMP-LIST: `autocomp_list=on|off`.
    autocomp_list: Option<bool>,
    autocomp_pop: Option<bool>,
    /// EMOJI: `emoji=on|off`.
    emoji: Option<bool>,
    /// WORDBONUS: `wordbonus=<plain file name>` (empty = off).
    wordbonus: Option<String>,
    /// SHORT-PRIOR keys (raw values, parsed by `ShortCfg::parse`).
    short: HashMap<String, String>,
    lexux: HashMap<String, String>,
}

/// Strict (`strict`): every non-empty line (after `#` comments) is `int8=on|off` (required), or the optional
/// `user_force=on|off` and `user_bonus2=<0..30>`; each key at most once. Not strict (an int8 flag replaces the file's
/// int8): only the USERLEX keys are read and validated, every other line is skipped.
fn model_cfg_with(file: &Path, strict: bool) -> Result<ModelCfg, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let (mut int8, mut force, mut bonus, mut repair) = (None, None, None, None);
    let mut fuzzy_words = None;
    let mut fuzzy_hints = None;
    let mut autocomp = None;
    let mut autocomp_list = None;
    let mut autocomp_pop = None;
    let mut emoji = None;
    let mut wordbonus: Option<String> = None;
    let mut short: HashMap<String, String> = HashMap::new();
    let mut lexux = HashMap::new();
    let on_off = |v: &str, key: &str| match v {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("bad {}: {key} must be on or off", file.display())),
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            if !strict {
                continue;
            }
            return Err(format!("bad {}: only int8, user_force, user_bonus2 and repair are allowed", file.display()));
        };
        let (k, v) = (k.trim(), v.trim());
        if !strict && k != "user_force" && k != "user_bonus2" && k != "repair" && k != "fuzzy_words" && k != "fuzzy_hints" && k != "autocomp" && k != "autocomp_list" && k != "autocomp_pop" && k != "emoji" && k != "wordbonus" && !SHORT_KEYS.contains(&k) && !LEXUX_KEYS.contains(&k) {
            continue;
        }
        let twice = |k: &str| format!("bad {}: {k} given twice", file.display());
        match k {
            "int8" => {
                if int8.replace(on_off(v, k)?).is_some() { return Err(twice(k)); }
            }
            "user_force" => {
                if force.replace(on_off(v, k)?).is_some() { return Err(twice(k)); }
            }
            "user_bonus2" => {
                let b: f64 = v.parse().ok().filter(|b: &f64| b.is_finite() && (0.0..=30.0).contains(b))
                    .ok_or_else(|| format!("bad {}: user_bonus2 must be a number in 0..30", file.display()))?;
                if bonus.replace(b).is_some() { return Err(twice(k)); }
            }
            "repair" => {
                let m = parse_repair(v).ok_or_else(|| format!("bad {}: repair must be off, auto or suggest", file.display()))?;
                if repair.replace(m).is_some() { return Err(twice(k)); }
            }
            "fuzzy_words" => {
                if fuzzy_words.replace(on_off(v, k)?).is_some() { return Err(twice(k)); }
            }
            "fuzzy_hints" => {
                if fuzzy_hints.replace(on_off(v, k)?).is_some() { return Err(twice(k)); }
            }
            "autocomp" => {
                if autocomp.replace(on_off(v, k)?).is_some() { return Err(twice(k)); }
            }
            "autocomp_pop" => {
                if autocomp_pop.replace(on_off(v, k)?).is_some() { return Err(twice(k)); }
            }
            "autocomp_list" => {
                if autocomp_list.replace(on_off(v, k)?).is_some() { return Err(twice(k)); }
            }
            "emoji" => {
                if emoji.replace(on_off(v, k)?).is_some() { return Err(twice(k)); }
            }
            "wordbonus" => {
                if wordbonus.replace(v.to_string()).is_some() { return Err(twice(k)); }
            }
            k if LEXUX_KEYS.contains(&k) => {
                if lexux.insert(k.to_string(), v.to_string()).is_some() { return Err(twice(k)); }
            }
            k if SHORT_KEYS.contains(&k) => {
                if short.insert(k.to_string(), v.to_string()).is_some() { return Err(twice(k)); }
            }
            _ => return Err(format!("bad {}: only int8, user_force, user_bonus2, repair and the SHORT-PRIOR keys are allowed", file.display())),
        }
    }
    let int8 = match int8 {
        Some(v) => v,
        None if !strict => false,
        None => return Err(format!("{} has no int8", file.display())),
    };
    Ok(ModelCfg { int8, user_force: force, user_bonus2: bonus, repair: repair.flatten(), fuzzy_words, fuzzy_hints, autocomp, autocomp_list, autocomp_pop, emoji, wordbonus, short, lexux })
}

#[cfg(test)]
mod int8_tests {
    use super::int8_setting;
    use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!("twime-int8-{}-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, text: &str) {
            fs::write(self.0.join("model.cfg"), text).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn flag_on_overrides_file() {
        let dir = TempDir::new();
        dir.write("int8=off\nextra=bad\n");
        assert_eq!(int8_setting(Some(true), Some(&dir.0)).unwrap(), (true, None));
    }

    #[test]
    fn flag_off_overrides_file() {
        let dir = TempDir::new();
        dir.write("int8=on\nextra=bad\n");
        assert_eq!(int8_setting(Some(false), Some(&dir.0)).unwrap(), (false, None));
    }

    #[test]
    fn file_on() {
        let dir = TempDir::new();
        dir.write("# model settings\nint8=on\n");
        assert_eq!(int8_setting(None, Some(&dir.0)).unwrap(), (true, Some(dir.0.join("model.cfg"))));
    }

    #[test]
    fn file_off() {
        let dir = TempDir::new();
        dir.write("int8=off\n");
        assert_eq!(int8_setting(None, Some(&dir.0)).unwrap(), (false, Some(dir.0.join("model.cfg"))));
    }

    #[test]
    fn user_rules_from_file() {
        use super::user_rules_setting;
        let dir = TempDir::new();
        dir.write("int8=on
user_force=on
user_bonus2=20
");
        assert_eq!(int8_setting(None, Some(&dir.0)).unwrap().0, true);
        assert_eq!(user_rules_setting(None, Some(&dir.0)).unwrap(), (Some(true), Some(20.0)));
        // An int8 flag does not touch the USERLEX keys of the file.
        assert_eq!(int8_setting(Some(false), Some(&dir.0)).unwrap(), (false, None));
        assert_eq!(user_rules_setting(Some(false), Some(&dir.0)).unwrap(), (Some(true), Some(20.0)));
        // With an int8 flag a bad int8 line or another key is skipped as before; the USERLEX keys are still validated.
        dir.write("int8=yes
extra=bad
user_force=on
");
        assert_eq!(user_rules_setting(Some(true), Some(&dir.0)).unwrap(), (Some(true), None));
        dir.write("int8=yes
user_bonus2=99
");
        assert!(user_rules_setting(Some(true), Some(&dir.0)).is_err());
        dir.write("int8=on
");
        assert_eq!(user_rules_setting(None, Some(&dir.0)).unwrap(), (None, None));
        for bad in ["int8=on
user_bonus2=31
", "int8=on
user_force=yes
", "int8=on
user_force=on
user_force=off
", "user_force=on
"] {
            dir.write(bad);
            assert!(int8_setting(None, Some(&dir.0)).is_err(), "{bad}");
        }
    }

    #[test]
    fn no_file() {
        let dir = TempDir::new();
        assert_eq!(int8_setting(None, Some(&dir.0)).unwrap(), (false, None));
        assert_eq!(int8_setting(None, None).unwrap(), (false, None));
    }

    #[test]
    fn bad_value() {
        let dir = TempDir::new();
        dir.write("int8=yes\n");
        assert!(int8_setting(None, Some(&dir.0)).is_err());
    }

    #[test]
    fn extra_key() {
        let dir = TempDir::new();
        dir.write("int8=on\nother=off\n");
        assert!(int8_setting(None, Some(&dir.0)).is_err());
    }

    #[test]
    fn malformed_or_repeated_lines() {
        let dir = TempDir::new();
        for text in ["int8=on
other
", "int8=on
int8 off
", "int8=on
int8=on
", "int8=on
=on
"] {
            dir.write(text);
            assert!(int8_setting(None, Some(&dir.0)).is_err(), "{text:?}");
        }
    }

    #[test]
    fn missing_key() {
        let dir = TempDir::new();
        dir.write("# no int8 key\n");
        assert!(int8_setting(None, Some(&dir.0)).is_err());
    }
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Run `Session::handle` of different connections one at a time (the plan's single inference thread).
    pub serial: bool,
    /// Accept `Event::SetContext` (measurement tools).
    pub allow_test_events: bool,
    /// Enable the fault-injection session ids (see `fault`).
    pub test_faults: bool,
    /// Directory of the uninstall marker (the install root).
    pub marker_dir: Option<PathBuf>,
    /// The assets use the document bigram cache: permits version 4 and advertises CAP_DOC_TEXT in version 5.
    pub doc_text: bool,
}

impl Options {
    /// Highest wire version; document access is negotiated separately in v5.
    pub fn max_version(&self) -> u16 {
        twime_proto::PROTOCOL_VERSION
    }
}

const MAX_SESSIONS_PER_CONN: usize = 256;

/// State shared by all connections.
struct Shared {
    gate: Mutex<()>,
    /// (connection id, session id) -> composing, for every open input session.
    open: Mutex<HashMap<(u64, u32), bool>>,
    abnormal: AtomicU32,
    next_conn: AtomicU64,
    lock: RunLock,
    marker: Option<PathBuf>,
    marker_seen: Mutex<(Instant, bool)>,
}

impl Shared {
    /// Uninstall marker present (checked at most every 100 ms).
    fn uninstalling(&self) -> bool {
        let Some(m) = &self.marker else { return false };
        let mut g = self.marker_seen.lock().unwrap_or_else(|p| p.into_inner());
        if g.0.elapsed() > Duration::from_millis(100) {
            *g = (Instant::now(), m.exists());
        }
        g.1
    }

    fn open(&self) -> std::sync::MutexGuard<'_, HashMap<(u64, u32), bool>> {
        self.open.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Fault injection (only with `Options::test_faults`), selected by the session id of a request:
/// `0xFFFF_xxxx` sleep xxxx ms (0xFFFF: forever) before answering; `0xFFFE_xxxx` exit the process immediately;
/// `0xFFFD_xxxx` answer with an Update whose commit text is xxxx bytes long.
fn fault(session: u32, conn: &ServerConn, seq: u32) -> bool {
    match session >> 16 {
        0xFFFF => {
            let ms = session & 0xFFFF;
            if ms == 0xFFFF {
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
            std::thread::sleep(Duration::from_millis(u64::from(ms)));
            send(conn, &Reply::Update { session, seq, update: Update::default() });
            true
        }
        0xFFFE => std::process::exit(3),
        0xFFFD => {
            let n = (session & 0xFFFF) as usize;
            let u = Update { commit: Some("x".repeat(n)), ..Update::default() };
            send(conn, &Reply::Update { session, seq, update: u });
            true
        }
        _ => false,
    }
}

fn send(conn: &ServerConn, r: &Reply) -> bool {
    conn.write(&r.encode()).is_ok()
}

/// Sends in the wire version the connection negotiated (1 until the shell sent `Hello`).
fn send_v(conn: &ServerConn, r: &Reply, version: u16) -> bool {
    let mut bytes = r.encode_for(version);
    if matches!(r, Reply::User(_)) && bytes.len() > MAX_FRAME {
        bytes = Reply::User(UserReply::Error(LexError::Full)).encode_for(version);
    }
    conn.write(&bytes).is_ok()
}

fn ack(conn: &ServerConn, req: &Request, version: u16, engine: &Engine) -> bool {
    let update = Update { toggle_key: engine.toggle_key(), ..Update::default() };
    send_v(conn, &Reply::Update { session: req.session, seq: req.seq, update }, version)
}

fn serve_conn(conn: ServerConn, engine: Arc<Engine>, opts: Options, sh: Arc<Shared>) {
    #[cfg(feature = "arch-prof")]
    let mut prof_file = std::env::var_os("TWIME_PROF_TSV").and_then(|p| {
        std::fs::OpenOptions::new().create(true).append(true).open(p)
            .map(std::io::BufWriter::new).map_err(|e| eprintln!("arch-prof: {e}")).ok()
    });
    let cid = sh.next_conn.fetch_add(1, Ordering::SeqCst);
    let mut sessions: HashMap<u32, twime_core::Session> = HashMap::new();
    // Wire version of this connection: 1 (the original bytes) until the shell negotiates more with `Hello`.
    let mut version: u16 = 1;
    // Set by the first session event: the version is fixed from then on, even after the last session closed.
    let mut session_events = false;
    loop {
        let Ok(Some(payload)) = conn.read() else { break };
        let Some(req) = Request::decode(&payload) else { break };
        // Control requests (the uninstall tool) never create a session.
        match req.event {
            Event::Hello(v) => {
                // Version negotiation; creates no session and a late Hello (after a session exists) keeps the version.
                if !session_events {
                    version = v.clamp(1, opts.max_version());
                    if version == 4 && !opts.doc_text { version = 3; }
                }
                let reply = if version >= 5 {
                    Reply::HelloCaps { version, caps: if opts.doc_text { CAP_DOC_TEXT } else { 0 } }
                } else { Reply::Hello { version } };
                if !send(&conn, &reply) {
                    break;
                }
                continue;
            }
            Event::User(ref request) => {
                if version < 5 { break; }
                let reply = match engine.user() {
                    None => UserReply::Error(LexError::Disabled),
                    Some(user) => match request {
                        UserReq::GetSettings | UserReq::List { .. } | UserReq::ReadingOptions(_) => user.service.handle(request.clone()),
                        _ => user.writer.submit(request.clone()).recv().unwrap_or(UserReply::Error(LexError::Io)),
                    },
                };
                if !send_v(&conn, &Reply::User(reply), version) { break; }
                continue;
            }
            Event::Status => {
                let (n, c) = {
                    let open = sh.open();
                    (open.len(), open.values().filter(|c| **c).count())
                };
                let r = Reply::Status {
                    sessions: n.min(u16::MAX as usize) as u16,
                    composing: c.min(u16::MAX as usize) as u16,
                    abnormal: sh.abnormal.load(Ordering::SeqCst).min(u32::from(u16::MAX)) as u16,
                };
                if !send(&conn, &r) {
                    break;
                }
                continue;
            }
            Event::Shutdown => {
                // Refused while any input session is open or an abnormal end was recorded (the tool must not get here then).
                let honored = sh.uninstalling() && sh.open().is_empty() && sh.abnormal.load(Ordering::SeqCst) == 0;
                let _ = ack(&conn, &req, version, &engine);
                if honored {
                    // Clean end: the lock goes away only if nothing abnormal was recorded.
                    sh.lock.release(sh.abnormal.load(Ordering::SeqCst) > 0);
                    std::process::exit(0);
                }
                continue;
            }
            _ => {}
        }
        session_events = true;
        if opts.test_faults && req.session >> 16 >= 0xFFFD && fault(req.session, &conn, req.seq) {
            continue;
        }
        if matches!(req.event, Event::SetContext(_)) && !opts.allow_test_events {
            break;
        }
        // Document text requires version 4 or the explicit version 5 capability.
        if matches!(req.event, Event::DocText(_)) && (version < 4 || (version >= 5 && !opts.doc_text)) {
            break;
        }
        if matches!(req.event, Event::AddToLexicon) && version < 5 { break; }
        if version < 7 && matches!(req.event, Event::Idle(_) | Event::DownCompletion(_)) {
            if !ack(&conn, &req, version, &engine) { break; }
            continue;
        }
        if matches!(req.event, Event::Idle(_)) && !sessions.contains_key(&req.session) {
            if !ack(&conn, &req, version, &engine) { break; }
            continue;
        }
        if req.event == Event::Close {
            sessions.remove(&req.session);
            sh.open().remove(&(cid, req.session));
            if !ack(&conn, &req, version, &engine) {
                break;
            }
            continue;
        }
        if sh.uninstalling() {
            // Refuse input; the shell ends its composition (plans 4.1) and closes the session.
            if !send(&conn, &Reply::Disabled { session: req.session, seq: req.seq }) {
                break;
            }
            sh.open().insert((cid, req.session), false);
            continue;
        }
        if !sessions.contains_key(&req.session) && sessions.len() >= MAX_SESSIONS_PER_CONN {
            break;
        }
        let session = sessions.entry(req.session).or_insert_with(|| {
            let mut s = engine.new_session();
            // Version 3 connections get the English / punctuation behaviour; older ones behave as before.
            s.set_ux(version >= 3);
            s.set_lexux_peer(version >= 6);
            s.set_pop_peer(version >= 7);
            s
        });
        if let Event::Idle(generation) = req.event {
            let t = Instant::now();
            let result = catch_unwind(AssertUnwindSafe(|| {
                let guard = opts.serial.then(|| sh.gate.lock().unwrap_or_else(|p| p.into_inner()));
                let snapshot = session.prepare_idle(req.seq, req.ack, generation);
                drop(guard);
                let ranked = snapshot.map(|s| s.compute()).unwrap_or_default();
                let _guard = opts.serial.then(|| sh.gate.lock().unwrap_or_else(|p| p.into_inner()));
                session.finish_idle(generation, ranked)
            }));
            let Ok(mut update) = result else { break };
            update.engine_us = t.elapsed().as_micros().min(u32::MAX as u128) as u32;
            // Idle itself never edits text, but a failed acknowledgement can clear/restore the previous write.
            if req.ack.is_some_and(|a| a.commit == twime_proto::Applied::Failed || a.comp == twime_proto::Applied::Failed) {
                sh.open().insert((cid, req.session), update.composing);
            }
            if !send_v(&conn, &Reply::Update { session: req.session, seq: req.seq, update }, version) { break; }
            continue;
        }
        let guard = opts.serial.then(|| sh.gate.lock().unwrap_or_else(|p| p.into_inner()));
        #[cfg(feature = "arch-prof")]
        let prof_event = match &req.event {
            Event::Key(_) => "Key", Event::CandidatePick(_) => "Select",
            Event::Focus(_) => "Focus", Event::ExternalEdit => "ExternalEdit",
            Event::CompositionAborted => "CompositionAborted", Event::Unsynced => "Unsynced",
            Event::Password(_) => "Password", Event::Close => "Close", Event::Ping => "Ping",
            Event::SetContext(_) => "SetContext", Event::Status => "Status",
            Event::Shutdown => "Shutdown", Event::Hello(_) => "Hello",
            Event::ToggleMode => "ToggleMode", Event::DocText(_) => "DocText",
            Event::Idle(_) => "Idle", Event::DownCompletion(_) => "DownCompletion",
            Event::AddToLexicon => "AddToLexicon", Event::User(_) => "User",
        };
        #[cfg(feature = "arch-prof")]
        let _ = twime_model::prof::take();
        let t = Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| session.handle(req.seq, req.ack, req.event)));
        #[cfg(feature = "arch-prof")]
        let total_ns = t.elapsed().as_nanos();
        let us = t.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
        #[cfg(feature = "arch-prof")]
        let prof = twime_model::prof::take();
        drop(guard);
        #[cfg(feature = "arch-prof")]
        if let Some(f) = &mut prof_file {
            use std::io::Write;
            if let Err(e) = writeln!(f, "{cid}\t{}\t{prof_event}\t{total_ns}\t{}\t{}\t{}\t{}\t{}",
                req.seq, prof.forward_ns, prof.head_ns, prof.calls, prof.l, prof.nsyl).and_then(|_| f.flush()) {
                eprintln!("arch-prof: {e}");
            }
        }
        let Ok(mut update) = r else { break };
        update.engine_us = us;
        sh.open().insert((cid, req.session), update.composing);
        if !send_v(&conn, &Reply::Update { session: req.session, seq: req.seq, update }, version) {
            break;
        }
    }
    // A connection that still has open sessions ended without closing them: record it before they are released.
    {
        let mut open = sh.open();
        if open.keys().any(|(c, _)| *c == cid) {
            sh.abnormal.fetch_add(1, Ordering::SeqCst);
            sh.lock.record_abnormal();
        }
        open.retain(|(c, _), _| *c != cid);
    }
    conn.disconnect();
}

/// Accepts connections forever (one thread per connection).
pub fn serve(engine: Arc<Engine>, server: PipeServer, opts: Options, lock: RunLock) -> std::io::Result<()> {
    let sh = Arc::new(Shared {
        gate: Mutex::new(()),
        open: Mutex::new(HashMap::new()),
        abnormal: AtomicU32::new(0),
        next_conn: AtomicU64::new(1),
        lock,
        marker: opts.marker_dir.clone().map(|d| d.join(twime_proto::ids::UNINSTALL_MARKER)),
        marker_seen: Mutex::new((Instant::now() - Duration::from_secs(1), false)),
    });
    loop {
        let conn = server.accept()?;
        let (e, o, s) = (engine.clone(), opts.clone(), sh.clone());
        std::thread::spawn(move || serve_conn(conn, e, o, s));
    }
}
