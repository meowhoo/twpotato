//! Candidate window: a small owned popup drawn with GDI in the host process.
//!
//! No activation (`WS_EX_NOACTIVATE`, `MA_NOACTIVATE`), owned by the host window so it stays above it, sized and
//! fonted from the host window's DPI (the IME never declares DPI awareness itself), placed under the composition and
//! kept inside the monitor work area. It only draws and reports clicks; selection logic lives in the engine.

use std::cell::RefCell;

use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            BeginPaint, InvalidateRect, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, EndPaint, FillRect, GetDC,
            GetMonitorInfoW, GetTextExtentPoint32W, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
            PAINTSTRUCT, PS_SOLID, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW, Rectangle,
        },
        UI::{
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, MA_NOACTIVATE, RegisterClassW,
                SW_HIDE, SW_SHOWNOACTIVATE, SetWindowPos, ShowWindow, SWP_NOACTIVATE, WM_ERASEBKGND, WM_LBUTTONDOWN,
                WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER, WNDCLASSW, SetTimer, KillTimer, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
                WS_BORDER, HWND_TOPMOST, WINDOW_STYLE,
            },
        },
    },
    core::{PCWSTR, w},
};

pub const CLASS_NAME: PCWSTR = w!("TwImeCandidate");
const HINT_CLASS: PCWSTR = w!("TwImeLexuxHint");
const LABEL_CLASS: PCWSTR = w!("TwImeLabel");

struct State {
    hwnd: HWND,
    items: Vec<String>,
    selected: usize,
    dpi: u32,
    /// left, top, right, bottom of each item (items wrap to further rows when the window would not fit the work area)
    cells: Vec<(i32, i32, i32, i32)>,
    /// Page indicator ("2/5"), drawn after the last item; not clickable.
    info: Option<((i32, i32, i32, i32), String)>,
    on_pick: Option<Box<dyn Fn(usize)>>,
    add: Option<(i32, i32, i32, i32)>,
    on_add: Option<Box<dyn Fn()>>,
    /// A bare label (the mode label): items are drawn without their number.
    plain: bool,
}

thread_local! {
    static CAND: RefCell<Option<State>> = const { RefCell::new(None) };
    static HINT: RefCell<Option<State>> = const { RefCell::new(None) };
    static FLASH: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_window<R>(hwnd: HWND, f: impl FnOnce(&RefCell<Option<State>>) -> R) -> R {
    if HINT.with(|c| c.borrow().as_ref().is_some_and(|s| s.hwnd == hwnd)) { return HINT.with(f); }
    let flash = FLASH.with(|c| c.borrow().as_ref().is_some_and(|s| s.hwnd == hwnd));
    if flash { FLASH.with(f) } else { CAND.with(f) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn font(dpi: u32) -> windows::Win32::Graphics::Gdi::HFONT {
    // 14 pt at the host's DPI.
    let h = -((14 * dpi as i32) / 72);
    unsafe { CreateFontW(h, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET, Default::default(), Default::default(), Default::default(), Default::default(), w!("Microsoft JhengHei UI")) }
}

fn module() -> HINSTANCE {
    HINSTANCE(crate::MODULE.load(std::sync::atomic::Ordering::SeqCst) as _)
}

fn ensure_class() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: module(), lpszClassName: CLASS_NAME, ..Default::default() };
        RegisterClassW(&wc);
        let wc = WNDCLASSW { lpszClassName: LABEL_CLASS, ..wc };
        RegisterClassW(&wc);
        RegisterClassW(&WNDCLASSW { lpszClassName: HINT_CLASS, ..wc });
    });
}

fn label(i: usize, s: &str, plain: bool) -> Vec<u16> {
    if plain { wide(s) } else { wide(&format!("{}{}", i + 1, s)) }
}

/// Result of packing the items into rows.
#[derive(Debug, PartialEq)]
pub(crate) struct Packed {
    pub w: i32,
    pub h: i32,
    pub cells: Vec<(i32, i32, i32, i32)>,
    pub info: Option<(i32, i32, i32, i32)>,
}

/// Packs cells of the given text widths left to right; a cell that would push the row past `max_w` starts a new row (a
/// row always takes at least one cell). The page indicator (`info_w` > 0) follows the last cell, on a new row if needed.
/// Pure, so it can be tested with injected widths.
pub(crate) fn pack(widths: &[i32], info_w: i32, line_h: i32, pad: i32, max_w: i32) -> Packed {
    let row_h = line_h + 2 * pad;
    let (mut x, mut row, mut right) = (pad, 0i32, 0i32);
    let mut cells = vec![];
    let place = |cw: i32, x: &mut i32, row: &mut i32| -> (i32, i32, i32, i32) {
        if *x > pad && *x + cw + pad > max_w {
            *x = pad;
            *row += 1;
        }
        let r = (*x, *row * row_h, *x + cw, (*row + 1) * row_h);
        *x += cw;
        r
    };
    for &tw in widths {
        let r = place(tw + 2 * pad, &mut x, &mut row);
        right = right.max(r.2);
        cells.push(r);
    }
    let info = (info_w > 0).then(|| {
        let r = place(info_w + 2 * pad, &mut x, &mut row);
        right = right.max(r.2);
        r
    });
    Packed { w: right + pad, h: (row + 1) * row_h, cells, info }
}

/// Append the page and action as ordinary packed cells, then take their rectangles out of the candidate list.
fn pack_with_add(widths: &[i32], info_w: i32, add_w: i32, line_h: i32, pad: i32, max_w: i32) -> (Packed, Option<(i32, i32, i32, i32)>) {
    if add_w == 0 {
        return (pack(widths, info_w, line_h, pad, max_w), None);
    }
    let mut all = widths.to_vec();
    if info_w > 0 { all.push(info_w); }
    all.push(add_w);
    let mut p = pack(&all, 0, line_h, pad, max_w);
    let add = p.cells.pop();
    if info_w > 0 { p.info = p.cells.pop(); }
    (p, add)
}

/// Size of the window for these items (also fills the cell rectangles). `max_w` is the usable width.
#[cfg(test)]
fn layout(hwnd: HWND, items: &[String], info: &str, dpi: u32, max_w: i32, plain: bool) -> Packed {
    layout_with_add(hwnd, items, info, dpi, max_w, plain, false).0
}

fn layout_with_add(hwnd: HWND, items: &[String], info: &str, dpi: u32, max_w: i32, plain: bool, add: bool) -> (Packed, Option<(i32, i32, i32, i32)>) {
    unsafe {
        let dc = GetDC(Some(hwnd));
        let f = font(dpi);
        let old = SelectObject(dc, HGDIOBJ(f.0));
        let pad = (6 * dpi as i32) / 96;
        let mut line_h = 0;
        let mut extent = |t: &[u16]| {
            let mut sz = SIZE::default();
            let _ = GetTextExtentPoint32W(dc, t, &mut sz);
            line_h = line_h.max(sz.cy);
            sz.cx
        };
        let widths: Vec<i32> = items.iter().enumerate().map(|(i, it)| extent(&label(i, it, plain))).collect();
        let info_w = if info.is_empty() { 0 } else { extent(&wide(info)) };
        let add_w = if add { extent(&wide("＋詞庫")) } else { 0 };
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(f.0));
        ReleaseDC(Some(hwnd), dc);
        pack_with_add(&widths, info_w, add_w, line_h, pad, max_w)
    }
}


/// Shows (or updates) the list below `caret` (screen coordinates of the composition), owned by `owner`.
pub fn show(owner: HWND, caret: RECT, items: &[String], selected: usize, page: Option<(usize, usize)>, on_pick: Box<dyn Fn(usize)>, on_add: Option<Box<dyn Fn()>>) {
    show_impl(owner, caret, items, selected, page, on_pick, on_add, false, false);
}

/// Independent mode/notice label, hidden after `ms` milliseconds or by `hide`; keeps the candidate list intact.
pub fn flash(owner: HWND, caret: RECT, text: &str, ms: u32) {
    show_impl(owner, caret, &[text.to_string()], 0, None, Box::new(|_| {}), None, true, false);
    let hwnd = FLASH.with(|c| c.borrow().as_ref().map(|s| s.hwnd));
    if let Some(h) = hwnd {
        unsafe {
            SetTimer(Some(h), FLASH_TIMER, ms, None);
        }
    }
}

const FLASH_TIMER: usize = 77;

/// Shared candidate / tooltip placement in the anchor monitor's work area.
/// Coordinates stay signed, including monitors above or left of the primary.
fn popup_position(caret: RECT, wa: RECT, w: i32, h: i32, dpi: u32) -> (i32, i32) {
    let gap = (2 * dpi as i32) / 96;
    let (mut x, mut y) = (caret.left, caret.bottom + gap);
    if y + h > wa.bottom { y = caret.top - h - gap; }
    if x + w > wa.right { x = wa.right - w; }
    (x.max(wa.left), y.max(wa.top))
}

fn monitor_anchor(caret: RECT, hint: bool) -> POINT {
    POINT { x: caret.left, y: if hint { caret.bottom.saturating_sub(1) } else { caret.bottom } }
}

/// Only hints avoid the composition after clamping; candidate placement stays unchanged.
fn hint_position(caret: RECT, wa: RECT, w: i32, h: i32, dpi: u32) -> Option<(i32, i32)> {
    let (x, y) = popup_position(caret, wa, w, h, dpi);
    let fits = |x: i32, y: i32| {
        x >= wa.left && y >= wa.top && x + w <= wa.right && y + h <= wa.bottom
            && (x + w <= caret.left || x >= caret.right || y + h <= caret.top || y >= caret.bottom)
    };
    if fits(x, y) { return Some((x, y)); }
    let gap = (2 * dpi as i32) / 96;
    let y = caret.top.min(wa.bottom - h).max(wa.top);
    [caret.right + gap, caret.left - w - gap].into_iter()
        .find(|&x| fits(x, y)).map(|x| (x, y))
}

fn show_impl(owner: HWND, mut caret: RECT, items: &[String], selected: usize, page: Option<(usize, usize)>, on_pick: Box<dyn Fn(usize)>, on_add: Option<Box<dyn Fn()>>, plain: bool, hint: bool) {
    ensure_class();
    unsafe {
        // A notice has its own window and sits below the candidate list when one is open.
        if plain && !hint && let Some(r) = rect() {
            caret.bottom = caret.bottom.max(r.bottom);
        }
        let slot = if hint { &HINT } else if plain { &FLASH } else { &CAND };
        let existing = slot.with(|c| c.borrow().as_ref().map(|s| s.hwnd));
        let hwnd = match existing {
            Some(h) => h,
            None => match CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | if hint { windows::Win32::UI::WindowsAndMessaging::WS_EX_TRANSPARENT } else { Default::default() },
                if hint { HINT_CLASS } else if plain { LABEL_CLASS } else { CLASS_NAME },
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0 | WS_BORDER.0),
                0,
                0,
                10,
                10,
                if owner.is_invalid() { None } else { Some(owner) },
                None,
                Some(module()),
                None,
            ) {
                Ok(h) => h,
                Err(_) => return,
            },
        };
        if hint {
            let name: Vec<u16> = items.concat().encode_utf16().chain(Some(0)).collect();
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(hwnd, PCWSTR(name.as_ptr()));
        }
        let dpi = if owner.is_invalid() { 96 } else { GetDpiForWindow(owner).max(96) };
        // Under the composition, moved up above it or left if it would leave the monitor; items wrap to more rows when the
        // work area is narrower than the single row would be.
        let mon = MonitorFromPoint(monitor_anchor(caret, hint), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        let info_text = page.map(|(p, n)| format!("{p}/{n}")).unwrap_or_default();
        let (p, add) = layout_with_add(hwnd, items, &info_text, dpi, (wa.right - wa.left).max(100), plain, on_add.is_some());
        if let Some((l, t, r, b)) = add {
            crate::trace::log(&format!("cands add_cell={l},{t},{r},{b}"));
        }
        let (w, h) = (p.w, p.h);
        let (cells, info) = (p.cells, p.info.map(|r| (r, info_text.clone())));
        let position = if hint { hint_position(caret, wa, w, h, dpi) } else { Some(popup_position(caret, wa, w, h, dpi)) };
        slot.with(|c| *c.borrow_mut() = Some(State { hwnd, items: items.to_vec(), selected, dpi, cells, info, on_pick: Some(on_pick), add, on_add, plain }));
        let Some((x, y)) = position else { hide_slot(slot); return; };
        let _ = KillTimer(Some(hwnd), FLASH_TIMER);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

pub fn hide() {
    hide_slot(&CAND);
    hide_slot(&FLASH);
}

fn hide_slot(slot: &'static std::thread::LocalKey<RefCell<Option<State>>>) {
    slot.with(|c| {
        if let Some(s) = c.borrow_mut().take() {
            unsafe {
                let _ = ShowWindow(s.hwnd, SW_HIDE);
                let _ = DestroyWindow(s.hwnd);
            }
        }
    });
}

/// Window rectangle (tests / diagnostics).
pub fn rect() -> Option<RECT> {
    CAND.with(|c| {
        c.borrow().as_ref().map(|s| {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(s.hwnd, &mut r);
            }
            r
        })
    })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == windows::Win32::UI::WindowsAndMessaging::WM_NCHITTEST
        && HINT.with(|c| c.borrow().as_ref().is_some_and(|s| s.hwnd == hwnd)) {
        return LRESULT(windows::Win32::UI::WindowsAndMessaging::HTTRANSPARENT as isize);
    }
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_TIMER if wp.0 == FLASH_TIMER => {
                hide_slot(&FLASH);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lp.0 & 0xFFFF) as i16 as i32;
                let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
                let add = with_window(hwnd, |c| c.borrow().as_ref().and_then(|s| s.add).is_some_and(|(l, t, r, b)| x >= l && x < r && y >= t && y < b));
                if add {
                    let cb = with_window(hwnd, |c| c.borrow_mut().as_mut().and_then(|s| s.on_add.take()));
                    if let Some(cb) = cb {
                        let _ = crate::guard(|| cb());
                        with_window(hwnd, |c| {
                            if let Some(s) = c.borrow_mut().as_mut() && s.on_add.is_none() {
                                s.on_add = Some(cb);
                            }
                        });
                    }
                    return LRESULT(0);
                }
                let hit = with_window(hwnd, |c| c.borrow().as_ref().filter(|s| !s.plain).and_then(|s| s.cells.iter().position(|(l, t, r, b)| x >= *l && x < *r && y >= *t && y < *b)));
                if let Some(i) = hit {
                    // Take the callback out while it runs (it may close this window).
                    let cb = CAND.with(|c| c.borrow_mut().as_mut().and_then(|s| s.on_pick.take()));
                    if let Some(cb) = cb {
                        let _ = crate::guard(|| cb(i));
                        CAND.with(|c| {
                            if let Some(s) = c.borrow_mut().as_mut() {
                                s.on_pick = Some(cb);
                            }
                        });
                    }
                }
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                with_window(hwnd, |c| {
                    if let Some(s) = c.borrow().as_ref() {
                        let mut r = RECT::default();
                        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut r);
                        let bg = CreateSolidBrush(COLORREF(0x00FF_FFFF));
                        FillRect(dc, &r, bg);
                        let _ = DeleteObject(HGDIOBJ(bg.0));
                        let f = font(s.dpi);
                        let old = SelectObject(dc, HGDIOBJ(f.0));
                        SetBkMode(dc, TRANSPARENT);
                        let pad = (6 * s.dpi as i32) / 96;
                        for (i, it) in s.items.iter().enumerate() {
                            let (l, t, rr, b) = s.cells[i];
                            if i == s.selected {
                                let hi = CreateSolidBrush(COLORREF(0x00F0_D8B0)); // light blue (BGR)
                                let cell = RECT { left: l, top: t, right: rr, bottom: b };
                                FillRect(dc, &cell, hi);
                                let _ = DeleteObject(HGDIOBJ(hi.0));
                            }
                            SetTextColor(dc, COLORREF(0x0000_0000));
                            let _ = TextOutW(dc, l + pad, t + pad, &label(i, it, s.plain));
                        }
                        if let Some(((l, t, _, _), text)) = &s.info {
                            SetTextColor(dc, COLORREF(0x0080_8080));
                            let _ = TextOutW(dc, l + pad, t + pad, &wide(text));
                        }
                        if let Some((l, t, _, _)) = s.add {
                            SetTextColor(dc, COLORREF(0x0000_0000));
                            let _ = TextOutW(dc, l + pad, t + pad, &wide("＋詞庫"));
                        }
                        SelectObject(dc, old);
                        let _ = DeleteObject(HGDIOBJ(f.0));
                        let _ = (CreatePen, PS_SOLID, Rectangle);
                    }
                });
                let _ = EndPaint(hwnd, &ps);
                let _ = HMODULE::default();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_scales_with_dpi() {
        let items: Vec<String> = ["今", "金", "斤", "襟"].iter().map(|s| s.to_string()).collect();
        let big = 100_000;
        let (p1, p2, p3) = (layout(HWND::default(), &items, "", 96, big, false), layout(HWND::default(), &items, "", 144, big, false), layout(HWND::default(), &items, "", 192, big, false));
        let ((w1, h1), (w2, h2), (w3, h3)) = ((p1.w, p1.h), (p2.w, p2.h), (p3.w, p3.h));
        assert_eq!(p1.cells.len(), 4);
        let near = |a: i32, b: i32, k: f64| ((a as f64 * k) - b as f64).abs() <= a as f64 * k * 0.15;
        assert!(near(w1, w2, 1.5) && near(h1, h2, 1.5), "{w1}x{h1} -> {w2}x{h2}");
        assert!(near(w1, w3, 2.0) && near(h1, h3, 2.0), "{w1}x{h1} -> {w3}x{h3}");
    }
}

#[cfg(test)]
mod pack_tests {
    use super::pack;

    #[test]
    fn add_cell_follows_page_and_wraps_with_it() {
        let (p, add) = super::pack_with_add(&[20, 20], 12, 40, 18, 6, 120);
        assert_eq!(p.cells, vec![(6, 0, 38, 30), (38, 0, 70, 30)]);
        assert_eq!(p.info, Some((70, 0, 94, 30)));
        assert_eq!(add, Some((6, 30, 58, 60)));
        assert_eq!((p.w, p.h), (100, 60));
        let (p, add) = super::pack_with_add(&[20], 0, 40, 18, 6, 120);
        assert_eq!(add, Some((38, 0, 90, 30)));
        assert!(p.info.is_none());
        assert_eq!(super::pack_with_add(&[20], 12, 0, 18, 6, 120), (pack(&[20], 12, 18, 6, 120), None));
    }

    #[test]
    fn single_row_when_it_fits_and_the_indicator_follows() {
        let p = pack(&[20, 20, 20], 12, 18, 6, 1000);
        assert_eq!(p.cells, vec![(6, 0, 38, 30), (38, 0, 70, 30), (70, 0, 102, 30)]);
        assert_eq!(p.info, Some((102, 0, 126, 30)));
        assert_eq!((p.w, p.h), (132, 30));
    }

    #[test]
    fn wraps_nine_six_character_words_inside_small_work_areas_at_every_scale() {
        // A six-character word with its digit label is about 7 em wide; em = 14 pt at the host dpi.
        for (dpi, work_w) in [(96, 1280), (144, 1280), (192, 1280), (192, 1024), (144, 800)] {
            let em = 14 * dpi / 72;
            let widths = vec![7 * em; 9];
            let pad = 6 * dpi / 96;
            let p = pack(&widths, 4 * em, em + 4, pad, work_w);
            assert!(p.w <= work_w, "dpi {dpi} work {work_w}: width {} > {work_w}", p.w);
            assert_eq!(p.cells.len(), 9);
            // Cells never overlap and the last row holds at least one cell.
            for (i, a) in p.cells.iter().enumerate() {
                assert!(a.2 <= p.w && a.3 <= p.h);
                for b in &p.cells[i + 1..] {
                    assert!(a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1, "{a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn a_cell_wider_than_the_area_still_gets_its_own_row() {
        let p = pack(&[500, 30], 0, 10, 2, 100);
        assert_eq!(p.cells[0].1, 0);
        assert_eq!(p.cells[1].1, 14);
        assert!(p.info.is_none());
    }
}

pub fn hint_hide() { hide_slot(&HINT); }
#[cfg(test)]
pub(crate) fn hint_window() -> Option<HWND> {
    HINT.with(|slot| slot.borrow().as_ref().map(|s| s.hwnd))
}
fn hint_items(hint: &twime_proto::LexuxHint) -> Vec<String> {
    let mut items = vec!["按反斜線切換：".to_string()];
    let labels = hint.labels();
    let count = labels.len();
    items.extend(labels.into_iter().enumerate().map(|(i, label)| {
        if i + 1 < count { format!("{label}　") } else { label }
    }));
    items
}
pub fn hint_visible() -> bool {
    HINT.with(|slot| slot.borrow().as_ref().is_some_and(|s| unsafe {
        windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(s.hwnd).as_bool()
    }))
}
pub fn completion_show(owner: HWND, rect: RECT, hint: &twime_proto::CompletionHint) -> bool {
    let mut items = vec!["↓ 選取　".to_string()];
    let labels = hint.labels();
    let n = labels.len();
    items.extend(labels.into_iter().enumerate().map(|(i, s)| if i+1 < n { format!("{s} ") } else { s }));
    show_impl(owner, rect, &items, usize::MAX, None, Box::new(|_| {}), None, true, true);
    hint_visible()
}
pub fn hint_show(owner: HWND, rect: RECT, hint: &twime_proto::LexuxHint) -> bool {
    let items = hint_items(hint);
    // item 0 is the lead text; with a hidden option zero, listed item i is cycle index i + 1 (AUTOCOMP), and the hidden
    // current option highlights nothing
    let highlight = if !hint.hide_first { hint.current + 1 } else if hint.current == 0 { usize::MAX } else { hint.current };
    show_impl(owner, rect, &items, highlight, None, Box::new(|_| {}), None, true, true);
    HINT.with(|slot| slot.borrow().as_ref().is_some_and(|s| unsafe {
        windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(s.hwnd).as_bool()
    }))
}

#[cfg(test)]
mod hint_tests {
    use super::*;

    fn r(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }
    #[test]
    fn popup_below_composition() {
        assert!(popup_position(r(100, 100, 140, 120), r(0, 0, 1000, 800), 200, 40, 96) == (100, 122));
    }
    #[test]
    fn popup_flips_above_at_work_area_bottom() {
        assert!(popup_position(r(100, 760, 140, 780), r(0, 0, 1000, 800), 200, 40, 96) == (100, 718));
    }
    #[test]
    fn popup_moves_left_at_right_edge() {
        assert!(popup_position(r(950, 100, 980, 120), r(0, 0, 1000, 800), 200, 40, 96) == (800, 122));
    }
    #[test]
    fn popup_keeps_negative_monitor_coordinates_and_scaled_gap() {
        let wa = r(-1920, -1080, 0, 0);
        assert!(popup_position(r(-1800, -900, -1700, -880), wa, 200, 40, 144) == (-1800, -877));
        assert!(popup_position(r(-50, -40, -10, 0), wa, 200, 40, 144) == (-200, -83));
    }

    #[test]
    fn tall_composition_hint_uses_right_space_after_clamping() {
        let caret = r(100, 40, 140, 760);
        let wa = r(0, 0, 1000, 800);
        assert!(popup_position(caret, wa, 200, 80, 96) == (100, 0));
        assert!(hint_position(caret, wa, 200, 80, 96) == Some((142, 40)));
    }

    #[test]
    fn tall_composition_hint_uses_left_space_when_right_is_full() {
        assert!(hint_position(r(900, 40, 940, 760), r(0, 0, 1000, 800), 200, 80, 96) == Some((698, 40)));
        assert!(hint_position(r(-100, -760, -60, -40), r(-1000, -800, 0, 0), 200, 80, 144) == Some((-303, -760)));
    }

    #[test]
    fn hint_with_no_nonoverlapping_space_is_hidden() {
        assert!(hint_position(r(40, 40, 960, 760), r(0, 0, 1000, 800), 200, 80, 96).is_none());
    }

    #[test]
    fn vertically_adjacent_monitors_keep_candidate_boundary_anchor() {
        // Inject two touching work areas; the boundary belongs to the lower monitor.
        for boundary in [0, 800] {
            let caret = r(100, boundary - 20, 140, boundary);
            let upper = r(0, boundary - 800, 1000, boundary);
            let lower = r(0, boundary, 1000, boundary + 800);
            let area = |point: POINT| if point.y < boundary { upper } else { lower };
            let candidate = monitor_anchor(caret, false);
            let hint = monitor_anchor(caret, true);
            assert!(candidate.x == caret.left && candidate.y == caret.bottom);
            assert!(hint.x == caret.left && hint.y == caret.bottom - 1);
            assert!(popup_position(caret, area(candidate), 200, 80, 96) == (100, boundary + 2));
            assert!(hint_position(caret, area(hint), 200, 80, 96) == Some((100, boundary - 102)));
        }
    }

    #[test]
    fn candidates_match_original_formula_and_regular_hints_match_candidates() {
        for wa in [r(0, 0, 1000, 800), r(-1000, -800, 0, 0)] {
            for dpi in [96, 144, 192] {
                for (dx, dy) in [(100, 100), (100, 760), (950, 100)] {
                    let caret = r(wa.left + dx, wa.top + dy, wa.left + dx + 40, wa.top + dy + 20);
                    let gap = (2 * dpi as i32) / 96;
                    let mut y = caret.bottom + gap;
                    if y + 40 > wa.bottom { y = caret.top - 40 - gap; }
                    let mut x = caret.left;
                    if x + 200 > wa.right { x = wa.right - 200; }
                    let expected = (x.max(wa.left), y.max(wa.top));
                    assert!(popup_position(caret, wa, 200, 40, dpi) == expected);
                    assert!(hint_position(caret, wa, 200, 40, dpi) == Some(expected));
                }
            }
        }
    }

    #[test]
    fn frozen_labels_highlight_and_accessible_window_name_follow_cycle() {
        check_frozen_labels(false, 3);
    }

    #[test]
    fn automatic_labels_keep_original_at_one_and_accessible_name_follows_cycle() {
        check_frozen_labels(true, 2);
    }

    fn check_frozen_labels(auto: bool, count: usize) {
        use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
        let mut h = twime_proto::LexuxHint {
            options: (1..=count).map(|n| n.to_string()).collect(), current: 0, auto, hide_first: false,
        };
        for current in (0..count).cycle().take(count * 2 + 1) {
            h.current = current;
            let items = hint_items(&h);
            assert!(items.len() == count + 1 && items.concat() == h.text());
            for (i, item) in items[1..].iter().enumerate() {
                assert!(item.contains("（原）") == (i == usize::from(auto)));
                assert!(item.contains("（目前）") == (i == current));
            }
            assert!(items[1..].iter().all(|s| s.starts_with("\\ ")));
            assert!(items[1..count].iter().all(|s| s.ends_with('　')));
            hint_show(HWND::default(), RECT::default(), &h);
            HINT.with(|slot| {
                let state = slot.borrow();
                let s = state.as_ref().expect("count=1");
                assert!(s.plain && s.items == items && s.selected == current + 1);
                assert!(s.cells.len() == items.len() && s.on_add.is_none());
                let mut name = vec![0u16; h.text().encode_utf16().count() + 1];
                let count = unsafe { GetWindowTextW(s.hwnd, &mut name) };
                assert!(String::from_utf16_lossy(&name[..count as usize]) == h.text());
            });
        }
        hint_hide();
    }
}
