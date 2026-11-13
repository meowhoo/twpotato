use std::{collections::HashSet, path::Path};
use twime_proto::{Mode, ToggleKey, UserSettings};
#[derive(Debug, PartialEq, Eq)]
pub struct ParseError(pub usize);
#[derive(Debug)]
enum Value {
    Text(String),
    Bool(bool),
    Int(i64),
}
fn value(raw: &str) -> Option<Value> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        let mut text = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    let tail: String = chars.collect();
                    if !tail.trim_start().is_empty() && !tail.trim_start().starts_with('#') {
                        return None;
                    }
                    return Some(Value::Text(text));
                }
                '\\' => {
                    text.push(match chars.next()? {
                        '"' => '"',
                        '\\' => '\\',
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        _ => return None,
                    });
                }
                c if c.is_control() => return None,
                _ => text.push(c),
            }
        }
        None
    } else {
        let v = raw.split('#').next()?.trim();
        match v {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => {
                let digits = v.strip_prefix(['+', '-']).unwrap_or(v);
                if digits.is_empty()
                    || !digits.bytes().all(|c| c.is_ascii_digit())
                    || (digits.len() > 1 && digits.starts_with('0'))
                {
                    return None;
                }
                v.parse().ok().map(Value::Int)
            }
        }
    }
}
pub fn parse(text: &str) -> Result<UserSettings, ParseError> {
    let mut out = UserSettings::default();
    let mut seen = HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=').ok_or(ParseError(i + 1))?;
        let k = k.trim();
        if k.is_empty()
            || !k
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || !seen.insert(k)
        {
            return Err(ParseError(i + 1));
        }
        let v = value(v).ok_or(ParseError(i + 1))?;
        match (k, v) {
            ("version", Value::Int(1)) => {}
            ("toggle_key", Value::Text(v)) => {
                out.toggle_key = match v.as_str() {
                    "shift" => ToggleKey::Shift,
                    "left_shift" => ToggleKey::LeftShift,
                    "right_shift" => ToggleKey::RightShift,
                    "none" => ToggleKey::None,
                    _ => return Err(ParseError(i + 1)),
                }
            }
            ("default_mode", Value::Text(v)) => {
                out.default_mode = match v.as_str() {
                    "chinese" => Mode::Chinese,
                    "english" => Mode::English,
                    _ => return Err(ParseError(i + 1)),
                }
            }
            ("punctuation", Value::Text(v)) => {
                out.punct_half = match v.as_str() {
                    "full" => false,
                    "half" => true,
                    _ => return Err(ParseError(i + 1)),
                }
            }
            ("symbol_table", Value::Bool(v)) => out.symbol_table = v,
            ("cache", Value::Bool(v)) => out.cache = v,
            (
                "version" | "toggle_key" | "default_mode" | "punctuation" | "symbol_table"
                | "cache",
                _,
            ) => return Err(ParseError(i + 1)),
            _ => {}
        }
    }
    Ok(out)
}
pub fn serialize(s: &UserSettings) -> String {
    let key = match s.toggle_key {
        ToggleKey::Shift => "shift",
        ToggleKey::LeftShift => "left_shift",
        ToggleKey::RightShift => "right_shift",
        ToggleKey::None => "none",
    };
    format!(
        "# 地瓜輸入法 設定\nversion = 1\ntoggle_key = \"{key}\" # shift | left_shift | right_shift | none\ndefault_mode = \"{}\" # chinese | english\npunctuation = \"{}\" # full | half\nsymbol_table = {} # ` 開小符號表\ncache = {} # 預留：目前不生效\n",
        if s.default_mode == Mode::Chinese {
            "chinese"
        } else {
            "english"
        },
        if s.punct_half { "half" } else { "full" },
        s.symbol_table,
        s.cache
    )
}
pub fn legacy_defaults(install_root: &Path) -> UserSettings {
    let mut s = UserSettings::default();
    if let Ok(text) = std::fs::read_to_string(install_root.join("ux.cfg")) {
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
            match k.as_str() {
                "shift_tap" => {
                    s.toggle_key = if matches!(v.as_str(), "off" | "0" | "false") {
                        ToggleKey::None
                    } else {
                        ToggleKey::Shift
                    }
                }
                "default_mode" => {
                    s.default_mode = if v == "english" {
                        Mode::English
                    } else {
                        Mode::Chinese
                    }
                }
                "punct" => s.punct_half = v == "half",
                "symbol_table" => s.symbol_table = !matches!(v.as_str(), "off" | "0" | "false"),
                _ => {}
            }
        }
    }
    s
}
