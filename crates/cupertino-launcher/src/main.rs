//! The `cupertino-launcher` process: shows the launcher panel on a layer surface and toggles it on
//! `org.cupertino.Launcher1.Toggle`. See the README for how to run it.

mod app;

use cupertino_diagnostic_events as diag;
use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-launcher [--no-files] [--terminal PROGRAM]";

fn main() -> ExitCode {
    diag::init(diag::component!(), "launcher", diag::Sink::UserStore);
    let options = match app::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("cupertino-launcher: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // `run` only returns on failure
    let Err(e) = app::run(&options);
    // a failure exit makes the server restart the process
    diag::exit_error(&e);
    eprintln!("cupertino-launcher: {e}");
    ExitCode::FAILURE
}
