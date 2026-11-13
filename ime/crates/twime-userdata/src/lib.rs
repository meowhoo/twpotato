//! Shared offline/engine user data service. All mutation is published only after rename succeeds.
pub mod autocomp_cfg;
pub mod lexicon;
pub mod paths;
pub mod settings;
pub mod store;
use lexicon::Line;
pub use lexicon::validate;
pub use paths::user_dir;
use paths::{LEXICON_FILE, SETTINGS_FILE};
pub use settings::legacy_defaults;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};
use twime_model::Assets;
use twime_proto::{LexError, USER_PAGE_BYTES, UserReply, UserReq, UserSettings, encoded_len_entry};
struct State {
    lines: Vec<Line>,
    file_error: bool,
    last_error: Option<String>,
    settings_stat: Option<store::Stat>,
    lexicon_stat: Option<store::Stat>,
}
pub struct Service {
    dir: PathBuf,
    assets: Arc<Assets>,
    legacy: UserSettings,
    settings: RwLock<Arc<(u64, UserSettings)>>,
    state: RwLock<State>,
    error_gen: AtomicU64,
    // Serializes poll and mutations, including across different Writer instances.
    mutation: Mutex<()>,
    #[cfg(test)]
    reloads: AtomicU64,
}
impl Service {
    pub fn open(dir: PathBuf, assets: Arc<Assets>, install_root: Option<PathBuf>) -> Arc<Self> {
        let legacy = install_root
            .as_deref()
            .map(legacy_defaults)
            .unwrap_or_default();
        let service = Arc::new(Self {
            dir,
            assets,
            legacy: legacy.clone(),
            settings: RwLock::new(Arc::new((0, legacy))),
            state: RwLock::new(State {
                lines: vec![],
                file_error: false,
                last_error: None,
                settings_stat: None,
                lexicon_stat: None,
            }),
            error_gen: AtomicU64::new(0),
            mutation: Mutex::new(()),
            #[cfg(test)]
            reloads: AtomicU64::new(0),
        });
        if std::fs::create_dir_all(&service.dir).is_err() {
            service.failure(LexError::Io);
        }
        service.load_settings();
        service.load_lexicon();
        service
    }
    pub fn settings(&self) -> Arc<(u64, UserSettings)> {
        self.settings.read().unwrap().clone()
    }
    pub fn error_gen(&self) -> u64 {
        self.error_gen.load(Ordering::Acquire)
    }
    fn failure(&self, error: LexError) -> UserReply {
        // No user text is included in diagnostics.
        self.state.write().unwrap().last_error = Some(format!("使用者資料操作失敗：{error:?}"));
        self.error_gen.fetch_add(1, Ordering::Release);
        UserReply::Error(error)
    }
    fn publish_settings(&self, settings: UserSettings, file_error: bool) {
        let mut snap = self.settings.write().unwrap();
        if snap.1 != settings {
            *snap = Arc::new((snap.0 + 1, settings));
        }
        drop(snap);
        self.state.write().unwrap().file_error = file_error;
    }
    fn publish_lines(&self, lines: Vec<Line>) {
        let lex = self.assets.build_user_lexicon(lines.iter().filter_map(|l| {
            if let Line::Entry(e) = l {
                validate(&self.assets, e).ok()
            } else {
                None
            }
        }));
        self.assets.set_user(Some(Arc::new(lex)));
        self.state.write().unwrap().lines = lines;
    }
    fn load_settings(&self) {
        #[cfg(test)]
        self.reloads.fetch_add(1, Ordering::Relaxed);
        let path = self.dir.join(SETTINGS_FILE);
        // Record stat before reading: an overlapping replacement remains visible to the next poll.
        let stat = store::stat(&path).ok().flatten();
        let result = store::read(&path).and_then(|bytes| match bytes {
            None => Ok(self.legacy.clone()),
            Some(bytes) => std::str::from_utf8(&bytes)
                .ok()
                .and_then(|s| settings::parse(s).ok())
                .ok_or(LexError::Io),
        });
        match result {
            Ok(s) => self.publish_settings(s, false),
            Err(e) => {
                self.publish_settings(self.legacy.clone(), true);
                self.failure(e);
            }
        }
        self.state.write().unwrap().settings_stat = stat;
    }
    fn load_lexicon(&self) {
        #[cfg(test)]
        self.reloads.fetch_add(1, Ordering::Relaxed);
        let path = self.dir.join(LEXICON_FILE);
        let stat = store::stat(&path).ok().flatten();
        match self.read_lines() {
            Ok(lines) => self.publish_lines(lines),
            Err(e) => {
                self.failure(e);
            }
        }
        self.state.write().unwrap().lexicon_stat = stat;
    }
    fn read_lines(&self) -> Result<Vec<Line>, LexError> {
        lexicon::parse(
            &store::read(&self.dir.join(LEXICON_FILE))?.unwrap_or_default(),
            &self.assets,
        )
    }
    fn settings_reply(&self) -> UserReply {
        let settings = self.settings().1.clone();
        let state = self.state.read().unwrap();
        UserReply::Settings {
            settings,
            file_error: state.file_error,
            last_error: state.last_error.clone(),
        }
    }
    pub fn handle(&self, req: UserReq) -> UserReply {
        match req {
            UserReq::GetSettings => self.settings_reply(),
            UserReq::List {
                query,
                offset,
                limit,
            } => {
                let state = self.state.read().unwrap();
                let matches: Vec<_> = state
                    .lines
                    .iter()
                    .filter_map(|l| match l {
                        Line::Entry(e)
                            if query.is_empty()
                                || e.word.contains(&query)
                                || e.reading
                                    .chars()
                                    .filter(|c| !c.is_whitespace())
                                    .collect::<String>()
                                    .starts_with(&query) =>
                        {
                            Some(e)
                        }
                        _ => None,
                    })
                    .collect();
                let total = matches.len() as u32;
                let mut entries = vec![];
                let mut bytes = 18; // Reply tag, User tag, four u32 fields.
                for e in matches
                    .into_iter()
                    .skip(offset as usize)
                    .take(usize::from(limit))
                {
                    let size = encoded_len_entry(e);
                    if !entries.is_empty() && bytes + size > USER_PAGE_BYTES {
                        break;
                    }
                    bytes += size;
                    entries.push(e.clone());
                }
                UserReply::Page {
                    total,
                    offset,
                    entries,
                    invalid_lines: state
                        .lines
                        .iter()
                        .filter(|l| matches!(l, Line::Invalid(_)))
                        .count() as u32,
                }
            }
            UserReq::ReadingOptions(word) => {
                let mut per_char = vec![];
                for (i, c) in word.chars().enumerate() {
                    let readings = self.assets.char_readings(c);
                    if readings.is_empty() {
                        return UserReply::Error(LexError::UnknownChar(i.min(255) as u8));
                    }
                    per_char.push(readings);
                }
                UserReply::Readings {
                    default: self.assets.dict_readings(&word).into_iter().next(),
                    per_char,
                }
            }
            req => {
                let _serial = self.mutation.lock().unwrap();
                let result = self.mutate(req);
                match result {
                    Ok(reply) => reply,
                    Err(e) => self.failure(e),
                }
            }
        }
    }
    fn mutate(&self, req: UserReq) -> Result<UserReply, LexError> {
        let _lock = store::lock(&self.dir)?;
        if let UserReq::SetSettings(s) = req {
            // Read/parse even an invalid existing file; an explicit save repairs it.
            if let Some(bytes) = store::read(&self.dir.join(SETTINGS_FILE))? {
                let _ = std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|s| settings::parse(s).ok());
            }
            let path = self.dir.join(SETTINGS_FILE);
            store::write_atomic(&path, settings::serialize(&s).as_bytes())?;
            self.publish_settings(s, false);
            self.state.write().unwrap().settings_stat = store::stat(&path).ok().flatten();
            return Ok(self.settings_reply());
        }
        let mut lines = self.read_lines()?;
        let reply = match req {
            UserReq::Add(entries) => {
                let (mut added, mut updated) = (0, 0);
                let mut rejected = vec![];
                for (i, mut e) in entries.into_iter().enumerate() {
                    if let Err(error) = validate(&self.assets, &e) {
                        rejected.push((i as u32, error));
                        continue;
                    }
                    e.reading = lexicon::normalize_reading(&e.reading).unwrap().join(" ");
                    match lexicon::upsert(&mut lines, e) {
                        Ok(true) => added += 1,
                        Ok(false) => updated += 1,
                        Err(error) => rejected.push((i as u32, error)),
                    }
                }
                UserReply::Added {
                    added,
                    updated,
                    rejected,
                }
            }
            UserReq::Remove(keys) => {
                let keys: std::collections::HashSet<_> = keys
                    .into_iter()
                    .filter_map(|(word, reading)| {
                        Some((word, lexicon::normalize_reading(&reading)?.join(" ")))
                    })
                    .collect();
                let before = lines.len();
                lines.retain(|l| !matches!(l, Line::Entry(e) if keys.contains(&(e.word.clone(),e.reading.clone()))));
                UserReply::Removed((before - lines.len()) as u32)
            }
            _ => unreachable!(),
        };
        let path = self.dir.join(LEXICON_FILE);
        store::write_atomic(&path, &lexicon::serialize(&lines))?;
        self.publish_lines(lines);
        self.state.write().unwrap().lexicon_stat = store::stat(&path).ok().flatten();
        Ok(reply)
    }
    pub fn poll(&self) {
        let _serial = self.mutation.lock().unwrap();
        let a = store::stat(&self.dir.join(SETTINGS_FILE));
        let b = store::stat(&self.dir.join(LEXICON_FILE));
        let (a, b) = match (a, b) {
            (Ok(a), Ok(b)) => (a, b),
            _ => {
                self.failure(LexError::Io);
                return;
            }
        };
        let state = self.state.read().unwrap();
        let (reload_a, reload_b) = (a != state.settings_stat, b != state.lexicon_stat);
        drop(state);
        if reload_a {
            self.load_settings();
        }
        if reload_b {
            self.load_lexicon();
        }
    }
    pub fn spawn_writer(self: &Arc<Self>) -> Writer {
        let (tx, rx) = mpsc::channel::<Job>();
        let service = self.clone();
        std::thread::spawn(move || {
            for job in rx {
                match job {
                    Job::Request(req, reply) => {
                        let error_gen = service.error_gen();
                        let result = service.handle(req);
                        if let Some(reply) = reply {
                            let _ = reply.send(result);
                        } else {
                            match result {
                                // Mutation errors are already published by handle(); read-only
                                // requests may return Error directly. Publish those as well.
                                UserReply::Error(error) if service.error_gen() == error_gen => {
                                    service.failure(error);
                                }
                                UserReply::Added { rejected, .. } if !rejected.is_empty() => {
                                    service.failure(rejected[0].1);
                                }
                                _ => {}
                            }
                        }
                    }
                    #[cfg(test)]
                    Job::Pause(req, reply, entered, resume) => {
                        let _ = entered.send(());
                        let _ = resume.recv();
                        let _ = reply.send(service.handle(req));
                    }
                }
            }
        });
        Writer { tx }
    }
}
enum Job {
    Request(UserReq, Option<mpsc::Sender<UserReply>>),
    #[cfg(test)]
    Pause(
        UserReq,
        mpsc::Sender<UserReply>,
        mpsc::Sender<()>,
        mpsc::Receiver<()>,
    ),
}
#[derive(Clone)]
pub struct Writer {
    tx: mpsc::Sender<Job>,
}
impl Writer {
    pub fn submit(&self, req: UserReq) -> mpsc::Receiver<UserReply> {
        let (tx, rx) = mpsc::channel();
        let _ = self.tx.send(Job::Request(req, Some(tx)));
        rx
    }
    pub fn submit_nowait(&self, req: UserReq) {
        let _ = self.tx.send(Job::Request(req, None));
    }
    #[cfg(test)]
    fn pause_for_test(&self, req: UserReq) -> (mpsc::Sender<()>, mpsc::Receiver<UserReply>) {
        let (entered, wait) = mpsc::channel();
        let (resume, rx) = mpsc::channel();
        let (reply, result) = mpsc::channel();
        self.tx.send(Job::Pause(req, reply, entered, rx)).unwrap();
        wait.recv().unwrap();
        (resume, result)
    }
}
#[cfg(test)]
mod tests;
