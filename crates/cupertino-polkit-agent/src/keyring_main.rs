//! The `cupertino-keyring-prompter` process, which the server starts for the session on a connection that may only
//! draw the authentication prompt. It owns the keyring daemon's prompter names on the session bus, so the keyring's
//! password questions are asked in the session's own prompt. See the README.

mod harden;
mod keyring_app;
mod logind;

use std::process::ExitCode;

use cupertino_diagnostic_events as diag;
use cupertino_polkit_agent::zeroing::ZeroOnFree;

/// The prompter handles passwords: nothing it frees keeps one.
#[global_allocator]
static ALLOCATOR: ZeroOnFree<std::alloc::System> = ZeroOnFree(std::alloc::System);

fn main() -> ExitCode {
    diag::init(
        diag::component!("cupertino-keyring-prompter"),
        "keyring",
        diag::Sink::UserStore,
    );
    if let Err(e) = harden::harden() {
        eprintln!("cupertino-keyring-prompter: hardening the process: {e}");
        diag::exit_error(&format!("hardening the process: {e}"));
        return ExitCode::FAILURE;
    }
    // `run` only returns on failure, which makes the server restart the process
    if let Err(e) = keyring_app::run() {
        eprintln!("cupertino-keyring-prompter: {e}");
        diag::exit_error(&e.to_string());
    }
    ExitCode::FAILURE
}
