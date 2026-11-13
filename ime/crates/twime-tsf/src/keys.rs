//! Virtual key to `twime_proto::Key`, and the modifier state (assumes a US QWERTY key layout).
//!
//! On a connection that did not negotiate protocol version 3 the keys and the Caps Lock bit that only exist from
//! version 3 on are never produced (`ux` false): such a connection sees exactly what it saw before.

use twime_proto::{Key, KeyEvent, Mods};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};

pub fn key_from_vk(vk: u32, ux: bool) -> Key {
    match vk {
        0x41..=0x5A => Key::Letter(vk as u8 + 32),
        0x30..=0x39 => Key::Digit((vk - 0x30) as u8),
        0xBD => Key::Minus,
        0xBC => Key::Comma,
        0xBE => Key::Period,
        0xBF => Key::Slash,
        0xBA => Key::Semicolon,
        0x20 => Key::Space,
        0x0D => Key::Enter,
        0x1B => Key::Esc,
        0x08 => Key::Backspace,
        0x2E => Key::Delete,
        0x09 => Key::Tab,
        0x25 => Key::Left,
        0x27 => Key::Right,
        0x26 => Key::Up,
        0x28 => Key::Down,
        0x24 => Key::Home,
        0x23 => Key::End,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        _ if ux => key_from_vk_v3(vk),
        _ => Key::Other,
    }
}

/// The keys of protocol version 3: VK_OEM_3 `` ` ``, VK_OEM_4 `[`, VK_OEM_6 `]`, VK_OEM_7 `'`, VK_OEM_5 `\`, VK_OEM_PLUS `=`
/// and the number pad (digits, decimal point, + - * /).
fn key_from_vk_v3(vk: u32) -> Key {
    match vk {
        0xC0 => Key::Backquote,
        0xDB => Key::LBracket,
        0xDD => Key::RBracket,
        0xDE => Key::Quote,
        0xDC => Key::Backslash,
        0xBB => Key::Equals,
        0x60..=0x69 => Key::NumpadDigit((vk - 0x60) as u8),
        0x6E => Key::NumpadDecimal,
        0x6B => Key::NumpadOp(0),
        0x6D => Key::NumpadOp(1),
        0x6A => Key::NumpadOp(2),
        0x6F => Key::NumpadOp(3),
        _ => Key::Other,
    }
}

/// Shift (generic, left or right) as a virtual key code.
pub fn is_shift_vk(vk: usize) -> bool {
    matches!(vk, 0x10 | 0xA0 | 0xA1)
}

fn down(vk: u16) -> bool {
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

/// Caps Lock is on (the toggle state, not whether the key is held).
pub fn caps_lock_on() -> bool {
    unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) & 1 != 0 }
}

/// Ctrl, Alt or a Windows key is held right now.
pub fn command_held() -> bool {
    down(VK_CONTROL.0) || down(VK_MENU.0) || down(VK_LWIN.0) || down(VK_RWIN.0)
}

/// Is the other Shift key (not the one with `vk` / `scan`) held? When the side is unknown both are asked.
pub fn other_shift_held(vk: usize, lparam: isize) -> bool {
    const VK_LSHIFT: u16 = 0xA0;
    const VK_RSHIFT: u16 = 0xA1;
    let scan = (lparam >> 16) & 0xFF;
    let left = vk == 0xA0 || (vk == 0x10 && scan == 0x2A);
    let right = vk == 0xA1 || (vk == 0x10 && scan == 0x36);
    match (left, right) {
        (true, _) => down(VK_RSHIFT),
        (_, true) => down(VK_LSHIFT),
        _ => down(VK_LSHIFT) && down(VK_RSHIFT),
    }
}

pub fn current_mods(ux: bool) -> Mods {
    let mut m = 0u8;
    if down(VK_SHIFT.0) {
        m |= Mods::SHIFT;
    }
    if down(VK_CONTROL.0) {
        m |= Mods::CTRL;
    }
    if down(VK_MENU.0) {
        m |= Mods::ALT;
    }
    if down(VK_LWIN.0) || down(VK_RWIN.0) {
        m |= Mods::WIN;
    }
    if ux && caps_lock_on() {
        m |= Mods::CAPS;
    }
    Mods(m)
}

pub fn key_event(vk: usize, is_down: bool, ux: bool) -> KeyEvent {
    KeyEvent { key: key_from_vk(vk as u32, ux), mods: current_mods(ux), down: is_down }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_3_keys_only_exist_on_a_version_3_connection() {
        for (vk, k) in [
            (0xC0u32, Key::Backquote),
            (0xDB, Key::LBracket),
            (0xDD, Key::RBracket),
            (0xDE, Key::Quote),
            (0xDC, Key::Backslash),
            (0xBB, Key::Equals),
            (0x60, Key::NumpadDigit(0)),
            (0x69, Key::NumpadDigit(9)),
            (0x6E, Key::NumpadDecimal),
            (0x6B, Key::NumpadOp(0)),
            (0x6D, Key::NumpadOp(1)),
            (0x6A, Key::NumpadOp(2)),
            (0x6F, Key::NumpadOp(3)),
        ] {
            assert_eq!(key_from_vk(vk, true), k, "{vk:#x}");
            assert_eq!(key_from_vk(vk, false), Key::Other, "{vk:#x} must stay unknown on an older connection");
        }
        // The keys of version 1 and 2 map the same in both.
        for vk in (0x41..=0x5A).chain(0x30..=0x39).chain([0xBD, 0xBC, 0xBE, 0xBF, 0xBA, 0x20, 0x0D, 0x1B, 0x08]) {
            assert_eq!(key_from_vk(vk, true), key_from_vk(vk, false));
            assert_ne!(key_from_vk(vk, false), Key::Other);
        }
        // Shift, Ctrl, Caps Lock and the function keys are not keys of the engine.
        for vk in [0x10, 0xA0, 0xA1, 0x11, 0x14, 0x70, 0x5B] {
            assert_eq!(key_from_vk(vk, true), Key::Other);
        }
    }
}
