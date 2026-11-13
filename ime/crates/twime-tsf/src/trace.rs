//! Optional trace file. Off unless `TWIME_TRACE=<path>` is in the environment of the host process when the TIP is
//! activated, or the flag file `trace.on` exists in the per-user data directory (`%LOCALAPPDATA%\TwPotato`; then the
//! trace goes to `tip-trace.log` there). Size caps: a trace file already over the cap is not opened and one process writes
//! at most the cap per activation -- 8 MB for the flag-file log, 64 MiB for a `TWIME_TRACE` file (CARET-MINTTY: a
//! forgotten variable cannot grow the file without bound); the file is never truncated, renamed or deleted. Lines carry
//! event names, key codes, booleans, numbers, profile GUIDs and microsecond timestamps only, never typed or converted text.

use std::{
    cell::RefCell,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

const FLAG_FILE: &str = "trace.on";
const FLAG_LOG: &str = "tip-trace.log";
const FLAG_LOG_CAP: u64 = 8 << 20;
const ENV_LOG_CAP: u64 = 64 << 20;

static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
/// Mirrors `FILE.is_some()` so that `enabled()` on the key path is one relaxed load, no lock.
static ON: AtomicBool = AtomicBool::new(false);
/// Bytes this process may still write in this activation (set from the mode's cap at activation).
static BUDGET: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<Instant> = OnceLock::new();

/// Called at activation: opens the trace file if the environment or the flag file asks for it.
pub fn init() {
    START.get_or_init(Instant::now);
    let env = std::env::var_os("TWIME_TRACE").map(PathBuf::from);
    let file = target(env, || twime_winpipe::state::data_dir()).and_then(|(p, cap)| {
        BUDGET.store(cap, Ordering::Relaxed);
        OpenOptions::new().create(true).append(true).open(p).ok()
    });
    ON.store(file.is_some(), Ordering::Relaxed);
    if let Ok(mut g) = FILE.lock() {
        *g = file;
    }
    GAP.with(|g| *g.borrow_mut() = Gap { activated: Some(Instant::now()), ..Gap::default() });
}

/// The trace file and its cap: the `TWIME_TRACE` path when the variable is set (never the flag file then), otherwise the
/// flag-file log; `None` when nothing asks for a trace or the file is already over its cap.
fn target(env: Option<PathBuf>, dir: impl FnOnce() -> PathBuf) -> Option<(PathBuf, u64)> {
    match env {
        Some(p) => below_cap(&p, ENV_LOG_CAP).then_some((p, ENV_LOG_CAP)),
        None => flag_path(&dir()).map(|p| (p, FLAG_LOG_CAP)),
    }
}

/// A missing file counts as empty.
fn below_cap(path: &std::path::Path, cap: u64) -> bool {
    std::fs::metadata(path).map_or(0, |m| m.len()) <= cap
}

/// Takes `len` bytes from `budget` when they fit; false (budget untouched) when they do not.
fn take(budget: &AtomicU64, len: u64) -> bool {
    budget.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |b| b.checked_sub(len)).is_ok()
}

/// The trace file the flag file asks for: `trace.on` present in `dir` and the log below its size cap.
fn flag_path(dir: &std::path::Path) -> Option<PathBuf> {
    if !dir.join(FLAG_FILE).is_file() {
        return None;
    }
    let log = dir.join(FLAG_LOG);
    below_cap(&log, FLAG_LOG_CAP).then_some(log)
}

/// Per-thread timing of "the engine is not usable": when the TIP was activated, when the engine was first spawned,
/// since when keys or syncs have found it not ready, and whether the first sync was reported.
#[derive(Default)]
struct Gap {
    activated: Option<Instant>,
    spawned: Option<Instant>,
    unready_since: Option<Instant>,
    unready_events: u32,
    first_sync_done: bool,
}

thread_local! {
    static GAP: RefCell<Gap> = RefCell::new(Gap::default());
}

fn ms_since(t: Option<Instant>) -> String {
    t.map_or_else(|| "none".into(), |t| t.elapsed().as_millis().to_string())
}

/// The shell started the engine process (the first start of this activation is the one that counts).
pub fn note_spawn() {
    if enabled() {
        GAP.with(|g| {
            g.borrow_mut().spawned.get_or_insert_with(Instant::now);
        });
    }
}

/// The pipe connected (a new connection of this activation): times since activation and since the engine start.
pub fn note_connected() {
    if enabled() {
        let (a, s) = GAP.with(|g| {
            let g = g.borrow();
            (ms_since(g.activated), ms_since(g.spawned))
        });
        log(&format!("link.connected since_activate_ms={a} since_spawn_ms={s}"));
    }
}

/// A key or a sync found the engine not usable (`reason` is a fixed label). The first one starts the gap.
pub fn note_unready(reason: &'static str) {
    if !enabled() {
        return;
    }
    let first = GAP.with(|g| {
        let mut g = g.borrow_mut();
        g.unready_events = g.unready_events.saturating_add(1);
        let first = g.unready_since.is_none();
        g.unready_since.get_or_insert_with(Instant::now);
        first
    });
    if first {
        log(&format!("unready.begin reason={reason}"));
    }
}

/// A context finished a successful sync with the engine: ends the gap and reports the cold-start times once.
pub fn note_sync_ok() {
    if !enabled() {
        return;
    }
    let (gap, events, since_activate, since_spawn, first) = GAP.with(|g| {
        let mut g = g.borrow_mut();
        let r = (g.unready_since.take(), std::mem::take(&mut g.unready_events), ms_since(g.activated), ms_since(g.spawned), !g.first_sync_done);
        g.first_sync_done = true;
        r
    });
    if let Some(since) = gap {
        log(&format!("unready.end ms={} events={events}", since.elapsed().as_millis()));
    }
    if first {
        log(&format!("first_sync since_activate_ms={since_activate} since_spawn_ms={since_spawn}"));
    }
}

/// `ITfInputProcessorProfileActivationSink::OnActivated`, record only: profile type, language, GUIDs, HKL, flags,
/// whether it is the activation (not a deactivation) and whether the profile is ours.
pub fn profile_activated(kind: u32, langid: u16, clsid: &windows::core::GUID, profile: &windows::core::GUID, hkl: isize, flags: u32) {
    if !enabled() {
        return;
    }
    log(&format!(
        "profile.activated type={kind} langid={langid:#06x} clsid={clsid:?} profile={profile:?} hkl={hkl:#x} flags={flags:#x} active={} ours={}",
        flags & windows::Win32::UI::TextServices::TF_IPSINK_FLAG_ACTIVE != 0,
        clsid.to_u128() == twime_proto::ids::CLSID_TIP
    ));
}

pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn log(msg: &str) {
    if !enabled() {
        return;
    }
    if let Ok(mut g) = FILE.lock()
        && let Some(f) = g.as_mut()
    {
        let us = START.get().map_or(0, |s| s.elapsed().as_micros());
        let line = format!(
            "{us}\tpid={}\ttid={}\t{msg}\n",
            std::process::id(),
            unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
        );
        if take(&BUDGET, line.len() as u64) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

// Decoders return fixed labels only; unknown bits remain visible in the raw value.
use windows::{Win32::UI::TextServices::*, core::Interface};

fn decode_flags(value: u32, flags: &[(u32, &'static str)]) -> String {
    flags
        .iter()
        .filter_map(|(bit, name)| (value & bit != 0).then_some(*name))
        .collect::<Vec<_>>()
        .join("|")
}

fn activation_flags(value: u32) -> String {
    decode_flags(
        value,
        &[
            (TF_TMAE_CONSOLE, "TF_TMAE_CONSOLE"),
            (TF_TMAE_COMLESS, "TF_TMAE_COMLESS"),
            (TF_TMAE_SECUREMODE, "TF_TMAE_SECUREMODE"),
            (TF_TMAE_UIELEMENTENABLEDONLY, "TF_TMAE_UIELEMENTENABLEDONLY"),
            (TF_TMAE_NOACTIVATETIP, "TF_TMAE_NOACTIVATETIP"),
            (TF_TMAE_WOW16, "TF_TMAE_WOW16"),
            (
                TF_TMAE_NOACTIVATEKEYBOARDLAYOUT,
                "TF_TMAE_NOACTIVATEKEYBOARDLAYOUT",
            ),
        ],
    )
}

fn active_flags(value: u32) -> String {
    decode_flags(
        value,
        &[
            (TF_TMF_CONSOLE, "TF_TMF_CONSOLE"),
            (TF_TMF_COMLESS, "TF_TMF_COMLESS"),
            (TF_TMF_SECUREMODE, "TF_TMF_SECUREMODE"),
            (TF_TMF_UIELEMENTENABLEDONLY, "TF_TMF_UIELEMENTENABLEDONLY"),
            (TF_TMF_NOACTIVATETIP, "TF_TMF_NOACTIVATETIP"),
            (TF_TMF_WOW16, "TF_TMF_WOW16"),
            (TF_TMF_ACTIVATED, "TF_TMF_ACTIVATED"),
            (TF_TMF_IMMERSIVEMODE, "TF_TMF_IMMERSIVEMODE"),
        ],
    )
}

fn status_flags(dynamic: u32, static_: u32) -> String {
    format!(
        "dynamic=[{}] static=[{}]",
        decode_flags(
            dynamic,
            &[
                (TS_SD_READONLY, "TS_SD_READONLY"),
                (TS_SD_LOADING, "TS_SD_LOADING")
            ]
        ),
        decode_flags(
            static_,
            &[
                (TS_SS_TRANSITORY, "TS_SS_TRANSITORY"),
                (TS_SS_NOHIDDENTEXT, "TS_SS_NOHIDDENTEXT")
            ]
        )
    )
}

pub fn activation(tm: &ITfThreadMgr, flags: u32) {
    if !enabled() {
        return;
    }
    log(&format!(
        "ActivateEx flags={flags:#010x} decoded=[{}]",
        activation_flags(flags)
    ));
    match tm.cast::<ITfThreadMgrEx>() {
        Ok(ex) => unsafe {
            let mut flags = 0;
            let hr = (ex.vtable().GetActiveFlags)(ex.as_raw(), &mut flags);
            log(&format!(
                "GetActiveFlags hr={:#010x} flags={flags:#010x} valid={} decoded=[{}]",
                hr.0,
                hr.is_ok(),
                active_flags(flags)
            ));
        },
        Err(e) => log(&format!(
            "GetActiveFlags QueryInterface hr={:#010x}",
            e.code().0
        )),
    }
}

pub fn context_status(ctx: &ITfContext, id: u32, phase: &'static str) {
    if !enabled() {
        return;
    }
    let mut status = TS_STATUS::default();
    let hr = unsafe { (ctx.vtable().GetStatus)(ctx.as_raw(), &mut status) };
    log(&format!(
        "context.status ctx={id} phase={phase} hr={:#010x} valid={} dwDynamicFlags={:#010x} dwStaticFlags={:#010x} {}",
        hr.0,
        hr.is_ok(),
        status.dwDynamicFlags,
        status.dwStaticFlags,
        status_flags(status.dwDynamicFlags, status.dwStaticFlags)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_and_active_flags_use_their_own_namespace() {
        assert_eq!(activation_flags(0), "");
        assert_eq!(
            activation_flags(TF_TMAE_CONSOLE | TF_TMAE_COMLESS | 0x100),
            "TF_TMAE_CONSOLE|TF_TMAE_COMLESS"
        );
        for (bit, label) in [
            (TF_TMAE_SECUREMODE, "TF_TMAE_SECUREMODE"),
            (TF_TMAE_UIELEMENTENABLEDONLY, "TF_TMAE_UIELEMENTENABLEDONLY"),
            (TF_TMAE_NOACTIVATETIP, "TF_TMAE_NOACTIVATETIP"),
            (TF_TMAE_WOW16, "TF_TMAE_WOW16"),
            (
                TF_TMAE_NOACTIVATEKEYBOARDLAYOUT,
                "TF_TMAE_NOACTIVATEKEYBOARDLAYOUT",
            ),
        ] {
            assert_eq!(activation_flags(bit), label);
        }
        assert_eq!(
            active_flags(TF_TMF_CONSOLE | TF_TMF_ACTIVATED),
            "TF_TMF_CONSOLE|TF_TMF_ACTIVATED"
        );
    }

    #[test]
    fn status_separates_static_and_dynamic_bits() {
        assert_eq!(
            status_flags(
                TS_SD_READONLY | TS_SD_LOADING,
                TS_SS_TRANSITORY | TS_SS_NOHIDDENTEXT
            ),
            "dynamic=[TS_SD_READONLY|TS_SD_LOADING] static=[TS_SS_TRANSITORY|TS_SS_NOHIDDENTEXT]"
        );
        assert_eq!(
            status_flags(
                TS_SS_TRANSITORY | TS_SS_NOHIDDENTEXT,
                TS_SD_READONLY | TS_SD_LOADING
            ),
            "dynamic=[] static=[]"
        );
        assert_eq!(status_flags(0x8000, 0x8000), "dynamic=[] static=[]");
    }
}

#[cfg(test)]
mod flag_tests {
    use super::*;

    #[test]
    fn flag_file_switches_the_trace_on_and_the_size_cap_off() {
        let dir = std::env::temp_dir().join(format!("twime-trace-flag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(flag_path(&dir), None);
        std::fs::write(dir.join(FLAG_FILE), "").unwrap();
        assert_eq!(flag_path(&dir), Some(dir.join(FLAG_LOG)));
        std::fs::write(dir.join(FLAG_LOG), vec![b'x'; FLAG_LOG_CAP as usize + 1]).unwrap();
        assert_eq!(flag_path(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_mode_has_its_own_cap() {
        let dir = std::env::temp_dir().join(format!("twime-trace-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Flag-file mode: the flag-file log with the 8 MB cap.
        std::fs::write(dir.join(FLAG_FILE), "").unwrap();
        assert_eq!(target(None, || dir.clone()), Some((dir.join(FLAG_LOG), FLAG_LOG_CAP)));
        // Environment mode: the variable's file with the 64 MiB cap, never the flag file (not even when over its cap).
        let env = dir.join("env-trace.log");
        assert_eq!(target(Some(env.clone()), || unreachable!()), Some((env.clone(), ENV_LOG_CAP)));
        let f = std::fs::File::create(&env).unwrap();
        f.set_len(ENV_LOG_CAP).unwrap();
        assert_eq!(target(Some(env.clone()), || dir.clone()), Some((env.clone(), ENV_LOG_CAP)));
        f.set_len(ENV_LOG_CAP + 1).unwrap();
        assert_eq!(target(Some(env.clone()), || dir.clone()), None);
        // A file between the two caps is fine for the environment mode only.
        f.set_len(FLAG_LOG_CAP + 1).unwrap();
        assert_eq!(target(Some(env.clone()), || dir.clone()), Some((env.clone(), ENV_LOG_CAP)));
        drop(f);
        std::fs::rename(&env, dir.join(FLAG_LOG)).unwrap();
        assert_eq!(target(None, || dir.clone()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn budget_is_taken_only_when_the_line_fits() {
        let b = AtomicU64::new(10);
        assert!(take(&b, 4));
        assert!(take(&b, 6));
        assert!(!take(&b, 1));
        assert_eq!(b.load(Ordering::Relaxed), 0);
        let b = AtomicU64::new(5);
        assert!(!take(&b, 6));
        assert_eq!(b.load(Ordering::Relaxed), 5);
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    /// The only test that turns the process-wide trace on (it sets `TWIME_TRACE`), so it does not race with others.
    #[test]
    fn gap_events_are_logged_once_and_carry_only_numbers_and_guids() {
        let path = std::env::temp_dir().join(format!("twime-trace-gap-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        unsafe { std::env::set_var("TWIME_TRACE", &path) };
        init();
        note_unready("key_not_ready");
        note_unready("key_not_ready");
        note_spawn();
        note_connected();
        note_sync_ok();
        note_sync_ok();
        let g = windows::core::GUID::from_u128(twime_proto::ids::CLSID_TIP);
        profile_activated(1, 0x0404, &g, &g, 0x04040404, 1);
        let text = std::fs::read_to_string(&path).unwrap();
        // Other tests of the crate run on other threads while the trace is on: keep this thread's lines only.
        let mine = format!("tid={}	", unsafe { windows::Win32::System::Threading::GetCurrentThreadId() });
        let text: String = text.lines().filter(|l| l.contains(&mine)).collect::<Vec<_>>().join("
");
        unsafe { std::env::remove_var("TWIME_TRACE") };
        ON.store(false, Ordering::Relaxed);
        if let Ok(mut f) = FILE.lock() { *f = None; }
        let _ = std::fs::remove_file(&path);
        assert_eq!(text.matches("unready.begin reason=key_not_ready").count(), 1);
        assert_eq!(text.matches("unready.end ms=").count(), 1);
        assert!(text.contains("events=2"));
        assert_eq!(text.matches("first_sync since_activate_ms=").count(), 1);
        assert!(text.contains("link.connected since_activate_ms="));
        assert!(text.contains("profile.activated type=1 langid=0x0404"));
        assert!(text.contains("active=true ours=true"));
    }
}
