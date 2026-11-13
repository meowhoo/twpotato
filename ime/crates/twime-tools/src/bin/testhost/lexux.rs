//! Explicit-only LEXUX shell scenarios. Fixtures are read into memory; no text is logged.
//! TH_LEXUX_CASES supplies the frozen step expectations. TH_LEXUX_LAT supplies
//! the frozen latency cases. The runner starts separate on/off engine deployments
//! in alternating order (three rounds), passing TH_LEXUX_ROUND and TH_LEXUX_ON.
use super::*;
use serde::Deserialize;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};
use windows::{
    Win32::{
        Foundation::{E_FAIL, E_POINTER, RECT},
        UI::TextServices::*,
    },
    core::{BOOL, GUID, Interface, Result, implement},
};

pub const WM_SNAPSHOT: u32 = WM_APP + 10;
pub const WM_CATEGORY: u32 = WM_APP + 11;
static KEYS_DONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn key_done() { KEYS_DONE.fetch_add(1, Ordering::SeqCst); }
pub fn category_matches(want: bool) -> bool { category_present().is_ok_and(|p|p==want) }
const HINT_GUID: GUID = GUID::from_u128(0x05ab532e_2f4e_48d5_9c40_b05ba1d5a306);
static SUPPRESS: AtomicBool = AtomicBool::new(false);
static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
thread_local! {
    static OBSERVED: RefCell<Observed> = RefCell::new(Observed::default());
    static SUBSCRIPTION: RefCell<Option<(ITfSource, u32, ITfUIElementSink)>> = const { RefCell::new(None) };
}
#[derive(Default)]
struct Observed {
    active: BTreeSet<u32>,
    begin: u32,
    update: u32,
    end: u32,
    errors: u32,
    text: Option<String>,
}
#[derive(Default, Clone)]
struct Snapshot {
    valid: bool,
    document: String,
    marked: Vec<bool>,
    tooltip: Option<String>,
    window_name: Option<String>,
    uia_name: Option<String>,
    hint_hwnd: isize,
    visible: bool,
    position_ok: bool,
    popup_open: bool,
    begin: u32,
    update: u32,
    end: u32,
    errors: u32,
    active: usize,
}
#[implement(ITfUIElementSink)]
struct Observer {
    mgr: ITfUIElementMgr,
}
impl Observer {
    fn tooltip(&self, id: u32) -> Option<String> {
        unsafe {
            self.mgr
                .GetUIElement(id)
                .ok()?
                .cast::<ITfToolTipUIElement>()
                .ok()?
                .GetString()
                .ok()
                .map(|s| s.to_string())
        }
    }
}
impl ITfUIElementSink_Impl for Observer_Impl {
    fn BeginUIElement(&self, id: u32, show: *mut BOOL) -> Result<()> {
        if show.is_null() {
            return Err(E_POINTER.into());
        }
        if let Some(text) = self.tooltip(id) {
            OBSERVED.with(|o| {
                let mut o = o.borrow_mut();
                o.begin += 1;
                if !o.active.insert(id) {
                    o.errors += 1;
                }
                o.text = Some(text);
            });
            if SUPPRESS.load(Ordering::SeqCst) {
                unsafe {
                    *show = BOOL(0);
                }
            }
        }
        Ok(())
    }
    fn UpdateUIElement(&self, id: u32) -> Result<()> {
        if let Some(text) = self.tooltip(id) {
            OBSERVED.with(|o| {
                let mut o = o.borrow_mut();
                o.update += 1;
                if !o.active.contains(&id) {
                    o.errors += 1;
                }
                o.text = Some(text);
            });
        }
        Ok(())
    }
    fn EndUIElement(&self, id: u32) -> Result<()> {
        OBSERVED.with(|o| {
            let mut o = o.borrow_mut();
            if o.active.remove(&id) {
                o.end += 1;
                o.text = None;
            }
        });
        Ok(())
    }
}
pub fn attach(tm: &ITfThreadMgr) -> Result<()> {
    unsafe {
        let source: ITfSource = tm.cast()?;
        let observer: ITfUIElementSink = Observer { mgr: tm.cast()? }.into();
        let cookie = source.AdviseSink(&ITfUIElementSink::IID, &observer)?;
        SUBSCRIPTION.with(|s| *s.borrow_mut() = Some((source, cookie, observer)));
    }
    Ok(())
}
pub fn detach() {
    if let Some((source, cookie, _)) = SUBSCRIPTION.with(|s| s.borrow_mut().take()) {
        let _ = unsafe { source.UnadviseSink(cookie) };
    }
}
unsafe fn range_text(range: &ITfRange, ec: u32) -> Result<String> {
    let r = unsafe { range.Clone()? };
    let mut text = Vec::new();
    loop {
        let mut b = [0u16; 256];
        let mut n = 0;
        unsafe {
            r.GetText(ec, TF_TF_MOVESTART, &mut b, &mut n)?;
        }
        text.extend_from_slice(&b[..n as usize]);
        if n < b.len() as u32 {
            break;
        }
    }
    Ok(String::from_utf16_lossy(&text))
}
fn window_snapshot(s: &mut Snapshot) {
    use windows::Win32::UI::{
        WindowsAndMessaging::*,
    };
    unsafe {
        s.popup_open = FindWindowW(w!("TwImeCandidate"), None).is_ok_and(|h| {
            let mut pid = 0;
            GetWindowThreadProcessId(h, Some(&mut pid));
            pid == std::process::id() && IsWindowVisible(h).as_bool()
        });
        let Ok(h) = FindWindowW(w!("TwImeLexuxHint"), None) else {
            return;
        };
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid != std::process::id() {
            return;
        }
        s.visible = IsWindowVisible(h).as_bool();
        let mut b = vec![0u16; GetWindowTextLengthW(h).max(0) as usize + 1];
        let n = GetWindowTextW(h, &mut b);
        s.window_name = Some(String::from_utf16_lossy(&b[..n as usize]));
        // Only the HWND crosses to the worker; no UIA COM object leaves its apartment.
        s.hint_hwnd = h.0 as isize;
    }
}

fn position_matches(composition: RECT, popup: RECT, work: RECT) -> bool {
    popup.left >= work.left && popup.right <= work.right
        && popup.top >= work.top && popup.bottom <= work.bottom
        && (popup.top >= composition.bottom || popup.bottom <= composition.top
            || popup.left >= composition.right || popup.right <= composition.left)
}

struct WorkerMta;
impl WorkerMta {
    fn initialize() -> Result<Self> {
        use windows::Win32::System::Com::COINIT_MULTITHREADED;
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()?; }
        Ok(Self)
    }
}
impl Drop for WorkerMta {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize(); }
    }
}
fn uia_name_on_worker(h: isize) -> Option<String> {
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
    if h == 0 {
        return None;
    }
    unsafe {
        let automation: Result<IUIAutomation> =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        automation
            .and_then(|a| a.ElementFromHandle(Ui::h(h)))
            .and_then(|e| e.CurrentName())
            .ok()
            .map(|v| v.to_string())
    }
}

pub fn snapshot_on_ui() {
    let mut snap = Snapshot::default();
    HOST_TM.with(|h| {
        let Some((tm, tid)) = h.borrow().clone() else {
            return;
        };
        let Ok(ctx) = (unsafe { tm.GetFocus().and_then(|d| d.GetTop()) }) else {
            return;
        };
        let output = std::rc::Rc::new(RefCell::new(Snapshot::default()));
        let out = output.clone();
        let ctx2 = ctx.clone();
        let es: ITfEditSession = HostEs {
            f: RefCell::new(Some(Box::new(move |ec| {
                let read = (|| -> Result<Snapshot> {
                    unsafe {
                        let all = ctx2.GetStart(ec)?;
                        let end = ctx2.GetEnd(ec)?;
                        all.ShiftEndToRange(ec, &end, TF_ANCHOR_END)?;
                        let document = range_text(&all, ec)?;
                        let mut marked = vec![false; document.encode_utf16().count()];
                        let prop = ctx2.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                        let mut ranges = None;
                        prop.EnumRanges(ec, &mut ranges, &all)?;
                        let ranges = ranges.ok_or(windows::core::Error::from(E_FAIL))?;
                        let cat: ITfCategoryMgr =
                            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
                        loop {
                            let mut rs = [None];
                            let mut n = 0;
                            ranges.Next(&mut rs, &mut n)?;
                            if n == 0 {
                                break;
                            }
                            let r = rs[0].as_ref().ok_or(windows::core::Error::from(E_FAIL))?;
                            let value = prop.GetValue(ec, r)?;
                            let Ok(atom) = i32::try_from(&value) else {
                                continue;
                            };
                            if cat.GetGUID(atom as u32).ok() != Some(HINT_GUID) {
                                continue;
                            }
                            let prefix = all.Clone()?;
                            prefix.ShiftEndToRange(ec, r, TF_ANCHOR_START)?;
                            let start = range_text(&prefix, ec)?.encode_utf16().count();
                            let len = range_text(r, ec)?.encode_utf16().count();
                            if start + len > marked.len() {
                                return Err(E_FAIL.into());
                            }
                            marked[start..start + len].fill(true);
                        }
                        let mut position_ok = false;
                        if let Ok(cc) = ctx2.cast::<ITfContextComposition>() {
                            if let Ok(e) = cc.EnumCompositions() {
                                let mut c = [None];
                                let mut n = 0;
                                if e.Next(&mut c, &mut n).is_ok() && n == 1 {
                                    if let Some(c) = &c[0] {
                                        if let (Ok(v), Ok(r)) = (ctx2.GetActiveView(), c.GetRange())
                                        {
                                            let mut rect = RECT::default();
                                            let mut clipped = BOOL(0);
                                            if v.GetTextExt(ec, &r, &mut rect, &mut clipped).is_ok()
                                            {
                                                use windows::Win32::UI::WindowsAndMessaging::{
                                                    FindWindowW, GetWindowRect,
                                                };
                                                if let Ok(h) =
                                                    FindWindowW(w!("TwImeLexuxHint"), None)
                                                {
                                                    let mut wr = RECT::default();
                                                    if GetWindowRect(h, &mut wr).is_ok() {
                                                        use windows::Win32::Graphics::Gdi::{
                                                            GetMonitorInfoW, MonitorFromPoint,
                                                            MONITORINFO, MONITOR_DEFAULTTONEAREST,
                                                        };
                                                        let monitor = MonitorFromPoint(
                                                            windows::Win32::Foundation::POINT {
                                                                x: rect.left, y: rect.bottom.saturating_sub(1),
                                                            }, MONITOR_DEFAULTTONEAREST);
                                                        let mut mi = MONITORINFO {
                                                            cbSize: size_of::<MONITORINFO>() as u32,
                                                            ..Default::default()
                                                        };
                                                        position_ok = GetMonitorInfoW(monitor, &mut mi).as_bool()
                                                            && position_matches(rect, wr, mi.rcWork);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(Snapshot {
                            valid: true,
                            document,
                            marked,
                            position_ok,
                            ..Default::default()
                        })
                    }
                })();
                if let Ok(s) = read {
                    *out.borrow_mut() = s;
                }
            }))),
        }
        .into();
        if unsafe { ctx.RequestEditSession(tid, &es, TF_ES_SYNC | TF_ES_READ) }
            .is_ok_and(|h| h.is_ok())
        {
            snap = output.borrow().clone();
        }
    });
    OBSERVED.with(|o| {
        let o = o.borrow();
        snap.begin = o.begin;
        snap.update = o.update;
        snap.end = o.end;
        snap.errors = o.errors;
        snap.active = o.active.len();
        snap.tooltip = o.text.clone();
    });
    window_snapshot(&mut snap);
    *SNAPSHOT.lock().unwrap() = Some(snap);
}
fn snapshot(ui: Ui) -> Snapshot {
    unsafe {
        SendMessageW(Ui::h(ui.main), WM_SNAPSHOT, None, None);
    }
    // Release the snapshot mutex before UIA may call back into the pumping UI thread.
    let mut snap = SNAPSHOT.lock().unwrap().take().unwrap_or_default();
    snap.uia_name = uia_name_on_worker(snap.hint_hwnd);
    snap
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedHint {
    options: Vec<String>,
    current: usize,
    #[serde(default)]
    auto: bool,
}
impl ExpectedHint {
    fn text(&self) -> String {
        twime_proto::LexuxHint { options: self.options.clone(), current: self.current, auto: self.auto, hide_first: false }.text()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    /// One key name, one Unicode keyboard glyph, or a shell action.
    input: String,
    document: String,
    ranges: Vec<(usize, usize)>,
    hint: Option<ExpectedHint>,
    #[serde(default)]
    suppressed: bool,
    #[serde(default)]
    popup: Option<PopupExpected>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PopupExpected {
    open: bool,
    /// Cumulative TSF Begin, End, active counts at this step.
    counts: [u32; 3],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    steps: Vec<Step>,
    #[serde(default)]
    auto: bool,
    #[serde(default)]
    options: usize,
    #[serde(default)]
    scenarios: Vec<Scenario>,
}
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Scenario {
    Underline,
    Disappear,
    Move,
    Commit,
    SameText,
    ApplyFail,
    Position,
    Candidates,
    Focus,
    UiLifecycle,
    Suppress,
    Uia,
}
fn functional_coverage(cases: &[Case]) -> bool {
    [
        Scenario::Underline,
        Scenario::Disappear,
        Scenario::Move,
        Scenario::Commit,
        Scenario::SameText,
        Scenario::ApplyFail,
        Scenario::Position,
        Scenario::Candidates,
        Scenario::Focus,
        Scenario::UiLifecycle,
        Scenario::Suppress,
        Scenario::Uia,
    ]
    .iter()
    .all(|s| cases.iter().any(|c| c.scenarios.contains(s)))
}
fn key_name(s: &str) -> Option<Key> {
    match s {
        "backslash" => Some(Key::Backslash),
        "enter" => Some(Key::Enter),
        "escape" => Some(Key::Esc),
        "space" => Some(Key::Space),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "backspace" => Some(Key::Backspace),
        "delete" => Some(Key::Delete),
        "down" => Some(Key::Down),
        "up" => Some(Key::Up),
        _ => {
            let mut c = s.chars();
            let k = key_of(c.next()?);
            if c.next().is_some() { None } else { k }
        }
    }
}
fn marks(n: usize, ranges: &[(usize, usize)]) -> Option<Vec<bool>> {
    let mut out = vec![false; n];
    for &(start, len) in ranges {
        let end = start.checked_add(len)?;
        if end > n {
            return None;
        }
        out[start..end].fill(true);
    }
    Some(out)
}
fn matches(s: &Snapshot, step: &Step) -> bool {
    let blocked = step.popup.as_ref().is_some_and(|p| p.open);
    let shown = step.hint.is_some() && !step.suppressed && !blocked;
    let text = step.hint.as_ref().filter(|_| !blocked).map(ExpectedHint::text);
    s.valid
        && s.document == step.document
        && marks(s.marked.len(), &step.ranges).is_some_and(|m| m == s.marked)
        && s.tooltip == text
        && s.visible == shown
        && (!shown || (s.position_ok && s.window_name == text && s.uia_name == text))
        && s.errors == 0
        && s.begin == s.end + s.active as u32
        && step.popup.as_ref().is_none_or(|p| s.popup_open == p.open
            && [s.begin, s.end, s.active as u32] == p.counts)
}
fn action(ui: Ui, input: &str) -> bool {
    match input {
        "focus-b" => unsafe {
            SendMessageW(
                Ui::h(ui.main),
                WM_FOCUS_EDIT,
                Some(WPARAM(ui.b as usize)),
                None,
            );
            true
        },
        "focus-a" => focus_a(ui),
        "suppress-on" => {
            SUPPRESS.store(true, Ordering::SeqCst);
            true
        }
        "suppress-off" => {
            SUPPRESS.store(false, Ordering::SeqCst);
            true
        }
        "snapshot" => true,
        _ => {
            if let Some(k) = key_name(input) {
                send_vk(vk_of(k));
                true
            } else {
                false
            }
        }
    }
}
enum StepOutcome {
    Key(f64),
    Control,
}
impl StepOutcome {
    fn sample_ms(&self, swap_only: bool, is_swap: bool) -> Option<f64> {
        match self {
            Self::Key(ms) if !swap_only || is_swap => Some(*ms),
            _ => None,
        }
    }
}
fn key_count(cases: &[Case]) -> usize {
    cases.iter().flat_map(|c| &c.steps)
        .filter(|s| key_name(&s.input).is_some()).count()
}
fn completed_key(before: u64, done: u64) -> bool {
    done > before
}
fn ended(s: &Snapshot) -> bool {
    s.valid && s.begin == s.end && s.active == 0 && s.errors == 0
        && s.tooltip.is_none() && !s.visible
}
fn wait_for_completion(before: Option<u64>, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if before.is_none_or(|n| completed_key(n, KEYS_DONE.load(Ordering::SeqCst))) && ready() {
            return true;
        }
        if Instant::now() > deadline || aborted() {
            return false;
        }
        std::thread::yield_now();
    }
}
fn run_step(ui: Ui, step: &Step) -> Option<StepOutcome> {
    // The timer starts immediately before SendInput. Completion includes host
    // text, GUID_PROP_ATTRIBUTE, TSF tooltip and Win32/UIA presentation.
    let before = KEYS_DONE.load(Ordering::SeqCst);
    let needs_key = key_name(&step.input).is_some();
    let start = needs_key.then(Instant::now);
    if !action(ui, &step.input) {
        return None;
    }
    wait_for_completion(needs_key.then_some(before), || matches(&snapshot(ui), step))
        .then(|| match start {
            Some(start) => StepOutcome::Key(start.elapsed().as_secs_f64() * 1000.),
            None => StepOutcome::Control,
        })
}
fn read_cases(var: &str) -> Option<Vec<Case>> {
    let p = std::env::var_os(var)?;
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}
fn category_present() -> Result<bool> {
    unsafe {
        let cat: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        let en = cat.EnumCategoriesInItem(&GUID::from_u128(ids::CLSID_TIP))?;
        loop {
            let mut g = [GUID::zeroed()];
            let mut n = 0;
            en.Next(&mut g, Some(&mut n)).ok()?;
            if n == 0 {
                return Ok(false);
            }
            if g[0] == GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER {
                return Ok(true);
            }
        }
    }
}
pub fn run(ui: Ui, names: &[String], report: &mut Report) -> bool {
    let selected = names.iter().any(|n| n.starts_with("lexux-"));
    if !selected {
        return false;
    }
    // This runner owns no windows. Keep MTA alive until all UIA interfaces are dropped.
    let Ok(_mta) = WorkerMta::initialize() else {
        report.check("lexux-mta", false, || "count=1".into());
        return true;
    };
    if names
        .iter()
        .any(|n| n == "lexux-category-present" || n == "lexux-category-absent")
    {
        let want = names.iter().any(|n| n == "lexux-category-present");
        report.check(
            "lexux-category",
            unsafe { SendMessageW(Ui::h(ui.main), WM_CATEGORY, Some(WPARAM(want as usize)), None).0 == 1 },
            || "count=1".into(),
        );
    }
    if names.iter().any(|n| n == "lexux-tsf") {
        let Some(cases) = read_cases("TH_LEXUX_CASES") else {
            report.check("lexux-fixtures", false, || "count=0".into());
            return true;
        };
        if !functional_coverage(&cases) {
            report.check("lexux-coverage", false, || "count=0".into());
            return true;
        }
        for case in cases {
            clear(ui);
            let mut ok = true;
            for step in &case.steps {
                ok &= run_step(ui, step).is_some();
                if !ok {
                    break;
                }
            }
            report.check("lexux-tsf", ok, || "count=1".into());
        }
        let before = KEYS_DONE.load(Ordering::SeqCst);
        press_fast(Key::Esc);
        let mut s = Snapshot::default();
        let finished = wait_for_completion(Some(before), || {
            s = snapshot(ui);
            ended(&s)
        });
        report.check(
            "lexux-ui-pairs",
            finished,
            || {
                format!(
                    "begin={} update={} end={} active={} errors={}",
                    s.begin, s.update, s.end, s.active, s.errors
                )
            },
        );
    }
    if names
        .iter()
        .any(|n| matches!(n.as_str(), "lexux-typing" | "lexux-lexdense" | "lexux-swap"))
    {
        latency(ui, names, report);
    }
    SUPPRESS.store(false, Ordering::SeqCst);
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Latency {
    typing: Vec<Case>,
    lexdense: Vec<Case>,
    swap: Vec<Case>,
    enabled: bool,
    automatic: bool,
    round: usize,
}
#[derive(serde::Serialize, Deserialize)]
struct LatStat {
    scenario: String,
    enabled: bool,
    round: usize,
    samples: usize,
    p99_ms: f64,
    failures: usize,
}
fn valid_comparison(stats: &[LatStat]) -> bool {
    ["lexux-typing", "lexux-lexdense", "lexux-swap"]
        .iter()
        .all(|name| {
            let rows: Vec<_> = stats.iter().filter(|s| s.scenario == *name).collect();
            rows.len() == 6
                && rows.iter().enumerate().all(|(i, s)| {
                    s.round == i / 2
                        && s.enabled == (i % 2 == 1)
                        && s.samples > 0
                        && s.failures == 0
                        && s.p99_ms.is_finite()
                        && s.p99_ms >= 0.
                })
                && (0..3).all(|r| {
                    if *name == "lexux-swap" {
                        rows[2 * r + 1].p99_ms <= 10.
                    } else {
                        rows[2 * r + 1].p99_ms - rows[2 * r].p99_ms <= 2.
                    }
                })
        })
}
/// Pure aggregation: this entry returns before creating windows or activating TSF.
pub fn compare_file(path: &str) -> bool {
    let stats = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<LatStat>>(&b).ok());
    let Some(stats) = stats else {
        println!("LEXUX_COMPARE count=0 pass=0");
        return false;
    };
    let ok = valid_comparison(&stats);
    println!("LEXUX_COMPARE count={} pass={}", stats.len(), ok as u8);
    ok
}
fn p99(samples: &mut [f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f64::total_cmp);
    Some(samples[((samples.len() * 99).div_ceil(100) - 1).min(samples.len() - 1)])
}
fn latency(ui: Ui, names: &[String], report: &mut Report) {
    let manifest = std::env::var_os("TH_LEXUX_LAT")
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Latency>(&b).ok());
    let Some(m) = manifest else {
        report.check("lexux-lat-fixtures", false, || "count=0".into());
        return;
    };
    if m.round >= 3
        || key_count(&m.typing) != 500
        || m.lexdense.len() < 5
        || m.swap.len() < 20
        || (m.automatic && m.swap[..20].iter().filter(|c| c.auto).count() < 5)
    {
        report.check("lexux-lat-counts", false, || "count=0".into());
        return;
    }
    for (name, cases, limit) in [
        ("lexux-typing", m.typing.as_slice(), usize::MAX),
        ("lexux-lexdense", m.lexdense.as_slice(), 5),
        ("lexux-swap", m.swap.as_slice(), 20),
    ] {
        if !names.iter().any(|n| n == name) {
            continue;
        }
        let cases = &cases[..cases.len().min(limit)];
        let mut samples = vec![];
        let mut failures = 0;
        for i in 0..20 + cases.len() {
            let warm = i < 20;
            let case = &cases[if warm { i % cases.len() } else { i - 20 }];
            clear(ui);
            let mut swaps = 0;
            for step in &case.steps {
                let is_swap = step.input == "backslash";
                if is_swap {
                    swaps += 1;
                }
                match run_step(ui, step) {
                    Some(outcome) => {
                        if !warm {
                            if let Some(ms) = outcome.sample_ms(name == "lexux-swap", is_swap) {
                                samples.push(ms);
                            }
                        }
                    }
                    None => {
                        failures += 1;
                        break;
                    }
                }
            }
            if name == "lexux-swap" && m.enabled && (case.options < 2 || swaps != case.options) {
                failures += 1;
            }
        }
        let q = p99(&mut samples);
        report.check(
            "lexux-lat",
            failures == 0
                && q.is_some()
                && (name != "lexux-swap" || !m.enabled || q.unwrap() <= 10.),
            || format!("failures={failures}"),
        );
        println!(
            "LEXUX_LAT scenario={} enabled={} round={} samples={} p99_ms={:.6} failures={}",
            name,
            m.enabled as u8,
            m.round,
            samples.len(),
            q.unwrap_or(0.),
            failures
        );
        let row = LatStat {
            scenario: name.into(),
            enabled: m.enabled,
            round: m.round,
            samples: samples.len(),
            p99_ms: q.unwrap_or(0.),
            failures,
        };
        // JSON contains statistics only. The external runner collects six alternating
        // invocations into an array for lexux-lat-compare; inputs remain frozen.
        if let Ok(json) = serde_json::to_string(&row) {
            println!("LEXUX_STAT {json}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Copy of data/lexux/hint-window-cases.json (numbers only), kept next to the test so the public tree builds.
    const WINDOW_CASES: &str = include_str!("hint-window-cases.json");
    #[test]
    fn window_position_expectations_cover_below_flip_right_and_negative_monitors() {
        let cases: serde_json::Value = serde_json::from_str(WINDOW_CASES).unwrap();
        let rect = |v: &serde_json::Value| RECT {
            left: v[0].as_i64().unwrap() as i32, top: v[1].as_i64().unwrap() as i32,
            right: v[2].as_i64().unwrap() as i32, bottom: v[3].as_i64().unwrap() as i32,
        };
        for case in cases["position"].as_array().unwrap() {
            let (comp, popup, work) = (rect(&case["composition"]), rect(&case["expected"]), rect(&case["work"]));
            assert!(position_matches(comp, popup, work));
            assert!(!position_matches(comp, comp, work));
            let outside = RECT { left: work.right, right: work.right + 200, ..popup };
            assert!(!position_matches(comp, outside, work));
        }
    }
    fn popup_expectations(index: usize) {
        let cases: serde_json::Value = serde_json::from_str(WINDOW_CASES).unwrap();
        let steps: Vec<Step> = serde_json::from_value(cases["lifecycle"][index]["steps"].clone()).unwrap();
        for step in steps {
            let p = step.popup.as_ref().unwrap();
            let text = step.hint.as_ref().filter(|_| !p.open).map(ExpectedHint::text);
            let mut snap = Snapshot {
                valid: true, position_ok: true, popup_open: p.open,
                visible: text.is_some(), tooltip: text.clone(), window_name: text.clone(), uia_name: text,
                begin: p.counts[0], end: p.counts[1], active: p.counts[2] as usize,
                ..Default::default()
            };
            assert!(matches(&snap, &step));
            if p.open || step.hint.is_none() {
                snap.visible = true;
                assert!(!matches(&snap, &step));
                snap.visible = false;
            }
            snap.begin += 1;
            snap.end += 1;
            assert!(!matches(&snap, &step));
        }
    }
    #[test]
    fn candidates_hide_and_reappear_expectations() { popup_expectations(0); }
    #[test]
    fn symbols_hide_and_reappear_expectations() { popup_expectations(1); }
    #[test]
    fn candidates_close_after_expiry_expectations() { popup_expectations(2); }
    #[test]
    fn symbols_close_after_expiry_expectations() { popup_expectations(3); }

    #[test]
    fn full_cycle_expectations_include_original_and_current_for_all_interfaces() {
        let mut step: Step = serde_json::from_str(
            r#"{"input":"snapshot","document":"123","ranges":[],"hint":{"options":["45","67","89"],"current":0}}"#,
        ).unwrap();
        let mut s = Snapshot {
            valid: true, document: "123".into(), marked: vec![false; 3],
            visible: true, position_ok: true, begin: 1, active: 1,
            ..Default::default()
        };
        for (current, expected) in [
            (0, "按反斜線切換：\\ 45（原）（目前）　\\ 67　\\ 89"),
            (1, "按反斜線切換：\\ 45（原）　\\ 67（目前）　\\ 89"),
            (2, "按反斜線切換：\\ 45（原）　\\ 67　\\ 89（目前）"),
            (0, "按反斜線切換：\\ 45（原）（目前）　\\ 67　\\ 89"),
        ] {
            step.hint.as_mut().unwrap().current = current;
            assert!(step.hint.as_ref().unwrap().text() == expected);
            s.tooltip = Some(expected.into());
            s.window_name = Some(expected.into());
            s.uia_name = Some(expected.into());
            assert!(matches(&s, &step));
            step.hint.as_mut().unwrap().current = (current + 1) % 3;
            assert!(!matches(&s, &step));
        }
    }
    #[test]
    fn lifecycle_requires_processed_escape_and_ended_hint() {
        assert!(!completed_key(7, 7));
        assert!(completed_key(7, 8));
        let mut s = Snapshot {
            valid: true, begin: 1, active: 1, visible: true,
            tooltip: Some("1".into()), ..Default::default()
        };
        assert!(!ended(&s));
        s.end = 1;
        s.active = 0;
        assert!(!ended(&s));
        s.visible = false;
        s.tooltip = None;
        assert!(ended(&s));
        s.errors = 1;
        assert!(!ended(&s));
        s.errors = 0;
        s.valid = false;
        assert!(!ended(&s));
    }
    #[test]
    fn latency_counts_and_samples_only_keys() {
        let inputs = ["snapshot", "focus-a", "focus-b", "suppress-on", "suppress-off"];
        let mut case = Case {
            steps: vec![], auto: false, options: 0, scenarios: vec![],
        };
        for i in 0..500 {
            case.steps.push(Step {
                input: inputs[i % inputs.len()].into(), document: String::new(),
                ranges: vec![], hint: None, suppressed: false, popup: None,
            });
        }
        assert!(key_count(std::slice::from_ref(&case)) == 0);
        for _ in 0..500 {
            case.steps.push(Step {
                input: "space".into(), document: String::new(),
                ranges: vec![], hint: None, suppressed: false, popup: None,
            });
        }
        assert!(key_count(&[case]) == 500);
        assert!(StepOutcome::Control.sample_ms(false, false).is_none());
        assert!(StepOutcome::Control.sample_ms(true, true).is_none());
        assert!(StepOutcome::Key(2.).sample_ms(false, false) == Some(2.));
        assert!(StepOutcome::Key(2.).sample_ms(true, false).is_none());
        assert!(StepOutcome::Key(2.).sample_ms(true, true) == Some(2.));
    }
    #[test]
    fn utf16_marks_clear_move_and_bounds() {
        assert!(marks(5, &[(1, 2)]) == Some(vec![false, true, true, false, false]));
        assert!(marks(5, &[]).unwrap().iter().all(|x| !*x));
        assert!(marks(5, &[(4, 2)]).is_none());
        assert!(marks(5, &[(usize::MAX, 2)]).is_none());
    }
    #[test]
    fn snapshot_requires_attributes_uia_and_lifecycle() {
        let step = Step {
            input: "snapshot".into(),
            document: "123".into(),
            ranges: vec![(1, 2)],
            hint: Some(ExpectedHint { options: vec!["45".into(), "67".into()], current: 1, auto: false }),
            suppressed: false, popup: None,
        };
        let mut s = Snapshot {
            valid: true,
            document: "123".into(),
            marked: vec![false, true, true],
            tooltip: Some(step.hint.as_ref().unwrap().text()),
            window_name: Some(step.hint.as_ref().unwrap().text()),
            uia_name: Some(step.hint.as_ref().unwrap().text()),
            visible: true,
            position_ok: true,
            begin: 1,
            active: 1,
            ..Default::default()
        };
        assert!(matches(&s, &step));
        s.tooltip = Some("0".into());
        assert!(!matches(&s, &step));
        s.tooltip = Some(step.hint.as_ref().unwrap().text());
        s.uia_name = Some("0".into());
        assert!(!matches(&s, &step));
        s.uia_name = Some(step.hint.as_ref().unwrap().text());
        s.marked[0] = true;
        assert!(!matches(&s, &step));
        s.marked[0] = false;
        s.uia_name = None;
        assert!(!matches(&s, &step));
    }
    #[test]
    fn quantile_nearest_rank_and_empty() {
        assert!(p99(&mut []).is_none());
        assert!(p99(&mut [2., 1., 3.]) == Some(3.));
    }
    #[test]
    fn key_parser_rejects_unknown_actions() {
        assert!(key_name("backslash") == Some(Key::Backslash));
        assert!(key_name("unknown").is_none());
    }
    #[test]
    fn three_round_comparison_enforces_order_thresholds_and_counts() {
        let mut rows = vec![];
        for name in ["lexux-typing", "lexux-lexdense", "lexux-swap"] {
            for i in 0..6 {
                rows.push(LatStat {
                    scenario: name.into(),
                    enabled: i % 2 == 1,
                    round: i / 2,
                    samples: 500,
                    p99_ms: if i % 2 == 0 { 1. } else { 2. },
                    failures: 0,
                });
            }
        }
        assert!(valid_comparison(&rows));
        rows[1].p99_ms = 4.;
        assert!(!valid_comparison(&rows));
        rows[1].p99_ms = 2.;
        rows[17].p99_ms = 11.;
        assert!(!valid_comparison(&rows));
        rows[17].p99_ms = 2.;
        rows[0].enabled = true;
        assert!(!valid_comparison(&rows));
        assert!(!valid_comparison(&[]));
    }
}

#[cfg(test)]
mod worker_mta_tests {
    use super::*;

    #[test]
    fn uia_worker_uses_mta() {
        use windows::Win32::System::Com::{APTTYPE, APTTYPEQUALIFIER, APTTYPE_MTA, CoGetApartmentType};
        let _mta = WorkerMta::initialize().expect("count=1");
        let mut apartment = APTTYPE::default();
        let mut qualifier = APTTYPEQUALIFIER::default();
        unsafe { CoGetApartmentType(&mut apartment, &mut qualifier).expect("count=1"); }
        assert_eq!(apartment, APTTYPE_MTA);
        assert!(uia_name_on_worker(0).is_none(), "count=1");
    }
}
