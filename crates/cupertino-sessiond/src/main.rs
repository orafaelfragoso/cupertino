//! The `cupertino-sessiond` process: the manager, or with the argument `worker` one worker. See the README.

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
        _ => {
            eprintln!("usage: cupertino-sessiond [worker]");
            ExitCode::FAILURE
        }
    }
}
