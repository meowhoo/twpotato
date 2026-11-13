//! P0 plausibility checks for the TSF `GetTextExt` rectangle (CARET-P0-plan).
//!
//! Some hosts (CUAS / system-caret-only controls) answer `GetTextExt` with a non-empty, on-screen but wrong rectangle
//! that the monitor-only check in `edit.rs` cannot see. These pure checks reject such rectangles so the caller falls
//! through to the next layer. All coordinates share the host thread's (possibly DPI-virtualized) space, so the
//! tolerance unit `u = 24 * dpi / 96` is in the same pixels as the rectangles.

use windows::Win32::Foundation::RECT;

#[derive(Clone, Copy)]
pub struct Mon {
    pub monitor: RECT,
    pub work: RECT,
}

#[derive(Clone, Copy)]
pub struct Host {
    pub root: isize,
    pub rect: RECT,
    /// The host has a title bar (`WS_CAPTION`); unknown counts as false.
    pub captioned: bool,
}

/// The last `text_ext` that passed every check and was used; only for the stale-position test, never a position source.
#[derive(Clone, Copy)]
pub struct Memo {
    pub root: isize,
    pub text: RECT,
    pub host: RECT,
}

pub struct Inputs<'a> {
    pub text: RECT,
    /// Visible, non-minimized, non-empty top-level host window; `None` skips the host-based checks.
    pub host: Option<Host>,
    /// Same-thread system caret in screen coordinates plus the root window of `hwndCaret`.
    pub sys_caret: Option<(isize, RECT)>,
    pub monitors: &'a [Mon],
    pub dpi: u32,
    pub prev: Option<Memo>,
    /// Client area of the host top-level window in screen coordinates; `None` skips the client-area check.
    pub host_client: Option<RECT>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Reject {
    OutsideHost,
    OutsideClient,
    HostTopLeft,
    Corner,
    SysCaretConflict,
    Stale,
}

impl Reject {
    pub fn name(self) -> &'static str {
        match self {
            Reject::OutsideHost => "outside_host",
            Reject::OutsideClient => "outside_client",
            Reject::HostTopLeft => "host_topleft",
            Reject::Corner => "corner",
            Reject::SysCaretConflict => "syscaret_conflict",
            Reject::Stale => "stale",
        }
    }
}

/// Vertical text_ext / system caret gap (in lines) beyond which the two are in conflict. 4 lines = 96 px at 96 dpi:
/// the measured wrong rectangle was 116 px off, a classic Notepad pair 6 px; wrapped compositions longer than this
/// only degrade to the system caret (error bounded by the same limit).
pub const CONFLICT_LINES: i64 = 4;

/// One text line at 96 dpi (matches the probe's N = 24 px).
pub fn unit(dpi: u32) -> i32 {
    let dpi = if dpi == 0 { 96 } else { dpi.min(1000) };
    (24 * dpi as i32 + 48) / 96
}

fn same(a: &RECT, b: &RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

fn inflate(r: &RECT, d: i32) -> RECT {
    RECT {
        left: r.left.saturating_sub(d),
        top: r.top.saturating_sub(d),
        right: r.right.saturating_add(d),
        bottom: r.bottom.saturating_add(d),
    }
}

/// Inclusive overlap, so zero-width carets still intersect.
fn touches(a: &RECT, b: &RECT) -> bool {
    a.left <= b.right && b.left <= a.right && a.top <= b.bottom && b.top <= a.bottom
}

/// The centre of `t` lies in `r` (edges included; computed in i64, no overflow).
fn centre_inside(r: &RECT, t: &RECT) -> bool {
    let (cx2, cy2) = (t.left as i64 + t.right as i64, t.top as i64 + t.bottom as i64);
    2 * r.left as i64 <= cx2 && cx2 <= 2 * r.right as i64 && 2 * r.top as i64 <= cy2 && cy2 <= 2 * r.bottom as i64
}

fn vertical_gap(a: &RECT, b: &RECT) -> i64 {
    let (at, ab, bt, bb) = (a.top as i64, a.bottom as i64, b.top as i64, b.bottom as i64);
    (at - bb).max(bt - ab).max(0)
}

/// A real title bar: `WS_CAPTION` set and the client area starts clearly below the window top (custom-frame windows keep
/// the style but extend the client area over the frame, see `WM_NCCALCSIZE`).
pub fn has_title_bar(style_caption: bool, client_top_offset: i32, dpi: u32) -> bool {
    style_caption && 2 * client_top_offset as i64 > unit(dpi) as i64
}

/// A single-line-sized rectangle glued to the top-left corner of a captioned window (outer rectangle, so a real
/// client-area caret is at least a frame plus title bar away). Measured bad CUAS answer: offset (+3,-1), 36x50 at 288 dpi;
/// legit minima there: Notepad (73,200), Edge (416,86); multi-line WPF (27,-22) is excluded by the height limit.
fn host_top_left(t: &RECT, h: &Host, u: i32) -> bool {
    let height = t.bottom as i64 - t.top as i64;
    h.captioned
        && 2 * height <= 3 * u as i64
        && 2 * (t.left as i64 - h.rect.left as i64) <= u as i64
        && 2 * (t.top as i64 - h.rect.top as i64) <= u as i64
}

fn on_corner(t: &RECT, monitors: &[Mon]) -> bool {
    monitors.iter().any(|m| {
        [m.monitor, m.work].iter().any(|s| {
            (t.left == s.left && t.top == s.top) || (t.right == s.right && t.bottom == s.bottom)
        })
    })
}

/// The system caret counts as a referee only when it belongs to the host and lies inside it.
fn referee(i: &Inputs, u: i32) -> Option<RECT> {
    let (host, (root, c)) = (i.host?, i.sys_caret?);
    (root == host.root
        && c.right >= c.left
        && c.bottom > c.top
        && touches(&inflate(&host.rect, u), &c))
    .then_some(c)
}

/// Trace line explaining the referee decision (no text content).
pub fn describe(i: &Inputs) -> String {
    let u = unit(i.dpi);
    let host = i.host;
    let (has, same_root, inside, gap) = match i.sys_caret {
        None => (false, false, false, -1),
        Some((root, c)) => (
            true,
            host.is_some_and(|h| h.root == root),
            host.is_some_and(|h| touches(&inflate(&h.rect, u), &c)),
            vertical_gap(&i.text, &c),
        ),
    };
    format!(
        "caret.referee host={} captioned={} topleft={} syscaret={} same_root={} inside_host={} eligible={} gap={} limit={} dpi={}",
        host.is_some(),
        host.is_some_and(|h| h.captioned),
        host.is_some_and(|h| host_top_left(&i.text, &h, u)),
        has,
        same_root,
        inside,
        referee(i, u).is_some(),
        gap,
        CONFLICT_LINES * u as i64,
        i.dpi
    )
}

pub fn check_text_ext(i: &Inputs) -> Result<(), Reject> {
    let u = unit(i.dpi);
    let t = &i.text;
    // 1. Outside the host top-level window (Weasel `enhanced_position`; our measured FAR case).
    if let Some(h) = i.host
        && !touches(&inflate(&h.rect, u), t)
    {
        return Err(Reject::OutsideHost);
    }
    // 1b. Glued to the top-left of a captioned host: independent of the system caret.
    if let Some(h) = i.host
        && host_top_left(t, &h, u)
    {
        return Err(Reject::HostTopLeft);
    }
    let referee = referee(i, u);
    // 1c. Centre outside the host's client area while the trustworthy system caret is inside it (CARET-MINTTY, Codex
    //     r1-r3 P2: a host that overhangs the screen origin -- maximized on a left monitor, or dragged across the
    //     primary's left and/or top edge -- still touches the bogus origin rectangle through its frame or title bar,
    //     possibly overlapping the client area by a few pixels). Real text, even a tall multi-line rectangle cut by the
    //     screen edge, has its centre in the client area; text mostly scrolled out of view only falls back to the system
    //     caret, which is inside the client area by construction.
    if let (Some(client), Some(c)) = (i.host_client, referee.as_ref())
        && touches(&client, c)
        && !centre_inside(&client, t)
    {
        return Err(Reject::OutsideClient);
    }
    let agrees = |c: &RECT| vertical_gap(t, c) <= CONFLICT_LINES * u as i64;
    // 2. Exact monitor / work-area corner (includes the origin), unless the system caret corroborates it.
    if on_corner(t, i.monitors) && !referee.as_ref().is_some_and(agrees) {
        return Err(Reject::Corner);
    }
    // 3. Vertical conflict with a trustworthy system caret. Horizontal is not compared: the caret usually sits at the
    //    end of a (possibly wrapped) composition while GetTextExt reports its start.
    if referee.as_ref().is_some_and(|c| !agrees(c)) {
        return Err(Reject::SysCaretConflict);
    }
    // 4. Identical rectangle although the same-size host window was translated.
    if let (Some(h), Some(p)) = (i.host, i.prev)
        && p.root == h.root
        && same(&p.text, t)
        && (p.host.right - p.host.left, p.host.bottom - p.host.top)
            == (h.rect.right - h.rect.left, h.rect.bottom - h.rect.top)
        && (p.host.left, p.host.top) != (h.rect.left, h.rect.top)
    {
        return Err(Reject::Stale);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn r(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }
    const SCREEN: Mon = Mon { monitor: r(0, 0, 3036, 1613), work: r(0, 0, 3036, 1573) };
    const ROOT: isize = 0x1234;

    fn base(text: RECT, host: RECT) -> Inputs<'static> {
        static MONS: [Mon; 1] = [SCREEN];
        Inputs {
            text,
            host: Some(Host { root: ROOT, rect: host, captioned: false }),
            sys_caret: None,
            monitors: &MONS,
            dpi: 96,
            prev: None,
            host_client: None,
        }
    }

    #[test]
    fn far_case_text_ext_outside_host_with_valid_system_caret() {
        // Measured: text_ext 39,27,75,84 while the window sits at 9,1035,768,1416 and the caret at (393,1368).
        let mut i = base(r(39, 27, 75, 84), r(9, 1035, 768, 1416));
        i.sys_caret = Some((ROOT, r(393, 1340, 394, 1368)));
        assert_eq!(check_text_ext(&i), Err(Reject::OutsideHost));
    }

    #[test]
    fn far_case_inside_host_is_caught_by_system_caret_conflict() {
        // Same wrong rectangle but the window happens to cover it: only the system caret can tell.
        let mut i = base(r(39, 27, 75, 84), r(0, 0, 768, 1416));
        i.sys_caret = Some((ROOT, r(393, 1340, 394, 1368)));
        assert_eq!(check_text_ext(&i), Err(Reject::SysCaretConflict));
    }

    #[test]
    fn classic_notepad_keeps_text_ext_when_both_agree_within_6px() {
        let mut i = base(r(83, 210, 94, 252), r(40, 100, 800, 700));
        i.sys_caret = Some((ROOT, r(83, 210, 84, 250)));
        assert_eq!(check_text_ext(&i), Ok(()));
    }

    #[test]
    fn wrapped_long_composition_and_wide_horizontal_gap_are_not_conflicts() {
        let host = r(0, 0, 1000, 800);
        for caret in [r(900, 210, 901, 234), r(80, 210 + 3 * 24, 81, 234 + 3 * 24)] {
            let mut i = base(r(80, 210, 100, 234), host);
            i.sys_caret = Some((ROOT, caret));
            assert_eq!(check_text_ext(&i), Ok(()), "{caret:?}");
        }
        let mut i = base(r(80, 210, 100, 234), host);
        i.sys_caret = Some((ROOT, r(80, 210 + 6 * 24, 81, 234 + 6 * 24)));
        assert_eq!(check_text_ext(&i), Err(Reject::SysCaretConflict));
    }

    #[test]
    fn measured_normal_case_bad_rect_in_top_left_of_host_is_a_conflict() {
        // Sandbox win32-syscaret normal: text_ext 13,9,49,59 inside the host, system caret 148,175,150,199 (116 px apart).
        // The DPI-unaware host reports dpi 96 (logical pixels), and a DPI-aware 300% host the same rectangles at 288.
        let mut i = base(r(13, 9, 49, 59), r(0, 0, 768, 600));
        i.sys_caret = Some((ROOT, r(148, 175, 150, 199)));
        assert_eq!(check_text_ext(&i), Err(Reject::SysCaretConflict));
        assert!(describe(&i).contains("eligible=true gap=116 limit=96"));
        // The same pair stays trusted for a DPI-aware 300% host, where 116 px is under two lines: bounded error.
        i.dpi = 288;
        assert_eq!(check_text_ext(&i), Ok(()));
    }

    fn captioned(text: RECT, host: RECT, dpi: u32) -> Inputs<'static> {
        let mut i = base(text, host);
        i.host = Some(Host { root: ROOT, rect: host, captioned: true });
        i.dpi = dpi;
        i
    }

    #[test]
    fn title_bar_needs_client_area_below_window_top() {
        assert!(has_title_bar(true, 31, 96));
        assert!(!has_title_bar(true, 0, 96)); // custom frame: client area covers the whole window
        assert!(!has_title_bar(true, 12, 96));
        assert!(!has_title_bar(false, 31, 96));
        assert!(has_title_bar(true, 80, 288));
        assert!(!has_title_bar(true, 36, 288));
    }

    #[test]
    fn host_top_left_anchor_rule() {
        // 288 dpi (u = 72); host window outer rectangle at (200,100).
        let host = r(200, 100, 1400, 900);
        let at = |dx, dy, w, h| r(200 + dx, 100 + dy, 200 + dx + w, 100 + dy + h);
        // Measured bad rectangle: offset (+3,-1), 36x50.
        assert_eq!(check_text_ext(&captioned(at(3, -1, 36, 50), host, 288)), Err(Reject::HostTopLeft));
        // Legit single lines: Notepad (73,200), Edge without UIA (416,86), and a WPF multi-line (27,-22) that is 5.3 units tall.
        assert_eq!(check_text_ext(&captioned(at(73, 200, 20, 50), host, 288)), Ok(()));
        assert_eq!(check_text_ext(&captioned(at(416, 86, 20, 50), host, 288)), Ok(()));
        assert_eq!(check_text_ext(&captioned(at(27, -22, 300, 382), host, 288)), Ok(()));
        // Height just over 1.5 units is not rejected, exactly 1.5 units is.
        assert_eq!(check_text_ext(&captioned(at(3, -1, 36, 109), host, 288)), Ok(()));
        assert_eq!(check_text_ext(&captioned(at(3, -1, 36, 108), host, 288)), Err(Reject::HostTopLeft));
        // Left / top limits are 0.5 unit (36 px here).
        assert_eq!(check_text_ext(&captioned(at(36, 36, 36, 50), host, 288)), Err(Reject::HostTopLeft));
        assert_eq!(check_text_ext(&captioned(at(37, 0, 36, 50), host, 288)), Ok(()));
        assert_eq!(check_text_ext(&captioned(at(0, 37, 36, 50), host, 288)), Ok(()));
        // Hosts without WS_CAPTION (or unknown style) never use the rule.
        assert_eq!(check_text_ext(&base(at(3, -1, 36, 50), host)), Ok(()));
        // 96 dpi (u = 24): limits 12 px, height 36 px.
        let host96 = r(50, 50, 450, 350);
        let at96 = |dx, dy, w, h| r(50 + dx, 50 + dy, 50 + dx + w, 50 + dy + h);
        assert_eq!(check_text_ext(&captioned(at96(1, 0, 12, 17), host96, 96)), Err(Reject::HostTopLeft));
        assert_eq!(check_text_ext(&captioned(at96(24, 67, 8, 17), host96, 96)), Ok(()));
        assert_eq!(check_text_ext(&captioned(at96(1, 0, 12, 37), host96, 96)), Ok(()));
        assert!(describe(&captioned(at96(1, 0, 12, 17), host96, 96)).contains("captioned=true topleft=true"));
    }

    #[test]
    fn origin_and_corner_rectangles_are_rejected() {
        let wide = r(-10, -10, 3100, 1700);
        for t in [
            r(0, 0, 1, 20),
            r(0, 0, 40, 20),
            r(3000, 1590, 3036, 1613), // bottom-right of monitor
            r(3000, 1550, 3036, 1573), // bottom-right of work area
        ] {
            assert_eq!(check_text_ext(&base(t, wide)), Err(Reject::Corner), "{t:?}");
        }
        // Non-corner rectangle with the same host is fine.
        assert_eq!(check_text_ext(&base(r(1, 0, 40, 20), wide)), Ok(()));
    }

    #[test]
    fn corner_is_accepted_only_when_the_system_caret_corroborates() {
        let mut i = base(r(0, 0, 1, 20), r(-10, -10, 800, 600));
        i.sys_caret = Some((ROOT, r(0, 4, 1, 24)));
        assert_eq!(check_text_ext(&i), Ok(()));
        i.sys_caret = Some((ROOT + 1, r(0, 4, 1, 24))); // another root window: not a referee
        assert_eq!(check_text_ext(&i), Err(Reject::Corner));
    }

    #[test]
    fn missing_host_skips_host_checks_and_system_caret_needs_a_host() {
        let mut i = base(r(500, 500, 520, 520), r(0, 0, 100, 100));
        assert_eq!(check_text_ext(&i), Err(Reject::OutsideHost));
        i.host = None;
        assert_eq!(check_text_ext(&i), Ok(()));
        i.sys_caret = Some((ROOT, r(2000, 1500, 2001, 1520)));
        assert_eq!(check_text_ext(&i), Ok(()));
    }

    #[test]
    fn system_caret_outside_host_is_not_a_referee() {
        let mut i = base(r(100, 100, 110, 120), r(0, 0, 800, 600));
        i.sys_caret = Some((ROOT, r(2000, 1500, 2001, 1520)));
        assert_eq!(check_text_ext(&i), Ok(()));
    }

    #[test]
    fn multi_monitor_negative_coordinates() {
        let mons = [
            Mon { monitor: r(-1920, 0, 0, 1080), work: r(-1920, 0, 0, 1040) },
            SCREEN,
        ];
        let mk = |text, host| Inputs {
            text,
            host: Some(Host { root: ROOT, rect: host, captioned: false }),
            sys_caret: None,
            monitors: &mons,
            dpi: 96,
            prev: None,
            host_client: None,
        };
        let host = r(-1500, 200, -700, 800);
        assert_eq!(check_text_ext(&mk(r(-1200, 400, -1190, 424), host)), Ok(()));
        // The origin of the left monitor is a corner of that monitor.
        assert_eq!(
            check_text_ext(&mk(r(-1920, 0, -1919, 20), r(-1950, -10, 100, 1100))),
            Err(Reject::Corner)
        );
        // Wrong rectangle on the primary monitor while the host lives on the left one.
        assert_eq!(check_text_ext(&mk(r(39, 27, 75, 84), host)), Err(Reject::OutsideHost));
    }

    #[test]
    fn dpi_scaling_of_the_conflict_threshold() {
        let host = r(0, 0, 3000, 1600);
        let text = r(100, 100, 110, 160);
        let caret = |dy| r(100, 160 + dy, 101, 160 + dy + 60);
        // 96 dpi: u = 24, limit 96. 288 dpi: u = 72, limit 288.
        let mut i = base(text, host);
        i.sys_caret = Some((ROOT, caret(200)));
        assert_eq!(check_text_ext(&i), Err(Reject::SysCaretConflict));
        i.dpi = 288;
        assert_eq!(check_text_ext(&i), Ok(()));
        i.sys_caret = Some((ROOT, caret(400)));
        assert_eq!(check_text_ext(&i), Err(Reject::SysCaretConflict));
        assert_eq!(unit(0), 24);
        assert_eq!(unit(144), 36);
        assert_eq!(unit(288), 72);
    }

    #[test]
    fn host_tolerance_scales_with_dpi() {
        let host = r(1000, 1000, 1500, 1500);
        let text = r(1000 - 40, 1200, 1000 - 30, 1220); // 30..40 px left of the window
        let mut i = base(text, host);
        assert_eq!(check_text_ext(&i), Err(Reject::OutsideHost)); // u = 24
        i.dpi = 192; // u = 48
        assert_eq!(check_text_ext(&i), Ok(()));
    }

    #[test]
    fn stale_rectangle_after_pure_translation_only() {
        let text = r(300, 300, 310, 324);
        let mut i = base(text, r(100, 100, 900, 700));
        i.prev = Some(Memo { root: ROOT, text, host: r(100, 100, 900, 700) });
        assert_eq!(check_text_ext(&i), Ok(())); // host unchanged
        i.host = Some(Host { root: ROOT, rect: r(100, 100, 1000, 800), captioned: false }); // resized from the right/bottom edge
        assert_eq!(check_text_ext(&i), Ok(()));
        i.host = Some(Host { root: ROOT, rect: r(120, 140, 920, 740), captioned: false }); // moved, same size
        assert_eq!(check_text_ext(&i), Err(Reject::Stale));
        i.host = Some(Host { root: ROOT + 1, rect: r(120, 140, 920, 740), captioned: false }); // different window
        assert_eq!(check_text_ext(&i), Ok(()));
        i.host = Some(Host { root: ROOT, rect: r(120, 140, 920, 740), captioned: false });
        i.text = r(320, 340, 330, 364); // rectangle moved with the window
        assert_eq!(check_text_ext(&i), Ok(()));
    }

    /// CARET-MINTTY measured (Sandbox, 288 dpi): mintty's composition GetTextExt is 13,9,49,59 wherever the window is.
    const MINTTY_BAD: RECT = r(13, 9, 49, 59);

    #[test]
    fn mintty_bad_rectangle_is_rejected_where_the_window_is() {
        // Window at (900,500): outside the host.
        let i = captioned(MINTTY_BAD, r(900, 500, 1660, 880), 288);
        assert_eq!(check_text_ext(&i), Err(Reject::OutsideHost));
        // Window at (10,10) and maximized (-18,-18): glued to the top-left of a captioned host.
        assert_eq!(check_text_ext(&captioned(MINTTY_BAD, r(10, 10, 770, 390), 288)), Err(Reject::HostTopLeft));
        assert_eq!(check_text_ext(&captioned(MINTTY_BAD, r(-18, -18, 3054, 1487), 288)), Err(Reject::HostTopLeft));
        // The real caret after the fallback (system caret 249,96 in the (10,10) window) would itself pass.
        assert_eq!(check_text_ext(&captioned(r(249, 96, 258, 134), r(10, 10, 770, 390), 288)), Ok(()));
    }

    #[test]
    fn outside_client_rejects_the_origin_rectangle_for_hosts_overhanging_the_origin() {
        // Codex r1 P2: host maximized on a left monitor overhangs the primary one by its invisible frame.
        let mons = [Mon { monitor: r(-3036, 0, 0, 1613), work: r(-3036, 0, 0, 1573) }, SCREEN];
        let mut i = captioned(r(13, 9, 203, 61), r(-3054, -18, 18, 1591), 288);
        i.monitors = Box::leak(Box::new(mons));
        i.sys_caret = Some((ROOT, r(-2800, 130, -2799, 180)));
        assert_eq!(check_text_ext(&i), Ok(())); // P0 alone accepted it
        i.host_client = Some(r(-3036, 69, 0, 1573));
        assert_eq!(check_text_ext(&i), Err(Reject::OutsideClient));
        // Real text inside that client area, also a zero-width caret exactly on its right edge, passes.
        i.text = r(-2800, 130, -2790, 180);
        assert_eq!(check_text_ext(&i), Ok(()));
        i.text = r(0, 130, 0, 180);
        assert_eq!(check_text_ext(&i), Ok(()));
        // Codex r2 P2: window dragged across the primary's left edge; the title bar covers the origin rectangle.
        let mut j = captioned(MINTTY_BAD, r(-100, 10, 660, 390), 288);
        j.sys_caret = Some((ROOT, r(139, 96, 148, 134)));
        assert_eq!(check_text_ext(&j), Ok(())); // P0 alone accepted it
        j.host_client = Some(r(-82, 85, 642, 372));
        assert_eq!(check_text_ext(&j), Err(Reject::OutsideClient));
        // Without a system caret inside the client area there is nothing better: the check does not fire.
        j.sys_caret = None;
        assert_eq!(check_text_ext(&j), Ok(()));
        // Codex r3 P2: across the left and top edges; the bogus rectangle overlaps the client area by 4 px.
        let mut k = captioned(MINTTY_BAD, r(-100, -20, 660, 360), 288);
        k.sys_caret = Some((ROOT, r(139, 66, 148, 104)));
        assert_eq!(check_text_ext(&k), Ok(())); // P0 alone accepted it
        k.host_client = Some(r(-82, 55, 642, 342));
        assert_eq!(check_text_ext(&k), Err(Reject::OutsideClient));
        // The real first-line caret of that window passes.
        k.text = r(139, 66, 148, 104);
        assert_eq!(check_text_ext(&k), Ok(()));
    }

    #[test]
    fn outside_client_keeps_tall_rectangles_cut_by_the_screen_edge_and_text_inside_the_client() {
        // Codex r2 P2: multi-line WPF-like rectangle whose top is above the screen still overlaps the client area.
        let mut i = captioned(r(227, -12, 527, 370), r(200, 10, 1400, 810), 288);
        i.host_client = Some(r(218, 85, 1382, 792));
        i.sys_caret = Some((ROOT, r(527, 330, 528, 370)));
        assert_eq!(check_text_ext(&i), Ok(()));
        // Ordinary single line inside the client area.
        i.text = r(300, 300, 336, 350);
        assert_eq!(check_text_ext(&i), Ok(()));
        // A system caret outside the client area (not a referee for this check) never triggers it.
        let mut k = base(r(5, 5, 15, 29), r(0, 0, 900, 700));
        k.host_client = Some(r(8, 40, 892, 692));
        k.sys_caret = Some((ROOT, r(890, 2, 891, 30)));
        assert_eq!(check_text_ext(&k), Ok(()));
    }
}
