use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use twime_engined::{
    int8_setting, lexux_setting, repair_setting, short_setting, user_rules_setting,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
fn fixture(text: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/lexux/config-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("model.cfg"), text).unwrap();
    p
}

#[test]
fn absent_and_partial_thresholds_disable_lexux() {
    assert!(lexux_setting(None, None).unwrap().is_none());
    for cfg in [
        "int8=on\n",
        "int8=off\nlexux_a=2\n",
        "int8=off\nlexux_b=off\n",
        "int8=off\nlexux_w=on\n",
    ] {
        let p = fixture(cfg);
        assert!(lexux_setting(None, Some(&p)).unwrap().is_none());
    }
}

#[test]
fn thresholds_read_with_or_without_int8_override() {
    for flag in [None, Some(false), Some(true)] {
        for (b, w) in [("off", "on"), ("-2", "off"), ("2", "on")] {
            let p = fixture(&format!("int8=on\nlexux_a=2\nlexux_b={b}\nlexux_w={w}\n"));
            let c = lexux_setting(flag, Some(&p)).unwrap().unwrap();
            assert!(
                c.a == 2.
                    && c.b
                        == if b == "off" {
                            None
                        } else {
                            Some(b.parse().unwrap())
                        }
            );
            assert!(c.w == (w == "on"));
            assert!(int8_setting(flag, Some(&p)).is_ok());
            assert!(user_rules_setting(flag, Some(&p)).is_ok());
            assert!(short_setting(flag, Some(&p)).is_ok());
            assert!(repair_setting(flag, Some(&p)).is_ok());
        }
    }
}

#[test]
fn malformed_duplicate_and_nonfinite_settings_rejected() {
    for tail in [
        "lexux_a=nan\nlexux_b=off",
        "lexux_a=2\nlexux_b=3",
        "lexux_a=2\nlexux_b=inf",
        "lexux_a=2\nlexux_b=off\nlexux_w=yes",
        "lexux_a=2\nlexux_a=2",
        "lexux_b=off\nlexux_b=off",
        "lexux_w=on\nlexux_w=off",
    ] {
        let p = fixture(&format!("int8=on\n{tail}\n"));
        for flag in [None, Some(false)] {
            assert!(lexux_setting(flag, Some(&p)).is_err());
        }
    }
}

#[test]
fn explicit_int8_flag_skips_only_unrelated_invalid_lines() {
    let p = fixture("int8=invalid\nunknown=1\nlexux_a=4\nlexux_b=off\n");
    assert!(lexux_setting(None, Some(&p)).is_err());
    assert!(lexux_setting(Some(true), Some(&p)).unwrap().unwrap().a == 4.);
}

#[test]
fn pop_default_value_duplicate_and_int8_override_validation() {
    use twime_engined::autocomp_pop_setting;
    assert!(!autocomp_pop_setting(None,None).unwrap());
    for flag in [None,Some(true),Some(false)] {
        for (value,want) in [("on",true),("off",false)] {
            let p=fixture(&format!("int8=on\nautocomp_pop={value}\n"));
            assert_eq!(autocomp_pop_setting(flag,Some(&p)).unwrap(),want);
        }
        for tail in ["autocomp_pop=yes", "autocomp_pop=on\nautocomp_pop=off"] {
            let p=fixture(&format!("int8=on\n{tail}\n"));
            assert!(autocomp_pop_setting(flag,Some(&p)).is_err());
        }
    }
}
