#![cfg(windows)]
//! Real command-line startup: asset discovery, explicit override, candidates and ready compatibility.
use std::{fs, io::{BufRead, BufReader}, path::PathBuf, process::{Child, Command, Stdio}};
use twime_core::keymap::key_of;
use twime_model::{Assets, pn::{self, BuildWord}};
use twime_proto::{Event, Key, KeyEvent, Reply, Request, Update};
use twime_winpipe::Client;

struct Process { child: Child, root: PathBuf }
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        assert!(self.root.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn call(c: &Client, seq: &mut u32, event: Event) -> Update {
    *seq += 1;
    let r = Request { session: 1, seq: *seq, ack: None, event };
    let bytes = c.call(&r.encode(), 5000).expect("pipe call");
    match Reply::decode(&bytes).expect("reply") {
        Reply::Update { update, .. } => update,
        _ => panic!("expected update"),
    }
}

#[test]
fn gen_asset_pair_missing_partial_present_and_explicit_override() {
    let mut reference = None;
    let mut reference_eff = None;
    for variant in ["absent", "fst_only", "lst_only", "present", "override", "pop_missing"] {
        let tag = format!("{}-{}-{variant}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let root = std::env::temp_dir().join(format!("twime-gen-{tag}"));
        fs::create_dir_all(&root).unwrap();
        let dict = "ㄑㄩㄢˊ 全 -1\nㄑㄩㄢˊ 權 -3\nㄌㄧˋ 力 -1\nㄌㄧˋ 利 -3\n";
        fs::write(root.join("data.txt"), dict).unwrap();
        fs::write(root.join("common-readings.txt"), "").unwrap();
        let assets = Assets::from_dictionary_text(dict, "").unwrap();
        // A tiny zero-layer model exercises the model-only general-word candidates.
        let vocab = twime_model::vocab::Vocab::new(assets.dictionary()).len();
        let mut model = b"TWG1".to_vec();
        for n in [vocab as u32, 32, 0, 32] { model.extend(n.to_le_bytes()); }
        model.resize(20 + 4 * (vocab * 32 + 32), 0);
        fs::write(root.join("model.bin"), model).unwrap();
        let build = |base: &str, word: &str| {
            let st = pn::build(&assets, &[BuildWord { word: word.into(), reading: "ㄑㄩㄢˊ-ㄌㄧˋ".into(), class: 0 }], &root.join(base)).unwrap();
            assert!(st.kept == 1);
        };
        if variant != "absent" && variant != "pop_missing" { build("gen", "權利"); }
        if variant == "fst_only" { fs::remove_file(root.join("gen.lst")).unwrap(); }
        if variant == "lst_only" { fs::remove_file(root.join("gen.fst")).unwrap(); }
        if variant == "override" { build("explicit", "權力"); }
        let pipe = format!("twime-gen-{tag}");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_twime-engined"));
        cmd.args(["--assets-dir"]).arg(&root)
            .args(["--lambda", "0", "--candidates", "--pipe", &pipe, "--data-dir"])
            .arg(root.join("state")).stdout(Stdio::piped()).stderr(Stdio::null());
        if variant == "override" { cmd.arg("--gen").arg(root.join("explicit")); }
        if variant == "pop_missing" { cmd.args(["--autocomp-pop", "on", "--autocomp-list", "on"]); }
        let mut process = Process { child: cmd.spawn().unwrap(), root };
        let mut ready = String::new();
        BufReader::new(process.child.stdout.take().unwrap()).read_line(&mut ready).unwrap();
        let (prefix, eff) = ready.trim_end().split_once(" eff=").expect("effective settings");
        let parts: Vec<_> = prefix.split_whitespace().collect();
        assert!(parts.len() == 4 && parts[0] == "ready");
        for (part, key) in parts[1..].iter().zip(["pid=", "load_ms=", "pipe="]) {
            assert!(part.starts_with(key));
        }
        assert!(parts[1][4..].parse::<u32>().is_ok());
        assert!(parts[2][8..].parse::<f64>().is_ok());
        if let Some(expected) = &reference_eff { assert!(expected == eff); }
        else { reference_eff = Some(eff.to_string()); }
        let c = Client::connect(&pipe, None, 1000).unwrap();
        let mut seq = 0;
        for ch in "ㄑㄩㄢˊㄌㄧˋ".chars() {
            call(&c, &mut seq, Event::Key(KeyEvent::press(key_of(ch).unwrap())));
        }
        let output = call(&c, &mut seq, Event::Ping).composition.unwrap().text();
        if let Some(expected) = &reference { assert!(expected == &output); }
        else { reference = Some(output); }
        let u = call(&c, &mut seq, Event::Key(KeyEvent::press(Key::Down)));
        let items = u.candidates.expect("candidates").items;
        let has = |word: &str| items.iter().any(|s| s == word);
        assert!(has("權利") == (variant == "present"));
        assert!(has("權力") == (variant == "override"));
    }
}
