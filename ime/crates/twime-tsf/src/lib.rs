//! TSF text input processor, a thin shell: all conversion happens in the engine process.
//!
//! The DLL is loaded into host processes: it never calls the model, never panics across the COM boundary
//! (every entry is guarded by `guard`), does nothing in `DllMain` and starts no thread.
//! Written from the Microsoft TSF documentation only.

#![cfg(windows)]
#![allow(non_snake_case)]

mod attribute;
mod hint;
mod cand;
mod caret_trust;
mod edit;
mod keys;
mod langbar;
#[cfg(test)]
mod icon_tests;
mod link;
mod shift;
mod tip;
mod trace;

use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicI32, AtomicIsize, Ordering},
};

use twime_proto::ids;
use windows::{
    Win32::{
        Foundation::{CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, HMODULE, S_FALSE, S_OK},
        System::{
            Com::{IClassFactory, IClassFactory_Impl},
            LibraryLoader::DisableThreadLibraryCalls,
        },
    },
    core::{BOOL, GUID, HRESULT, IUnknown, Interface, Ref, implement},
};

pub(crate) static MODULE: AtomicIsize = AtomicIsize::new(0);
/// Live COM objects plus server locks; `DllCanUnloadNow` is `S_OK` only at zero.
pub(crate) static LIVE: AtomicI32 = AtomicI32::new(0);

/// Runs `f`, turning a panic into `None` (a panic must never unwind into the host).
pub(crate) fn guard<T>(f: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(_) => {
            trace::log("PANIC caught at COM boundary");
            None
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(hinst: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(hinst.0 as isize, Ordering::SeqCst);
        unsafe {
            let _ = DisableThreadLibraryCalls(hinst);
        }
    }
    BOOL(1)
}

#[implement(IClassFactory)]
struct Factory;

impl Factory {
    fn new() -> Factory {
        LIVE.fetch_add(1, Ordering::SeqCst);
        Factory
    }
}
impl Drop for Factory {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> windows::core::Result<()> {
        guard(|| unsafe {
            if ppv.is_null() || riid.is_null() {
                return Err(E_POINTER.into());
            }
            *ppv = std::ptr::null_mut();
            if outer.is_some() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let obj: windows::Win32::UI::TextServices::ITfTextInputProcessorEx = tip::Tip::new().into();
            obj.query(riid, ppv).ok()
        })
        .unwrap_or(Err(E_FAIL.into()))
    }

    fn LockServer(&self, lock: BOOL) -> windows::core::Result<()> {
        if lock.as_bool() {
            LIVE.fetch_add(1, Ordering::SeqCst);
        } else {
            LIVE.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    guard(|| unsafe {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        *ppv = std::ptr::null_mut();
        if *rclsid != GUID::from_u128(ids::CLSID_TIP) {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let f: IClassFactory = Factory::new().into();
        f.query(riid, ppv)
    })
    .unwrap_or(E_FAIL)
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE.load(Ordering::SeqCst) <= 0 { S_OK } else { S_FALSE }
}
