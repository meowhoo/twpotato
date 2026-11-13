//! ZHUYIN-OUT evaluation: the only part that touches the feature itself (the snapshot build before it has a stub of this
//! file that reports no typed-bopomofo item).
use super::*;

/// Which items of the open list are the typed bopomofo.
pub(super) fn lit_flags(s: &Session) -> Vec<bool> {
    s.candidate_lit_flags()
}
