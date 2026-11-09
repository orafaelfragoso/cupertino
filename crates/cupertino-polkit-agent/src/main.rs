//! The `cupertino-polkit-agent` process, which the server starts for the session on a connection that may only draw the
//! authentication prompt. See the README.

mod app;
mod harden;
mod logind;

use std::process::ExitCode;

use cupertino_diagnostic_events as diag;
use cupertino_polkit_agent::zeroing::ZeroOnFree;

/// The agent handles passwords: nothing it frees keeps one.
#[global_allocator]
static ALLOCATOR: ZeroOnFree<std::alloc::System> = ZeroOnFree(std::alloc::System);

fn main() -> ExitCode {
    // first, before the process hardens itself (which forbids neither opening the user's event store nor writing it):
    // the panic hook and the store are in place for everything below
    diag::init(diag::component!(), "polkit", diag::Sink::UserStore);
    if let Err(e) = harden::harden() {
        eprintln!("cupertino-polkit-agent: hardening the process: {e}");
        diag::exit_error(&format!("hardening the process: {e}"));
        return ExitCode::FAILURE;
    }
    // `run` only returns on failure, which makes the server restart the process
    if let Err(e) = app::run() {
        eprintln!("cupertino-polkit-agent: {e}");
        diag::exit_error(&e);
    }
    ExitCode::FAILURE
}
