//! EMOJI (EMOJI-plan 3.1, 4 V1): the `emoji` key of model.cfg, the `--emoji` flag, the missing-file warning, and a real
//! startup with the table (ready line unchanged, one more line, emoji at the end of the list).
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use twime_engined::{emoji_setting, load_emoji};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Dir(PathBuf);
impl Dir {
    fn new(cfg: Option<&str>) -> Dir {
        let p = std::env::temp_dir().join(format!("twime-emoji-cfg-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&p).unwrap();
        if let Some(c) = cfg {
            fs::write(p.join("model.cfg"), c).unwrap();
        }
        Dir(p)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn model_cfg_key_parses_with_and_without_an_int8_flag() {
    assert_eq!(emoji_setting(None, None), Ok(false));
    assert_eq!(emoji_setting(None, Some(&Dir::new(None).0)), Ok(false));
    for flag in [None, Some(true), Some(false)] {
        assert_eq!(emoji_setting(flag, Some(&Dir::new(Some("int8=on\n")).0)), Ok(false));
        assert_eq!(emoji_setting(flag, Some(&Dir::new(Some("int8=on\nemoji=off\n")).0)), Ok(false));
        assert_eq!(emoji_setting(flag, Some(&Dir::new(Some("int8=on\nemoji=on # comment\n")).0)), Ok(true));
        assert!(emoji_setting(flag, Some(&Dir::new(Some("int8=on\nemoji=maybe\n")).0)).is_err());
        assert!(emoji_setting(flag, Some(&Dir::new(Some("int8=on\nemoji=on\nemoji=off\n")).0)).is_err());
    }
}

#[test]
fn table_loading() {
    // no directory / no file: off with a warning, not an error
    assert!(load_emoji(None).unwrap().is_none());
    let d = Dir::new(None);
    assert!(load_emoji(Some(&d.0)).unwrap().is_none());
    fs::write(d.0.join("emoji.tsv"), "#emoji v1 x\nㄇㄠ\t🐈 🐱\tcldr\n").unwrap();
    let t = load_emoji(Some(&d.0)).unwrap().unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t.lookup(&["ㄇㄠ".to_string()]), ["🐈", "🐱"]);
    // a malformed file is an error that names the file
    fs::write(d.0.join("emoji.tsv"), "ㄇㄠ\t🐈\n").unwrap();
    assert!(load_emoji(Some(&d.0)).unwrap_err().contains("emoji.tsv"));
}

#[cfg(windows)]
mod startup {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read},
        process::{Child, Command, Stdio},
    };
    use twime_core::keymap::key_of;
    use twime_model::Assets;
    use twime_proto::{Event, Key, KeyEvent, Reply, Request, Update};
    use twime_winpipe::Client;

    fn call(c: &Client, seq: &mut u32, event: Event) -> Update {
        *seq += 1;
        let bytes = c.call(&Request { session: 1, seq: *seq, ack: None, event }.encode(), 5000).expect("pipe call");
        match Reply::decode(&bytes).expect("reply") {
            Reply::Update { update, .. } => update,
            _ => panic!("expected update"),
        }
    }

    struct Run {
        child: Child,
        ready: String,
        lines: BufReader<std::process::ChildStdout>,
        client: Client,
        _dir: Dir,
    }
    impl Drop for Run {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// A tiny assets directory (two-word dictionary, zero-layer model).
    fn assets(model_cfg: &str, emoji_tsv: Option<&str>) -> Dir {
        let dir = Dir::new(Some(model_cfg));
        let dict = "ㄇㄠ 貓 -1
ㄇㄠ 錨 -2
";
        fs::write(dir.0.join("data.txt"), dict).unwrap();
        fs::write(dir.0.join("common-readings.txt"), "").unwrap();
        let vocab = twime_model::vocab::Vocab::new(Assets::from_dictionary_text(dict, "").unwrap().dictionary()).len();
        let mut model = b"TWG1".to_vec();
        for n in [vocab as u32, 32, 0, 32] {
            model.extend(n.to_le_bytes());
        }
        model.resize(20 + 4 * (vocab * 32 + 32), 0);
        fs::write(dir.0.join("model.bin"), model).unwrap();
        if let Some(t) = emoji_tsv {
            fs::write(dir.0.join("emoji.tsv"), t).unwrap();
        }
        dir
    }

    fn start(model_cfg: &str, args: &[&str], emoji_tsv: Option<&str>) -> Run {
        let dir = assets(model_cfg, emoji_tsv);
        let pipe = format!("twime-emoji-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let mut child = Command::new(env!("CARGO_BIN_EXE_twime-engined"))
            .arg("--assets-dir").arg(&dir.0)
            .args(["--lambda", "0", "--candidates", "--pipe", &pipe, "--data-dir"]).arg(dir.0.join("state"))
            .args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        lines.read_line(&mut ready).unwrap();
        assert!(ready.starts_with("ready "), "{ready}");
        let client = Client::connect(&pipe, None, 1000).unwrap();
        Run { child, ready, lines, client, _dir: dir }
    }

    impl Run {
        fn eff(&self) -> String {
            self.ready.split_once(" eff=").unwrap().1.trim_end().to_string()
        }
        /// The startup lines after the ready line (written in one go) up to the one starting with `prefix`.
        fn line_with(&mut self, prefix: &str) -> String {
            for _ in 0..12 {
                let mut line = String::new();
                self.lines.read_line(&mut line).unwrap();
                if line.starts_with(prefix) {
                    return line.trim_end().to_string();
                }
            }
            panic!("no startup line starting with {prefix}");
        }
        fn list(&self) -> Vec<String> {
            let mut seq = 0;
            for ch in "ㄇㄠ ".chars() {
                call(&self.client, &mut seq, Event::Key(KeyEvent::press(if ch == ' ' { Key::Space } else { key_of(ch).unwrap() })));
            }
            call(&self.client, &mut seq, Event::Key(KeyEvent::press(Key::Down))).candidates.expect("candidates").items
        }
        fn stderr(mut self) -> String {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let mut s = String::new();
            let _ = self.child.stderr.take().unwrap().read_to_string(&mut s);
            s
        }
    }

    const TSV: &str = "#emoji v1 t\nㄇㄠ\t🐈 🐱\tcldr\n";

    #[test]
    fn off_missing_and_on() {
        // off (absent): the table is not read, no emoji, no extra line
        let off = start("int8=off\n", &[], Some(TSV));
        let base = off.list();
        assert!(base.iter().all(|t| t != "🐈"));
        let eff_off = off.eff();
        assert!(off.stderr().is_empty());
        // on by the key but no emoji.tsv: nothing changes, one warning, startup works
        let miss = start("int8=off\nemoji=on\n", &[], None);
        assert_eq!(miss.list(), base);
        assert_eq!(miss.eff(), eff_off);
        let err = miss.stderr();
        assert!(err.contains("emoji.tsv is not installed"), "{err}");
        // on by the key: one more line, the ready line's settings unchanged, emoji at the end of the list
        let mut on = start("int8=off\nemoji=on\n", &[], Some(TSV));
        assert_eq!(on.eff(), eff_off);
        assert_eq!(on.line_with("emoji="), "emoji=on keys=1");
        // (the typed bopomofo, ZHUYIN-OUT, is the very last item of every list: the emoji sit just before it)
        let l = on.list();
        let n = base.len() - 1;
        assert_eq!(&l[..n], &base[..n]);
        assert_eq!(&l[n..l.len() - 1], ["🐈", "🐱"]);
        assert_eq!(l.last(), base.last());
        // the flag wins over the key in both directions
        let off_flag = start("int8=off\nemoji=on\n", &["--emoji", "off"], Some(TSV));
        assert_eq!(off_flag.list(), base);
        let mut on_flag = start("int8=off\n", &["--emoji", "on"], Some(TSV));
        assert_eq!(on_flag.line_with("emoji="), "emoji=on keys=1");
        assert_eq!(on_flag.list().len(), base.len() + 2);
    }

    #[test]
    fn a_malformed_table_stops_startup() {
        let dir = assets("int8=off\nemoji=on\n", Some("not a table\n"));
        let out = Command::new(env!("CARGO_BIN_EXE_twime-engined"))
            .arg("--assets-dir").arg(&dir.0)
            .args(["--lambda", "0", "--candidates", "--pipe", "twime-emoji-bad", "--data-dir"]).arg(dir.0.join("state"))
            .output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("emoji.tsv"), "{}", String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn an_unreadable_table_is_an_error_but_a_missing_one_is_not() {
    let d = Dir::new(None);
    // a directory where the file should be: not "missing", so startup must stop
    fs::create_dir(d.0.join("emoji.tsv")).unwrap();
    assert!(load_emoji(Some(&d.0)).unwrap_err().contains("cannot read"));
}
