//! The `cupertino-notifications` process: owns `org.freedesktop.Notifications`, shows banners
//! and the notification centre list on a layer surface and opens the list on
//! `org.cupertino.NotificationCenter1.ToggleCentre`. See the README for how to run it.

mod app;

use cupertino_diagnostic_events as diag;
use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-notifications [--replace]";

fn main() -> ExitCode {
    diag::init(diag::component!(), "notifications", diag::Sink::UserStore);
    let options = match app::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(arg) => {
            eprintln!("cupertino-notifications: unknown argument {arg}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // `run` only returns on failure
    let Err(e) = app::run(&options);
    if e.downcast_ref::<app::NameOwned>().is_some() {
        // not a failure: the server restarts a process that fails, and the name is not going to be free
        eprintln!(
            "cupertino-notifications: {e}; leaving it to that daemon, so there are no notifications from this session until it is restarted"
        );
        return ExitCode::SUCCESS;
    }
    // a failure exit makes the server restart the process
    diag::exit_error(&e);
    eprintln!("cupertino-notifications: {e}");
    ExitCode::FAILURE
}
