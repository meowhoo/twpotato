#![cfg(windows)]
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use twime_proto::{
    Event, LexEntry, LexError, Mode, Reply, Request, ToggleKey, USER_PAGE_BYTES, UserReply,
    UserReq, UserSettings,
};
use twime_winpipe::Client;

struct EngineProcess {
    child: Child,
    root: PathBuf,
    pipe: String,
}
impl EngineProcess {
    fn start(enable: &str) -> Self {
        let tag = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(format!("twime-user-engine-{tag}"));
        fs::create_dir_all(&root).unwrap();
        let exe = root.join("twime-engined.exe");
        // An isolated executable directory also tests user-data.on and legacy ux.cfg safely.
        fs::copy(env!("CARGO_BIN_EXE_twime-engined"), &exe).unwrap();
        fs::write(root.join("ux.cfg"), "shift_tap=off\n").unwrap();
        fs::write(
            root.join("dict.txt"),
            "ㄓˋ 志 -1\nㄓˋ 知 -2\nㄓˋ 智 -3\nㄓˋ 制 -4\nㄔ 志 -1\nㄔ 知 -2\nㄔ 智 -3\nㄔ 制 -4\nㄇㄧㄥˊ 明 -1\nㄇㄧㄥˊ 名 -2\n",
        )
        .unwrap();
        fs::write(root.join("aliases.txt"), "").unwrap();
        if enable == "marker" {
            fs::write(root.join("user-data.on"), "").unwrap();
        }
        let pipe = format!("twime-user-{tag}");
        let mut command = Command::new(exe);
        command
            .args([
                "--pipe",
                &pipe,
                "--no-model",
                "--lambda",
                "0",
                "--candidates",
                "--data-dir",
            ])
            .arg(root.join("state"))
            .arg("--data-txt")
            .arg(root.join("dict.txt"))
            .arg("--aliases")
            .arg(root.join("aliases.txt"))
            .arg("--model")
            .arg(root.join("unused.bin"))
            .env("LOCALAPPDATA", root.join("local"))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match enable {
            "dir" => {
                command.arg("--user-dir").arg(root.join("user"));
            }
            "flag" => {
                command.arg("--user-data");
            }
            _ => {}
        }
        let child = command.spawn().unwrap();
        let mut process = Self { child, root, pipe };
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if Client::connect(&process.pipe, None, 20).is_ok() {
                break;
            }
            assert!(
                process.child.try_wait().unwrap().is_none(),
                "engine exited before accepting connections"
            );
            assert!(Instant::now() < until, "engine startup timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        process
    }
    fn connect(&self) -> Client {
        Client::connect(&self.pipe, None, 500).unwrap()
    }
}
impl Drop for EngineProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn raw(c: &Client, session: u32, event: Event) -> Result<Vec<u8>, String> {
    c.call(
        &Request {
            session,
            seq: 1,
            ack: None,
            event,
        }
        .encode(),
        5000,
    )
    .map_err(|e| format!("{e:?}"))
}
fn call(c: &Client, session: u32, event: Event) -> Reply {
    Reply::decode(&raw(c, session, event).unwrap()).unwrap()
}
fn user(c: &Client, r: UserReq) -> UserReply {
    match call(c, 0, Event::User(r)) {
        Reply::User(r) => r,
        r => panic!("{r:?}"),
    }
}
fn hello5(c: &Client) {
    assert_eq!(
        call(c, 0, Event::Hello(5)),
        Reply::HelloCaps {
            version: 5,
            caps: 0
        }
    );
}

#[test]
fn disabled_creates_no_user_files_and_uses_legacy_toggle() {
    let p = EngineProcess::start("off");
    let c = p.connect();
    hello5(&c);
    assert_eq!(
        user(&c, UserReq::GetSettings),
        UserReply::Error(LexError::Disabled)
    );
    match call(&c, 1, Event::Ping) {
        Reply::Update { update, .. } => assert_eq!(update.toggle_key, ToggleKey::None),
        r => panic!("{r:?}"),
    }
    assert!(!p.root.join("user").exists());
    assert!(!p.root.join("local/TwPotato").exists());
}

#[test]
fn close_and_shutdown_ack_use_negotiated_version_and_current_toggle() {
    for enabled in ["off", "dir"] {
        let p = EngineProcess::start(enabled);
        let control = p.connect();
        hello5(&control);
        if enabled == "dir" {
            user(&control, UserReq::SetSettings(UserSettings {
                toggle_key: ToggleKey::RightShift, ..Default::default()
            }));
        }
        for requested in 1..=5 {
            let c = p.connect();
            call(&c, 0, Event::Hello(requested));
            let version = if requested == 4 { 3 } else { requested };
            for event in [Event::Close, Event::Shutdown] {
                let bytes = raw(&c, 42, event).unwrap();
                let expected = Reply::Update { session: 42, seq: 1, update: twime_proto::Update {
                    toggle_key: if enabled == "dir" { ToggleKey::RightShift } else { ToggleKey::None },
                    ..Default::default()
                }};
                assert_eq!(bytes, expected.encode_for(version));
                if version == 5 { assert_eq!(bytes[0], 7); }
            }
        }
    }
}

#[test]
fn control_requests_settings_and_lexicon() {
    let p = EngineProcess::start("dir");
    let c = p.connect();
    hello5(&c);
    let settings = UserSettings {
        toggle_key: ToggleKey::RightShift,
        default_mode: Mode::English,
        punct_half: true,
        symbol_table: false,
        ..Default::default()
    };
    for request in [UserReq::SetSettings(settings.clone()), UserReq::GetSettings] {
        assert!(
            matches!(user(&c, request), UserReply::Settings { settings: actual, file_error: false, .. } if actual == settings)
        );
    }
    // Control requests neither create sessions nor freeze negotiation.
    assert!(matches!(
        call(&c, 0, Event::Status),
        Reply::Status { sessions: 0, .. }
    ));
    assert_eq!(call(&c, 0, Event::Hello(4)), Reply::Hello { version: 3 });
    hello5(&c);
    match call(&c, 1, Event::Ping) {
        Reply::Update { update, .. } => {
            assert_eq!(update.toggle_key, ToggleKey::RightShift);
            assert_eq!(update.mode, Mode::English);
            assert!(update.punct_half);
        }
        r => panic!("{r:?}"),
    }
    assert!(matches!(
        user(
            &c,
            UserReq::SetSettings(UserSettings {
                toggle_key: ToggleKey::LeftShift,
                ..Default::default()
            })
        ),
        UserReply::Settings { .. }
    ));
    match call(&c, 1, Event::Ping) {
        Reply::Update { update, .. } => {
            assert_eq!(update.toggle_key, ToggleKey::LeftShift);
            assert_eq!(update.mode, Mode::English);
        }
        r => panic!("{r:?}"),
    }
    let entry = LexEntry {
        word: "知名".into(),
        reading: "ㄓˋ ㄇㄧㄥˊ".into(),
        weight: None,
    };
    assert!(
        matches!(user(&c, UserReq::Add(vec![entry.clone()])), UserReply::Added { added: 1, updated: 0, rejected } if rejected.is_empty())
    );
    assert!(
        matches!(user(&c, UserReq::List { query: "知".into(), offset: 0, limit: 10 }), UserReply::Page { total: 1, entries, .. } if entries == vec![entry.clone()])
    );
    assert!(
        matches!(user(&c, UserReq::ReadingOptions("知名".into())), UserReply::Readings { per_char, .. } if per_char == vec![vec!["ㄓˋ".to_string(), "ㄔ".to_string()], vec!["ㄇㄧㄥˊ".to_string()]])
    );
    assert_eq!(
        user(&c, UserReq::Remove(vec![(entry.word, entry.reading)])),
        UserReply::Removed(1)
    );
    assert!(p.root.join("user/settings.toml").is_file());
}

#[test]
fn version_and_capability_violations_disconnect() {
    let p = EngineProcess::start("dir");
    for event in [Event::User(UserReq::GetSettings), Event::AddToLexicon] {
        let c = p.connect();
        assert_eq!(call(&c, 0, Event::Hello(4)), Reply::Hello { version: 3 });
        assert!(raw(&c, 0, event).is_err());
    }
    let c = p.connect();
    hello5(&c);
    assert!(raw(&c, 1, Event::DocText("志明".into())).is_err());
}

#[test]
fn add_to_lexicon_returns_notice_without_committing_composition() {
    let p = EngineProcess::start("dir");
    let c = p.connect();
    hello5(&c);
    for ch in "ㄓˋㄇㄧㄥˊ".chars() {
        call(
            &c,
            1,
            Event::Key(twime_proto::KeyEvent::press(
                twime_core::keymap::key_of(ch).unwrap(),
            )),
        );
    }
    let before = call(&c, 1, Event::Ping);
    match (before, call(&c, 1, Event::AddToLexicon)) {
        (Reply::Update { update: before, .. }, Reply::Update { update: after, .. }) => {
            assert!(!after.handled && after.commit.is_none());
            assert_eq!(after.composition, before.composition);
            assert_eq!(after.notice.as_deref(), Some("已送出加入詞庫：志明"));
        }
        r => panic!("{r:?}"),
    }
    // A queued mutation is a FIFO barrier for the earlier asynchronous add.
    user(&c, UserReq::SetSettings(UserSettings::default()));
    assert!(
        matches!(user(&c, UserReq::List { query: String::new(), offset: 0, limit: 10 }), UserReply::Page { total: 1, entries, .. } if entries[0].word == "志明")
    );
    // A new composition must use the published word in its candidate list.
    for ch in "ㄓˋㄇㄧㄥˊ".chars() {
        call(&c, 2, Event::Key(twime_proto::KeyEvent::press(twime_core::keymap::key_of(ch).unwrap())));
    }
    match call(&c, 2, Event::Key(twime_proto::KeyEvent::press(twime_proto::Key::Down))) {
        Reply::Update { update, .. } => assert!(update.candidates.unwrap().items.iter().any(|w| w == "志明")),
        r => panic!("{r:?}"),
    }
}

#[test]
fn flag_and_executable_marker_enable_default_directory() {
    for mode in ["flag", "marker"] {
        let p = EngineProcess::start(mode);
        let c = p.connect();
        hello5(&c);
        assert!(matches!(
            user(&c, UserReq::GetSettings),
            UserReply::Settings { .. }
        ));
        assert!(p.root.join("local/TwPotato").is_dir());
    }
}

#[test]
fn oversized_user_reply_returns_full_and_keeps_connection_usable() {
    let p = EngineProcess::start("dir");
    let c = p.connect();
    hello5(&c);
    assert_eq!(
        user(&c, UserReq::ReadingOptions("志".repeat(10000))),
        UserReply::Error(LexError::Full)
    );
    assert!(matches!(
        user(&c, UserReq::GetSettings),
        UserReply::Settings { .. }
    ));
}

#[test]
fn background_poll_publishes_external_settings_on_next_event() {
    let p = EngineProcess::start("dir");
    let c = p.connect();
    hello5(&c);
    assert!(matches!(call(&c, 1, Event::Ping), Reply::Update { .. }));
    let settings = UserSettings {
        toggle_key: ToggleKey::LeftShift,
        punct_half: true,
        ..Default::default()
    };
    fs::write(
        p.root.join("user/settings.toml"),
        twime_userdata::settings::serialize(&settings),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(6);
    loop {
        if let Reply::Update { update, .. } = call(&c, 1, Event::Ping) {
            if update.toggle_key == ToggleKey::LeftShift {
                assert!(update.punct_half);
                break;
            }
        }
        assert!(Instant::now() < until, "poll did not publish settings");
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn list_pages_stop_at_sixty_kilobytes_and_continue_by_actual_count() {
    let p = EngineProcess::start("dir");
    let c = p.connect();
    hello5(&c);
    let entries: Vec<_> = (0..1600)
        .map(|index| {
            let mut i = index % 32;
            let word = (0..8)
                .map(|_| {
                    let ch = ['志', '知', '智', '制'][i % 4];
                    i /= 4;
                    ch
                })
                .collect();
            LexEntry {
                word,
                reading: (0..8).map(|bit| if (index / 32) >> bit & 1 == 0 { "ㄓˋ" } else { "ㄔ" }).collect::<Vec<_>>().join(" "),
                weight: Some(10.0),
            }
        })
        .collect();
    for batch in entries.chunks(300) {
        assert!(
            matches!(user(&c, UserReq::Add(batch.to_vec())), UserReply::Added { rejected, .. } if rejected.is_empty())
        );
    }
    let mut offset = 0;
    while offset < entries.len() as u32 {
        let bytes = raw(
            &c,
            0,
            Event::User(UserReq::List {
                query: String::new(),
                offset,
                limit: u16::MAX,
            }),
        )
        .unwrap();
        assert!(bytes.len() <= USER_PAGE_BYTES);
        match Reply::decode(&bytes).unwrap() {
            Reply::User(UserReply::Page {
                total: 1600,
                offset: actual,
                entries: page,
                ..
            }) => {
                assert_eq!(actual, offset);
                assert!(!page.is_empty());
                assert_eq!(page, entries[offset as usize..offset as usize + page.len()]);
                offset += page.len() as u32;
            }
            r => panic!("{r:?}"),
        }
    }
}
