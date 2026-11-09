//! The `cupertino-menubar` process: one menu bar per output as a client of the server, serving
//! the status notifier watcher on the session bus. See the
//! README for how to run it.

mod app;

use cupertino_diagnostic_events as diag;
use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-menubar";

fn main() -> ExitCode {
    diag::init(diag::component!(), "menubar", diag::Sink::UserStore);
    if let Some(arg) = std::env::args().nth(1) {
        diag::exit_error(&format_args!("unknown argument {arg}"));
        eprintln!("cupertino-menubar: unknown argument {arg}\n{USAGE}");
        return ExitCode::FAILURE;
    }
    // `run` only returns on failure
    let Err(e) = app::run();
    // a failure exit makes the server restart the process
    diag::exit_error(&e);
    eprintln!("cupertino-menubar: {e}");
    ExitCode::FAILURE
}
