//! One TSF display attribute for LEXUX hints. No host registration here.
use std::cell::{Cell, RefCell};
use windows::{
    Win32::{
        Foundation::{E_INVALIDARG, E_POINTER, S_FALSE},
        UI::TextServices::*,
    },
    core::{BSTR, GUID, Result, implement},
};

pub const GUID_HINT: GUID = GUID::from_u128(0x05ab532e_2f4e_48d5_9c40_b05ba1d5a306);
fn default_attribute() -> TF_DISPLAYATTRIBUTE {
    TF_DISPLAYATTRIBUTE {
        lsStyle: TF_LS_DOT,
        ..Default::default()
    }
}
struct Lifetime;
impl Lifetime {
    fn new() -> Self {
        crate::LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        crate::LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[implement(ITfDisplayAttributeInfo)]
struct Info {
    value: RefCell<TF_DISPLAYATTRIBUTE>,
    _live: Lifetime,
}
fn info() -> ITfDisplayAttributeInfo {
    Info {
        value: RefCell::new(default_attribute()),
        _live: Lifetime::new(),
    }
    .into()
}
impl ITfDisplayAttributeInfo_Impl for Info_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(GUID_HINT)
    }
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("LEXUX hint"))
    }
    fn GetAttributeInfo(&self, p: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        if p.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *p = *self.value.borrow();
        }
        Ok(())
    }
    fn SetAttributeInfo(&self, p: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        if p.is_null() {
            return Err(E_POINTER.into());
        }
        *self.value.borrow_mut() = unsafe { *p };
        Ok(())
    }
    fn Reset(&self) -> Result<()> {
        *self.value.borrow_mut() = default_attribute();
        Ok(())
    }
}
#[implement(IEnumTfDisplayAttributeInfo)]
struct Enumeration {
    consumed: Cell<bool>,
    _live: Lifetime,
}
pub fn enumeration() -> IEnumTfDisplayAttributeInfo {
    Enumeration {
        consumed: Cell::new(false),
        _live: Lifetime::new(),
    }
    .into()
}
pub fn get(guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
    if guid.is_null() {
        return Err(E_POINTER.into());
    }
    if unsafe { *guid } != GUID_HINT {
        return Err(E_INVALIDARG.into());
    }
    Ok(info())
}
impl IEnumTfDisplayAttributeInfo_Impl for Enumeration_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(Enumeration {
            consumed: Cell::new(self.consumed.get()),
            _live: Lifetime::new(),
        }
        .into())
    }
    fn Next(
        &self,
        n: u32,
        out: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> Result<()> {
        if n > 0 && out.is_null() || fetched.is_null() && n != 1 {
            return Err(E_POINTER.into());
        }
        let count = u32::from(n > 0 && !self.consumed.get());
        unsafe {
            if !fetched.is_null() {
                *fetched = count;
            }
            if count == 1 {
                out.write(Some(info()));
                self.consumed.set(true);
            }
        }
        if count < n {
            Err(windows::core::Error::from_hresult(S_FALSE))
        } else {
            Ok(())
        }
    }
    fn Reset(&self) -> Result<()> {
        self.consumed.set(false);
        Ok(())
    }
    fn Skip(&self, n: u32) -> Result<()> {
        let available = u32::from(!self.consumed.get());
        if n > 0 {
            self.consumed.set(true);
        }
        if n > available {
            Err(windows::core::Error::from_hresult(S_FALSE))
        } else {
            Ok(())
        }
    }
}

/// UTF-16 ranges; a segment may contain supplementary-plane characters.
pub fn ranges(segments: &[twime_proto::Segment]) -> Vec<(i32, i32)> {
    let mut offset = 0;
    segments
        .iter()
        .filter_map(|s| {
            let n = s.text.encode_utf16().count() as i32;
            let start = offset;
            offset += n;
            (s.hint != 0 && n > 0).then_some((start, n))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_roundtrip_and_enumeration() {
        unsafe {
            let e = enumeration();
            let mut a = [None];
            let mut n = 0;
            e.Next(&mut a, &mut n).unwrap();
            assert!(n == 1);
            let i = a[0].as_ref().unwrap();
            assert!(i.GetGUID().unwrap() == GUID_HINT);
            let mut attr = TF_DISPLAYATTRIBUTE::default();
            i.GetAttributeInfo(&mut attr).unwrap();
            assert!(attr.lsStyle == TF_LS_DOT && !attr.fBoldLine.as_bool());
            attr.lsStyle = TF_LS_SOLID;
            i.SetAttributeInfo(&attr).unwrap();
            i.Reset().unwrap();
            i.GetAttributeInfo(&mut attr).unwrap();
            assert!(attr.lsStyle == TF_LS_DOT);
            let cloned = e.Clone().unwrap();
            cloned.Next(&mut a, &mut n).unwrap();
            assert!(n == 0);
            e.Reset().unwrap();
            e.Next(&mut a, &mut n).unwrap();
            assert!(n == 1);
        }
        assert!(get(&GUID::zeroed()).is_err());
        assert!(get(std::ptr::null()).is_err());
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use twime_proto::Segment;
    #[test]
    fn utf16_pending_unmarked_and_hint_moves_without_text_change() {
        let mut parts = vec![
            Segment {
                text: "1".into(),
                hint: 0,
                ..Default::default()
            },
            Segment {
                text: "\u{20000}".into(),
                hint: 1,
                ..Default::default()
            },
            Segment {
                text: "23".into(),
                hint: 2,
                ..Default::default()
            },
            Segment {
                text: String::new(),
                hint: 2,
                ..Default::default()
            },
        ];
        assert!(ranges(&parts) == vec![(1, 2), (3, 2)]);
        parts[0].hint = 2;
        parts[1].hint = 0;
        parts[2].hint = 0;
        assert!(ranges(&parts) == vec![(0, 1)]);
        parts[0].hint = 0;
        assert!(ranges(&parts).is_empty());
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;
    use windows::core::Interface;
    #[test]
    fn tip_exposes_display_attribute_provider_without_activation() {
        let tip: ITfTextInputProcessorEx = crate::tip::Tip::new().into();
        let provider: ITfDisplayAttributeProvider = tip.cast().unwrap();
        unsafe { assert!(provider.GetDisplayAttributeInfo(&GUID_HINT).unwrap().GetGUID().unwrap() == GUID_HINT); }
    }
}
