// Virtual key codes (the position on an ANSI keyboard, whatever the layout prints) -> TWIME_KEY_*, and the modifier
// flags -> TWIME_MOD_*. Pure functions.

import AppKit
import CTwime

enum KeyMap {
    /// kVK_ANSI_* / kVK_* (Carbon HIToolbox Events.h) -> TWIME_KEY_*.
    static let table: [UInt16: UInt32] = {
        var t: [UInt16: UInt32] = [:]
        let letters: [(UInt16, Character)] = [
            (0, "a"), (11, "b"), (8, "c"), (2, "d"), (14, "e"), (3, "f"), (5, "g"), (4, "h"), (34, "i"), (38, "j"), (40, "k"),
            (37, "l"), (46, "m"), (45, "n"), (31, "o"), (35, "p"), (12, "q"), (15, "r"), (1, "s"), (17, "t"), (32, "u"),
            (9, "v"), (13, "w"), (7, "x"), (16, "y"), (6, "z"),
            (29, "0"), (18, "1"), (19, "2"), (20, "3"), (21, "4"), (23, "5"), (22, "6"), (26, "7"), (28, "8"), (25, "9"),
        ]
        for (code, ch) in letters { t[code] = UInt32(ch.asciiValue!) }
        let others: [(UInt16, Int32)] = [
            (27, TWIME_KEY_MINUS), (43, TWIME_KEY_COMMA), (47, TWIME_KEY_PERIOD), (44, TWIME_KEY_SLASH),
            (41, TWIME_KEY_SEMICOLON), (49, TWIME_KEY_SPACE), (36, TWIME_KEY_ENTER), (76, TWIME_KEY_ENTER),
            (53, TWIME_KEY_ESC), (51, TWIME_KEY_BACKSPACE), (117, TWIME_KEY_DELETE), (48, TWIME_KEY_TAB),
            (123, TWIME_KEY_LEFT), (124, TWIME_KEY_RIGHT), (126, TWIME_KEY_UP), (125, TWIME_KEY_DOWN),
            (115, TWIME_KEY_HOME), (119, TWIME_KEY_END), (116, TWIME_KEY_PAGE_UP), (121, TWIME_KEY_PAGE_DOWN),
            (50, TWIME_KEY_BACKQUOTE), (33, TWIME_KEY_LBRACKET), (30, TWIME_KEY_RBRACKET), (39, TWIME_KEY_QUOTE),
            (42, TWIME_KEY_BACKSLASH), (24, TWIME_KEY_EQUALS), (65, TWIME_KEY_NUMPAD_DECIMAL),
            (69, TWIME_KEY_NUMPAD_ADD), (78, TWIME_KEY_NUMPAD_SUBTRACT), (67, TWIME_KEY_NUMPAD_MULTIPLY),
            (75, TWIME_KEY_NUMPAD_DIVIDE),
        ]
        for (code, key) in others { t[code] = UInt32(key) }
        // Number pad 0..9.
        for (i, code) in ([82, 83, 84, 85, 86, 87, 88, 89, 91, 92] as [UInt16]).enumerated() { t[code] = UInt32(TWIME_KEY_NUMPAD_0) + UInt32(i) }
        return t
    }()

    static func key(_ keyCode: UInt16) -> UInt32 {
        table[keyCode] ?? UInt32(TWIME_KEY_OTHER)
    }

    static func mods(_ f: NSEvent.ModifierFlags) -> UInt32 {
        var m: Int32 = 0
        if f.contains(.shift) { m |= TWIME_MOD_SHIFT }
        if f.contains(.control) { m |= TWIME_MOD_CTRL }
        if f.contains(.option) { m |= TWIME_MOD_ALT }
        if f.contains(.command) { m |= TWIME_MOD_SUPER }
        if f.contains(.capsLock) { m |= TWIME_MOD_CAPS }
        return UInt32(m)
    }
}
