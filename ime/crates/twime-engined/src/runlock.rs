//! `running.lock`: held open (deny write / delete) for the life of the engine. Its content is the logon session id.
//! A lock file left behind means the previous engine did not end cleanly.
//! Unix: the lock is advisory and bound to the file itself, so the file is never removed (a removed path would let two
//! engines lock two different files); a clean end empties it instead, and content left behind is the evidence.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use twime_winpipe::state;

pub struct RunLock {
    file: Mutex<Option<File>>,
    #[cfg_attr(not(windows), allow(dead_code))]
    path: PathBuf,
    dir: PathBuf,
    pub logon: String,
    dirty: AtomicBool,
}

impl RunLock {
    /// Order (fixed by the plan): open without truncating and hold it; read the old content; append the old id to
    /// `unconfirmed.json` and make that durable; only then rewrite the lock with the new id. Any failure leaves the old
    /// lock untouched and the caller exits before accepting connections.
    pub fn acquire(dir: &Path, logon: String) -> std::io::Result<RunLock> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("running.lock");
        // share_mode 1 = FILE_SHARE_READ: others may read, nobody may write or delete while we run.
        #[cfg(windows)]
        let mut f = {
            use std::os::windows::fs::OpenOptionsExt;
            OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(1).open(&path)?
        };
        // Elsewhere: an exclusive advisory lock, held while the file stays open; a second engine fails here.
        #[cfg(not(windows))]
        let mut f = {
            let f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
            f.try_lock().map_err(|_| std::io::Error::new(std::io::ErrorKind::WouldBlock, "running.lock is held"))?;
            f
        };
        let mut old = String::new();
        f.read_to_string(&mut old)?;
        let old = old.trim();
        if !old.is_empty() {
            state::append_unconfirmed(dir, old)?;
        }
        f.set_len(0)?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(logon.as_bytes())?;
        f.sync_all()?;
        Ok(RunLock { file: Mutex::new(Some(f)), path, dir: dir.to_path_buf(), logon, dirty: AtomicBool::new(false) })
    }

    /// An input connection ended without closing its sessions: record this logon session first, then let the caller
    /// release them. If the record cannot be written the lock stays behind as the evidence.
    pub fn record_abnormal(&self) {
        if state::append_unconfirmed(&self.dir, &self.logon).is_err() {
            self.dirty.store(true, Ordering::SeqCst);
        }
    }

    /// Clean end of the process: the lock disappears only if nothing abnormal happened and every record was written.
    pub fn release(&self, abnormal: bool) {
        let clean = !abnormal && !self.dirty.load(Ordering::SeqCst);
        #[cfg(windows)]
        {
            *self.file.lock().unwrap_or_else(|p| p.into_inner()) = None;
            if clean {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        // Emptied while the lock is still held, then closed; the path stays.
        #[cfg(not(windows))]
        if let Some(f) = self.file.lock().unwrap_or_else(|p| p.into_inner()).take() {
            if clean {
                let _ = f.set_len(0).and_then(|()| f.sync_all());
            }
        }
    }
}
