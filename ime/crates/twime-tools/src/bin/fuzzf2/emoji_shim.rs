//! EMOJI evaluation: the only parts that touch the emoji feature itself (the snapshot build before it has a stub of this file).
use super::*;

/// `base` with the emoji table read from `path`.
pub(super) fn with_table(base: Config, path: &str) -> Config {
    let text = fs::read_to_string(path).expect("emoji.tsv");
    Config { emoji: Some(Arc::new(twime_model::emoji::EmojiTable::parse(&text).expect("emoji.tsv"))), ..base }
}

/// Which items of the open list are emoji.
pub(super) fn raw_flags(s: &Session) -> Vec<bool> {
    s.candidate_raw_flags()
}
