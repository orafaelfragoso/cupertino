//! The `cupertino-screenshot` process: one capture per run, started by the server for a screenshot shortcut on a
//! connection of its own (`WAYLAND_SOCKET`), or the portal's colour picker and screen-sharing choice with `--serve`.
//! See the README for the modes and the settings.

#![forbid(unsafe_code)]

use cupertino_diagnostic_events as diag;
use cupertino_screenshot::serve;
use cupertino_screenshot::shot::{self, Options};
use std::process::ExitCode;

const USAGE: &str =
    "usage: cupertino-screenshot --mode screen|selection|toolbar [--clipboard] | --serve";

fn main() -> ExitCode {
    diag::init(diag::component!(), "screenshot", diag::Sink::UserStore);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run = if args == ["--serve"] {
        serve::serve()
    } else {
        match Options::parse(args) {
            Ok(options) => shot::run(options),
            Err(problem) => {
                diag::exit_error(&problem);
                eprintln!("cupertino-screenshot: {problem}\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    };
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            diag::exit_error(&e);
            eprintln!("cupertino-screenshot: {e}");
            ExitCode::FAILURE
        }
    }
}
