//! Front-end logic shared by the Fcitx5, IBus and macOS shells (LINUX-plan 4.2 - 4.4): the engine connection
//! (find, start, reconnect), sessions and acknowledgements, the Shift single tap and the sensitive-field state.
//! No framework types. The C interface is `include/twime_shell.h` (`ffi.rs`); the Rust types are for the tests.
//!
//! Unix only. On Windows this crate is empty: the Windows shell is `twime-tsf`.
#![cfg(unix)]

mod ffi;
mod link;
mod paths;
mod shell;
mod shift;

pub use link::{CALL_TIMEOUT_MS, Config, SPAWN_EVERY, VERSION};
pub use shell::{Context, HOLD_LIMIT, Input, KeyCode, Reply, Shell, Ui};
