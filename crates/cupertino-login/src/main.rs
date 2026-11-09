//! The `cupertino-login` process. In a session it locks the session on a request, shows the lock screen on the server's
//! lock surfaces until the user authenticates, and shows the confirmation panels of the session agent. With `--greeter` it
//! is the login screen of the session manager. See the README for how to run it.

mod app;
mod greeter;
mod harden;
mod options;
mod roles;
mod screen;

use cupertino_diagnostic_events as diag;
use cupertino_login_ipc::zeroing::ZeroOnFree;
use options::{Options, USAGE};
use std::process::ExitCode;

/// The lock screen and the login screen handle passwords: nothing they free keeps one, the `CString` that PAM's binding
/// drops without clearing included.
#[global_allocator]
static ALLOCATOR: ZeroOnFree<std::alloc::System> = ZeroOnFree(std::alloc::System);

fn main() -> ExitCode {
    diag::init(diag::component!(), "login_ui", diag::Sink::UserStore);
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("cupertino-login: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = harden::harden() {
        diag::exit_error(&e);
        eprintln!("cupertino-login: hardening the process: {e}");
        return ExitCode::FAILURE;
    }
    let result = match options {
        Options::Session(options) => app::run(&options).map(|ended| match ended {
            // a success exit: the server does not restart the process
            app::Ended::NoLoginSession => eprintln!(
                "cupertino-login: this process is in no login session: there is no session to lock, exiting"
            ),
        }),
        // `run` only returns on failure
        Options::Greeter => greeter::run().map(|never| match never {}),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // a failure exit makes the server restart the process
        Err(e) => {
            diag::exit_error(&e);
            eprintln!("cupertino-login: {e}");
            ExitCode::FAILURE
        }
    }
}
