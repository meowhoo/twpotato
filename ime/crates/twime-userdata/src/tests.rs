use super::*;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
use twime_proto::{LexEntry, Mode, Reply, ToggleKey};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "twime-userdata-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn assets() -> Arc<Assets> {
    Arc::new(Assets::from_dictionary_text("ㄓˋ 志 -1\nㄓ 知 -2\nㄓˋ 知 -3\nㄇㄧㄥˊ 明 -1\nㄇㄧㄥˊ 名 -2\nㄓˋ-ㄇㄧㄥˊ 志明 -1\nㄓ-ㄇㄧㄥˊ 知名 -2\n", "").unwrap())
}
fn entry(word: &str) -> LexEntry {
    LexEntry {
        word: word.into(),
        reading: if word.starts_with('知') {
            "ㄓ ㄇㄧㄥˊ"
        } else {
            "ㄓˋ ㄇㄧㄥˊ"
        }
        .into(),
        weight: None,
    }
}
fn open(path: &Path) -> Arc<Service> {
    Service::open(path.into(), assets(), None)
}
fn page(s: &Service) -> Vec<LexEntry> {
    let UserReply::Page { entries, .. } = s.handle(UserReq::List {
        query: String::new(),
        offset: 0,
        limit: u16::MAX,
    }) else {
        panic!()
    };
    entries
}
#[test]
fn settings_roundtrip_and_strict_values() {
    for toggle_key in [
        ToggleKey::Shift,
        ToggleKey::LeftShift,
        ToggleKey::RightShift,
        ToggleKey::None,
    ] {
        let s = UserSettings {
            toggle_key,
            default_mode: Mode::English,
            punct_half: true,
            symbol_table: false,
            cache: true,
        };
        assert_eq!(settings::parse(&settings::serialize(&s)), Ok(s));
    }
    for text in [
        "cache = true\ncache = false",
        "cache = 1",
        "toggle_key = \"other\"",
        "default_mode = \"English\"",
        "punctuation = \"bad\"",
        "version = 2",
        "version = true",
        "version = 01",
        "[table]",
        "cache = true junk",
        "x = []",
        "x = \"abc",
        "x = \"a\"junk",
        "x = 1\nx = 2",
    ] {
        assert!(settings::parse(text).is_err(), "{text}");
    }
    assert_eq!(
        settings::parse(
            "# comment\nfuture = \"value # x\" # tail\nother = 42\ncache = false # comment"
        )
        .unwrap(),
        UserSettings::default()
    );
}
#[test]
fn legacy_defaults_exact_rules() {
    let d = Temp::new();
    assert_eq!(legacy_defaults(&d.0), UserSettings::default());
    for off in ["OFF", "0", "false"] {
        fs::write(d.0.join("ux.cfg"),format!(" Shift_Tap = {off}\nDEFAULT_MODE = English\npunct = HALF\nsymbol_table = {off}\nunknown = x\n")).unwrap();
        let s = legacy_defaults(&d.0);
        assert_eq!(s.toggle_key, ToggleKey::None);
        assert_eq!(s.default_mode, Mode::English);
        assert!(s.punct_half);
        assert!(!s.symbol_table);
    }
    fs::write(d.0.join("ux.cfg"),"shift_tap=off # not a comment\ndefault_mode=english # literal\npunct=half # literal\nsymbol_table=yes").unwrap();
    assert_eq!(legacy_defaults(&d.0), UserSettings::default());
}
#[test]
fn malformed_settings_never_overwritten_and_explicit_save_repairs() {
    let d = Temp::new();
    let install = Temp::new();
    fs::write(install.0.join("ux.cfg"), "shift_tap=off").unwrap();
    let raw = "toggle_key = 3\n# keep this";
    fs::write(d.0.join(SETTINGS_FILE), raw).unwrap();
    let s = Service::open(d.0.clone(), assets(), Some(install.0.clone()));
    assert_eq!(s.settings().1.toggle_key, ToggleKey::None);
    assert!(matches!(
        s.handle(UserReq::GetSettings),
        UserReply::Settings {
            file_error: true,
            last_error: Some(_),
            ..
        }
    ));
    s.poll();
    assert_eq!(fs::read_to_string(d.0.join(SETTINGS_FILE)).unwrap(), raw);
    let new = UserSettings::default();
    assert!(matches!(
        s.handle(UserReq::SetSettings(new.clone())),
        UserReply::Settings {
            file_error: false,
            ..
        }
    ));
    assert_eq!(
        settings::parse(&fs::read_to_string(d.0.join(SETTINGS_FILE)).unwrap()).unwrap(),
        new
    );
    assert_eq!(
        fs::read_to_string(d.0.join("settings.toml.bak")).unwrap(),
        raw
    );
}
#[test]
fn missing_files_use_legacy_without_creating_settings() {
    let d = Temp::new();
    let install = Temp::new();
    fs::write(
        install.0.join("ux.cfg"),
        "default_mode=english\nsymbol_table=off",
    )
    .unwrap();
    let s = Service::open(d.0.clone(), assets(), Some(install.0.clone()));
    assert_eq!(s.settings().1, legacy_defaults(&install.0));
    assert!(!d.0.join(SETTINGS_FILE).exists());
}
#[test]
fn tsv_bom_crlf_and_invalid_lines_stay_in_place() {
    let a = assets();
    let raw = "\u{feff}# header\r\n志明\tㄓˋ-ㄇㄧㄥˊ\t12\r\nbad\r\n# middle\r\n知名\tㄓ ㄇㄧㄥˊ\r\n志明\tbad\r\n\r\n";
    let mut lines = lexicon::parse(raw.as_bytes(), &a).unwrap();
    assert_eq!(lines.len(), 7);
    assert_eq!(lines[2], Line::Invalid("bad".into()));
    assert!(matches!(lines[5], Line::Invalid(_)));
    assert!(
        !lexicon::upsert(
            &mut lines,
            LexEntry {
                weight: Some(20.0),
                ..entry("志明")
            }
        )
        .unwrap()
    );
    let output = lexicon::serialize(&lines);
    assert_eq!(lexicon::parse(&output, &a).unwrap(), lines);
    let text = String::from_utf8(output).unwrap();
    assert!(text.starts_with("# header\n志明\tㄓˋ ㄇㄧㄥˊ\t20\nbad\n# middle\n"));
    assert!(text.ends_with("志明\tbad\n\n"));
    assert_eq!(
        String::from_utf8(lexicon::serialize(&[])).unwrap(),
        "# 地瓜輸入法 使用者詞庫 v1\n"
    );
}
#[test]
fn old_and_new_product_headers_are_comments() {
    let settings = UserSettings::default();
    let serialized = settings::serialize(&settings);
    assert!(serialized.starts_with("# 地瓜輸入法 設定\n"));
    for header in ["# TwIme 設定", "# 地瓜輸入法 設定"] {
        let text = serialized.replacen("# 地瓜輸入法 設定", header, 1);
        assert_eq!(settings::parse(&text), Ok(settings.clone()));
    }
    let a = assets();
    for header in ["# TwIme 使用者詞庫 v1", "# 地瓜輸入法 使用者詞庫 v1"] {
        let text = format!("{header}\n志明\tㄓˋ ㄇㄧㄥˊ\n");
        let lines = lexicon::parse(text.as_bytes(), &a).unwrap();
        assert_eq!(lines[0], Line::Comment(header.into()));
        assert_eq!(lines[1], Line::Entry(entry("志明")));
        assert_eq!(lexicon::serialize(&lines), text.as_bytes());
    }
}
#[test]
fn normalize_and_validate_weights() {
    assert_eq!(
        lexicon::normalize_reading(" ˙ㄉㄜ - ㄓˉ  ㄇㄧㄥˊ "),
        Some(vec!["ㄉㄜ˙".into(), "ㄓ".into(), "ㄇㄧㄥˊ".into()])
    );
    for text in ["", " - ", "ˉ"] {
        assert_eq!(lexicon::normalize_reading(text), None);
    }
    for w in [f32::NAN, f32::INFINITY, -0.01, 30.01] {
        assert_eq!(
            validate(
                &assets(),
                &LexEntry {
                    weight: Some(w),
                    ..entry("志明")
                }
            ),
            Err(LexError::Weight)
        );
    }
    for w in [None, Some(0.0), Some(30.0)] {
        assert!(
            validate(
                &assets(),
                &LexEntry {
                    weight: w,
                    ..entry("志明")
                }
            )
            .is_ok()
        );
    }
    for w in ["NaN", "inf", "-1", "31", "abc", ""] {
        let lines =
            lexicon::parse(format!("志明\tㄓˋ ㄇㄧㄥˊ\t{w}").as_bytes(), &assets()).unwrap();
        assert!(matches!(lines[0], Line::Invalid(_)));
    }
}
#[test]
fn upsert_normalized_keys_and_rejections() {
    let d = Temp::new();
    let s = open(&d.0);
    let mut e = entry("志明");
    e.reading = "ㄓˋ-ㄇㄧㄥˊ".into();
    let r = s.handle(UserReq::Add(vec![
        e,
        LexEntry {
            weight: Some(13.0),
            ..entry("志明")
        },
        LexEntry {
            word: "龘明".into(),
            ..entry("志明")
        },
    ]));
    assert_eq!(
        r,
        UserReply::Added {
            added: 1,
            updated: 1,
            rejected: vec![(2, LexError::UnknownChar(0))]
        }
    );
    assert_eq!(page(&s).len(), 1);
    assert_eq!(page(&s)[0].weight, Some(13.0));
    assert_eq!(
        s.handle(UserReq::Remove(vec![("志明".into(), "ㄓˋ-ㄇㄧㄥˊ".into())])),
        UserReply::Removed(1)
    );
    assert!(page(&s).is_empty());
}
#[test]
fn file_and_entry_limits() {
    assert_eq!(
        lexicon::parse(&vec![b'#'; lexicon::MAX_BYTES + 1], &assets()),
        Err(LexError::Full)
    );
    let mut lines = vec![Line::Entry(entry("志明")); lexicon::MAX_ENTRIES];
    assert_eq!(
        lexicon::upsert(&mut lines, entry("知名")),
        Err(LexError::Full)
    );
    assert!(!lexicon::upsert(&mut lines, entry("志明")).unwrap());
    assert_eq!(lines.len(), 1);
    let row = "志明\tㄓˋ ㄇㄧㄥˊ\n";
    assert_eq!(
        lexicon::parse(row.repeat(lexicon::MAX_ENTRIES + 1).as_bytes(), &assets()),
        Err(LexError::Full)
    );
}
#[test]
fn cross_process_lock_timeout() {
    let d = Temp::new();
    let _held = store::lock(&d.0).unwrap();
    let start = Instant::now();
    assert_eq!(store::lock(&d.0).unwrap_err(), LexError::Busy);
    assert!(start.elapsed() >= Duration::from_secs(2));
}
#[test]
fn atomic_backup_and_no_missing_formal_file() {
    let d = Temp::new();
    let path = d.0.join("atomic.txt");
    store::write_atomic(&path, b"old").unwrap();
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = running.clone();
    let p = path.clone();
    let reader = std::thread::spawn(move || {
        while flag.load(Ordering::Acquire) {
            let bytes = fs::read(&p).unwrap();
            assert!(bytes == b"old" || bytes == b"new");
        }
    });
    for _ in 0..30 {
        store::write_atomic(&path, b"new").unwrap();
        store::write_atomic(&path, b"old").unwrap();
    }
    running.store(false, Ordering::Release);
    reader.join().unwrap();
    assert_eq!(fs::read(d.0.join("atomic.txt.bak")).unwrap(), b"new");
    assert!(!d.0.join("atomic.txt.tmp").exists());
}
#[test]
fn write_failure_keeps_memory_and_records_error_generation() {
    let d = Temp::new();
    let s = open(&d.0);
    s.handle(UserReq::Add(vec![entry("志明")]));
    fs::create_dir(d.0.join("userlex.tsv.tmp")).unwrap();
    let error_before = s.error_gen();
    assert_eq!(
        s.handle(UserReq::Add(vec![entry("知名")])),
        UserReply::Error(LexError::Io)
    );
    assert_eq!(page(&s).len(), 1);
    assert!(s.error_gen() > error_before);
    assert!(matches!(
        s.handle(UserReq::GetSettings),
        UserReply::Settings {
            last_error: Some(_),
            ..
        }
    ));
    assert!(
        fs::read_to_string(d.0.join(LEXICON_FILE))
            .unwrap()
            .starts_with("志明")
    );
}
#[test]
fn poll_external_changes_and_skip_own_writes() {
    let d = Temp::new();
    let s = open(&d.0);
    let n = s.reloads.load(Ordering::Relaxed);
    s.handle(UserReq::Add(vec![entry("志明")]));
    s.handle(UserReq::SetSettings(UserSettings {
        cache: true,
        ..Default::default()
    }));
    s.poll();
    assert_eq!(s.reloads.load(Ordering::Relaxed), n);
    let before = s.settings();
    fs::write(d.0.join(SETTINGS_FILE), "default_mode = \"english\"\n").unwrap();
    fs::write(d.0.join(LEXICON_FILE), "# changed\n知名\tㄓ ㄇㄧㄥˊ\n").unwrap();
    s.poll();
    assert_eq!(s.settings().1.default_mode, Mode::English);
    assert!(s.settings().0 > before.0);
    assert_eq!(page(&s)[0].word, "知名");
    assert_eq!(s.reloads.load(Ordering::Relaxed), n + 2);
    fs::write(d.0.join(SETTINGS_FILE), "cache = invalid").unwrap();
    s.poll();
    assert!(matches!(
        s.handle(UserReq::GetSettings),
        UserReply::Settings {
            file_error: true,
            ..
        }
    ));
}
#[test]
fn two_services_always_reread_even_same_stat_content() {
    let d = Temp::new();
    let a = open(&d.0);
    let b = open(&d.0);
    a.handle(UserReq::Add(vec![entry("志明")]));
    b.handle(UserReq::Add(vec![entry("知名")]));
    a.handle(UserReq::Add(vec![entry("志名")]));
    assert_eq!(page(&a).len(), 3);
    let path = d.0.join(LEXICON_FILE);
    let old = fs::metadata(&path).unwrap().modified().unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let text = text.replace("志名", "知明");
    // The replacement has the same byte count. Restore mtime too: mutation must ignore stat.
    fs::write(&path, &text).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old))
        .unwrap();
    b.handle(UserReq::Add(vec![LexEntry {
        weight: Some(11.0),
        ..entry("志明")
    }]));
    let rows = page(&b);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|e| e.word == "知明"));
    assert!(!rows.iter().any(|e| e.word == "志名"));
}
#[test]
fn settings_services_preserve_legacy_and_cross_service_updates() {
    let d = Temp::new();
    let a = open(&d.0);
    let b = open(&d.0);
    a.handle(UserReq::SetSettings(UserSettings {
        cache: true,
        ..Default::default()
    }));
    b.poll();
    assert!(b.settings().1.cache);
    b.handle(UserReq::SetSettings(UserSettings {
        toggle_key: ToggleKey::None,
        ..b.settings().1.clone()
    }));
    a.poll();
    assert!(a.settings().1.cache);
    assert_eq!(a.settings().1.toggle_key, ToggleKey::None);
}
#[test]
fn reading_options_and_query_filters() {
    let d = Temp::new();
    let s = open(&d.0);
    s.handle(UserReq::Add(vec![entry("志明"), entry("知名")]));
    assert_eq!(
        s.handle(UserReq::ReadingOptions("志明".into())),
        UserReply::Readings {
            default: Some("ㄓˋ ㄇㄧㄥˊ".into()),
            per_char: vec![vec!["ㄓˋ".into()], vec!["ㄇㄧㄥˊ".into()]]
        }
    );
    assert_eq!(
        s.handle(UserReq::ReadingOptions("志龘".into())),
        UserReply::Error(LexError::UnknownChar(1))
    );
    for query in ["志", "ㄓˋㄇ"] {
        assert!(matches!(
            s.handle(UserReq::List {
                query: query.into(),
                offset: 0,
                limit: 20
            }),
            UserReply::Page { total: 1, .. }
        ));
    }
}
#[test]
fn pages_fit_encoded_budget_and_continue_without_loss() {
    let d = Temp::new();
    let a = Arc::new(
        Assets::from_dictionary_text("ㄓ 志 -1\nㄓ 知 -1\nㄓ 明 -1\nㄓ 名 -1\nㄔ 志 -1\nㄔ 知 -1\nㄔ 明 -1\nㄔ 名 -1\n", "").unwrap(),
    );
    let chars = ['志', '知', '明', '名'];
    let mut rows = vec![];
    for index in 0..5000 {
        let mut n = index % 32;
        let mut word = String::new();
        for _ in 0..8 {
            word.push(chars[n % 4]);
            n /= 4;
        }
        rows.push(LexEntry {
            word,
            reading: (0..8).map(|bit| if (index / 32) >> bit & 1 == 0 { "ㄓ" } else { "ㄔ" }).collect::<Vec<_>>().join(" "),
            weight: Some(30.0),
        });
    }
    let bytes = lexicon::serialize(&rows.iter().cloned().map(Line::Entry).collect::<Vec<_>>());
    fs::write(d.0.join(LEXICON_FILE), bytes).unwrap();
    let s = Service::open(d.0.clone(), a, None);
    let mut got = vec![];
    let mut pages = 0;
    while got.len() < rows.len() {
        let reply = s.handle(UserReq::List {
            query: String::new(),
            offset: got.len() as u32,
            limit: u16::MAX,
        });
        assert!(Reply::User(reply.clone()).encode_for(5).len() <= USER_PAGE_BYTES);
        let UserReply::Page { total, entries, .. } = reply else {
            panic!()
        };
        assert_eq!(total, 5000);
        assert!(!entries.is_empty());
        got.extend(entries);
        pages += 1;
    }
    assert!(pages > 1);
    assert_eq!(got, rows);
}
#[test]
fn writer_fifo_when_first_request_is_paused() {
    let d = Temp::new();
    let s = open(&d.0);
    let writer = s.spawn_writer();
    let (resume, a) = writer.pause_for_test(UserReq::Add(vec![entry("志明")]));
    let b = writer.submit(UserReq::Add(vec![entry("知名")]));
    assert!(a.try_recv().is_err());
    assert!(b.try_recv().is_err());
    assert!(page(&s).is_empty());
    resume.send(()).unwrap();
    for r in [a, b] {
        assert!(matches!(
            r.recv_timeout(Duration::from_secs(10)).unwrap(),
            UserReply::Added { added: 1, .. }
        ));
    }
    assert_eq!(
        page(&s).iter().map(|e| e.word.as_str()).collect::<Vec<_>>(),
        vec!["志明", "知名"]
    );
    writer.submit_nowait(UserReq::Remove(vec![("志明".into(), "ㄓˋ ㄇㄧㄥˊ".into())]));
    writer
        .submit(UserReq::GetSettings)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert_eq!(page(&s).len(), 1);
}

#[test]
fn reading_capacity_rejects_33rd_but_allows_updates_and_preserves_invalid_rows() {
    let d = Temp::new();
    let a = assets();
    let entries: Vec<_> = (0..33).map(|n| LexEntry {
        word: (0..6).map(|bit| if n >> bit & 1 == 0 { '志' } else { '知' }).collect(),
        reading: vec!["ㄓˋ"; 6].join(" "),
        weight: None,
    }).collect();
    let s = Service::open(d.0.clone(), a.clone(), None);
    assert_eq!(s.handle(UserReq::Add(entries.clone())), UserReply::Added {
        added: 32, updated: 0, rejected: vec![(32, LexError::Full)],
    });
    let mut updated = entries[0].clone();
    updated.weight = Some(30.0);
    assert_eq!(s.handle(UserReq::Add(vec![updated])), UserReply::Added {
        added: 0, updated: 1, rejected: vec![],
    });
    let raw = lexicon::serialize(&entries.iter().cloned().map(Line::Entry).collect::<Vec<_>>());
    fs::write(d.0.join(LEXICON_FILE), &raw).unwrap();
    s.poll();
    assert!(matches!(s.handle(UserReq::List { query: String::new(), offset: 0, limit: 100 }),
        UserReply::Page { total: 32, invalid_lines: 1, .. }));
    let parsed = lexicon::parse(&raw, &a).unwrap();
    assert!(matches!(&parsed[32], Line::Invalid(_)));
    assert_eq!(lexicon::serialize(&parsed), raw);
    s.handle(UserReq::Add(vec![entries[0].clone()]));
    assert_eq!(fs::read(d.0.join(LEXICON_FILE)).unwrap(), raw);
}

#[test]
fn nowait_errors_and_rejections_publish_error_generations() {
    let d = Temp::new();
    let s = open(&d.0);
    let writer = s.spawn_writer();
    let mut bad = entry("志明");
    bad.weight = Some(31.0);
    for request in [UserReq::Add(vec![bad]), UserReq::ReadingOptions("龘".into())] {
        let before = s.error_gen();
        writer.submit_nowait(request);
        let reply = writer.submit(UserReq::GetSettings).recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(s.error_gen(), before + 1);
        assert!(matches!(reply, UserReply::Settings { last_error: Some(_), .. }));
    }
    fs::remove_file(d.0.join("userdata.lock")).unwrap();
    fs::create_dir(d.0.join("userdata.lock")).unwrap();
    let before = s.error_gen();
    writer.submit_nowait(UserReq::Add(vec![entry("志明")]));
    writer.submit(UserReq::GetSettings).recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(s.error_gen(), before + 1);
}

#[test]
fn userlex_custom_reading_round_trip() {
    // USERLEX 1.6: 明 is not legal for ㄓˋ; the new engine keeps the line as an entry (custom reading), the weight stays
    // omitted, and serialization writes the line back unchanged. An unparseable line (what an old engine sees for a
    // custom line, or an unknown character here) is preserved verbatim.
    let mut a = Assets::from_dictionary_text("ㄓˋ 志 -1\nㄓ 知 -2\nㄓˋ 知 -3\nㄇㄧㄥˊ 明 -1\nㄇㄧㄥˊ 名 -2\nㄓˋ-ㄇㄧㄥˊ 志明 -1\nㄓ-ㄇㄧㄥˊ 知名 -2\n", "").unwrap();
    a.set_user_rules(true, 10.0).unwrap();
    let text = "明明\tㄓˋ ㄇㄧㄥˊ\n志明\tㄓˋ ㄇㄧㄥˊ\t13\n龘明\tㄓˋ ㄇㄧㄥˊ\n";
    let lines = lexicon::parse(text.as_bytes(), &a).unwrap();
    assert!(matches!(&lines[0], Line::Entry(e) if e.word == "明明" && e.weight.is_none()));
    // With the rules off (an old engine's behaviour) the custom line is kept verbatim as an invalid line.
    let old = lexicon::parse(text.as_bytes(), &assets()).unwrap();
    assert!(matches!(&old[0], Line::Invalid(raw) if raw == "明明\tㄓˋ ㄇㄧㄥˊ"));
    assert_eq!(String::from_utf8(lexicon::serialize(&old)).unwrap(), text);
    assert!(matches!(&lines[1], Line::Entry(e) if e.weight == Some(13.0)));
    assert!(matches!(&lines[2], Line::Invalid(raw) if raw == "龘明\tㄓˋ ㄇㄧㄥˊ"));
    assert_eq!(String::from_utf8(lexicon::serialize(&lines)).unwrap(), text);
}
