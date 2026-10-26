//! The `cupertino-login` process. In a session it locks the session on a request, shows the lock screen on the server's
//! lock surfaces until the user authenticates, and shows the confirmation panels of the session agent. With `--greeter` it
//! is the login screen of the session manager. See the README for how to run it.

mod app;
mod greeter;
mod options;
mod screen;

use options::{Options, USAGE};
use std::process::ExitCode;

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("cupertino-login: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let result = match options {
        // `run` only returns on failure
        Options::Session(options) => app::run(&options).map(|never| match never {}),
        // `run` only returns on failure
        Options::Greeter => greeter::run().map(|never| match never {}),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // a failure exit makes the server restart the process
        Err(e) => {
            eprintln!("cupertino-login: {e}");
            ExitCode::FAILURE
        }
    }
}
