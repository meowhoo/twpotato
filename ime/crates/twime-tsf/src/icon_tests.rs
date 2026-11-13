use std::collections::BTreeMap;

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn resource_groups_reference_all_eight_icon_sizes() {
    let bytes = include_bytes!("../res/twime-icons.res");
    let mut records = BTreeMap::new();
    let mut offset = 0;
    while offset < bytes.len() {
        assert_eq!(offset % 4, 0);
        let len = u32_at(bytes, offset) as usize;
        let header = u32_at(bytes, offset + 4) as usize;
        assert_eq!(header, 32);
        assert_eq!(u16_at(bytes, offset + 8), 0xffff);
        assert_eq!(u16_at(bytes, offset + 12), 0xffff);
        let kind = u16_at(bytes, offset + 10);
        let id = u16_at(bytes, offset + 14);
        let data = &bytes[offset + header..offset + header + len];
        if kind == 0 {
            assert_eq!((offset, id, len), (0, 0, 0));
        } else {
            assert!(records.insert((kind, id), data).is_none());
        }
        offset = (offset + header + len + 3) & !3;
    }
    assert_eq!(offset, bytes.len());
    assert_eq!(records.len(), 45);
    assert_eq!(
        records
            .keys()
            .filter(|(kind, _)| *kind == 14)
            .map(|(_, id)| *id)
            .collect::<Vec<_>>(),
        [101, 102, 103, 104, 105]
    );
    let mut referenced = std::collections::BTreeSet::new();
    for id in 101..=105 {
        let group = records[&(14, id)];
        assert_eq!(u16_at(group, 0), 0);
        assert_eq!(u16_at(group, 2), 1);
        assert_eq!(u16_at(group, 4), 8);
        assert_eq!(group.len(), 6 + 8 * 14);
        for (i, size) in [16, 20, 24, 32, 40, 48, 64, 256].into_iter().enumerate() {
            let entry = 6 + 14 * i;
            assert_eq!(group[entry] as u32, size % 256);
            assert_eq!(group[entry + 1] as u32, size % 256);
            assert_eq!(u16_at(group, entry + 4), 1);
            assert_eq!(u16_at(group, entry + 6), 32);
            let icon_id = u16_at(group, entry + 12);
            assert!(referenced.insert(icon_id));
            let icon = records[&(3, icon_id)];
            assert_eq!(u32_at(group, entry + 8) as usize, icon.len());
            assert_eq!(&icon[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(u32::from_be_bytes(icon[16..20].try_into().unwrap()), size);
            assert_eq!(u32::from_be_bytes(icon[20..24].try_into().unwrap()), size);
        }
    }
    assert_eq!(referenced.len(), 40);
}

#[test]
fn built_dll_loads_every_group_at_small_and_large_sizes() {
    use windows::{
        Win32::{
            Foundation::{FreeLibrary, HINSTANCE},
            System::LibraryLoader::{LOAD_LIBRARY_AS_DATAFILE, LoadLibraryExW},
            UI::WindowsAndMessaging::{
                DestroyIcon, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
            },
        },
        core::PCWSTR,
    };
    let dll = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("twime_tsf.dll");
    // `cargo test` alone need not build the cdylib. The .res test above is always mandatory.
    if !dll.exists() {
        eprintln!(
            "DLL icon test skipped: run cargo build -p twime-tsf first ({})",
            dll.display()
        );
        return;
    }
    let path: Vec<u16> = dll.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let module = LoadLibraryExW(PCWSTR(path.as_ptr()), None, LOAD_LIBRARY_AS_DATAFILE).unwrap();
        for id in 101..=105 {
            for size in [16, 32] {
                let icon = LoadImageW(
                    Some(HINSTANCE(module.0)),
                    PCWSTR(id as *const u16),
                    IMAGE_ICON,
                    size,
                    size,
                    LR_DEFAULTCOLOR,
                )
                .unwrap();
                DestroyIcon(HICON(icon.0)).unwrap();
            }
        }
        FreeLibrary(module).unwrap();
    }
}
use std::os::windows::ffi::OsStrExt;
