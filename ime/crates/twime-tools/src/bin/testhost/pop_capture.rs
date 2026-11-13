//! Capture only testhost-owned HWNDs, in-process, without activating any window.
use std::{ffi::c_void, path::Path, ptr::null_mut};
use windows::{core::GUID, Win32::{Foundation::{HWND, RECT}, Graphics::Gdi::*,
    UI::{HiDpi::*, WindowsAndMessaging::*}}};

#[repr(C)]
struct StartupInput { version: u32, callback: *mut c_void, no_thread: i32, no_codecs: i32 }
#[link(name = "gdiplus")]
unsafe extern "system" {
    fn GdiplusStartup(token: *mut usize, input: *const StartupInput, output: *mut c_void) -> i32;
    fn GdiplusShutdown(token: usize);
    fn GdipCreateBitmapFromHBITMAP(bitmap: *mut c_void, palette: *mut c_void, image: *mut *mut c_void) -> i32;
    fn GdipSaveImageToFile(image: *mut c_void, path: *const u16, encoder: *const GUID, params: *const c_void) -> i32;
    fn GdipDisposeImage(image: *mut c_void) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, dc: HDC, flags: u32) -> windows::core::BOOL;
}

pub(super) fn save(hwnd: HWND, path: &Path) -> Result<(), String> {
    unsafe {
        // This is the scenario worker thread, not the UI thread. Explicit awareness keeps
        // GetWindowRect, the window DC and bitmap dimensions in the same physical pixels.
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = capture(hwnd, path);
        if !previous.0.is_null() { let _ = SetThreadDpiAwarenessContext(previous); }
        result.map_err(|e| format!("{}: {e}", path.display()))
    }
}

unsafe fn capture(hwnd: HWND, path: &Path) -> Result<(), String> {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != std::process::id() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return Err("window is foreign, hidden or minimized".into());
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).map_err(|e| e.to_string())?;
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        if width <= 0 || height <= 0 { return Err("empty window bounds".into()); }
        let source = GetWindowDC(Some(hwnd));
        if source.is_invalid() { return Err("GetWindowDC failed".into()); }
        let dc = CreateCompatibleDC(Some(source));
        if dc.is_invalid() {
            ReleaseDC(Some(hwnd), source);
            return Err("CreateCompatibleDC failed".into());
        }
        let bitmap = CreateCompatibleBitmap(source, width, height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(dc);
            ReleaseDC(Some(hwnd), source);
            return Err("CreateCompatibleBitmap failed".into());
        }
        let old = SelectObject(dc, bitmap.into());
        // Full window bounds include non-client areas. PW_RENDERFULLCONTENT captures
        // the complete popup and host, including child controls; no desktop capture.
        let rendered = PrintWindow(hwnd, dc, 2).as_bool()
            || BitBlt(dc, 0, 0, width, height, Some(source), 0, 0, SRCCOPY).is_ok();
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        ReleaseDC(Some(hwnd), source);
        let result = if rendered { save_png(bitmap, path) } else { Err("PrintWindow/BitBlt failed".into()) };
        let _ = DeleteObject(bitmap.into());
        if result.is_ok() { println!("INFO\tpng\t{} {width}x{height} physical pixels", path.display()); }
        result
    }
}

unsafe fn save_png(bitmap: HBITMAP, path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    unsafe {
        let input = StartupInput { version: 1, callback: null_mut(), no_thread: 0, no_codecs: 0 };
        let mut token = 0;
        let status = GdiplusStartup(&mut token, &input, null_mut());
        if status != 0 { return Err(format!("GdiplusStartup status={status}")); }
        let mut image = null_mut();
        let mut status = GdipCreateBitmapFromHBITMAP(bitmap.0, null_mut(), &mut image);
        if status == 0 {
            let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let png = GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);
            status = GdipSaveImageToFile(image, name.as_ptr(), &png, std::ptr::null());
            GdipDisposeImage(image);
        }
        GdiplusShutdown(token);
        if status == 0 { Ok(()) } else { Err(format!("GDI+ PNG status={status}")) }
    }
}
