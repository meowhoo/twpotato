//! Explicit-only Sandbox P6 checks. Oracle uses a separate v7 session of the installed engine.
use super::*;
use std::{collections::BTreeSet, path::PathBuf};
#[path = "pop_capture.rs"]
mod capture;
use twime_proto::{Event, KeyEvent, Reply, Request, Update};
use windows::Win32::UI::WindowsAndMessaging::*;

const CASES: &[&str] = &["pop-idle", "pop-fast", "pop-digit", "pop-select", "pop-layout", "pop-dismiss"];
const FIXTURES: &[(&str, &str)] = &[("eryang", "ㄦˋㄧㄤˇ"), ("zhonghua", "ㄓㄨㄥ ㄏㄨㄚˊ")];

pub fn validate(names: &[String]) -> Result<(), String> {
    if std::env::var("TH_POP_SANDBOX").as_deref() != Ok("1")
        || std::env::var("USERNAME").as_deref() != Ok("WDAGUtilityAccount") {
        return Err("only the acpop-p6 Sandbox runner may enable pop-*".into());
    }
    if names.iter().any(|s| s != "pop-all" && !CASES.contains(&s.as_str())) {
        return Err("use pop-all or named pop-idle/fast/digit/select/layout/dismiss only".into());
    }
    if !out().is_dir() {
        return Err("TH_POP_OUT directory required".into());
    }
    Ok(())
}
fn out() -> PathBuf { std::env::var_os("TH_POP_OUT").map(PathBuf::from).unwrap_or_default() }
// LM order depends on the session context; numbering and membership must still match.
fn hint_options(name: &str, oracle: &[String]) -> Option<Vec<String>> {
    let body = name.strip_prefix("↓ 選取　")?;
    let q = oracle.len().min(9);
    if q == 0 { return None; }
    let mut actual = Vec::new();
    for (i, item) in body.split(' ').enumerate() {
        let value = item.strip_prefix(&format!("{}.", i + 1))?;
        if value.is_empty() { return None; }
        actual.push(value.to_owned());
    }
    let set: BTreeSet<_> = actual.iter().collect();
    // The TSF session ranks with its own left context (window's recent output, document text), the oracle session has
    // none: the LM may swap items near the top-q cut. Require the same count, distinct items, and at most two items
    // outside the oracle's top q (Sandbox run 20261030-132010: one swap per hint).
    let overlap = set.iter().filter(|x| oracle.iter().take(q).any(|o| o == **x)).count();
    (actual.len() == q && set.len() == q && overlap + 2 >= q)
        .then_some(actual)
}

fn accessible_candidate() -> Option<String> {
    use windows::Win32::{System::Com::{COINIT_MULTITHREADED, CoUninitialize}, UI::Accessibility::{CUIAutomation, IUIAutomation}};
    let h = window("TwImeCandidate")?;
    let title = text_of(h.0 as isize);
    if !title.is_empty() { return Some(title); }
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
        let name = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .and_then(|a| a.ElementFromHandle(h)).and_then(|e| e.CurrentName()).ok().map(|s| s.to_string());
        CoUninitialize();
        name.filter(|s| !s.is_empty())
    }
}
fn window(class: &str) -> Option<HWND> {
    unsafe {
        let h = FindWindowW(PCWSTR(wide(class).as_ptr()), None).ok()?;
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        (pid == std::process::id() && IsWindowVisible(h).as_bool()).then_some(h)
    }
}
fn hint() -> Option<String> {
    let h = window("TwImeLexuxHint")?;
    let mut b = [0u16; 8192];
    let n = unsafe { SendMessageW(h, WM_GETTEXT, Some(WPARAM(b.len())), Some(LPARAM(b.as_mut_ptr() as isize))) }.0;
    Some(String::from_utf16_lossy(&b[..(n.max(0) as usize).min(b.len())]))
}
fn shot(ui: Ui, tag: &str, r: &mut Report) {
    for (kind, h) in [("host", Some(Ui::h(ui.main))),
                      ("hint", window("TwImeLexuxHint")),
                      ("candidate", window("TwImeCandidate"))] {
        if let Some(h) = h {
            let result = capture::save(h, &out().join(format!("{tag}-{kind}.png")));
            r.check(&format!("{tag}/png-{kind}"), result.is_ok(), || result.unwrap_err());
        }
    }
}

struct Oracle { client: twime_winpipe::Client, seq: u32 }
impl Oracle {
    fn new() -> Result<Self, String> {
        let id = twime_winpipe::current_identity().map_err(|e| e.to_string())?;
        let pipe = std::env::var("TWIME_PIPE").unwrap_or_else(|_| twime_winpipe::pipe_name(&id));
        let client = twime_winpipe::Client::connect(&pipe, None, 500).map_err(|e| e.to_string())?;
        let req = Request { session: 0, seq: 0, ack: None, event: Event::Hello(7) };
        let reply = client.call(&req.encode(), 5000).map_err(|e| format!("{e:?}"))?;
        match Reply::decode(&reply) {
            Some(Reply::Hello { version: 7 } | Reply::HelloCaps { version: 7, .. }) => Ok(Self { client, seq: 0 }),
            _ => Err("installed engine does not negotiate v7".into()),
        }
    }
    fn call(&mut self, event: Event) -> Result<Update, String> {
        self.seq += 1;
        let req = Request { session: 6006, seq: self.seq, ack: None, event };
        let bytes = self.client.call(&req.encode(), 5000).map_err(|e| format!("{e:?}"))?;
        match Reply::decode(&bytes) {
            Some(Reply::Update { session: 6006, seq, update }) if seq == self.seq => Ok(update),
            _ => Err("unexpected oracle reply".into()),
        }
    }
    fn prepare(&mut self, phrase: &str) -> Result<(String, Vec<String>, String), String> {
        self.call(Event::Close)?;
        self.call(Event::Focus(true))?;
        let mut u = Update::default();
        for k in keys(phrase) { u = self.call(Event::Key(KeyEvent::press(k)))?; }
        let base = u.composition.as_ref().map(|c| c.text()).ok_or("no oracle composition")?;
        let h = self.call(Event::Idle(u.pop_gen))?.completion.ok_or("fixture has no completion")?;
        // Digit-without-Down must be checked against a fresh copy of the original composition.
        self.call(Event::Close)?;
        self.call(Event::Focus(true))?;
        for k in keys(phrase) { u = self.call(Event::Key(KeyEvent::press(k)))?; }
        self.call(Event::Idle(u.pop_gen))?;
        let digit = self.call(Event::Key(KeyEvent::press(Key::Digit(1))))?.composition.map(|c| c.text()).ok_or("digit lost composition")?;
        Ok((base, h.options, digit))
    }
}
impl Drop for Oracle { fn drop(&mut self) { let _ = self.call(Event::Close); } }
fn keys(s: &str) -> Vec<Key> { s.chars().map(|c| if c == ' ' { Key::Space } else { key_of(c).expect("fixture key") }).collect() }
// Timestamp immediately before SendInput of the last key; no typ() trailing sleeps.
fn type_phrase(s: &str) -> Instant {
    let ks = keys(s);
    let mut last = Instant::now();
    for k in ks { last = Instant::now(); send_vk(vk_of(k)); nap(25); }
    last
}
fn reset(ui: Ui) {
    unsafe {
        let _ = ShowWindow(Ui::h(ui.main), SW_SHOWNORMAL);
        let _ = MoveWindow(Ui::h(ui.main), 80, 80, 640, 260, true);
    }
    focus_a(ui); press(Key::Esc); press(Key::Esc); clear(ui);
}
fn absent_for(ms: u64) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(ms) {
        if hint().is_some() { return false; }
        nap(5);
    }
    true
}
fn skipped(tag: &str, reason: &str) { println!("NOT_RUN\t{tag}\t{reason}"); }

pub fn run(ui: Ui, names: &[String], r: &mut Report) -> bool {
    if !names.iter().any(|n| n.starts_with("pop-")) { return false; }
    let wanted: Vec<_> = CASES.iter().copied().filter(|c| names.iter().any(|n| n == "pop-all" || n == c)).collect();
    let mut oracle = match Oracle::new() {
        Ok(o) => o,
        Err(e) => {
            r.check("pop-oracle", false, || e.clone());
            for c in &wanted { skipped(c, &e); }
            return true;
        }
    };
    // Warm the shell connection, discard any input that raced activation.
    reset(ui);
    typ("ㄅ"); press(Key::Esc); clear(ui);
    let mut times = Vec::new();
    let mut round_p99 = Vec::new();
    for round in 1..=3 {
        let round_start = times.len();
        for &(fixture, phrase) in FIXTURES {
            let expected = oracle.prepare(phrase);
            let (base, options, digit) = match expected {
                Ok(v) => v,
                Err(e) => {
                    r.check(&format!("pop-oracle/{fixture}/{round}"), false, || e.clone());
                    for c in &wanted { skipped(&format!("{c}/{fixture}/{round}"), &e); }
                    continue;
                }
            };
            println!("INFO\tpop-oracle/{fixture}/{round}\tbase={base:?} options={options:?} digit={digit:?}");
            for case in &wanted {
                let tag = format!("{case}-{fixture}-r{round}");
                if aborted() { skipped(&tag, "foreground lost"); continue; }
                reset(ui);
                let sent = type_phrase(phrase);
                if *case == "pop-fast" {
                    // Continue before debounce with a pending bopomofo, then observe beyond the old deadline.
                    let before = hint().is_none();
                    let interval = sent.elapsed().as_millis();
                    send_vk(vk_of(Key::Digit(1)));
                    let hidden = wait_for(Duration::from_millis(120), || hint().is_none());
                    let stays = absent_for(350);
                    r.check(&tag, interval < 250 && before && hidden && stays && comp_text(ui).as_deref() == Some(&digit), || format!("interval={interval} before={before} hidden={hidden} stays={stays} comp={:?}", comp_text(ui)));
                    shot(ui, &format!("{tag}-continued"), r);
                    continue;
                }
                let mut visible_ms = 0.0;
                let visible = wait_for(Duration::from_secs(3), || {
                    if hint().is_some() { visible_ms = sent.elapsed().as_secs_f64()*1000.; true } else { false }
                });
                let name = hint();
                let actual_options = name.as_deref().and_then(|s| hint_options(s, &options));
                let valid = visible && visible_ms >= 250.0 && actual_options.is_some();
                println!("INFO\t{tag}\tlast_key_to_visible_ms={visible_ms:.3} name={name:?} (10ms polling + scheduling)");
                r.check(&format!("{tag}/visible"), valid && comp_text(ui).as_deref() == Some(&base), || format!("expected prefix/numbering and set={options:?} actual={name:?} elapsed={visible_ms:.3} comp={:?}", comp_text(ui)));
                shot(ui, &format!("{tag}-visible"), r);
                if !valid { skipped(&tag, "valid hint prerequisite failed"); continue; }
                let actual_options = actual_options.unwrap();
                times.push(visible_ms);
                match *case {
                    "pop-idle" => {},
                    "pop-digit" => {
                        press(Key::Digit(1));
                        let hidden = wait_for(Duration::from_millis(120), || hint().is_none());
                        r.check(&tag, hidden && absent_for(300) && comp_text(ui).as_deref() == Some(&digit) && window("TwImeCandidate").is_none(), || format!("comp={:?}, expected={digit:?}", comp_text(ui)));
                        shot(ui, &format!("{tag}-digit"), r);
                    },
                    "pop-select" | "pop-layout" => {
                        if *case == "pop-layout" {
                            let moved = unsafe { MoveWindow(Ui::h(ui.main), 180, 170, 640, 260, true).is_ok() };
                            nap(300);
                            r.check(&format!("{tag}/layout"), moved && hint() == name, || format!("moved={moved} before={name:?} after={:?}", hint()));
                            shot(ui, &format!("{tag}-layout"), r);
                        }
                        press(Key::Down);
                        let open = wait_for(Duration::from_secs(1), || window("TwImeCandidate").is_some());
                        r.check(&format!("{tag}/down"), open && hint().is_none(), || format!("open={open} hint={:?}", hint()));
                        let first = format!("{base}{}", actual_options[0]);
                        // release9 draws candidate text itself and has no candidate UIElement. Do not fabricate a label PASS.
                        match accessible_candidate() {
                            Some(label) => r.check(&format!("{tag}/candidate-name"), label.starts_with(&format!("1{first}")) || label.starts_with(&format!("1.{first}")), || format!("name={label:?}, expected first={first:?}")),
                            None if open => skipped(&format!("{tag}/candidate-name"), &format!("MANUAL_IMAGE {tag}-down-candidate.png; visually check item 1 = {first:?}; no readable UIA/WM_GETTEXT name")),
                            None => {},
                        }
                        // In-process PrintWindow only; never activate or launch a process between Down and Digit.
                        shot(ui, &format!("{tag}-down"), r);
                        if !open { skipped(&format!("{tag}/commit"), "candidate window missing"); continue; }
                        press(Key::Digit(1));
                        let expected = first;
                        r.check(&format!("{tag}/append"), comp_text(ui).as_deref() == Some(&expected), || format!("expected={expected:?} actual={:?}", comp_text(ui)));
                        shot(ui, &format!("{tag}-selected"), r);
                        press(Key::Enter); nap(100);
                        r.check(&format!("{tag}/commit"), text_of(ui.a) == expected && comp_text(ui).is_none(), || format!("expected={expected:?} actual={:?}", text_of(ui.a)));
                        shot(ui, &format!("{tag}-committed"), r);
                    },
                    "pop-dismiss" => {
                        press(Key::Esc);
                        let hidden = wait_for(Duration::from_millis(120), || hint().is_none());
                        r.check(&format!("{tag}/esc"), hidden && absent_for(300), || format!("hint={:?}", hint()));
                        shot(ui, &format!("{tag}-esc"), r);
                        unsafe { let _ = MoveWindow(Ui::h(ui.main), 180, 170, 640, 260, true); }
                        r.check(&format!("{tag}/no-revive"), absent_for(300), || format!("cancelled hint revived={:?}", hint()));
                        shot(ui, &format!("{tag}-cancelled-layout"), r);
                        reset(ui); type_phrase(phrase);
                        let shown = wait_for(Duration::from_secs(3), || hint().as_deref().and_then(|s| hint_options(s, &options)).is_some());
                        shot(ui, &format!("{tag}-before-blur"), r);
                        unsafe { SendMessageW(Ui::h(ui.main), WM_FOCUS_EDIT, Some(WPARAM(ui.b as usize)), None); }
                        let focused_b = unsafe { SendMessageW(Ui::h(ui.main), WM_QUERY_FOCUS, None, None).0 == ui.b };
                        let hidden = wait_for(Duration::from_millis(120), || hint().is_none());
                        r.check(&format!("{tag}/blur"), shown && focused_b && hidden && absent_for(300), || format!("shown={shown} focused_b={focused_b} hint={:?}", hint()));
                        shot(ui, &format!("{tag}-blur"), r);
                    },
                    _ => unreachable!(),
                }
            }
        }
        let mut sample: Vec<f64> = times[round_start..].to_vec();
        sample.sort_by(f64::total_cmp);
        if !sample.is_empty() {
            let p99 = sample[((sample.len() as f64 * 0.99) as usize).min(sample.len()-1)];
            round_p99.push(p99);
            println!("INFO\tpop-visible-round-{round}\tn={} p99_ms={p99:.3} (report only)", sample.len());
        }
    }
    reset(ui);
    times.sort_by(f64::total_cmp);
    if !times.is_empty() { println!("INFO\tpop-visible-latency\tn={} p99_ms={:.3} max_ms={:.3} (report only; small fixed fixture sample)", times.len(), times[((times.len() as f64 * 0.99) as usize).min(times.len()-1)], times[times.len()-1]); }
    if let Some(worst) = round_p99.iter().copied().reduce(f64::max) { println!("INFO\tpop-visible-worst-round-p99_ms={worst:.3}"); }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hint_contract_allows_reordering_and_two_context_differences_but_requires_numbering() {
        let oracle = vec!["化".into(), "化碳".into(), "化錳".into()];
        assert_eq!(hint_options("↓ 選取　1.化錳 2.化 3.化碳", &oracle).unwrap()[0], "化錳");
        assert!(hint_options("↓ 選取　1.化 2.化碳 3.化硫", &oracle).is_some());
        assert!(hint_options("↓ 選取　1.化 2.化鈦 3.化硫", &oracle).is_some());
        for bad in ["1.化 2.化碳 3.化錳", "↓ 選取　1.化 3.化碳 4.化錳",
                    "↓ 選取　1.化 2.化碳", "↓ 選取　1.化鉛 2.化鈦 3.化硫",
                    "↓ 選取　1.化 2.化碳 3.化碳", "↓ 選取　1.化 2.化碳 3."] {
            assert!(hint_options(bad, &oracle).is_none(), "{bad}");
        }
        assert!(hint_options("↓ 選取　", &[]).is_none());
        let many: Vec<_> = (1..=10).map(|i| format!("詞{i}")).collect();
        let numbered = |q| format!("↓ 選取　{}", (1..=q).map(|i| format!("{i}.詞{i}")).collect::<Vec<_>>().join(" "));
        assert!(hint_options(&numbered(9), &many).is_some());
        assert!(hint_options(&numbered(10), &many).is_none());
    }
    #[test]
    fn fixed_phrases_use_dachen_keys_and_finish_with_tone() {
        for (_, phrase) in FIXTURES { let k = keys(phrase); assert!(k.len() >= 4); assert!(matches!(k.last(), Some(Key::Digit(3 | 6)))); }
    }
}
