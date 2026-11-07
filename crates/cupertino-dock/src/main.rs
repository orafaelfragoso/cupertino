//! The `cupertino-dock` process: the dock as CupertinoKit windows on layer surfaces, starting on the first output, as
//! a client of the server. See the README for how to run it.

mod app;

use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-dock [--terminal PROGRAM]";

fn main() -> ExitCode {
    let options = match app::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("cupertino-dock: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // `run` only returns on failure
    let Err(e) = app::run(&options);
    // a failure exit makes the server restart the process
    eprintln!("cupertino-dock: {e}");
    ExitCode::FAILURE
}
