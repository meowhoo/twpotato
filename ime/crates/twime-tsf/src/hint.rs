//! Nonactivating tooltip and the TSF UI-element lifetime.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        UI::TextServices::*,
    },
    core::{BOOL, BSTR, GUID, Interface, Result, implement},
};

pub const GUID_TOOLTIP: GUID = GUID::from_u128(0x1b3a7674_352c_488e_b828_875936a47167);

/// Popup suppression ends the TSF element; a later valid update begins a fresh one.
pub(crate) fn available(next: Option<&twime_proto::LexuxHint>, candidates: bool, symbols: bool) -> bool {
    !candidates && !symbols && next.is_some_and(|h| h.valid())
}

struct State {
    completion: RefCell<Option<twime_proto::CompletionHint>>,
    text: RefCell<String>,
    cycle: RefCell<twime_proto::LexuxHint>,
    location: Cell<Option<(HWND, RECT)>>,
    permitted: Cell<bool>,
    shown: Cell<bool>,
    visible: Cell<bool>,
    active: Cell<bool>,
}
impl State {
    fn draw(&self) {
        crate::cand::hint_hide();
        self.visible.set(false);
        if self.active.get() && self.permitted.get() && self.shown.get() {
            if let Some((owner, rect)) = self.location.get() {
                self.visible.set(if let Some(h) = self.completion.borrow().as_ref() {
                    crate::cand::completion_show(owner, rect, h)
                } else { crate::cand::hint_show(owner, rect, &self.cycle.borrow()) });
            }
        }
    }
}
#[implement(ITfToolTipUIElement)]
struct Element {
    state: Rc<State>,
}
impl Drop for Element {
    fn drop(&mut self) {
        crate::LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl ITfUIElement_Impl for Element_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from(if self.state.completion.borrow().is_some() { "Completion hint" } else { "LEXUX hint" }))
    }
    fn GetGUID(&self) -> Result<GUID> {
        Ok(GUID_TOOLTIP)
    }
    fn Show(&self, show: BOOL) -> Result<()> {
        crate::guard(|| {
            if !self.state.active.get() { return; }
            self.state.shown.set(show.as_bool());
            self.state.draw();
        });
        Ok(())
    }
    fn IsShown(&self) -> Result<BOOL> {
        Ok(BOOL::from(
            self.state.visible.get(),
        ))
    }
}
impl ITfToolTipUIElement_Impl for Element_Impl {
    fn GetString(&self) -> Result<BSTR> {
        Ok(BSTR::from(self.state.text.borrow().as_str()))
    }
}
struct Active {
    mgr: ITfUIElementMgr,
    id: u32,
    state: Rc<State>,
    _element: ITfToolTipUIElement,
}
#[derive(Default)]
pub struct Hint {
    active: Option<Active>,
}
impl Hint {
    pub fn visible(&self) -> bool { self.active.as_ref().is_some_and(|a| a.state.visible.get()) && crate::cand::hint_visible() }
    pub fn update_completion(&mut self, tm: &ITfThreadMgr, owner: HWND, rect: RECT, next: &twime_proto::CompletionHint) {
        if !next.valid() { self.hide(); return; }
        let Ok(mgr) = tm.cast::<ITfUIElementMgr>() else { self.hide(); return; };
        let placeholder = twime_proto::LexuxHint { options: vec![" ".into(), " ".into()], current: 0, auto: false, hide_first: false };
        self.update_content(&mgr, owner, rect, &placeholder, Some(next));
    }

    pub fn hide(&mut self) {
        if let Some(a) = self.active.take() {
            a.state.active.set(false);
            a.state.draw();
            let _ = unsafe { a.mgr.EndUIElement(a.id) };
            crate::trace::log("hint.end count=1");
        }
        crate::cand::hint_hide();
    }
    pub fn update(&mut self, tm: &ITfThreadMgr, owner: HWND, rect: RECT, next: &twime_proto::LexuxHint) {
        let Ok(mgr) = tm.cast::<ITfUIElementMgr>() else {
            self.hide();
            return;
        };
        self.update_manager(&mgr, owner, rect, Some(next), false, false);
    }
    fn update_manager(&mut self, mgr: &ITfUIElementMgr, owner: HWND, rect: RECT, next: Option<&twime_proto::LexuxHint>, candidates: bool, symbols: bool) {
        if !available(next, candidates, symbols) { self.hide(); return; }
        self.update_content(mgr, owner, rect, next.unwrap(), None);
    }
    fn update_content(&mut self, mgr: &ITfUIElementMgr, owner: HWND, rect: RECT, next: &twime_proto::LexuxHint, completion: Option<&twime_proto::CompletionHint>) {
        // A single string is exposed by TSF and by the Win32 UIA proxy.
        let text = completion.map_or_else(|| next.text(), |c| c.text());
        if let Some(a) = &self.active {
            *a.state.completion.borrow_mut() = completion.cloned();
            *a.state.text.borrow_mut() = text;
            *a.state.cycle.borrow_mut() = next.clone();
            a.state.location.set(Some((owner, rect)));
            if unsafe { a.mgr.UpdateUIElement(a.id) }.is_err() {
                self.hide();
                return;
            }
            a.state.draw();
            crate::trace::log("hint.update count=1");
            return;
        }
        let state = Rc::new(State {
            completion: RefCell::new(completion.cloned()),
            text: RefCell::new(text),
            cycle: RefCell::new(next.clone()),
            location: Cell::new(Some((owner, rect))),
            permitted: Cell::new(false),
            shown: Cell::new(true),
            visible: Cell::new(false),
            active: Cell::new(true),
        });
        crate::LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let element: ITfToolTipUIElement = Element {
            state: state.clone(),
        }
        .into();
        let mut show = BOOL(1);
        let mut id = 0;
        if unsafe { mgr.BeginUIElement(&element, &mut show, &mut id) }.is_err() {
            state.active.set(false);
            state.draw();
            return;
        }
        state.permitted.set(show.as_bool());
        self.active = Some(Active {
            mgr: mgr.clone(),
            id,
            state: state.clone(),
            _element: element,
        });
        state.draw();
        crate::trace::log(&format!("hint.begin count=1 draw={}", show.as_bool()));
    }
}
impl Drop for Hint {
    fn drop(&mut self) {
        self.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_suppression_and_ended_element_cannot_draw() {
        let state = Rc::new(State {
            completion: RefCell::new(None),
            text: RefCell::new(String::from("\\ 123")),
            cycle: RefCell::new(twime_proto::LexuxHint { options: vec!["1".into(), "2".into()], current: 0, auto: false, hide_first: false }),
            location: Cell::new(None),
            permitted: Cell::new(false),
            shown: Cell::new(true),
            visible: Cell::new(false),
            active: Cell::new(true),
        });
        crate::LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let e: ITfToolTipUIElement = Element {
            state: state.clone(),
        }
        .into();
        unsafe {
            assert!(e.GetString().unwrap().to_string() == *state.text.borrow());
            e.Show(true).unwrap();
            assert!(!e.IsShown().unwrap().as_bool());
            state.permitted.set(true);
            state.draw();
            assert!(!e.IsShown().unwrap().as_bool());
            state.active.set(false);
            e.Show(true).unwrap();
            assert!(!e.IsShown().unwrap().as_bool());
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use windows::{Win32::Foundation::E_FAIL, core::Ref};
    #[derive(Default)]
    struct Counts {
        begin: Cell<u32>,
        update: Cell<u32>,
        end: Cell<u32>,
        fail_begin: Cell<bool>,
        fail_update: Cell<bool>,
        allow_show: Cell<bool>,
        element: RefCell<Option<ITfUIElement>>,
    }
    #[implement(ITfUIElementMgr)]
    struct Manager {
        counts: Rc<Counts>,
    }
    impl ITfUIElementMgr_Impl for Manager_Impl {
        fn BeginUIElement(
            &self,
            element: Ref<ITfUIElement>,
            show: *mut BOOL,
            id: *mut u32,
        ) -> Result<()> {
            if self.counts.fail_begin.get() {
                return Err(E_FAIL.into());
            }
            self.counts.begin.set(self.counts.begin.get() + 1);
            *self.counts.element.borrow_mut() = element.as_ref().cloned();
            unsafe {
                *show = BOOL::from(self.counts.allow_show.get());
                *id = 7;
            }
            Ok(())
        }
        fn UpdateUIElement(&self, id: u32) -> Result<()> {
            assert!(id == 7);
            self.counts.update.set(self.counts.update.get() + 1);
            if self.counts.fail_update.get() {
                Err(E_FAIL.into())
            } else {
                Ok(())
            }
        }
        fn EndUIElement(&self, id: u32) -> Result<()> {
            assert!(id == 7);
            self.counts.end.set(self.counts.end.get() + 1);
            Ok(())
        }
        fn GetUIElement(&self, _: u32) -> Result<ITfUIElement> {
            self.counts
                .element
                .borrow()
                .clone()
                .ok_or_else(|| E_FAIL.into())
        }
        fn EnumUIElements(&self) -> Result<IEnumTfUIElements> {
            Err(E_FAIL.into())
        }
    }
    fn popup_cycle(candidates: bool, symbols: bool, expire: bool) {
        let counts = Rc::new(Counts::default());
        let mgr: ITfUIElementMgr = Manager { counts: counts.clone() }.into();
        let cycle = twime_proto::LexuxHint { options: vec!["1".into(), "2".into()], current: 0, auto: false, hide_first: false };
        let mut hint = Hint::default();
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), false, false);
        let retained = counts.element.borrow().clone().unwrap();
        assert!(!available(Some(&cycle), candidates, symbols));
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), candidates, symbols);
        assert!(hint.active.is_none() && counts.begin.get() == 1 && counts.end.get() == 1);
        unsafe {
            retained.Show(true).unwrap();
            assert!(!retained.IsShown().unwrap().as_bool());
        }
        let next = if expire { None } else { Some(&cycle) };
        hint.update_manager(&mgr, HWND::default(), RECT::default(), next, false, false);
        assert!(counts.begin.get() == if expire { 1 } else { 2 });
        assert!(hint.active.is_some() != expire);
        hint.hide();
        assert!(counts.begin.get() == counts.end.get());
        let invalid = twime_proto::LexuxHint { options: vec![], current: 0, auto: false, hide_first: false };
        assert!(!available(Some(&invalid), false, false));
    }
    #[test]
    fn candidates_end_and_begin_again_when_hint_remains_valid() { popup_cycle(true, false, false); }
    #[test]
    fn symbols_end_and_begin_again_when_hint_remains_valid() { popup_cycle(false, true, false); }
    #[test]
    fn candidates_close_after_hint_expiry_does_not_begin() { popup_cycle(true, false, true); }
    #[test]
    fn symbols_close_after_hint_expiry_does_not_begin() { popup_cycle(false, true, true); }

    #[test]
    fn no_space_then_space_reports_actual_visibility_and_balances_lifetime() {
        use windows::Win32::{
            Foundation::POINT,
            Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
            UI::WindowsAndMessaging::IsWindowVisible,
        };
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            assert!(GetMonitorInfoW(MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTONEAREST), &mut mi).as_bool());
        }
        let blocked = mi.rcWork;
        let room = RECT { left: blocked.left, top: blocked.top, right: blocked.left + 20, bottom: blocked.top + 20 };
        let counts = Rc::new(Counts::default());
        counts.allow_show.set(true);
        let mgr: ITfUIElementMgr = Manager { counts: counts.clone() }.into();
        let cycle = twime_proto::LexuxHint { options: vec!["1".into(), "2".into()], current: 0, auto: false, hide_first: false };
        let mut hint = Hint::default();
        hint.update_manager(&mgr, HWND::default(), blocked, Some(&cycle), false, false);
        let element = counts.element.borrow().clone().unwrap();
        assert!(crate::cand::hint_window().is_none());
        unsafe { assert!(!element.IsShown().unwrap().as_bool()); }
        assert!(counts.begin.get() == 1 && counts.end.get() == 0);
        for (rect, expected) in [(room, true), (blocked, false), (room, true)] {
            hint.update_manager(&mgr, HWND::default(), rect, Some(&cycle), false, false);
            let hwnd = crate::cand::hint_window();
            assert!(hwnd.is_some() == expected);
            unsafe {
                assert!(element.IsShown().unwrap().as_bool() == expected);
                if let Some(hwnd) = hwnd { assert!(IsWindowVisible(hwnd).as_bool()); }
            }
            assert!(counts.begin.get() == 1 && counts.end.get() == 0);
        }
        unsafe {
            element.Show(false).unwrap();
            assert!(!element.IsShown().unwrap().as_bool());
            assert!(crate::cand::hint_window().is_none());
            element.Show(true).unwrap();
            assert!(element.IsShown().unwrap().as_bool());
            assert!(IsWindowVisible(crate::cand::hint_window().unwrap()).as_bool());
        }
        hint.hide();
        assert!(crate::cand::hint_window().is_none());
        unsafe { assert!(!element.IsShown().unwrap().as_bool()); }
        assert!(counts.begin.get() == 1 && counts.update.get() == 3 && counts.end.get() == 1);
    }

    #[test]
    fn ended_element_show_preserves_new_visible_window_and_counts() {
        use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
        let counts = Rc::new(Counts::default());
        counts.allow_show.set(true);
        let mgr: ITfUIElementMgr = Manager { counts: counts.clone() }.into();
        let cycle = twime_proto::LexuxHint { options: vec!["1".into(), "2".into()], current: 0, auto: false, hide_first: false };
        let mut hint = Hint::default();
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), false, false);
        let old = counts.element.borrow().clone().unwrap();
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), true, false);
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), false, false);
        let current = counts.element.borrow().clone().unwrap();
        let hwnd = crate::cand::hint_window().unwrap();
        for show in [true, false] {
            unsafe {
                assert!(current.IsShown().unwrap().as_bool() && IsWindowVisible(hwnd).as_bool());
                old.Show(show).unwrap();
                assert!(!old.IsShown().unwrap().as_bool());
                assert!(current.IsShown().unwrap().as_bool() && IsWindowVisible(hwnd).as_bool());
            }
            assert!(crate::cand::hint_window() == Some(hwnd));
            assert!(counts.begin.get() == 2 && counts.update.get() == 0 && counts.end.get() == 1);
        }
        hint.hide();
        assert!(counts.begin.get() == 2 && counts.end.get() == 2);
        assert!(crate::cand::hint_window().is_none());
    }

    #[test]
    fn begin_update_end_and_failure_balance_without_windows_or_registration() {
        let counts = Rc::new(Counts::default());
        let mgr: ITfUIElementMgr = Manager {
            counts: counts.clone(),
        }
        .into();
        let mut hint = Hint::default();
        let mut cycle = twime_proto::LexuxHint { options: vec!["1".into(), "2".into()], current: 0, auto: false, hide_first: false };
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), false, false);
        let retained = counts.element.borrow().clone().unwrap();
        unsafe {
            assert!(!retained.IsShown().unwrap().as_bool());
        }
        cycle.current = 1;
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), false, false);
        let tip: ITfToolTipUIElement = retained.cast().unwrap();
        unsafe {
            assert!(tip.GetString().unwrap().to_string() == cycle.text());
        }
        counts.fail_update.set(true);
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), false, false);
        hint.hide();
        unsafe {
            retained.Show(true).unwrap();
            assert!(!retained.IsShown().unwrap().as_bool());
        }
        assert!(counts.begin.get() == 1 && counts.update.get() == 2 && counts.end.get() == 1);
        counts.fail_begin.set(true);
        hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), false, false);
        assert!(hint.active.is_none() && counts.end.get() == 1);
    }

    fn check_get_string_cycle(auto: bool, count: usize) {
        let counts = Rc::new(Counts::default());
        let mgr: ITfUIElementMgr = Manager { counts: counts.clone() }.into();
        let mut hint = Hint::default();
        let mut cycle = twime_proto::LexuxHint {
            options: (1..=count).map(|n| n.to_string()).collect(), current: 0, auto, hide_first: false,
        };
        for current in (0..count).cycle().take(count * 2 + 1) {
            cycle.current = current;
            hint.update_manager(&mgr, HWND::default(), RECT::default(), Some(&cycle), false, false);
            let tip: ITfToolTipUIElement = counts.element.borrow().as_ref().unwrap().cast().unwrap();
            let text = unsafe { tip.GetString().unwrap().to_string() };
            assert!(text == cycle.text());
            let labels: Vec<_> = text.strip_prefix("按反斜線切換：").unwrap().split('　').collect();
            for (i, label) in labels.iter().enumerate() {
                assert!(label.contains("（原）") == (i == usize::from(auto)));
                assert!(label.contains("（目前）") == (i == current));
            }
        }
        hint.hide();
        assert!(counts.begin.get() == 1 && counts.end.get() == 1);
    }

    #[test]
    fn ordinary_get_string_keeps_original_at_zero_through_cycles() {
        check_get_string_cycle(false, 3);
    }

    #[test]
    fn automatic_get_string_keeps_original_at_one_through_cycles() {
        check_get_string_cycle(true, 2);
    }
}
