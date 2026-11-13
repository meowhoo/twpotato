use serde::{Deserialize, Serialize};
use std::{
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};
use twime_model::{Assets, ModelPaths};
use twime_proto::{
    LexEntry, LexError, Mode, ToggleKey, USER_PAGE_BYTES, UserReply, UserReq, UserSettings,
};
use twime_userdata::{
    Service,
    lexicon::{self, Line},
};

pub trait Link {
    fn call(&self, req: UserReq) -> Result<UserReply, String>;
}

pub fn install_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("TWIME_INSTALL_ROOT") {
        return Ok(root.into());
    }
    std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .map(PathBuf::from)
        .ok_or("找不到安裝目錄".into())
}

pub struct LocalLink {
    root: PathBuf,
    dir: PathBuf,
    assets: Mutex<Option<Arc<Assets>>>,
    service: Mutex<Option<Arc<Service>>>,
}
impl LocalLink {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn validate_export_path(&self, path: &Path) -> Result<(), String> {
        validate_export_path(path, &self.dir)
    }
    pub fn new(root: PathBuf, dir: PathBuf) -> Self {
        Self {
            root,
            dir,
            assets: Mutex::new(None),
            service: Mutex::new(None),
        }
    }
    pub fn assets(&self) -> Result<Arc<Assets>, String> {
        let mut slot = self.assets.lock().map_err(|_| "字典服務忙碌")?;
        if let Some(assets) = slot.as_ref() {
            return Ok(assets.clone());
        }
        let mut assets = Assets::load_dictionary_only(&ModelPaths {
            data_txt: self.root.join("assets/data.txt"),
            aliases: self.root.join("assets/common-readings.txt"),
            model: PathBuf::new(),
            prior: None,
        })
        .map_err(|e| format!("無法讀取安裝目錄的字典：{e}"))?;
        // Offline editing validates with the same user-lexicon rules as the installed engine (USERLEX 1.6: custom
        // readings need `user_force=on` in the assets model.cfg).
        let (force, bonus2) = user_rules(&self.root.join("assets/model.cfg"))?;
        assets
            .set_user_rules(force, bonus2)
            .map_err(|_| "model.cfg 的 user_bonus2 不合法".to_string())?;
        let assets = Arc::new(assets);
        *slot = Some(assets.clone());
        Ok(assets)
    }
    /// Live check of a reading typed for `word` (USERLEX 1.6 / 1.3), against the installed dictionary: the syllable
    /// table and legal pairs are the engine's (same data.txt and alias table); the engine validates again on `lex_add`.
    pub fn check_reading(&self, word: &str, readings: &[String]) -> Result<CheckDto, String> {
        let assets = self.assets()?;
        check_reading_in(&assets, word, readings)
    }
}

/// `user_force` / `user_bonus2` of the assets model.cfg, read like the engine reads them (twime-engined `model_cfg_with`,
/// non-strict: other lines are skipped). No file: the old rules (off, 10), like an engine without these keys. A file that
/// cannot be read, a bad value or a key given twice is an error, as it is for the engine (which then does not start).
pub fn user_rules(file: &Path) -> Result<(bool, f64), String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((false, twime_model::user::DEFAULT_BONUS)),
        Err(e) => return Err(format!("無法讀取 {}：{e}", file.display())),
    };
    let (mut force, mut bonus) = (None, None);
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) else {
            continue;
        };
        match k {
            "user_force" => {
                let on = match v {
                    "on" => true,
                    "off" => false,
                    _ => return Err(format!("{} 的 user_force 必須是 on 或 off", file.display())),
                };
                if force.replace(on).is_some() {
                    return Err(format!("{} 的 user_force 重複", file.display()));
                }
            }
            "user_bonus2" => {
                let b = v
                    .parse::<f64>()
                    .ok()
                    .filter(|b| b.is_finite() && (0.0..=30.0).contains(b))
                    .ok_or_else(|| format!("{} 的 user_bonus2 必須是 0～30 的數值", file.display()))?;
                if bonus.replace(b).is_some() {
                    return Err(format!("{} 的 user_bonus2 重複", file.display()));
                }
            }
            _ => {}
        }
    }
    Ok((force.unwrap_or(false), bonus.unwrap_or(twime_model::user::DEFAULT_BONUS)))
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SyllableCheck {
    /// Normalized spelling (first tone without a mark, neutral tone `˙` at the end).
    pub syllable: String,
    /// In the syllable table.
    pub known: bool,
    /// The character is known but this pair is not in the legal reading table (a custom reading of this word).
    pub custom: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CheckDto {
    pub syllables: Vec<SyllableCheck>,
    /// The word passes the same validation as adding it under the installed rules (the engine validates again on add).
    pub ok: bool,
    /// Why not, when `ok` is false.
    pub reason: Option<String>,
    /// USERLEX 1.3 hint for 2-character words: the data.txt word of the same reading with the highest frequency.
    pub hint: Option<String>,
}
/// `readings`: one entry per character, possibly still empty while the user is typing; each filled position is checked
/// at once, the whole word only when every position is filled.
pub fn check_reading_in(assets: &Assets, word: &str, readings: &[String]) -> Result<CheckDto, String> {
    let chars: Vec<char> = word.chars().collect();
    if chars.is_empty() || chars.len() > 8 {
        return Err(error(LexError::Length));
    }
    if readings.len() != chars.len() {
        return Err(format!("讀音有 {} 個位置，詞有 {} 個字", readings.len(), chars.len()));
    }
    let mut syllables = vec![];
    for (i, raw) in readings.iter().enumerate() {
        syllables.push(if raw.trim().is_empty() {
            String::new()
        } else {
            match lexicon::normalize_reading(raw).as_deref() {
                Some([one]) => one.clone(),
                _ => return Err(format!("第 {} 個讀音只能是一個音節", i + 1)),
            }
        });
    }
    let complete = syllables.iter().all(|s| !s.is_empty());
    let (known_chars, known_syllables) = assets.dictionary().vocabulary();
    let mut checks = vec![];
    // Same rules as the lexicon service (custom pairs only with user_force=on).
    let verdict = complete.then(|| {
        lexicon::validate(
            assets,
            &LexEntry {
                word: word.into(),
                reading: syllables.join(" "),
                weight: None,
            },
        )
    });
    let ok = matches!(verdict, Some(Ok(_)));
    for (c, s) in chars.iter().zip(&syllables) {
        let known = !s.is_empty() && known_syllables.binary_search(s).is_ok();
        let char_known = known_chars.binary_search(c).is_ok();
        checks.push(SyllableCheck {
            syllable: s.clone(),
            known,
            custom: known && char_known && !assets.is_legal_char(s, *c),
        });
    }
    let hint = if ok && chars.len() == 2 {
        let entries = assets.dictionary().lookup(&syllables);
        let own = entries.iter().find(|e| e.word == word).map(|e| e.log10);
        entries
            .iter()
            .filter(|e| e.word != word)
            .max_by(|a, b| a.log10.total_cmp(&b.log10).then(b.line.cmp(&a.line)))
            .filter(|top| own.is_none_or(|o| o < top.log10))
            .map(|top| {
                format!(
                    "這個詞的優先度會提高，可能影響同音詞『{}』；打錯時可從候選改選",
                    top.word
                )
            })
    } else {
        None
    };
    Ok(CheckDto {
        syllables: checks,
        ok,
        reason: verdict.and_then(Result::err).map(error),
        hint,
    })
}
impl Link for LocalLink {
    fn call(&self, req: UserReq) -> Result<UserReply, String> {
        let mut slot = self.service.lock().map_err(|_| "離線服務忙碌")?;
        if slot.is_none() {
            *slot = Some(Service::open(
                self.dir.clone(),
                self.assets()?,
                Some(self.root.clone()),
            ));
        }
        let service = slot.as_ref().unwrap();
        service.poll();
        Ok(service.handle(req))
    }
}

#[cfg(windows)]
pub struct PipeLink {
    pub pipe: String,
    pub image: Option<String>,
}
#[cfg(windows)]
impl PipeLink {
    pub fn configured(root: &std::path::Path) -> Result<Self, String> {
        if let Ok(pipe) = std::env::var("TWIME_PIPE") {
            return Ok(Self {
                pipe: twime_winpipe::qualify(&pipe),
                image: None,
            });
        }
        let id = twime_winpipe::current_identity().map_err(|e| e.to_string())?;
        Ok(Self {
            pipe: twime_winpipe::pipe_name(&id),
            image: Some(root.join("twime-engined.exe").to_string_lossy().into()),
        })
    }
}
#[cfg(windows)]
impl Link for PipeLink {
    fn call(&self, req: UserReq) -> Result<UserReply, String> {
        use twime_proto::{Event, Reply, Request};
        let client = twime_winpipe::Client::connect(&self.pipe, self.image.as_deref(), 200)
            .map_err(|e| format!("引擎未連線：{e}"))?;
        let exchange = |seq, event| -> Result<Reply, String> {
            let bytes = client
                .call(
                    &Request {
                        session: 0,
                        seq,
                        ack: None,
                        event,
                    }
                    .encode(),
                    5000,
                )
                .map_err(|e| format!("引擎通訊失敗：{e:?}"))?;
            Reply::decode(&bytes).ok_or("引擎回覆格式錯誤".into())
        };
        match exchange(1, Event::Hello(5))? {
            Reply::HelloCaps { version, .. } if version >= 5 => {}
            _ => return Err("引擎版本不足，需要第 5 版協定".into()),
        }
        match exchange(2, Event::User(req))? {
            Reply::User(reply) => Ok(reply),
            _ => Err("引擎未回傳使用者資料".into()),
        }
    }
}

pub struct Backend {
    pub local: LocalLink,
    #[cfg(windows)]
    pub pipe: PipeLink,
    status: Mutex<Status>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub online: bool,
    pub note: String,
}
impl Backend {
    pub fn configured() -> Result<Self, String> {
        let root = install_root()?;
        let dir = std::env::var_os("TWIME_USER_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(twime_userdata::user_dir);
        Ok(Self {
            #[cfg(windows)]
            pipe: PipeLink::configured(&root)?,
            local: LocalLink::new(root, dir),
            status: Mutex::new(Status {
                online: false,
                note: String::new(),
            }),
        })
    }
    pub fn status(&self) -> Status {
        // Probe every time without loading local assets merely to display connectivity.
        #[cfg(windows)]
        {
            let result = self.pipe.call(UserReq::GetSettings);
            let online = matches!(&result, Ok(UserReply::Settings { .. }));
            let note = match result {
                Err(e) => e,
                Ok(UserReply::Error(e)) => error(e),
                _ => String::new(),
            };
            *self.status.lock().unwrap() = Status { online, note };
        }
        self.status.lock().unwrap().clone()
    }
}
impl Link for Backend {
    fn call(&self, req: UserReq) -> Result<UserReply, String> {
        #[cfg(windows)]
        match self.pipe.call(req.clone()) {
            Ok(UserReply::Error(LexError::Disabled)) => {
                *self.status.lock().unwrap() = Status {
                    online: false,
                    note: error(LexError::Disabled),
                };
            }
            Ok(reply) => {
                *self.status.lock().unwrap() = Status {
                    online: true,
                    note: String::new(),
                };
                return Ok(reply);
            }
            Err(e) => {
                *self.status.lock().unwrap() = Status {
                    online: false,
                    note: e,
                };
            }
        }
        self.local.call(req)
    }
}

pub fn error(e: LexError) -> String {
    match e {
        LexError::Length => "詞長必須為 1～8 個字".into(),
        LexError::UnknownChar(i) => format!("第 {} 個字不在字表內", i as usize + 1),
        LexError::IllegalSyllable(i) => format!("第 {} 個音節不合法", i as usize + 1),
        LexError::IllegalChar(i) => format!("第 {} 個字與讀音不合", i as usize + 1),
        LexError::Weight => "權重必須為 0～30 的有限數值".into(),
        LexError::Full => "詞庫已滿（筆數、同讀音詞數或檔案大小超過上限）".into(),
        LexError::Busy => "檔案忙碌，請稍後重試".into(),
        LexError::Io => "檔案讀寫失敗，請檢查路徑與權限".into(),
        LexError::Disabled => "引擎尚未啟用使用者資料，改用離線編輯".into(),
        LexError::BadReading => "讀音格式不合法".into(),
    }
}
fn diagnostic(text: Option<String>) -> Option<String> {
    text.map(|text| {
        // Service diagnostics currently contain the Rust error name; do not expose that name in the UI.
        for (code, e) in [
            ("Io", LexError::Io),
            ("Busy", LexError::Busy),
            ("Full", LexError::Full),
            ("Disabled", LexError::Disabled),
            ("Length", LexError::Length),
            ("Weight", LexError::Weight),
            ("BadReading", LexError::BadReading),
        ] {
            if text.ends_with(code) {
                return error(e);
            }
        }
        for (name, make) in [
            ("UnknownChar(", LexError::UnknownChar as fn(u8) -> LexError),
            ("IllegalSyllable(", LexError::IllegalSyllable),
            ("IllegalChar(", LexError::IllegalChar),
        ] {
            if let Some((_, suffix)) = text.rsplit_once(name) {
                if let Some(i) = suffix.strip_suffix(')').and_then(|s| s.parse().ok()) {
                    return error(make(i));
                }
            }
        }
        text
    })
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToggleDto {
    Shift,
    LeftShift,
    RightShift,
    None,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeDto {
    Chinese,
    English,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsDto {
    pub toggle_key: ToggleDto,
    pub default_mode: ModeDto,
    pub punct_half: bool,
    pub symbol_table: bool,
    pub cache: bool,
}
impl From<UserSettings> for SettingsDto {
    fn from(s: UserSettings) -> Self {
        Self {
            toggle_key: match s.toggle_key {
                ToggleKey::Shift => ToggleDto::Shift,
                ToggleKey::LeftShift => ToggleDto::LeftShift,
                ToggleKey::RightShift => ToggleDto::RightShift,
                ToggleKey::None => ToggleDto::None,
            },
            default_mode: if s.default_mode == Mode::Chinese {
                ModeDto::Chinese
            } else {
                ModeDto::English
            },
            punct_half: s.punct_half,
            symbol_table: s.symbol_table,
            cache: s.cache,
        }
    }
}
impl From<SettingsDto> for UserSettings {
    fn from(s: SettingsDto) -> Self {
        Self {
            toggle_key: match s.toggle_key {
                ToggleDto::Shift => ToggleKey::Shift,
                ToggleDto::LeftShift => ToggleKey::LeftShift,
                ToggleDto::RightShift => ToggleKey::RightShift,
                ToggleDto::None => ToggleKey::None,
            },
            default_mode: if s.default_mode == ModeDto::Chinese {
                Mode::Chinese
            } else {
                Mode::English
            },
            punct_half: s.punct_half,
            symbol_table: s.symbol_table,
            cache: s.cache,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntryDto {
    pub word: String,
    pub reading: String,
    pub weight: Option<f32>,
}
impl From<LexEntry> for EntryDto {
    fn from(e: LexEntry) -> Self {
        Self {
            word: e.word,
            reading: e.reading,
            weight: e.weight,
        }
    }
}
impl From<EntryDto> for LexEntry {
    fn from(e: EntryDto) -> Self {
        Self {
            word: e.word,
            reading: e.reading,
            weight: e.weight,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplyDto {
    Settings {
        settings: SettingsDto,
        file_error: bool,
        last_error: Option<String>,
    },
    Page {
        total: u32,
        offset: u32,
        entries: Vec<EntryDto>,
        invalid_lines: u32,
    },
    Added {
        added: u32,
        updated: u32,
        rejected: Vec<Rejection>,
    },
    Removed {
        removed: u32,
    },
    Readings {
        default: Option<String>,
        per_char: Vec<Vec<String>>,
    },
}
#[derive(Clone, Debug, Serialize)]
pub struct Rejection {
    pub line: u32,
    pub reason: String,
}
pub fn dto(reply: UserReply) -> Result<ReplyDto, String> {
    Ok(match reply {
        UserReply::Settings {
            settings,
            file_error,
            last_error,
        } => ReplyDto::Settings {
            settings: settings.into(),
            file_error,
            last_error: diagnostic(last_error),
        },
        UserReply::Page {
            total,
            offset,
            entries,
            invalid_lines,
        } => ReplyDto::Page {
            total,
            offset,
            entries: entries.into_iter().map(Into::into).collect(),
            invalid_lines,
        },
        UserReply::Added {
            added,
            updated,
            rejected,
        } => ReplyDto::Added {
            added,
            updated,
            rejected: rejected
                .into_iter()
                .map(|(i, e)| Rejection {
                    line: i + 1,
                    reason: error(e),
                })
                .collect(),
        },
        UserReply::Removed(removed) => ReplyDto::Removed { removed },
        UserReply::Readings { default, per_char } => ReplyDto::Readings { default, per_char },
        UserReply::Error(e) => return Err(error(e)),
    })
}

#[derive(Debug, Default, Serialize)]
pub struct ImportResult {
    pub added: u32,
    pub updated: u32,
    pub rejected: u32,
    pub details: Vec<Rejection>,
}
impl ImportResult {
    fn reject(&mut self, line: u32, reason: String) {
        self.rejected += 1;
        self.details.push(Rejection { line, reason });
        self.details.sort_by_key(|r| r.line);
        self.details.truncate(20);
    }
}
/// Include the entire Request envelope, not just the entry payload.
pub fn batch_entries(entries: Vec<(u32, LexEntry)>) -> Vec<Vec<(u32, LexEntry)>> {
    let mut batches = vec![];
    let mut batch = vec![];
    let mut size = 16; // Request tag, session/seq u32, ack flag, Event/User tags, count u32.
    for item in entries {
        let len = twime_proto::encoded_len_entry(&item.1);
        if !batch.is_empty() && size + len > USER_PAGE_BYTES {
            batches.push(std::mem::take(&mut batch));
            size = 16;
        }
        size += len;
        batch.push(item);
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}
fn invalid_reason(raw: &str, assets: &Assets) -> String {
    let fields: Vec<_> = raw.split('\t').collect();
    if !(2..=3).contains(&fields.len()) {
        return "TSV 每行須包含詞、讀音與選填權重，以 Tab 分隔".into();
    }
    let Some(reading) = lexicon::normalize_reading(fields[1]) else {
        return error(LexError::BadReading);
    };
    let weight = if fields.len() == 3 {
        match fields[2].parse::<f32>() {
            Ok(w) => Some(w),
            Err(_) => return error(LexError::Weight),
        }
    } else {
        None
    };
    match lexicon::validate(
        assets,
        &LexEntry {
            word: fields[0].into(),
            reading: reading.join(" "),
            weight,
        },
    ) {
        Err(e) => error(e),
        Ok(_) => error(LexError::Full),
    }
}
pub fn import_bytes(
    link: &dyn Link,
    assets: &Assets,
    bytes: &[u8],
) -> Result<ImportResult, String> {
    let lines = lexicon::parse(bytes, assets).map_err(error)?;
    let mut result = ImportResult::default();
    let mut valid = vec![];
    for (i, line) in lines.into_iter().enumerate() {
        match line {
            Line::Entry(e) => valid.push((i as u32 + 1, e)),
            Line::Invalid(raw) => result.reject(i as u32 + 1, invalid_reason(&raw, assets)),
            _ => {}
        }
    }
    for batch in batch_entries(valid) {
        match link.call(UserReq::Add(batch.iter().map(|(_, e)| e.clone()).collect()))? {
            UserReply::Added {
                added,
                updated,
                rejected,
            } => {
                result.added += added;
                result.updated += updated;
                for (i, e) in rejected {
                    let line = batch.get(i as usize).ok_or("匯入回覆的行號錯誤")?.0;
                    result.reject(line, error(e));
                }
            }
            UserReply::Error(e) => return Err(error(e)),
            _ => return Err("匯入回覆格式錯誤".into()),
        }
    }
    result.details.sort_by_key(|r| r.line);
    Ok(result)
}
// Resolve existing ancestors (including junctions/symlinks) while also allowing a new file.
fn normalized_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            _ => resolved.push(component.as_os_str()),
        }
    }
    let mut missing = vec![];
    loop {
        match resolved.canonicalize() {
            Ok(mut path) => {
                for part in missing.into_iter().rev() {
                    path.push(part);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let part = resolved.file_name().ok_or("無法確認匯出路徑")?.to_owned();
                missing.push(part);
                if !resolved.pop() {
                    return Err("無法確認匯出路徑".into());
                }
            }
            Err(e) => return Err(format!("無法確認匯出路徑：{e}")),
        }
    }
}
fn validate_export_path(path: &Path, user_dir: &Path) -> Result<(), String> {
    let path = normalized_path(path)?;
    let dir = normalized_path(user_dir)?;
    // Windows path names are case insensitive, including not-yet-created components.
    #[cfg(windows)]
    let (path, dir) = (
        PathBuf::from(path.to_string_lossy().to_lowercase()),
        PathBuf::from(dir.to_string_lossy().to_lowercase()),
    );
    if path.starts_with(dir) {
        return Err("不能匯出到詞庫資料夾本身".into());
    }
    Ok(())
}

fn export_snapshot(link: &dyn Link) -> Result<Option<Vec<LexEntry>>, String> {
    let mut lines = vec![];
    let mut offset = 0;
    let mut expected_total = None;
    loop {
        match link.call(UserReq::List {
            query: String::new(),
            offset,
            limit: 200,
        })? {
            UserReply::Page {
                entries,
                total,
                offset: reply_offset,
                ..
            } => {
                if reply_offset != offset
                    || expected_total.is_some_and(|n| n != total)
                    || (entries.is_empty() && offset < total)
                {
                    return Ok(None);
                }
                expected_total = Some(total);
                offset += entries.len() as u32;
                lines.extend(entries);
                if offset >= total {
                    return Ok((offset == total).then_some(lines));
                }
            }
            UserReply::Error(e) => return Err(error(e)),
            _ => return Err("匯出回覆格式錯誤".into()),
        }
    }
}
pub fn export_bytes(link: &dyn Link) -> Result<Vec<u8>, String> {
    for _ in 0..3 {
        let first = export_snapshot(link)?;
        let second = export_snapshot(link)?;
        if let (Some(first), Some(second)) = (first, second) {
            if first.len() == second.len() && first == second {
                return Ok(lexicon::serialize(
                    &second.into_iter().map(Line::Entry).collect::<Vec<_>>(),
                ));
            }
        }
    }
    Err("匯出期間詞庫有變動，請重試".into())
}

#[cfg(test)]
mod tests;
