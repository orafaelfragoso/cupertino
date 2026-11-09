//! The `cupertino-sessiond` process: the manager, with the argument `worker` one worker, or with `--check-config` the check
//! of the settings file. See the README.

use std::process::ExitCode;

use cupertino_diagnostic_events as diag;
use cupertino_sessiond::zeroing::ZeroOnFree;
use cupertino_sessiond::{manager, worker};

/// The manager and the worker handle passwords: nothing they free keeps one.
#[global_allocator]
static ALLOCATOR: ZeroOnFree<std::alloc::System> = ZeroOnFree(std::alloc::System);

fn main() -> ExitCode {
    // first, in the manager and in each worker: the panic hook and the journal are in place for everything below,
    // before either hardens itself (which forbids neither the journal's socket nor a state file)
    diag::init(diag::component!(), "login", diag::Sink::Journal);
    let mut arguments = std::env::args().skip(1);
    match (arguments.next().as_deref(), arguments.next()) {
        (None, None) => manager::run(),
        (Some("worker"), None) => worker::run(),
        (Some("--check-config"), None) => manager::check_config(),
        _ => {
            eprintln!("usage: cupertino-sessiond [worker | --check-config]");
            diag::exit_error(&"usage: cupertino-sessiond [worker | --check-config]");
            ExitCode::FAILURE
        }
    }
}
