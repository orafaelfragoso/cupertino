//! The `cupertino-windowmanager` process: the stage policy of every output, as a client of the
//! server. See the README for how to run it.

mod app;

use cupertino_diagnostic_events as diag;
use std::process::ExitCode;

fn main() -> ExitCode {
    diag::init(diag::component!(), "windowmanager", diag::Sink::UserStore);
    if let Some(arg) = std::env::args().nth(1) {
        eprintln!(
            "cupertino-windowmanager: unexpected argument {arg:?}\nusage: cupertino-windowmanager"
        );
        return ExitCode::FAILURE;
    }
    // `run` only returns on failure
    let Err(e) = app::run();
    // a failure exit makes the server restart the process
    diag::exit_error(&e);
    eprintln!("cupertino-windowmanager: {e}");
    ExitCode::FAILURE
}
