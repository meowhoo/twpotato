use super::*;
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
use twime_proto::{Event, Request};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "twime-settings-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("assets")).unwrap();
        fs::write(
            path.join("assets/data.txt"),
            "ㄓˋ 志 -1\nㄓ 知 -2\nㄓˋ 知 -3\nㄇㄧㄥˊ 明 -1\nㄇㄧㄥˊ 名 -2\nㄓˋ-ㄇㄧㄥˊ 志明 -1\n",
        )
        .unwrap();
        fs::write(path.join("assets/common-readings.txt"), "").unwrap();
        Self(path)
    }
    fn link(&self) -> LocalLink {
        LocalLink::new(self.0.clone(), self.0.join("user"))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn entry() -> LexEntry {
    LexEntry {
        word: "志明".into(),
        reading: "ㄓˋ ㄇㄧㄥˊ".into(),
        weight: Some(12.5),
    }
}
fn list(link: &dyn Link) -> UserReply {
    link.call(UserReq::List {
        query: String::new(),
        offset: 0,
        limit: 50,
    })
    .unwrap()
}

#[test]
fn dto_roundtrips_and_errors_are_chinese() {
    for toggle_key in [
        ToggleKey::Shift,
        ToggleKey::LeftShift,
        ToggleKey::RightShift,
        ToggleKey::None,
    ] {
        for default_mode in [Mode::Chinese, Mode::English] {
            let s = UserSettings {
                toggle_key,
                default_mode,
                punct_half: true,
                symbol_table: false,
                cache: true,
            };
            let json = serde_json::to_string(&SettingsDto::from(s.clone())).unwrap();
            assert_eq!(
                UserSettings::from(serde_json::from_str::<SettingsDto>(&json).unwrap()),
                s
            );
        }
    }
    let json = serde_json::to_string(&EntryDto::from(entry())).unwrap();
    assert_eq!(
        LexEntry::from(serde_json::from_str::<EntryDto>(&json).unwrap()),
        entry()
    );
    let replies = [
        UserReply::Settings {
            settings: UserSettings::default(),
            file_error: true,
            last_error: Some("使用者資料操作失敗：Io".into()),
        },
        list(&Temp::new().link()),
        UserReply::Added {
            added: 1,
            updated: 2,
            rejected: vec![(0, LexError::Weight)],
        },
        UserReply::Removed(1),
        UserReply::Readings {
            default: Some("ㄓˋ".into()),
            per_char: vec![vec!["ㄓˋ".into()]],
        },
    ];
    for reply in replies {
        let value = serde_json::to_value(dto(reply).unwrap()).unwrap();
        assert!(value["kind"].is_string());
    }
    assert_eq!(
        dto(UserReply::Error(LexError::Busy)).unwrap_err(),
        "檔案忙碌，請稍後重試"
    );
    assert_eq!(
        diagnostic(Some("使用者資料操作失敗：IllegalChar(1)".into())),
        Some("第 2 個字與讀音不合".into())
    );
    assert!(serde_json::from_str::<SettingsDto>(r#"{"toggle_key":"invalid","default_mode":"chinese","punct_half":false,"symbol_table":true,"cache":false}"#).is_err());
}

#[test]
fn local_add_list_remove_settings_and_external_poll() {
    let temp = Temp::new();
    let link = temp.link();
    assert!(!temp.0.join("user").exists(), "service is lazy");
    assert_eq!(
        link.call(UserReq::Add(vec![entry()])).unwrap(),
        UserReply::Added {
            added: 1,
            updated: 0,
            rejected: vec![]
        }
    );
    assert!(matches!(list(&link), UserReply::Page { entries, .. } if entries == vec![entry()]));
    assert!(
        matches!(link.call(UserReq::ReadingOptions("志明".into())).unwrap(), UserReply::Readings { default: Some(_), per_char } if per_char.len() == 2)
    );
    let settings = UserSettings {
        toggle_key: ToggleKey::RightShift,
        default_mode: Mode::English,
        ..UserSettings::default()
    };
    assert!(
        matches!(link.call(UserReq::SetSettings(settings.clone())).unwrap(), UserReply::Settings { settings: s, file_error: false, .. } if s == settings)
    );
    fs::write(
        temp.0.join("user/settings.toml"),
        "version = 1\ntoggle_key = \"none\"\n",
    )
    .unwrap();
    assert!(matches!(
        link.call(UserReq::GetSettings).unwrap(),
        UserReply::Settings {
            settings: UserSettings {
                toggle_key: ToggleKey::None,
                ..
            },
            ..
        }
    ));
    assert_eq!(
        link.call(UserReq::Remove(vec![(entry().word, entry().reading)]))
            .unwrap(),
        UserReply::Removed(1)
    );
    assert!(matches!(list(&link), UserReply::Page { total: 0, .. }));
}

#[test]
fn import_batches_use_actual_wire_budget_for_long_entries() {
    let e = LexEntry {
        word: "志明志明志明志明".into(),
        reading: "ㄓˋ ㄇㄧㄥˊ ㄓˋ ㄇㄧㄥˊ ㄓˋ ㄇㄧㄥˊ ㄓˋ ㄇㄧㄥˊ".into(),
        weight: Some(30.),
    };
    let batches = batch_entries((1..=4000).map(|i| (i, e.clone())).collect());
    assert!(batches.len() > 1);
    assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), 4000);
    for batch in batches {
        let encoded = Request {
            session: 0,
            seq: 2,
            ack: None,
            event: Event::User(UserReq::Add(batch.into_iter().map(|(_, e)| e).collect())),
        }
        .encode();
        assert!(encoded.len() <= USER_PAGE_BYTES, "{}", encoded.len());
    }
}

struct Rejecting {
    sizes: Mutex<Vec<usize>>,
}
impl Link for Rejecting {
    fn call(&self, req: UserReq) -> Result<UserReply, String> {
        let UserReq::Add(entries) = req else { panic!() };
        self.sizes.lock().unwrap().push(entries.len());
        Ok(UserReply::Added {
            added: 0,
            updated: 0,
            rejected: entries
                .iter()
                .enumerate()
                .map(|(i, _)| (i as u32, LexError::Full))
                .collect(),
        })
    }
}
#[test]
fn import_aggregates_parse_and_service_rejections_with_original_lines() {
    let temp = Temp::new();
    let assets = temp.link().assets().unwrap();
    let link = Rejecting {
        sizes: Mutex::new(vec![]),
    };
    let mut bytes = b"# comment\n".to_vec();
    for _ in 0..5000 {
        bytes.extend_from_slice(
            "志明志明志明志明\tㄓˋ ㄇㄧㄥˊ ㄓˋ ㄇㄧㄥˊ ㄓˋ ㄇㄧㄥˊ ㄓˋ ㄇㄧㄥˊ\t12\nbad\n"
                .as_bytes(),
        );
    }
    let result = import_bytes(&link, &assets, &bytes).unwrap();
    assert_eq!(result.rejected, 10000);
    assert_eq!(result.details.len(), 20);
    assert_eq!(
        result.details.iter().map(|r| r.line).collect::<Vec<_>>(),
        (2..=21).collect::<Vec<_>>()
    );
    assert!(link.sizes.lock().unwrap().len() > 1);
}

#[test]
fn export_reimports_identical_entries_and_failures_propagate() {
    let a = Temp::new();
    let b = Temp::new();
    let first = a.link();
    let second = b.link();
    let bytes = "\u{feff}# comment\r\n志明\tㄓˋ-ㄇㄧㄥˊ\t12.5\r\n知名\tㄓ ㄇㄧㄥˊ\r\nbad\r\n";
    let imported = import_bytes(&first, &first.assets().unwrap(), bytes.as_bytes()).unwrap();
    assert_eq!((imported.added, imported.rejected), (2, 1));
    assert_eq!(imported.details[0].line, 4);
    let out = export_bytes(&first).unwrap();
    assert_eq!(
        import_bytes(&second, &second.assets().unwrap(), &out)
            .unwrap()
            .added,
        2
    );
    assert_eq!(list(&first), list(&second));
    struct Broken;
    impl Link for Broken {
        fn call(&self, _: UserReq) -> Result<UserReply, String> {
            Ok(UserReply::Error(LexError::Io))
        }
    }
    assert!(export_bytes(&Broken).is_err());
    assert!(
        import_bytes(
            &Broken,
            &first.assets().unwrap(),
            "志明\tㄓˋ ㄇㄧㄥˊ\n".as_bytes()
        )
        .is_err()
    );
}

#[test]
fn export_destination_rejects_entire_normalized_user_directory() {
    let temp = Temp::new();
    let link = temp.link();
    // The directory and export file need not exist yet.
    for name in [
        "",
        "userlex.tsv",
        "userlex.tsv.bak",
        "userlex.tsv.tmp",
        "settings.toml",
        "settings.toml.bak",
        "settings.toml.tmp",
        "userdata.lock",
        "nested/export.tsv",
        "../user/./export.tsv",
    ] {
        assert_eq!(
            link.validate_export_path(&temp.0.join("user").join(name))
                .unwrap_err(),
            "不能匯出到詞庫資料夾本身",
            "{name}"
        );
    }
    fs::create_dir_all(temp.0.join("user/nested")).unwrap();
    fs::write(temp.0.join("user/userlex.tsv"), b"original").unwrap();
    assert!(
        link.validate_export_path(&temp.0.join("user/nested/../userlex.tsv"))
            .is_err()
    );
    assert_eq!(
        fs::read(temp.0.join("user/userlex.tsv")).unwrap(),
        b"original"
    );
    for path in ["export.tsv", "user-copy/export.tsv", "user/../export.tsv"] {
        link.validate_export_path(&temp.0.join(path)).unwrap();
    }
    #[cfg(windows)]
    assert!(
        link.validate_export_path(&temp.0.join("USER/EXPORT.TSV"))
            .is_err()
    );
}

#[test]
fn export_destination_resolves_directory_aliases() {
    let temp = Temp::new();
    fs::create_dir_all(temp.0.join("user")).unwrap();
    let alias = temp.0.join("alias");
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(temp.0.join("user"), &alias);
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(temp.0.join("user"), &alias);
    if let Err(e) = result {
        // Windows may not grant symlink creation to the test process.
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        return;
    }
    assert_eq!(
        temp.link()
            .validate_export_path(&alias.join("export.tsv"))
            .unwrap_err(),
        "不能匯出到詞庫資料夾本身"
    );
    #[cfg(windows)]
    fs::remove_dir(&alias).unwrap();
    #[cfg(unix)]
    fs::remove_file(&alias).unwrap();
}

// Each offset-zero request begins another full scan; changes happen between scans.
struct ChangingExport {
    versions: Vec<Vec<LexEntry>>,
    scans: Mutex<usize>,
    offsets: Mutex<Vec<u32>>,
}
impl Link for ChangingExport {
    fn call(&self, req: UserReq) -> Result<UserReply, String> {
        let UserReq::List {
            query,
            offset,
            limit,
        } = req
        else {
            panic!()
        };
        assert!(query.is_empty());
        assert_eq!(limit, 200);
        self.offsets.lock().unwrap().push(offset);
        let mut scans = self.scans.lock().unwrap();
        if offset == 0 {
            *scans += 1;
        }
        let entries = &self.versions[(*scans - 1).min(self.versions.len() - 1)];
        Ok(UserReply::Page {
            total: entries.len() as u32,
            offset,
            entries: entries
                .iter()
                .skip(offset as usize)
                .take(limit as usize)
                .cloned()
                .collect(),
            invalid_lines: 0,
        })
    }
}
fn exporting(versions: Vec<Vec<LexEntry>>) -> ChangingExport {
    ChangingExport {
        versions,
        scans: Mutex::new(0),
        offsets: Mutex::new(vec![]),
    }
}
#[test]
fn export_compares_two_complete_multi_page_scans() {
    let entries = vec![entry(); 201];
    let link = exporting(vec![entries.clone()]);
    assert_eq!(
        export_bytes(&link).unwrap(),
        lexicon::serialize(&entries.into_iter().map(Line::Entry).collect::<Vec<_>>())
    );
    assert_eq!(*link.offsets.lock().unwrap(), vec![0, 200, 0, 200]);
    let empty = exporting(vec![vec![]]);
    assert_eq!(export_bytes(&empty).unwrap(), lexicon::serialize(&[]));
    assert_eq!(*empty.scans.lock().unwrap(), 2);
}
#[test]
fn export_retries_count_and_content_changes_then_exports_stable_version() {
    let original = vec![entry(); 201];
    let mut changed = original.clone();
    changed[200].weight = Some(25.);
    let shorter = vec![entry(); 200];
    let link = exporting(vec![
        original,
        changed.clone(),
        changed,
        shorter.clone(),
        shorter.clone(),
        shorter.clone(),
    ]);
    assert_eq!(
        export_bytes(&link).unwrap(),
        lexicon::serialize(&shorter.into_iter().map(Line::Entry).collect::<Vec<_>>())
    );
    assert_eq!(*link.scans.lock().unwrap(), 6);
}
#[test]
fn export_fails_after_three_unstable_pairs() {
    let mut changed = entry();
    changed.weight = Some(25.);
    let link = exporting(
        (0..6)
            .map(|i| vec![if i % 2 == 0 { entry() } else { changed.clone() }; 201])
            .collect(),
    );
    assert_eq!(
        export_bytes(&link).unwrap_err(),
        "匯出期間詞庫有變動，請重試"
    );
    assert_eq!(*link.scans.lock().unwrap(), 6);
}

#[cfg(windows)]
#[test]
fn backend_falls_back_on_private_missing_pipe_and_local_is_lazy() {
    let temp = Temp::new();
    let backend = Backend {
        local: temp.link(),
        pipe: PipeLink {
            pipe: format!(
                "twime-settings-absent-{}",
                temp.0.file_name().unwrap().to_string_lossy()
            ),
            image: None,
        },
        status: Mutex::new(Status {
            online: false,
            note: String::new(),
        }),
    };
    assert!(!backend.status().online);
    assert!(!temp.0.join("user").exists());
    assert!(matches!(
        backend.call(UserReq::Add(vec![entry()])).unwrap(),
        UserReply::Added { added: 1, .. }
    ));
}

#[cfg(windows)]
#[test]
fn private_engine_and_local_replies_match() {
    use std::{
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let exe = base.join("../../target-setui/release/twime-engined.exe");
    if !exe.is_file() {
        eprintln!("略過管道整合測試：{} 不存在", exe.display());
        return;
    }
    let temp = Temp::new();
    // Copy beside an empty install root: a real install's ux.cfg / marker never affects the test.
    let private_exe = temp.0.join("twime-engined.exe");
    fs::copy(exe, &private_exe).unwrap();
    let data = base.join("../../../third_party/mcbopomofo/data.txt");
    let aliases = base.join("../../../engine/data/common-readings.txt");
    fs::copy(&data, temp.0.join("assets/data.txt")).unwrap();
    fs::copy(&aliases, temp.0.join("assets/common-readings.txt")).unwrap();
    let pipe = format!(
        "twime-settings-integration-{}",
        temp.0.file_name().unwrap().to_string_lossy()
    );
    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Process(
        Command::new(private_exe)
            .args(["--no-model", "--data-txt"])
            .arg(data)
            .arg("--aliases")
            .arg(aliases)
            .arg("--model")
            .arg(temp.0.join("unused-model.bin"))
            .arg("--pipe")
            .arg(&pipe)
            .arg("--user-dir")
            .arg(temp.0.join("engine-user"))
            .arg("--data-dir")
            .arg(temp.0.join("engine-state"))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let link = PipeLink { pipe, image: None };
    let local = temp.link();
    let until = Instant::now() + Duration::from_secs(20);
    while link.call(UserReq::GetSettings).is_err() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "private engine exited early"
        );
        assert!(Instant::now() < until, "private engine startup timed out");
        std::thread::sleep(Duration::from_millis(30));
    }
    let settings = UserSettings {
        toggle_key: ToggleKey::LeftShift,
        default_mode: Mode::English,
        punct_half: true,
        symbol_table: false,
        cache: false,
    };
    for request in [
        UserReq::GetSettings,
        UserReq::Add(vec![LexEntry {
            word: "設定".into(),
            reading: "ㄕㄜˋ ㄉㄧㄥˋ".into(),
            weight: Some(15.),
        }]),
        UserReq::List {
            query: String::new(),
            offset: 0,
            limit: 50,
        },
        UserReq::ReadingOptions("設定".into()),
        UserReq::SetSettings(settings),
        UserReq::Remove(vec![("設定".into(), "ㄕㄜˋ ㄉㄧㄥˋ".into())]),
        UserReq::List {
            query: String::new(),
            offset: 0,
            limit: 50,
        },
    ] {
        assert_eq!(
            link.call(request.clone()).unwrap(),
            local.call(request).unwrap()
        );
    }
}

#[test]
fn reading_check_normalises_marks_custom_pairs_rejects_unknown_syllables_and_hints_homophones() {
    let temp = Temp::new();
    let assets = temp.link().assets().unwrap();
    // First tone without a mark (ˉ dropped) and the neutral tone before or after: one spelling.
    let r = check_reading_in(&assets, "知", &rd("ㄓˉ")).unwrap();
    assert_eq!(r.syllables[0].syllable, "ㄓ");
    assert_eq!(check_reading_in(&assets, "知", &rd("˙ㄓ")).unwrap().syllables[0].syllable, "ㄓ˙");
    // Legal pair, custom pair (known character and syllable, pair not in the legal table), unknown syllable.
    let r = check_reading_in(&assets, "志明", &rd("ㄓˋ ㄇㄧㄥˊ")).unwrap();
    assert!(r.ok && r.syllables.iter().all(|s| s.known && !s.custom));
    // Old rules (no model.cfg): the custom pair is marked and rejected, with the service's reason.
    let r = check_reading_in(&assets, "明志", &rd("ㄓˋ ㄇㄧㄥˊ")).unwrap();
    assert!(!r.ok && r.syllables.iter().all(|s| s.known && s.custom), "{r:?}");
    assert_eq!(r.reason.as_deref(), Some("第 1 個字與讀音不合"));
    // Installed rules with user_force=on: the same custom pair is accepted.
    fs::write(temp.0.join("assets/model.cfg"), "int8=on\nuser_force=on\n").unwrap();
    let forced = temp.link().assets().unwrap();
    let r = check_reading_in(&forced, "明志", &rd("ㄓˋ ㄇㄧㄥˊ")).unwrap();
    assert!(r.ok && r.reason.is_none() && r.syllables.iter().all(|s| s.custom), "{r:?}");
    let r = check_reading_in(&assets, "志明", &rd("ㄓˋ ㄅㄧㄤ")).unwrap();
    assert!(!r.ok && r.syllables[0].known && !r.syllables[1].known);
    assert!(check_reading_in(&assets, "志明", &rd("ㄓˋ")).is_err());
    assert!(check_reading_in(&assets, "", &[]).is_err());
    // USERLEX 1.3 hint: a new 2-character word next to a more frequent data.txt word of the same reading.
    let hint = check_reading_in(&assets, "知名", &rd("ㄓˋ ㄇㄧㄥˊ")).unwrap().hint.unwrap();
    assert!(hint.contains("『志明』"), "{hint}");
    // The most frequent word itself gets no hint; neither do other lengths.
    assert_eq!(check_reading_in(&assets, "志明", &rd("ㄓˋ ㄇㄧㄥˊ")).unwrap().hint, None);
    assert_eq!(check_reading_in(&assets, "志", &rd("ㄓˋ")).unwrap().hint, None);
}

#[test]
fn offline_rules_follow_model_cfg_and_old_new_rules_round_trip_one_lexicon_file() {
    assert_eq!(user_rules(Path::new("does-not-exist.cfg")), Ok((false, 10.0)));
    let temp = Temp::new();
    fs::write(temp.0.join("assets/model.cfg"), "# made by a tool\nint8=on\nuser_force=on\nuser_bonus2=15\n").unwrap();
    assert_eq!(user_rules(&temp.0.join("assets/model.cfg")), Ok((true, 15.0)));
    // Values the engine refuses are errors here too (the offline service then does not open).
    for bad in ["user_force=yes\n", "user_bonus2=31\n", "user_force=on\nuser_force=off\n", "user_bonus2=x\n"] {
        let f = temp.0.join("bad.cfg");
        fs::write(&f, bad).unwrap();
        assert!(user_rules(&f).is_err(), "{bad:?}");
    }
    assert_eq!(user_rules(&temp.0.join("assets")), Err(format!("無法讀取 {}：{}", temp.0.join("assets").display(), fs::read_to_string(temp.0.join("assets")).unwrap_err())));
    let custom = LexEntry { word: "明志".into(), reading: "ㄓˋ ㄇㄧㄥˊ".into(), weight: None };
    // New rules (as the installed engine): the custom reading is accepted offline.
    let new = temp.link();
    assert!(matches!(new.call(UserReq::Add(vec![custom.clone()])).unwrap(), UserReply::Added { added: 1, .. }));
    let file = temp.0.join("user").join(twime_userdata::paths::LEXICON_FILE);
    let custom_line = fs::read_to_string(&file).unwrap().lines().find(|l| l.starts_with("明志")).unwrap().to_string();
    // Old rules (no USERLEX keys) on the same file: the custom row is skipped but kept byte for byte on rewrite.
    fs::remove_file(temp.0.join("assets/model.cfg")).unwrap();
    let old = temp.link();
    assert!(matches!(list(&old), UserReply::Page { total: 0, invalid_lines: 1, .. }));
    assert!(matches!(old.call(UserReq::Add(vec![entry()])).unwrap(), UserReply::Added { added: 1, .. }));
    let after = fs::read_to_string(&file).unwrap();
    assert!(after.lines().any(|l| l == custom_line), "{after:?}");
    // Back with the new rules both rows are entries.
    fs::write(temp.0.join("assets/model.cfg"), "int8=on\nuser_force=on\n").unwrap();
    assert!(matches!(list(&temp.link()), UserReply::Page { total: 2, invalid_lines: 0, .. }));
}

/// One reading per character, as the frontend sends them ("" = not typed yet).
fn rd(s: &str) -> Vec<String> {
    s.split(' ').map(String::from).collect()
}

#[test]
fn reading_check_validates_filled_positions_while_others_are_still_empty() {
    let temp = Temp::new();
    let assets = temp.link().assets().unwrap();
    let r = check_reading_in(&assets, "志明", &["ㄅㄧㄤ".into(), String::new()]).unwrap();
    assert!(!r.ok && r.reason.is_none() && r.hint.is_none());
    assert!(!r.syllables[0].known && r.syllables[1].syllable.is_empty() && !r.syllables[1].known);
    let r = check_reading_in(&assets, "明志", &["ㄓˋ".into(), String::new()]).unwrap();
    assert!(!r.ok && r.syllables[0].known && r.syllables[0].custom);
    assert!(check_reading_in(&assets, "志明", &["ㄓˋ ㄇㄧㄥˊ".into(), String::new()]).is_err());
}
