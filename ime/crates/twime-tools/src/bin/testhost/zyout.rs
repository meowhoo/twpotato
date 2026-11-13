//! ZHUYIN-OUT (ZHUYIN-OUT-plan 6.3), named only (`zy-all` or `zy-<name>`), Sandbox only: scenarios of a text file
//! (`TH_ZY_SCEN`: name, Tab, keys) are typed into the host's edit control through the TIP (the direct-drive path of the other
//! scenarios: the host offers each key to the TIP's key sink first) and the document text and the composition are compared
//! code point by code point with the expectation file (`TH_ZY_EXPECT`: name, Tab, document text [, Tab, composition]).
//!
//! Keys: bopomofo symbols and tone marks (typed through the standard layout), `[SP] [ENTER] [DOWN] [UP] [ESC] [BS] [DEL] [END]
//! [HOME] [LEFT] [RIGHT] [PGDN] [PGUP] [TAB]`, `[S+ENTER]` (Shift+Enter), `[D1]`..`[D9]` (a number key of the candidate list),
//! `[K:a]` (a letter key typed raw),
//! `[SHOT]` (PNG of the host window and of the candidate window), `[WAIT]` (300 ms), `[FOCUS]` (focus the second edit and
//! back: the composition is committed through `Event::Focus(false)`), `[KILL]` (the engine process is terminated: the TIP's
//! fail-over commits the composition text). Lines starting with `#` are comments. In the expectation file `-` is an empty
//! document text / no composition, `*` skips the composition check, `?` in a text stands for any one character.
use super::*;
use std::path::PathBuf;
#[path = "pop_capture.rs"]
mod capture;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId, IsWindowVisible};

/// Refuses to run outside the Sandbox runner (as `pop::validate`).
pub fn validate(names: &[String]) -> Result<(), String> {
    if std::env::var("TH_ZY_SANDBOX").as_deref() != Ok("1") || std::env::var("USERNAME").as_deref() != Ok("WDAGUtilityAccount") {
        return Err("only the zyout Sandbox runner may enable zy-*".into());
    }
    if !out().is_dir() {
        return Err("TH_ZY_OUT directory required".into());
    }
    for v in ["TH_ZY_SCEN", "TH_ZY_EXPECT"] {
        if !std::env::var_os(v).map(PathBuf::from).is_some_and(|p| p.is_file()) {
            return Err(format!("{v} file required"));
        }
    }
    if names.iter().any(|n| n != "zy-all" && !n.starts_with("zy-")) {
        return Err("use zy-all or zy-<name> only".into());
    }
    Ok(())
}

fn out() -> PathBuf {
    std::env::var_os("TH_ZY_OUT").map(PathBuf::from).unwrap_or_default()
}

fn table(var: &str) -> Vec<(String, String)> {
    let text = std::env::var_os(var).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    text.trim_start_matches('\u{FEFF}')
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_string(), b.trim_end_matches('\r').to_string())))
        .collect()
}

fn window(class: &str) -> Option<HWND> {
    unsafe {
        let h = FindWindowW(windows::core::PCWSTR(wide(class).as_ptr()), None).ok()?;
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        (pid == std::process::id() && IsWindowVisible(h).as_bool()).then_some(h)
    }
}

fn shot(ui: Ui, tag: &str, r: &mut Report) {
    for (kind, h) in [("host", Some(Ui::h(ui.main))), ("candidate", window("TwImeCandidate"))] {
        if let Some(h) = h {
            let result = capture::save(h, &out().join(format!("{tag}-{kind}.png")));
            r.check(&format!("{tag}/png-{kind}"), result.is_ok(), || result.unwrap_err());
        }
    }
}

/// The code points of a text as `U+XXXX` (for a mismatch line).
fn points(s: &str) -> String {
    s.chars().map(|c| format!("U+{:04X}", c as u32)).collect::<Vec<_>>().join(" ")
}

/// Code point by code point equality, `?` in `want` matching any one character.
fn same(got: &str, want: &str) -> bool {
    let (g, w): (Vec<char>, Vec<char>) = (got.chars().collect(), want.chars().collect());
    g.len() == w.len() && g.iter().zip(&w).all(|(a, b)| *b == '?' || a == b)
}

/// Sends the keys of one scenario line.
fn drive(ui: Ui, scen: &str, keys: &str, r: &mut Report) {
    let mut shots = 0;
    let mut rest = keys;
    while let Some(c) = rest.chars().next() {
        if c == '[' {
            let end = rest.find(']').unwrap_or(rest.len() - 1);
            let tok = &rest[1..end];
            rest = &rest[end + 1..];
            match tok {
                "SP" => press(Key::Space),
                "ENTER" => press(Key::Enter),
                "DOWN" => press(Key::Down),
                "UP" => press(Key::Up),
                "ESC" => press(Key::Esc),
                "BS" => press(Key::Backspace),
                "DEL" => press(Key::Delete),
                "END" => press(Key::End),
                "HOME" => press(Key::Home),
                "LEFT" => press(Key::Left),
                "RIGHT" => press(Key::Right),
                "PGDN" => press(Key::PageDown),
                "PGUP" => press(Key::PageUp),
                "TAB" => press(Key::Tab),
                "S+ENTER" => press_shift(Key::Enter),
                "WAIT" => nap(300),
                "FOCUS" => {
                    focus_b_then_a(ui);
                    focus_a_again(ui);
                }
                "KILL" => {
                    r.check(&format!("{scen}: engine process found"), kill_engine(), || "no engine".into());
                    nap(150);
                }
                "SHOT" => {
                    nap(150);
                    shots += 1;
                    shot(ui, &format!("{scen}-{shots}"), r);
                }
                k if k.len() == 3 && k.starts_with("K:") && k.as_bytes()[2].is_ascii_lowercase() => press(Key::Letter(k.as_bytes()[2])),
                d if d.len() == 2 && d.starts_with('D') && d.as_bytes()[1].is_ascii_digit() => press(Key::Digit(d.as_bytes()[1] - b'0')),
                other => r.check(&format!("{scen}: key token [{other}]"), false, || "unknown token".into()),
            }
            nap(60);
        } else {
            let n = c.len_utf8();
            if c == '\u{3000}' || c == ' ' {
                press(Key::Space);
            } else if key_of(c).is_some() {
                typ(&rest[..n]);
            } else {
                r.check(&format!("{scen}: no key for U+{:04X}", c as u32), false, || "unknown".into());
            }
            rest = &rest[n..];
        }
    }
}

pub fn run(ui: Ui, names: &[String], r: &mut Report) -> bool {
    if !names.iter().any(|n| n.starts_with("zy-")) {
        return false;
    }
    let scen = table("TH_ZY_SCEN");
    let expect = table("TH_ZY_EXPECT");
    r.check("zy: scenarios found", !scen.is_empty(), || "empty TH_ZY_SCEN".into());
    // warm the shell connection, discard any input that raced activation
    typ("ㄅ");
    press(Key::Esc);
    clear(ui);
    for (name, keys) in &scen {
        if !names.iter().any(|n| n == "zy-all" || n.strip_prefix("zy-") == Some(name.as_str())) {
            continue;
        }
        let want = expect.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        let Some(want) = want else {
            r.check(&format!("zy-{name}: expectation present"), false, || "no line in TH_ZY_EXPECT".into());
            continue;
        };
        let (want_doc, want_comp) = match want.split_once('\t') {
            Some((d, c)) => (d.to_string(), c.to_string()),
            None => (want.clone(), "-".to_string()),
        };
        let norm = |s: &str| if s == "-" { String::new() } else { s.to_string() };
        clear(ui);
        press(Key::Esc);
        drive(ui, &format!("zy-{name}"), keys, r);
        nap(200);
        let doc = text_of(ui.a);
        let comp = comp_text(ui).unwrap_or_default();
        let want_doc = norm(&want_doc);
        r.check(&format!("zy-{name}: document text"), same(&doc, &want_doc), || format!("got {} want {}", points(&doc), points(&want_doc)));
        if !want_doc.contains('?') {
            r.check(&format!("zy-{name}: UTF-16 length"), doc.encode_utf16().count() == want_doc.encode_utf16().count(), || format!("{} want {}", doc.encode_utf16().count(), want_doc.encode_utf16().count()));
        }
        if want_comp != "*" {
            let want_comp = norm(&want_comp);
            r.check(&format!("zy-{name}: composition"), same(&comp, &want_comp), || format!("got {} want {}", points(&comp), points(&want_comp)));
        }
        // leave nothing behind for the next scenario
        press(Key::Esc);
        clear(ui);
        if engine_pid().is_none() {
            ready_engine();
            nap(700);
        }
    }
    true
}
