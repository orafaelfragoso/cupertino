//! The `cupertino-sessiond` process: the manager, or with the argument `worker` one worker. See the README.

use std::process::ExitCode;

use cupertino_sessiond::{manager, worker};

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
