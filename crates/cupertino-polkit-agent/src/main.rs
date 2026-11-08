//! The `cupertino-polkit-agent` process, which the server starts for the session on a connection that may only draw the
//! authentication prompt. See the README.

mod app;
mod harden;
mod logind;

use std::process::ExitCode;

use cupertino_polkit_agent::zeroing::ZeroOnFree;

/// The agent handles passwords: nothing it frees keeps one.
#[global_allocator]
static ALLOCATOR: ZeroOnFree<std::alloc::System> = ZeroOnFree(std::alloc::System);

fn main() -> ExitCode {
    if let Err(e) = harden::harden() {
        eprintln!("cupertino-polkit-agent: hardening the process: {e}");
        return ExitCode::FAILURE;
    }
    // `run` only returns on failure, which makes the server restart the process
    if let Err(e) = app::run() {
        eprintln!("cupertino-polkit-agent: {e}");
    }
    ExitCode::FAILURE
}
