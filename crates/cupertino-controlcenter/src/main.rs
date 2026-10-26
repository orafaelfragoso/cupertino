//! The `cupertino-controlcenter` process: shows the control panel on a layer surface and carries out the calls of
//! `org.cupertino.ControlPanel1`. See the README for how to run it.

mod app;

use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-controlcenter [--fake-backends] [--alsa-card N]";

fn main() -> ExitCode {
    let options = match app::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("cupertino-controlcenter: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // `run` only returns on failure
    let Err(e) = app::run(&options);
    // a failure exit makes the server restart the process
    eprintln!("cupertino-controlcenter: {e}");
    ExitCode::FAILURE
}
