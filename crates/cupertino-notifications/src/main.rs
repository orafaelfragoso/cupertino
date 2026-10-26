//! The `cupertino-notifications` process: owns `org.freedesktop.Notifications`, shows banners
//! and the notification centre list on a layer surface and opens the list on
//! `org.cupertino.NotificationCenter1.ToggleCentre`. See the README for how to run it.

mod app;

use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-notifications [--replace]";

fn main() -> ExitCode {
    let options = match app::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(arg) => {
            eprintln!("cupertino-notifications: unknown argument {arg}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // `run` only returns on failure
    let Err(e) = app::run(&options);
    // a failure exit makes the server restart the process
    eprintln!("cupertino-notifications: {e}");
    ExitCode::FAILURE
}
