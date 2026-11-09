//! The `cupertino-screenshot` process: one capture per run, started by the server for a screenshot shortcut on a
//! connection of its own (`WAYLAND_SOCKET`). See the README for the modes and the settings.

mod app;
mod capture;
mod deliver;

use cupertino_diagnostic_events as diag;
use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-screenshot --mode screen|selection|toolbar [--clipboard]";

fn main() -> ExitCode {
    diag::init(diag::component!(), "screenshot", diag::Sink::UserStore);
    let options = match app::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("cupertino-screenshot: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match app::run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            diag::exit_error(&e);
            eprintln!("cupertino-screenshot: {e}");
            ExitCode::FAILURE
        }
    }
}
