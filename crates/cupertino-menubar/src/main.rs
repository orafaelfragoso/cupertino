//! The `cupertino-menubar` process: one menu bar per output as a client of the server, serving
//! the status notifier watcher on the session bus. See the
//! README for how to run it.

mod app;

use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-menubar";

fn main() -> ExitCode {
    if let Some(arg) = std::env::args().nth(1) {
        eprintln!("cupertino-menubar: unknown argument {arg}\n{USAGE}");
        return ExitCode::FAILURE;
    }
    // `run` only returns on failure
    let Err(e) = app::run();
    // a failure exit makes the server restart the process
    eprintln!("cupertino-menubar: {e}");
    ExitCode::FAILURE
}
