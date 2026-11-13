#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod autocomp;
mod backend;
use backend::{Backend, EntryDto, ImportResult, Link, ReplyDto, SettingsDto};
use tauri::State;
use tauri_plugin_dialog::DialogExt;
use twime_proto::UserReq;

#[tauri::command]
async fn status(state: State<'_, Backend>) -> Result<backend::Status, String> {
    Ok(state.status())
}
#[tauri::command]
async fn get_settings(state: State<'_, Backend>) -> Result<ReplyDto, String> {
    backend::dto(state.call(UserReq::GetSettings)?)
}
#[tauri::command]
async fn set_settings(
    state: State<'_, Backend>,
    settings: SettingsDto,
) -> Result<ReplyDto, String> {
    backend::dto(state.call(UserReq::SetSettings(settings.into()))?)
}
#[tauri::command]
async fn get_autocomp(state: State<'_, Backend>) -> Result<autocomp::AutocompDto, String> {
    Ok(autocomp::get(state.local.dir(), state.local.root()))
}
#[tauri::command]
async fn set_autocomp(state: State<'_, Backend>, list: bool, pop: bool) -> Result<autocomp::AutocompDto, String> {
    autocomp::set(state.local.dir(), list, pop)?;
    Ok(autocomp::get(state.local.dir(), state.local.root()))
}
/// Restarts the engine so the saved switches apply. Only call after `set_autocomp` succeeded.
#[tauri::command]
async fn restart_engine(state: State<'_, Backend>) -> Result<String, String> {
    if let Some(m) = autocomp::restart_blocked() {
        return Ok(m.into());
    }
    #[cfg(windows)]
    {
        let ctl = autocomp::RealCtl { pipe: state.pipe.pipe.clone(), root: state.local.root().to_path_buf() };
        return autocomp::restart_with(&ctl).map(|r| autocomp::restart_message(r).into());
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        Err("此平台不支援".into())
    }
}
#[tauri::command]
async fn lex_list(
    state: State<'_, Backend>,
    query: String,
    offset: u32,
    limit: u16,
) -> Result<ReplyDto, String> {
    backend::dto(state.call(UserReq::List {
        query,
        offset,
        limit: limit.min(200),
    })?)
}
#[tauri::command]
async fn lex_add(
    state: State<'_, Backend>,
    word: String,
    reading: String,
    weight: Option<f32>,
) -> Result<ReplyDto, String> {
    backend::dto(state.call(UserReq::Add(
        vec![EntryDto { word, reading, weight }.into()],
    ))?)
}
#[tauri::command]
async fn lex_remove(
    state: State<'_, Backend>,
    word: String,
    reading: String,
) -> Result<ReplyDto, String> {
    backend::dto(state.call(UserReq::Remove(vec![(word, reading)]))?)
}
#[tauri::command]
async fn reading_options(state: State<'_, Backend>, word: String) -> Result<ReplyDto, String> {
    backend::dto(state.call(UserReq::ReadingOptions(word))?)
}
#[tauri::command]
async fn check_reading(
    state: State<'_, Backend>,
    word: String,
    readings: Vec<String>,
) -> Result<backend::CheckDto, String> {
    state.local.check_reading(&word, &readings)
}
#[tauri::command]
async fn import_file(
    app: tauri::AppHandle,
    state: State<'_, Backend>,
) -> Result<Option<ImportResult>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("詞庫 TSV", &["tsv", "txt"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let bytes = twime_userdata::store::read(&path)
        .map_err(backend::error)?
        .ok_or("找不到匯入檔案")?;
    let assets = state.local.assets()?;
    backend::import_bytes(state.inner(), &assets, &bytes).map(Some)
}
#[tauri::command]
async fn export_file(
    app: tauri::AppHandle,
    state: State<'_, Backend>,
) -> Result<Option<String>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("詞庫 TSV", &["tsv"])
        .set_file_name("userlex.tsv")
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    state.local.validate_export_path(&path)?;
    let bytes = backend::export_bytes(state.inner())?;
    std::fs::write(&path, bytes).map_err(|e| format!("匯出寫入失敗：{e}"))?;
    Ok(Some("詞庫已匯出".into()))
}
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Backend::configured().expect("無法初始化設定程式"))
        .invoke_handler(tauri::generate_handler![
            status,
            get_settings,
            set_settings,
            get_autocomp,
            set_autocomp,
            restart_engine,
            lex_list,
            lex_add,
            lex_remove,
            reading_options,
            check_reading,
            import_file,
            export_file
        ])
        .run(tauri::generate_context!())
        .expect("設定程式執行失敗");
}
