//! Platform-independent input core: standard bopomofo keyboard, syllable composer, composition / candidate state
//! machine (`Session`) and conversion through `twime-model`.
//!
//! No Windows types, no clock, no file access: the same `KeyEvent` sequence always yields the same `Update` sequence.

pub mod fuzzy;
pub mod keymap;
mod engine_user;
pub use engine_user::UserLink;
mod session;

pub use session::{Config, Engine, Repair, RepairMode, Session};
