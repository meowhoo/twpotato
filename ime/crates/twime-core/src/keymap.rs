//! Standard bopomofo keyboard layout (table driven; another layout means another table).

use twime_proto::Key;

/// Slot of a bopomofo symbol within a syllable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Initial,
    Medial,
    Final,
    Tone,
}

/// Key position -> bopomofo symbol (standard layout).
const TABLE: &[(Key, char)] = &[
    (Key::Digit(1), 'ㄅ'),
    (Key::Letter(b'q'), 'ㄆ'),
    (Key::Letter(b'a'), 'ㄇ'),
    (Key::Letter(b'z'), 'ㄈ'),
    (Key::Digit(2), 'ㄉ'),
    (Key::Letter(b'w'), 'ㄊ'),
    (Key::Letter(b's'), 'ㄋ'),
    (Key::Letter(b'x'), 'ㄌ'),
    (Key::Digit(3), 'ˇ'),
    (Key::Letter(b'e'), 'ㄍ'),
    (Key::Letter(b'd'), 'ㄎ'),
    (Key::Letter(b'c'), 'ㄏ'),
    (Key::Digit(4), 'ˋ'),
    (Key::Letter(b'r'), 'ㄐ'),
    (Key::Letter(b'f'), 'ㄑ'),
    (Key::Letter(b'v'), 'ㄒ'),
    (Key::Digit(5), 'ㄓ'),
    (Key::Letter(b't'), 'ㄔ'),
    (Key::Letter(b'g'), 'ㄕ'),
    (Key::Letter(b'b'), 'ㄖ'),
    (Key::Digit(6), 'ˊ'),
    (Key::Letter(b'y'), 'ㄗ'),
    (Key::Letter(b'h'), 'ㄘ'),
    (Key::Letter(b'n'), 'ㄙ'),
    (Key::Digit(7), '˙'),
    (Key::Letter(b'u'), 'ㄧ'),
    (Key::Letter(b'j'), 'ㄨ'),
    (Key::Letter(b'm'), 'ㄩ'),
    (Key::Digit(8), 'ㄚ'),
    (Key::Letter(b'i'), 'ㄛ'),
    (Key::Letter(b'k'), 'ㄜ'),
    (Key::Comma, 'ㄝ'),
    (Key::Digit(9), 'ㄞ'),
    (Key::Letter(b'o'), 'ㄟ'),
    (Key::Letter(b'l'), 'ㄠ'),
    (Key::Period, 'ㄡ'),
    (Key::Digit(0), 'ㄢ'),
    (Key::Letter(b'p'), 'ㄣ'),
    (Key::Semicolon, 'ㄤ'),
    (Key::Slash, 'ㄥ'),
    (Key::Minus, 'ㄦ'),
];

/// The bopomofo symbol a key types, if any.
pub fn symbol(k: Key) -> Option<char> {
    TABLE.iter().find(|(key, _)| *key == k).map(|(_, c)| *c)
}

/// The key that types a symbol (inverse; used by test drivers).
pub fn key_of(c: char) -> Option<Key> {
    TABLE.iter().find(|(_, s)| *s == c).map(|(k, _)| *k)
}

/// The half-width character a key types on a US keyboard with or without Shift (number pad keys ignore Shift).
/// `None` for keys that type nothing (Space, Enter, arrows, ...).
pub fn char_of(k: Key, shift: bool) -> Option<char> {
    const SHIFTED_DIGITS: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];
    let pick = |plain: char, shifted: char| Some(if shift { shifted } else { plain });
    match k {
        Key::Letter(c) => Some(if shift { c.to_ascii_uppercase() as char } else { c as char }),
        Key::Digit(d) if d <= 9 => Some(if shift { SHIFTED_DIGITS[d as usize] } else { (b'0' + d) as char }),
        Key::Minus => pick('-', '_'),
        Key::Equals => pick('=', '+'),
        Key::Comma => pick(',', '<'),
        Key::Period => pick('.', '>'),
        Key::Slash => pick('/', '?'),
        Key::Semicolon => pick(';', ':'),
        Key::Quote => pick('\'', '"'),
        Key::Backquote => pick('`', '~'),
        Key::LBracket => pick('[', '{'),
        Key::RBracket => pick(']', '}'),
        Key::Backslash => pick('\\', '|'),
        Key::NumpadDigit(d) if d <= 9 => Some((b'0' + d) as char),
        Key::NumpadDecimal => Some('.'),
        Key::NumpadOp(o) => ['+', '-', '*', '/'].get(o as usize).copied(),
        _ => None,
    }
}

/// The full-width (Chinese) punctuation a key outputs in full-width style, for the keys that have one.
pub fn full_of(k: Key, shift: bool) -> Option<&'static str> {
    Some(match (k, shift) {
        (Key::Quote, false) => "、",
        (Key::LBracket, false) => "「",
        (Key::RBracket, false) => "」",
        (Key::Comma, true) => "，",
        (Key::Period, true) => "。",
        (Key::Slash, true) => "？",
        (Key::Semicolon, true) => "：",
        (Key::Quote, true) => "；",
        (Key::Digit(1), true) => "！",
        (Key::Digit(9), true) => "（",
        (Key::Digit(0), true) => "）",
        (Key::Digit(6), true) => "……",
        (Key::Minus, true) => "——",
        (Key::LBracket, true) => "『",
        (Key::RBracket, true) => "』",
        _ => return None,
    })
}

/// The character a key outputs in half-width punctuation style: the Shift partners of the keys that have a full-width
/// form give the unshifted-by-Shift mark (`Shift+,` is `,`, not `<`); everything else is the plain US character.
pub fn half_of(k: Key, shift: bool) -> Option<char> {
    match (k, shift) {
        (Key::Comma, true) => Some(','),
        (Key::Period, true) => Some('.'),
        (Key::Slash, true) => Some('?'),
        (Key::Semicolon, true) => Some(':'),
        (Key::Quote, true) => Some(';'),
        _ => char_of(k, shift),
    }
}

/// The character the key position stands for in the raw-key rescue (`Shift+Enter`): Space is a blank.
pub fn raw_char(k: Key) -> Option<char> {
    if k == Key::Space { Some(' ') } else { char_of(k, false) }
}

/// The nine entries of the small symbol table (opened by the backquote key), selected by the digits 1..=9.
pub const SYMBOL_TABLE: [&str; 12] = ["<", ">", "\"", "_", "^", "《", "》", "〈", "〉", "／", "±", "％"];
/// Symbols per page of the table (the digit keys 1-9); the table has two pages (9 + 3).
pub const SYMBOL_PAGE: usize = 9;

pub fn slot_of(c: char) -> Option<Slot> {
    Some(match c {
        'ㄅ'..='ㄙ' => Slot::Initial,
        'ㄧ' | 'ㄨ' | 'ㄩ' => Slot::Medial,
        'ㄚ'..='ㄦ' => Slot::Final,
        'ˊ' | 'ˇ' | 'ˋ' | '˙' => Slot::Tone,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_a_bijection_over_the_37_symbols_and_5_tone_keys() {
        assert_eq!(TABLE.len(), 41);
        for (i, (k, c)) in TABLE.iter().enumerate() {
            assert_eq!(symbol(*k), Some(*c));
            assert_eq!(key_of(*c), Some(*k));
            assert!(slot_of(*c).is_some(), "{c}");
            for (k2, c2) in &TABLE[i + 1..] {
                assert!(k != k2 && c != c2);
            }
        }
        // 21 initials, 3 medials, 13 finals, 4 tone marks.
        let count = |s: Slot| TABLE.iter().filter(|(_, c)| slot_of(*c) == Some(s)).count();
        assert_eq!((count(Slot::Initial), count(Slot::Medial), count(Slot::Final), count(Slot::Tone)), (21, 3, 13, 4));
    }

    #[test]
    fn well_known_positions() {
        assert_eq!(symbol(Key::Digit(1)), Some('ㄅ'));
        assert_eq!(symbol(Key::Letter(b'u')), Some('ㄧ'));
        assert_eq!(symbol(Key::Minus), Some('ㄦ'));
        assert_eq!(symbol(Key::Digit(6)), Some('ˊ'));
        assert_eq!(symbol(Key::Letter(b'3')), None);
    }
}
