#![cfg(windows)]
mod common;
use std::{fs, io::{BufRead,BufReader}, process::{Command,Stdio}, path::PathBuf};
use twime_model::{Assets,pn::{self,BuildWord}};

struct Proc { child: std::process::Child, root: PathBuf }
impl Drop for Proc {
    fn drop(&mut self) {
        let _=self.child.kill();let _=self.child.wait();
        assert!(self.root.starts_with(std::env::temp_dir()));let _=fs::remove_dir_all(&self.root);
    }
}
#[test]
fn closed_startup_stdout_keeps_pipe_service_alive() {
    if crate::common::engine_start_refused() {
        return;
    }
    use twime_proto::{Event, Reply, Request};
    use twime_winpipe::Client;
    let tag=format!("srchj-closed-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let root=std::env::temp_dir().join(&tag);fs::create_dir_all(&root).unwrap();
    fs::write(root.join("data.txt"),"ㄅㄚ 八 -1\n").unwrap();
    fs::write(root.join("common-readings.txt"),"").unwrap();
    let child=Command::new(env!("CARGO_BIN_EXE_twime-engined"))
        .arg("--assets-dir").arg(&root).args(["--no-model","--lambda","0","--pipe",&tag,"--data-dir"])
        .arg(root.join("state")).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut proc=Proc{child,root};
    // Close before ready, making any startup write fail deterministically.
    drop(proc.child.stdout.take());
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
    let client=loop {
        if let Ok(c)=Client::connect(&tag,None,100) {break c;}
        assert!(proc.child.try_wait().unwrap().is_none());
        assert!(std::time::Instant::now()<deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let req=Request{session:1,seq:1,ack:None,event:Event::Ping};
    let bytes=client.call(&req.encode(),5000).unwrap();
    assert!(matches!(Reply::decode(&bytes).unwrap(),Reply::Update{..}));
}
#[test]
fn genj_discovery_override_status_and_partial_failure() {
    if crate::common::engine_start_refused() {
        return;
    }
    for variant in ["absent","present","override","fst_only","cfg_missing","tier_count","tier_value","cfg_invalid"] {
        let tag=format!("srchj-{}-{}-{variant}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let root=std::env::temp_dir().join(&tag);fs::create_dir_all(&root).unwrap();
        let dict="ㄅㄚ 八 -1\nㄅㄚ 巴 -2\n";
        fs::write(root.join("data.txt"),dict).unwrap();fs::write(root.join("common-readings.txt"),"").unwrap();
        let a=Assets::from_dictionary_text(dict,"").unwrap();
        let base=root.join(if variant=="override" {"explicit"} else {"genj"});
        if variant!="absent" {
            pn::build(&a,&[BuildWord{word:"巴巴巴".into(),reading:"ㄅㄚ-ㄅㄚ-ㄅㄚ".into(),class:1}],&base).unwrap();
            fs::write(base.with_extension("tier"),[1]).unwrap();
            fs::write(base.with_extension("cfg"),"b3_a=0\nb3_b=8\nb3_c=off\nb3_d=4\nh=4\n").unwrap();
            match variant {
                "fst_only" => {for ext in ["lst","tier","cfg"] {fs::remove_file(base.with_extension(ext)).unwrap();}},
                "cfg_missing" => {fs::remove_file(base.with_extension("cfg")).unwrap();},
                "tier_count" => fs::write(base.with_extension("tier"),[1,1]).unwrap(),
                "tier_value" => fs::write(base.with_extension("tier"),[4]).unwrap(),
                "cfg_invalid" => fs::write(base.with_extension("cfg"),"b3_a=NaN\nb3_b=8\nb3_c=0\nb3_d=0\nh=4\n").unwrap(),
                _ => {},
            }
        }
        let mut cmd=Command::new(env!("CARGO_BIN_EXE_twime-engined"));
        cmd.arg("--assets-dir").arg(&root).args(["--no-model","--lambda","0","--pipe",&tag,"--data-dir"]).arg(root.join("state")).stdout(Stdio::piped()).stderr(Stdio::piped());
        if variant=="override" {cmd.arg("--genj").arg(&base);}
        let mut proc=Proc{child:cmd.spawn().unwrap(),root};
        let valid=["absent","present","override"].contains(&variant);
        let mut reader=BufReader::new(proc.child.stdout.take().unwrap());
        let mut first=String::new();reader.read_line(&mut first).unwrap();
        if valid {
            assert!(first.starts_with("ready pid="));assert!(first.contains(" eff="));
            let mut status=String::new();reader.read_line(&mut status).unwrap();
            if variant=="absent" { assert!(status.trim()=="genj off"); }
            else {assert!(status.starts_with("genj on count=1 b3_a=0 b3_b=8 b3_c=off b3_d=4 h=4"));}
        } else {
            assert!(first.is_empty());assert!(!proc.child.wait().unwrap().success());
        }
    }
}
