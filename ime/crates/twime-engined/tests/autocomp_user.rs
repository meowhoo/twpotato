//! SETTINGS-AUTOCOMP: flag > user autocomp.cfg > model.cfg > off, per key; a bad user file is ignored whole.
use std::{path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};
use twime_engined::{autocomp_list_effective as list, autocomp_pop_effective as pop, autocomp_user_cfg, autocomp_user_summary};
use twime_userdata::autocomp_cfg::{AutocompCfg, Loaded};

static N: AtomicUsize = AtomicUsize::new(0);
fn dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("twime-acuser-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn model(text: &str) -> PathBuf {
    let p = dir("model");
    std::fs::write(p.join("model.cfg"), text).unwrap();
    p
}

#[test]
fn unset_everywhere_is_off() {
    assert!(!list(None, None, None, None).unwrap());
    assert!(!pop(None, None, None, None).unwrap());
    let m = model("int8=on\n");
    assert!(!list(None, None, None, Some(&m)).unwrap());
}
#[test]
fn model_cfg_is_the_fallback_and_user_overrides_per_key() {
    let m = model("int8=on\nautocomp_list=on\nautocomp_pop=on\n");
    assert!(list(None, None, None, Some(&m)).unwrap());
    assert!(!list(None, Some(false), None, Some(&m)).unwrap());
    assert!(pop(None, None, None, Some(&m)).unwrap());
    assert!(!pop(None, Some(false), None, Some(&m)).unwrap());
    // user turns on what model.cfg leaves off
    let m = model("int8=on\n");
    assert!(list(None, Some(true), None, Some(&m)).unwrap());
    assert!(pop(None, Some(true), None, Some(&m)).unwrap());
    // a key the user file does not mention stays with model.cfg
    let m = model("int8=on\nautocomp_pop=on\n");
    assert!(pop(None, None, None, Some(&m)).unwrap());
    assert!(list(None, Some(true), None, Some(&m)).unwrap());
}
#[test]
fn flag_beats_user_file() {
    let m = model("int8=on\n");
    assert!(!list(Some(false), Some(true), None, Some(&m)).unwrap());
    assert!(pop(Some(true), Some(false), None, Some(&m)).unwrap());
}
#[test]
fn model_cfg_stays_strict_only_when_consulted() {
    let bad = model("int8=on\nautocomp_list=maybe\n");
    assert!(list(None, None, None, Some(&bad)).is_err());
    // an override above it means model.cfg is not consulted for that key
    assert!(list(None, Some(true), None, Some(&bad)).unwrap());
}
#[test]
fn user_file_states_and_fail_closed() {
    let d = dir("user");
    assert_eq!(autocomp_user_cfg(true, &d), Loaded::Absent);
    twime_userdata::autocomp_cfg::save(&d, true, false).unwrap();
    assert_eq!(autocomp_user_cfg(true, &d), Loaded::Ok(AutocompCfg { list: Some(true), pop: Some(false) }));
    assert_eq!(autocomp_user_summary(autocomp_user_cfg(true, &d)), "autocomp_user=list:on,pop:off");
    // user data off: the file is not read
    assert_eq!(autocomp_user_cfg(false, &d), Loaded::Absent);
    for junk in [&b"\xff\xfe"[..], b"autocomp_list=on\nbogus=1\n", b"autocomp_list=on\nautocomp_list=on\n", b"autocomp_list on\n"] {
        std::fs::write(d.join("autocomp.cfg"), junk).unwrap();
        let l = autocomp_user_cfg(true, &d);
        assert_eq!(l, Loaded::Bad);
        assert_eq!(l.effective(), AutocompCfg::default());
        assert_eq!(autocomp_user_summary(l), "autocomp_user=bad");
        // the bad file changes nothing: model.cfg / off decide
        let m = model("int8=on\nautocomp_list=on\n");
        assert!(list(None, l.effective().list, None, Some(&m)).unwrap());
        assert!(!pop(None, l.effective().pop, None, Some(&m)).unwrap());
    }
    // a directory where the file should be: unreadable => Bad, not a panic
    let d2 = dir("dirfile");
    std::fs::create_dir_all(d2.join("autocomp.cfg")).unwrap();
    assert_eq!(autocomp_user_cfg(true, &d2), Loaded::Bad);
}
