//! The `cupertino-windowmanager` process: the stage policy of every output, as a client of the
//! server. See the README for how to run it.

mod app;

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(arg) = std::env::args().nth(1) {
        eprintln!("cupertino-windowmanager: unexpected argument {arg:?}\nusage: cupertino-windowmanager");
        return ExitCode::FAILURE;
    }
    // `run` only returns on failure
    let Err(e) = app::run();
    // a failure exit makes the server restart the process
    eprintln!("cupertino-windowmanager: {e}");
    ExitCode::FAILURE
}
