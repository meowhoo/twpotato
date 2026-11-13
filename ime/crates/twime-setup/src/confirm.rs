//! Fail-closed confirmation before a TwPotato/TwIme uninstall may proceed: reads `unconfirmed.json` and
//! `running.lock` itself and, if the engine is up, asks it for its session counts over a verified connection.
//! Anything unknown is "cannot confirm".

use std::{
    fs::OpenOptions,
    io::Read,
    os::windows::fs::OpenOptionsExt,
    time::{Duration, Instant},
};

use twime_proto::{Event, Reply, Request};
use twime_winpipe::{Client, state};

/// Logon session ids that currently exist (`hhhhhhhh:llllllll`).
pub fn live_sessions() -> Result<Vec<String>, String> {
    use windows::Win32::{
        Foundation::LUID,
        Security::Authentication::Identity::{LsaEnumerateLogonSessions, LsaFreeReturnBuffer},
    };
    unsafe {
        let (mut n, mut p) = (0u32, std::ptr::null_mut::<LUID>());
        let st = LsaEnumerateLogonSessions(&mut n, &mut p);
        if st.0 != 0 || p.is_null() {
            return Err(format!("LsaEnumerateLogonSessions failed (status {:#x})", st.0));
        }
        let ids = std::slice::from_raw_parts(p, n as usize).iter().map(|l| format!("{:08x}:{:08x}", l.HighPart as u32, l.LowPart)).collect();
        let _ = LsaFreeReturnBuffer(p as *const _);
        Ok(ids)
    }
}

/// One of the recorded ids still means "unconfirmed" if it is the current logon session or the session still exists.
fn blocking(id: &str, current: &str, live: &[String]) -> bool {
    id == current || live.iter().any(|l| l == id)
}

/// Compare normalized image paths without filesystem or process access.
pub(super) fn image_allowed(image: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|p| super::legacy::same_path(std::path::Path::new(image), std::path::Path::new(p)))
}

pub(super) fn connect_verified(pipe: &str, allowed: &[String]) -> Result<Option<Client>, String> {
    if allowed.is_empty() { return Ok(None); }
    let c = match Client::connect(pipe, None, 0) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot identify the engine: {e}")),
    };
    let image = twime_winpipe::image_path_of_pid(c.server_pid)
        .map_err(|e| format!("cannot identify the engine: {e}"))?;
    if !image_allowed(&image, allowed) { return Err("cannot identify the engine: image is not allowed".into()); }
    Ok(Some(c))
}

/// Status of the running engine through a verified control connection: (sessions, composing, abnormal).
pub fn engine_status(pipe: &str, allowed: &[String]) -> Result<Option<(u16, u16, u16)>, String> {
    let Some(c) = connect_verified(pipe, allowed)? else { return Ok(None); };
    let req = Request { session: 0, seq: 1, ack: None, event: Event::Status };
    let bytes = c.call(&req.encode(), 2000).map_err(|e| format!("engine status failed: {e:?}"))?;
    match Reply::decode(&bytes) {
        Some(Reply::Status { sessions, composing, abnormal }) => Ok(Some((sessions, composing, abnormal))),
        _ => Err("engine sent an unexpected answer".into()),
    }
}

/// Asks the (marker-aware) engine to end; waits for the pipe to disappear.
pub fn shutdown_engine(pipe: &str, allowed: &[String]) -> Result<(), String> {
    if let Some(c) = connect_verified(pipe, allowed)? {
        let req = Request { session: 0, seq: 2, ack: None, event: Event::Shutdown };
        let _ = c.call(&req.encode(), 2000);
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        if connect_verified(pipe, allowed)?.is_none() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("the engine did not end after the shutdown request".into())
}

/// Read all state directories before counting verified engines against occupied locks.
pub(super) fn check_many_with(dirs: &[std::path::PathBuf], wait: Duration,
    status: &mut dyn FnMut() -> Result<Vec<(u16, u16, u16)>, String>,
) -> Result<(), String> {
    let mut held_count = 0;
    let current = twime_winpipe::logon_session_id().map_err(|e| format!("cannot read own logon session: {e}"))?;
    let live = live_sessions()?;
    for data_dir in dirs {
        // 1. unconfirmed.json
        let ids = state::read_unconfirmed(data_dir).map_err(|e| format!("cannot confirm: unconfirmed.json unreadable or damaged ({e})"))?;
        if let Some(id) = ids.iter().find(|i| blocking(i, &current, &live)) {
            return Err(format!("cannot confirm: an earlier input session ({id}) ended abnormally and its logon session still exists; close the applications that used the input method or log out, then retry"));
        }
        // 2. running.lock
        let lock = data_dir.join("running.lock");
        let held = match OpenOptions::new().read(true).write(true).share_mode(7).open(&lock) {
            Ok(mut f) => {
                let mut s = String::new();
                f.read_to_string(&mut s).map_err(|e| format!("cannot confirm: running.lock unreadable ({e})"))?;
                let id = s.trim();
                if !id.is_empty() && blocking(id, &current, &live) {
                    return Err(format!("cannot confirm: a previous engine ({id}) did not end cleanly and its logon session still exists"));
                }
                false
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) if e.raw_os_error() == Some(32) => true, // sharing violation: an engine may hold it
            Err(e) => return Err(format!("cannot confirm: running.lock cannot be examined ({e})")),
        };
        if held { held_count += 1; }
    }
    // 3. a live engine must confirm over a verified connection
    let t = Instant::now();
    loop {
        let statuses = status()?;
        if statuses.len() < held_count {
            return Err("cannot confirm: running.lock is held but not enough verified engines can be reached".into());
        }
        if statuses.iter().any(|(_, _, a)| *a > 0) {
            return Err("cannot confirm: an input connection ended abnormally since the engine started".into());
        }
        if statuses.iter().all(|(s, _, _)| *s == 0) { return Ok(()); }
        if t.elapsed() >= wait { return Err("cannot confirm: input sessions still open".into()); }
        std::thread::sleep(Duration::from_millis(200));
    }
}
