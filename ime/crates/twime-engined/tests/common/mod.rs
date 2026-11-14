//! Shared by the integration tests that start the engine process.

/// The engine refuses an elevated (high-integrity) token and has no development exception (src/main.rs). Hosted CI
/// runners run tests elevated, so a test that starts the engine skips itself there; on a normal desktop it runs.
#[cfg(windows)]
pub fn engine_start_refused() -> bool {
    let refused = twime_winpipe::current_token_security().map_or(false, |t| t.integrity_rid >= 0x3000);
    if refused {
        eprintln!("skipped: the engine refuses an elevated token (run the tests from a non-elevated shell)");
    }
    refused
}
