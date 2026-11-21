//! The `cupertino-prompts` process, which the server starts for the session on a connection with the `session`,
//! `prompt` and `consent` roles. See the README.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use cupertino_diagnostic_events as diag;
use cupertino_login_ipc::zeroing::ZeroOnFree;

/// The prompts handle passwords: nothing the process frees keeps one.
#[global_allocator]
static ALLOCATOR: ZeroOnFree<std::alloc::System> = ZeroOnFree(std::alloc::System);

fn main() -> ExitCode {
    // first, before the process hardens itself (which forbids neither opening the user's event store nor writing it):
    // the panic hook and the store are in place for everything below
    diag::init(diag::component!(), "prompts", diag::Sink::UserStore);
    if let Err(e) = cupertino_kit::process::harden() {
        eprintln!("cupertino-prompts: hardening the process: {e}");
        diag::exit_error(&format!("hardening the process: {e}"));
        return ExitCode::FAILURE;
    }
    // the prompts run for the whole session: `run` returns only when a connection is lost, and the server starts the
    // program again
    if let Err(e) = cupertino_prompts::run(cupertino_prompts::Options::installed()) {
        eprintln!("cupertino-prompts: {e}");
        diag::exit_error(&e);
    }
    ExitCode::FAILURE
}
