//! Apartment-local language bar button. Never hold a borrow across a sink callback.
use crate::{LIVE, MODULE, guard, trace};
use std::{
    cell::{Cell, RefCell},
    os::windows::ffi::OsStrExt,
    rc::Rc,
    sync::atomic::Ordering,
};
use twime_proto::{Mode, ids};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER, GetLastError,
            HINSTANCE, LPARAM, POINT, RECT, SetLastError, WIN32_ERROR, WPARAM,
        },
        System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        System::Threading::{
            CreateProcessW, GetCurrentThreadId, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
            STARTUPINFOW,
        },
        UI::{
            HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi},
            TextServices::*,
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu, GetForegroundWindow,
                GetWindowThreadProcessId, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
                MF_GRAYED, MF_STRING, PostMessageW, SM_CXSMICON, SM_CYSMICON, TPM_NONOTIFY,
                TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
            },
        },
    },
    core::{BOOL, BSTR, GUID, IUnknown, Interface, PCWSTR, Ref, Result, implement, w},
};

/// Taskbar theme: HKCU `Software\Microsoft\Windows\CurrentVersion\Themes\Personalize` `SystemUsesLightTheme` (1 = light);
/// None when it cannot be read.
fn light_taskbar() -> Option<bool> {
    let (mut value, mut size) = (0u32, std::mem::size_of::<u32>() as u32);
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    status.ok().ok().map(|()| value != 0)
}

/// Mode button icon group (res/generate.ps1): 102 中 / 103 英 are dark glyphs for a light taskbar, 104 中 / 105 英
/// light glyphs for a dark taskbar. An unreadable theme uses the dark-taskbar pair.
fn mode_icon_id(mode: Mode, light: Option<bool>) -> u16 {
    let base = if light == Some(true) { 102 } else { 104 };
    if mode == Mode::English { base + 1 } else { base }
}

fn use_foreground_menu(foreground_thread: Option<u32>, current_thread: u32) -> bool {
    foreground_thread.is_some_and(|thread| thread != 0 && thread == current_thread)
}

fn settings_menu(pt: &POINT) -> Result<()> {
    let root = crate::link::dll_dir();
    // Never resolve a failed module lookup relative to the host's working directory.
    if !root.is_absolute() {
        return Err(E_FAIL.into());
    }
    let exe = root.join("twime-settings.exe");
    let available = exe.is_file();
    unsafe {
        let owner = GetForegroundWindow();
        let foreground_thread =
            (!owner.is_invalid()).then(|| GetWindowThreadProcessId(owner, None));
        let launch = if !use_foreground_menu(foreground_thread, GetCurrentThreadId()) {
            trace::log(&format!("langbar.menu path=direct available={available}"));
            available
        } else {
            // Keep the host's foreground window and focus, including active composition.
            trace::log("langbar.menu path=host_foreground");
            let menu = CreatePopupMenu()?;
            let result: Result<bool> = (|| {
                AppendMenuW(
                    menu,
                    if available {
                        MF_STRING
                    } else {
                        MF_STRING | MF_GRAYED
                    },
                    1,
                    w!("設定…"),
                )?;
                SetLastError(WIN32_ERROR(0));
                let command = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                    pt.x,
                    pt.y,
                    None,
                    owner,
                    None,
                )
                .0;
                let error = GetLastError().0;
                // Also required after cancellation by an outside click or Esc.
                let posted = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
                trace::log(&format!("langbar.menu post_null={posted:?}"));
                trace::log(&format!(
                    "langbar.menu command={command} error={error} available={available}"
                ));
                Ok(command == 1 && available)
            })();
            let destroyed = DestroyMenu(menu);
            trace::log(&format!("langbar.menu destroy_menu={destroyed:?}"));
            result?
        };
        if launch {
            let exe: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
            let dir: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
            let startup = STARTUPINFOW {
                cb: size_of::<STARTUPINFOW>() as u32,
                ..Default::default()
            };
            let mut process = PROCESS_INFORMATION::default();
            let launched = CreateProcessW(
                PCWSTR(exe.as_ptr()),
                None,
                None,
                None,
                false,
                PROCESS_CREATION_FLAGS(0),
                None,
                PCWSTR(dir.as_ptr()),
                &startup,
                &mut process,
            );
            trace::log(&format!("langbar.launch result={launched:?}"));
            launched?;
            let _ = CloseHandle(process.hThread);
            let _ = CloseHandle(process.hProcess);
        }
        Ok(())
    }
}

pub(crate) fn displayed_mode(focused: Option<Mode>, previous: Mode) -> Mode {
    focused.unwrap_or(previous)
}
pub(crate) fn needs_update(previous: (usize, Mode), current: (usize, Mode)) -> bool {
    previous != current
}

pub(crate) struct State {
    active: Cell<bool>,
    shown: Cell<(usize, Mode)>,
    sinks: RefCell<Vec<(u32, ITfLangBarItemSink)>>,
    next_cookie: Cell<u32>,
}
impl State {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            active: Cell::new(true),
            shown: Cell::new((0, Mode::Chinese)),
            sinks: RefCell::new(Vec::new()),
            next_cookie: Cell::new(1),
        })
    }
    pub(crate) fn deactivate(&self) {
        self.active.set(false);
        self.shown.set((0, Mode::Chinese));
        let sinks = self.sinks.take();
        // Release COM references only after ending the RefCell borrow. A sink may
        // own the button/source and re-enter us from its final Release.
        drop(sinks);
    }
    pub(crate) fn update(&self, focus: usize, mode: Option<Mode>) {
        if !self.active.get() {
            return;
        }
        let old = self.shown.get();
        let current = (focus, displayed_mode(mode, old.1));
        self.shown.set(current);
        if needs_update(old, current) {
            let sinks = self.sinks.borrow().clone();
            for (_, sink) in sinks {
                // A preceding callback may have deactivated the service.
                if !self.active.get() {
                    break;
                }
                let result = unsafe { sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP) };
                trace::log(&format!(
                    "langbar.update mode={:?} result={result:?}",
                    current.1
                ));
            }
        }
    }
}

#[implement(ITfLangBarItemButton, ITfSource)]
pub(crate) struct Button {
    state: Rc<State>,
    click: Box<dyn Fn()>,
}
impl Button {
    pub(crate) fn create(state: Rc<State>, click: impl Fn() + 'static) -> ITfLangBarItem {
        LIVE.fetch_add(1, Ordering::SeqCst);
        let button: ITfLangBarItemButton = Self {
            state,
            click: Box::new(click),
        }
        .into();
        button.cast().expect("button inherits ITfLangBarItem")
    }
}
impl Drop for Button {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    guard(f).unwrap_or(Err(E_FAIL.into()))
}
impl ITfLangBarItem_Impl for Button_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        guarded(|| {
            if info.is_null() {
                return Err(E_POINTER.into());
            }
            let mut value = TF_LANGBARITEMINFO {
                clsidService: GUID::from_u128(ids::CLSID_TIP),
                guidItem: GUID_LBI_INPUTMODE,
                dwStyle: TF_LBI_STYLE_BTN_BUTTON,
                ulSort: 0,
                ..Default::default()
            };
            for (slot, ch) in value
                .szDescription
                .iter_mut()
                .zip("中／英切換".encode_utf16())
            {
                *slot = ch;
            }
            unsafe {
                *info = value;
            }
            Ok(())
        })
    }
    fn GetStatus(&self) -> Result<u32> {
        guarded(|| Ok(0))
    }
    fn Show(&self, _: BOOL) -> Result<()> {
        guarded(|| Ok(()))
    }
    fn GetTooltipString(&self) -> Result<BSTR> {
        guarded(|| {
            Ok(BSTR::from(if self.state.shown.get().1 == Mode::English {
                "英文模式（Shift 切換）；右鍵：設定"
            } else {
                "中文模式（Shift 切換）；右鍵：設定"
            }))
        })
    }
}
impl ITfLangBarItemButton_Impl for Button_Impl {
    fn OnClick(&self, click: TfLBIClick, pt: &POINT, _: *const RECT) -> Result<()> {
        guarded(|| {
            if !self.state.active.get() {
                return Ok(());
            }
            trace::log(&format!("langbar.click kind={}", click.0));
            if click == TF_LBI_CLK_RIGHT {
                let result = settings_menu(pt);
                trace::log(&format!("langbar.menu result={result:?}"));
                return result;
            }
            if click != TF_LBI_CLK_LEFT {
                return Err(E_NOTIMPL.into());
            }
            (self.click)();
            Ok(())
        })
    }
    fn InitMenu(&self, _: Ref<ITfMenu>) -> Result<()> {
        guarded(|| Err(E_NOTIMPL.into()))
    }
    fn OnMenuSelect(&self, _: u32) -> Result<()> {
        guarded(|| Err(E_NOTIMPL.into()))
    }
    fn GetIcon(&self) -> Result<HICON> {
        guarded(|| unsafe {
            // Re-read on every call: a theme change shows up at the next mode or focus update.
            let id = mode_icon_id(self.state.shown.get().1, light_taskbar());
            let dpi = GetDpiForSystem();
            let image = LoadImageW(
                Some(HINSTANCE(MODULE.load(Ordering::SeqCst) as *mut _)),
                PCWSTR(id as *const u16),
                IMAGE_ICON,
                GetSystemMetricsForDpi(SM_CXSMICON, dpi),
                GetSystemMetricsForDpi(SM_CYSMICON, dpi),
                LR_DEFAULTCOLOR,
            )?;
            // No LR_SHARED: the caller owns and destroys this fresh HICON.
            Ok(HICON(image.0))
        })
    }
    fn GetText(&self) -> Result<BSTR> {
        guarded(|| {
            Ok(BSTR::from(if self.state.shown.get().1 == Mode::English {
                "英"
            } else {
                "中"
            }))
        })
    }
}
impl ITfSource_Impl for Button_Impl {
    fn AdviseSink(&self, iid: *const GUID, unknown: Ref<IUnknown>) -> Result<u32> {
        guarded(|| {
            if iid.is_null() {
                return Err(E_POINTER.into());
            }
            if unsafe { *iid } != ITfLangBarItemSink::IID {
                return Err(E_NOINTERFACE.into());
            }
            let sink = unknown.ok()?.cast()?;
            if !self.state.active.get() {
                return Err(E_FAIL.into());
            }
            let cookie = self.state.next_cookie.get();
            self.state
                .next_cookie
                .set(cookie.checked_add(1).ok_or(E_FAIL)?);
            self.state.sinks.borrow_mut().push((cookie, sink));
            Ok(cookie)
        })
    }
    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        guarded(|| {
            let removed = {
                let mut sinks = self.state.sinks.borrow_mut();
                let index = sinks
                    .iter()
                    .position(|(id, _)| *id == cookie)
                    .ok_or(E_INVALIDARG)?;
                sinks.remove(index)
            };
            drop(removed);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_menu_requires_foreground_on_current_thread() {
        assert!(use_foreground_menu(Some(42), 42));
        assert!(!use_foreground_menu(Some(7), 42));
        assert!(!use_foreground_menu(None, 42));
        assert!(!use_foreground_menu(Some(0), 42));
        assert!(!use_foreground_menu(Some(0), 0));
    }

    #[test]
    fn mode_icon_follows_taskbar_theme_and_defaults_to_dark() {
        assert_eq!(mode_icon_id(Mode::Chinese, Some(true)), 102);
        assert_eq!(mode_icon_id(Mode::English, Some(true)), 103);
        assert_eq!(mode_icon_id(Mode::Chinese, Some(false)), 104);
        assert_eq!(mode_icon_id(Mode::English, Some(false)), 105);
        assert_eq!(mode_icon_id(Mode::Chinese, None), 104);
        assert_eq!(mode_icon_id(Mode::English, None), 105);
        // The real read must agree with reg.exe (read-only) about the current user's value.
        let out = std::process::Command::new("reg")
            .args([r"query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "/v", "SystemUsesLightTheme"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let expected = if !out.status.success() {
            None
        } else if text.contains("0x1") {
            Some(true)
        } else if text.contains("0x0") {
            Some(false)
        } else {
            panic!("unexpected reg output: {text}")
        };
        assert_eq!(light_taskbar(), expected);
    }

    #[implement(ITfLangBarItemSink)]
    struct Sink {
        source: ITfSource,
        item: ITfLangBarItemButton,
        cookie: Rc<Cell<u32>>,
        observed: Rc<RefCell<Vec<String>>>,
    }
    impl ITfLangBarItemSink_Impl for Sink_Impl {
        fn OnUpdate(&self, flags: u32) -> Result<()> {
            guarded(|| unsafe {
                assert_eq!(flags, TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP);
                self.observed
                    .borrow_mut()
                    .push(self.item.GetText()?.to_string());
                // TSF can re-enter us from OnUpdate, including removing its subscription.
                self.source.UnadviseSink(self.cookie.get())
            })
        }
    }
    #[test]
    fn com_button_text_click_and_reentrant_sink_subscription() {
        let state = State::new();
        let clicks = Rc::new(Cell::new(0));
        let count = clicks.clone();
        let item = Button::create(state.clone(), move || count.set(count.get() + 1));
        let button: ITfLangBarItemButton = item.cast().unwrap();
        let source: ITfSource = item.cast().unwrap();
        let cookie = Rc::new(Cell::new(0));
        let observed = Rc::new(RefCell::new(Vec::new()));
        let sink: ITfLangBarItemSink = Sink {
            source: source.clone(),
            item: button.clone(),
            cookie: cookie.clone(),
            observed: observed.clone(),
        }
        .into();
        unsafe {
            let mut info = TF_LANGBARITEMINFO::default();
            item.GetInfo(&mut info).unwrap();
            assert_eq!(info.guidItem, GUID_LBI_INPUTMODE);
            assert_eq!(info.clsidService, GUID::from_u128(ids::CLSID_TIP));
            assert_eq!(info.dwStyle, TF_LBI_STYLE_BTN_BUTTON);
            assert_eq!(button.GetText().unwrap().to_string(), "中");
            cookie.set(source.AdviseSink(&ITfLangBarItemSink::IID, &sink).unwrap());
            state.update(1, Some(Mode::English));
            assert_eq!(*observed.borrow(), ["英"]);
            assert_eq!(
                button.GetTooltipString().unwrap().to_string(),
                "英文模式（Shift 切換）；右鍵：設定"
            );
            state.update(1, Some(Mode::Chinese));
            assert_eq!(observed.borrow().len(), 1);
            assert_eq!(button.GetText().unwrap().to_string(), "中");
            button
                .OnClick(TF_LBI_CLK_LEFT, POINT::default(), std::ptr::null())
                .unwrap();
            assert_eq!(clicks.get(), 1);
            assert_eq!(
                button
                    .OnClick(TfLBIClick(-1), POINT::default(), std::ptr::null())
                    .unwrap_err()
                    .code(),
                E_NOTIMPL
            );
            assert_eq!(clicks.get(), 1);
        }
    }
    #[implement(ITfLangBarItemSink)]
    struct HoldingSink {
        source: ITfSource,
        state: std::rc::Weak<State>,
        updates: Rc<Cell<u32>>,
        drops: Rc<Cell<u32>>,
        deactivate_on_update: bool,
    }
    impl ITfLangBarItemSink_Impl for HoldingSink_Impl {
        fn OnUpdate(&self, _: u32) -> Result<()> {
            self.updates.set(self.updates.get() + 1);
            if self.deactivate_on_update {
                self.state.upgrade().unwrap().deactivate();
            }
            Ok(())
        }
    }
    impl Drop for HoldingSink {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
            // Final Release may re-enter the source: no sinks borrow may survive.
            let _ = unsafe { self.source.UnadviseSink(1) };
        }
    }
    fn holding_sink(
        state: &Rc<State>,
        source: &ITfSource,
        deactivate_on_update: bool,
    ) -> (ITfLangBarItemSink, Rc<Cell<u32>>, Rc<Cell<u32>>) {
        let updates = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        let sink = HoldingSink {
            source: source.clone(),
            state: Rc::downgrade(state),
            updates: updates.clone(),
            drops: drops.clone(),
            deactivate_on_update,
        }
        .into();
        (sink, updates, drops)
    }
    #[test]
    fn deactivate_clears_sinks_breaks_cycles_and_rejects_new_subscriptions() {
        let state = State::new();
        let weak = Rc::downgrade(&state);
        let item = Button::create(state.clone(), || panic!("inactive button clicked"));
        let source: ITfSource = item.cast().unwrap();
        let button: ITfLangBarItemButton = item.cast().unwrap();
        let (sink, updates, drops) = holding_sink(&state, &source, false);
        unsafe {
            source.AdviseSink(&ITfLangBarItemSink::IID, &sink).unwrap();
        }
        state.update(1, Some(Mode::English));
        assert_eq!(updates.get(), 1);
        state.deactivate();
        assert!(state.sinks.borrow().is_empty());
        assert_eq!(state.shown.get(), (0, Mode::Chinese));
        state.update(2, Some(Mode::English));
        assert_eq!(updates.get(), 1);
        unsafe {
            assert!(source.AdviseSink(&ITfLangBarItemSink::IID, &sink).is_err());
            button
                .OnClick(TF_LBI_CLK_LEFT, POINT::default(), std::ptr::null())
                .unwrap();
        }
        drop(sink);
        assert_eq!(drops.get(), 1);

        // A second sink holds the source with no external references to the sink.
        let fresh = State::new();
        let fresh_item = Button::create(fresh.clone(), || {});
        let fresh_source: ITfSource = fresh_item.cast().unwrap();
        let (sink, _, released) = holding_sink(&fresh, &fresh_source, false);
        unsafe {
            fresh_source
                .AdviseSink(&ITfLangBarItemSink::IID, &sink)
                .unwrap();
        }
        drop(sink);
        drop(fresh_source);
        drop(fresh_item);
        fresh.deactivate();
        assert_eq!(released.get(), 1);
        drop(button);
        drop(source);
        drop(item);
        drop(state);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn reentrant_deactivate_skips_remaining_snapshot_notifications() {
        let state = State::new();
        let item = Button::create(state.clone(), || {});
        let source: ITfSource = item.cast().unwrap();
        let (first, first_updates, _) = holding_sink(&state, &source, true);
        let (second, second_updates, _) = holding_sink(&state, &source, false);
        unsafe {
            source.AdviseSink(&ITfLangBarItemSink::IID, &first).unwrap();
            source
                .AdviseSink(&ITfLangBarItemSink::IID, &second)
                .unwrap();
        }
        state.update(1, Some(Mode::English));
        assert_eq!(first_updates.get(), 1);
        assert_eq!(second_updates.get(), 0);
        state.update(2, Some(Mode::Chinese));
        assert_eq!(first_updates.get(), 1);
        assert_eq!(second_updates.get(), 0);
    }
    #[test]
    fn display_follows_focus_and_keeps_last_mode_without_ready_context() {
        for previous in [Mode::Chinese, Mode::English] {
            assert_eq!(displayed_mode(None, previous), previous);
            for mode in [Mode::Chinese, Mode::English] {
                assert_eq!(displayed_mode(Some(mode), previous), mode);
            }
        }
    }
    #[test]
    fn notify_on_mode_or_context_change_only() {
        assert!(!needs_update((1, Mode::Chinese), (1, Mode::Chinese)));
        assert!(needs_update((1, Mode::Chinese), (1, Mode::English)));
        assert!(needs_update((1, Mode::Chinese), (2, Mode::Chinese)));
        assert!(needs_update((1, Mode::English), (0, Mode::English)));
    }
}
