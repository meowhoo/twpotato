#![cfg(windows)]
//! WORDBONUS (WORDBONUS-plan 3): `wordbonus=` in the assets model.cfg is the only switch; a stray file is ignored,
//! a named file that is missing / truncated / built for other assets / a bad name stops the engine before ready.
use std::{fs, io::{BufRead, BufReader}, path::PathBuf, process::{Command, Stdio}};
use twime_model::{Assets, wordbonus};

struct Proc { child: std::process::Child, root: PathBuf }
impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        assert!(self.root.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(&self.root);
    }
}

const DICT: &str = "ㄕㄜˊ 蛇 -1\nㄕㄜˊ 什 -2\nㄇㄛ˙ 麼 -1\n";

fn row() -> Vec<wordbonus::WbRow> {
    vec![wordbonus::WbRow { reading: "ㄕㄜˊ-ㄇㄛ˙".into(), word: "什麼".into(), bonus: 6.0 }]
}

#[test]
fn model_cfg_switch_and_failures() {
    for variant in ["absent", "stray_file_ignored", "present", "empty_value", "missing", "truncated", "other_assets", "path_name", "given_twice"] {
        let tag = format!("wb-{}-{}-{variant}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let root = std::env::temp_dir().join(&tag);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("data.txt"), DICT).unwrap();
        fs::write(root.join("common-readings.txt"), "").unwrap();
        let a = Assets::from_dictionary_text(DICT, "").unwrap();
        let good = wordbonus::build(&a, &row()).unwrap();
        let cfg = match variant {
            "absent" => "int8=off\n".to_string(),
            "stray_file_ignored" => { fs::write(root.join("wordbonus.bin"), &good).unwrap(); "int8=off\n".to_string() }
            "present" => { fs::write(root.join("wordbonus.bin"), &good).unwrap(); "int8=off\nwordbonus=wordbonus.bin\n".to_string() }
            "empty_value" => { fs::write(root.join("wordbonus.bin"), &good).unwrap(); "int8=off\nwordbonus=\n".to_string() }
            "missing" => "int8=off\nwordbonus=wordbonus.bin\n".to_string(),
            "truncated" => { fs::write(root.join("wordbonus.bin"), &good[..good.len() - 3]).unwrap(); "int8=off\nwordbonus=wordbonus.bin\n".to_string() }
            "other_assets" => {
                let other = Assets::from_dictionary_text(&format!("{DICT}ㄇㄛ˙ 嘛 -2\n"), "").unwrap();
                fs::write(root.join("wordbonus.bin"), wordbonus::build(&other, &row()).unwrap()).unwrap();
                "int8=off\nwordbonus=wordbonus.bin\n".to_string()
            }
            "path_name" => { fs::write(root.join("wordbonus.bin"), &good).unwrap(); "int8=off\nwordbonus=..\\wordbonus.bin\n".to_string() }
            _ => { fs::write(root.join("wordbonus.bin"), &good).unwrap(); "int8=off\nwordbonus=wordbonus.bin\nwordbonus=wordbonus.bin\n".to_string() }
        };
        fs::write(root.join("model.cfg"), cfg).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_twime-engined"));
        cmd.arg("--assets-dir").arg(&root).args(["--no-model", "--lambda", "0", "--pipe", &tag, "--data-dir"]).arg(root.join("state")).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut proc = Proc { child: cmd.spawn().unwrap(), root };
        let mut reader = BufReader::new(proc.child.stdout.take().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        if ["absent", "stray_file_ignored", "present", "empty_value"].contains(&variant) {
            assert!(first.starts_with("ready pid="), "{variant}: {first:?}");
            let mut rest = String::new();
            reader.read_line(&mut rest).unwrap(); // genj line
            let mut third = String::new();
            reader.read_line(&mut third).unwrap();
            if variant == "present" { assert_eq!(third.trim(), "wordbonus on rows=1"); } else { assert!(!third.starts_with("wordbonus"), "{variant}: {third:?}"); }
        } else {
            assert!(first.is_empty(), "{variant}: {first:?}");
            assert!(!proc.child.wait().unwrap().success(), "{variant}");
        }
    }
}
