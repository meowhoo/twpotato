use crate::{lexicon::MAX_BYTES, paths::LOCK_FILE};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};
use twime_proto::LexError;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    pub modified: Option<SystemTime>,
    pub len: u64,
}
pub fn stat(path: &Path) -> io::Result<Option<Stat>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some(Stat {
            modified: m.modified().ok(),
            len: m.len(),
        })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
/// Bounded reads also protect against a file growing after its metadata was sampled.
pub fn read(path: &Path) -> Result<Option<Vec<u8>>, LexError> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(LexError::Io),
    };
    let mut bytes = vec![];
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| LexError::Io)?;
    if bytes.len() > MAX_BYTES {
        return Err(LexError::Full);
    }
    Ok(Some(bytes))
}
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), LexError> {
    if bytes.len() > MAX_BYTES {
        return Err(LexError::Full);
    }
    match fs::copy(path, sibling(path, ".bak")) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(LexError::Io),
    }
    let tmp = sibling(path, ".tmp");
    let result = (|| -> io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|_| LexError::Io)
}
pub fn lock(dir: &Path) -> Result<File, LexError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
        .map_err(|_| LexError::Io)?;
    let start = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if start.elapsed() >= Duration::from_secs(2) {
                    return Err(LexError::Busy);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return Err(LexError::Io),
        }
    }
}
