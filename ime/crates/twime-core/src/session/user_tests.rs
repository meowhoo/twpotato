use super::*;
use std::{fs, path::PathBuf, time::Duration};
use twime_proto::{UserReply, UserSettings};

struct Fixture {
    dir: PathBuf,
    engine: Arc<Engine>,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "twime-core-user-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let assets = Arc::new(
            Assets::from_dictionary_text(
                "ㄓˋ 志 -1\nㄓˋ 知 -2\nㄇㄧㄥˊ 明 -1\nㄇㄧㄥˊ 名 -2\n",
                "",
            )
            .unwrap(),
        );
        let service = twime_userdata::Service::open(dir.clone(), assets.clone(), None);
        let writer = service.spawn_writer();
        Self {
            dir,
            engine: Engine::with_user(
                assets,
                Config::default(),
                Some(UserLink { service, writer }),
                ToggleKey::None,
            ),
        }
    }
    fn barrier(&self) -> UserReply {
        self.engine
            .user()
            .unwrap()
            .writer
            .submit(UserReq::GetSettings)
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
    }
    fn settings(&self, s: UserSettings) {
        assert!(matches!(
            self.engine
                .user()
                .unwrap()
                .writer
                .submit(UserReq::SetSettings(s))
                .recv()
                .unwrap(),
            UserReply::Settings { .. }
        ));
    }
    fn session(&self) -> Session {
        let mut s = self.engine.new_session();
        s.set_ux(true);
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn event(s: &mut Session, ev: Event) -> Update {
    s.handle(s.last_seq + 1, None, ev)
}
fn compose(s: &mut Session, text: &str) {
    for c in text.chars() {
        s.on_key(KeyEvent::press(keymap::key_of(c).unwrap()));
    }
}
fn notice(s: &mut Session) -> String {
    let u = event(s, Event::AddToLexicon);
    assert!(!u.handled && u.commit.is_none());
    u.notice.unwrap()
}

#[test]
fn add_conditions_and_one_shot_notices() {
    let f = Fixture::new();
    let mut disabled = Engine::new(f.engine.assets.clone(), Config::default()).new_session();
    disabled.set_ux(true);
    assert_eq!(notice(&mut disabled), "詞庫未啟用");
    let mut old = f.engine.new_session();
    assert!(event(&mut old, Event::AddToLexicon).notice.is_none());
    let mut s = f.session();
    assert_eq!(notice(&mut s), "組字未完成");
    compose(&mut s, "ㄓ");
    assert_eq!(notice(&mut s), "組字未完成");
    event(&mut s, Event::ExternalEdit);
    s.st.run = "English".into();
    assert_eq!(notice(&mut s), "英文片段無法加入詞庫");
    event(&mut s, Event::ExternalEdit);
    s.on_key(KeyEvent::press(Key::Backquote));
    assert_eq!(notice(&mut s), "符號表無法加入詞庫");
    event(&mut s, Event::ExternalEdit);
    for _ in 0..9 {
        compose(&mut s, "ㄓˋ");
    }
    assert_eq!(notice(&mut s), "最多 8 字");
    event(&mut s, Event::ExternalEdit);
    compose(&mut s, "ㄓˋㄇㄧㄥˊ");
    let before = s.comp();
    assert_eq!(notice(&mut s), "已送出加入詞庫：志明");
    assert_eq!(s.comp(), before);
    assert!(s.pending.as_ref().unwrap().before.is_none());
    assert!(event(&mut s, Event::Ping).notice.is_none());
    f.barrier();
    let reply = f.engine.user().unwrap().service.handle(UserReq::List {
        query: String::new(),
        offset: 0,
        limit: 10,
    });
    assert!(
        matches!(reply, UserReply::Page { entries, .. } if entries.len() == 1 && entries[0].word == "志明")
    );
    // A fresh candidate query reads the published lexicon even with no model.
    let mut s2 = f.session();
    compose(&mut s2, "ㄓˋㄇㄧㄥˊ");
    assert!(
        s2.on_key(KeyEvent::press(Key::Down))
            .candidates
            .unwrap()
            .items
            .iter()
            .any(|w| w == "志明")
    );
    s.st.conv.as_mut().unwrap().chars[0] = Some('明');
    assert_eq!(notice(&mut s), "字與讀音不合");
    s.st.conv = None;
    assert_eq!(notice(&mut s), "組字未完成");
}

#[test]
fn settings_refresh_without_mode_or_open_symbol_changes() {
    let f = Fixture::new();
    let settings = UserSettings {
        punct_half: true,
        symbol_table: false,
        default_mode: Mode::English,
        toggle_key: ToggleKey::RightShift,
        ..Default::default()
    };
    f.settings(settings);
    let mut s = f.session();
    assert_eq!(s.mode(), Mode::English);
    let u = event(&mut s, Event::Ping);
    assert!(u.punct_half);
    assert_eq!(u.toggle_key, ToggleKey::RightShift);
    event(&mut s, Event::ToggleMode);
    f.settings(UserSettings::default());
    let u = s.on_key(KeyEvent::press(Key::Backquote));
    assert!(u.symbols && !u.punct_half);
    f.settings(UserSettings {
        symbol_table: false,
        toggle_key: ToggleKey::LeftShift,
        ..Default::default()
    });
    let u = event(&mut s, Event::Ping);
    assert!(u.symbols);
    assert_eq!(u.toggle_key, ToggleKey::LeftShift);
    assert_eq!(s.mode(), Mode::Chinese);
    s.on_key(KeyEvent::press(Key::Esc));
    assert!(!s.on_key(KeyEvent::press(Key::Backquote)).symbols);
    let mut legacy = Engine::with_user(
        f.engine.assets.clone(),
        Config::default(),
        None,
        ToggleKey::None,
    )
    .new_session();
    assert_eq!(event(&mut legacy, Event::Ping).toggle_key, ToggleKey::None);
}

#[test]
fn write_failure_is_reported_on_next_event() {
    let f = Fixture::new();
    // Deterministic IO failure: the lock-file path is a directory.
    fs::create_dir(f.dir.join("userdata.lock")).unwrap();
    let mut s = f.session();
    compose(&mut s, "ㄓˋ");
    assert_eq!(notice(&mut s), "已送出加入詞庫：志");
    f.barrier();
    assert_eq!(
        event(&mut s, Event::Ping).notice.as_deref(),
        Some("詞庫寫入失敗")
    );
    assert!(event(&mut s, Event::Ping).notice.is_none());
    assert!(matches!(
        f.engine.user().unwrap().service.handle(UserReq::List {
            query: String::new(),
            offset: 0,
            limit: 10
        }),
        UserReply::Page { total: 0, .. }
    ));
}

#[test]
fn add_to_full_lexicon_reports_async_rejection_on_next_event() {
    let f = Fixture::new();
    fs::write(f.dir.join("userlex.tsv"), "志明\tㄓˋ ㄇㄧㄥˊ\n".repeat(twime_userdata::lexicon::MAX_ENTRIES)).unwrap();
    let mut s = f.session();
    compose(&mut s, "ㄓˋ");
    assert_eq!(notice(&mut s), "已送出加入詞庫：志");
    f.barrier();
    assert_eq!(event(&mut s, Event::Ping).notice.as_deref(), Some("詞庫寫入失敗"));
    assert!(event(&mut s, Event::Ping).notice.is_none());
    assert!(matches!(f.barrier(), UserReply::Settings { last_error: Some(_), .. }));
    assert_eq!(f.engine.user().unwrap().service.error_gen(), 1);
}

#[test]
fn locked_writer_does_not_block_keys_and_b_is_used_after_publication() {
    let f = Fixture::new();
    let dict = "ㄓˋ 志 -1\nㄓˋ 知 -2\nㄇㄧㄥˊ 明 -1\nㄇㄧㄥˊ 名 -2\n";
    let (dictionary, _) = twime_model::lexicon::Dictionary::parse(dict);
    let vocab = twime_model::vocab::Vocab::new(&dictionary);
    let assets = Arc::new(Assets::from_dictionary_model(dict, "",
        twime_model::n1::N1::random(vocab.len(), 32, 1, 11)).unwrap());
    let service = twime_userdata::Service::open(f.dir.clone(), assets.clone(), None);
    let writer = service.spawn_writer();
    let engine = Engine::with_user(assets.clone(), Config { lambda: 0.0, ..Default::default() },
        Some(UserLink { service, writer: writer.clone() }), ToggleKey::Shift);
    let syllables = vec!["ㄓˋ".into(), "ㄇㄧㄥˊ".into()];
    let before = assets.convert("", &syllables, &[], 0.0).text().unwrap();
    // B deliberately differs from the model's initial output.
    let b_word = if before == "知名" { "志明" } else { "知名" };
    let entry = |word: &str| LexEntry { word: word.into(), reading: "ㄓˋ ㄇㄧㄥˊ".into(), weight: Some(30.0) };
    let lock = twime_userdata::store::lock(&f.dir).unwrap();
    let a = writer.submit(UserReq::Add(vec![LexEntry { word: "志".into(), reading: "ㄓˋ".into(), weight: None }]));
    let b = writer.submit(UserReq::Add(vec![entry(b_word)]));
    let (done, wait) = std::sync::mpsc::channel();
    let input_engine = engine.clone();
    let task = std::thread::spawn(move || {
        let mut s = input_engine.new_session();
        s.set_ux(true);
        let u = event(&mut s, Event::Key(KeyEvent::press(keymap::key_of('ㄓ').unwrap())));
        done.send(u).unwrap();
    });
    let result = wait.recv_timeout(Duration::from_secs(1));
    let unpublished = b.try_recv().is_err();
    drop(lock); // Always release before asserting, including on a broken blocking path.
    assert!(result.unwrap().handled);
    assert!(unpublished);
    task.join().unwrap();
    assert!(matches!(a.recv_timeout(Duration::from_secs(5)).unwrap(), UserReply::Added { added: 1, rejected, .. } if rejected.is_empty()));
    assert!(matches!(b.recv_timeout(Duration::from_secs(5)).unwrap(), UserReply::Added { added: 1, rejected, .. } if rejected.is_empty()));
    assert_eq!(assets.convert("", &syllables, &[], 0.0).text().as_deref(), Some(b_word));
    let mut next = engine.new_session();
    next.set_ux(true);
    compose(&mut next, "ㄓˋㄇㄧㄥˊ");
    assert_eq!(next.st.conv.as_ref().unwrap().text().as_deref(), Some(b_word));
}

/// ZHUYIN-OUT 3.1: a composition holding the typed bopomofo is not added to the lexicon (and does not panic).
#[test]
fn a_literal_cannot_be_added_to_the_lexicon() {
    let f = Fixture::new();
    let mut s = f.session();
    compose(&mut s, "ㄓˋ");
    s.on_key(KeyEvent::press(Key::Down));
    s.on_key(KeyEvent::press(Key::End));
    s.on_key(KeyEvent::press(Key::Enter));
    assert!(s.has_lit() && s.committable());
    let before = s.comp();
    assert_eq!(notice(&mut s), "含注音原樣，無法加入詞庫");
    assert_eq!(s.comp(), before);
    assert!(s.pending.as_ref().unwrap().before.is_none());
    f.barrier();
    let reply = f.engine.user().unwrap().service.handle(UserReq::List { query: String::new(), offset: 0, limit: 10 });
    assert!(matches!(reply, UserReply::Page { entries, .. } if entries.is_empty()));
}
