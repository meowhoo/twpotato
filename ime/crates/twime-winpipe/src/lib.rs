//! IPC plumbing shared by the engine, the shells and the tools: a named pipe on Windows (`win.rs`), a Unix domain
//! socket elsewhere (`unix.rs`). Both export the same `PipeServer` / `ServerConn` / `Client` surface.

// rust-toolchain.toml names no host, so that Linux and macOS can use it. On Windows the product is built with MSVC only
// (static C runtime, see .cargo/config.toml; the released behaviour depends on its math functions): a machine whose
// rustup default host is the GNU one must not switch silently.
#[cfg(all(windows, not(target_env = "msvc")))]
compile_error!("Windows builds need the MSVC toolchain: `rustup set default-host x86_64-pc-windows-msvc`, or RUSTUP_TOOLCHAIN=1.98.1-x86_64-pc-windows-msvc");

pub mod state;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
