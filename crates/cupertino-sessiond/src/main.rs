//! The `cupertino-sessiond` process: the manager, with the argument `worker` one worker, or with `--check-config` the check
//! of the settings file. See the README.

use std::process::ExitCode;

use cupertino_sessiond::zeroing::ZeroOnFree;
use cupertino_sessiond::{manager, worker};

/// The manager and the worker handle passwords: nothing they free keeps one.
#[global_allocator]
static ALLOCATOR: ZeroOnFree<std::alloc::System> = ZeroOnFree(std::alloc::System);

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    match (arguments.next().as_deref(), arguments.next()) {
        (None, None) => manager::run(),
        (Some("worker"), None) => worker::run(),
        (Some("--check-config"), None) => manager::check_config(),
        _ => {
            eprintln!("usage: cupertino-sessiond [worker | --check-config]");
            ExitCode::FAILURE
        }
    }
}
