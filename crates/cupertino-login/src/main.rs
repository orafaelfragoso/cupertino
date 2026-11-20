//! The `cupertino-login` process. In a user's session it is the lock screen: it locks the session on a request and
//! shows the lock screen on the server's lock surfaces until the user authenticates ([`cupertino_login::lock_screen`]).
//! With `--greeter` it is the login screen of the session manager ([`cupertino_login::greeter`]). See the README for
//! how to run it.

#![forbid(unsafe_code)]

use cupertino_diagnostic_events as diag;
use cupertino_login::options::{Options, USAGE};
use cupertino_login::{greeter, lock_screen};
use cupertino_login_ipc::zeroing::ZeroOnFree;
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
            diag::exit_error(&problem);
            eprintln!("cupertino-login: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = cupertino_kit::process::harden() {
        diag::exit_error(&e);
        eprintln!("cupertino-login: hardening the process: {e}");
        return ExitCode::FAILURE;
    }
    let result = match options {
        Options::Session(options) => lock_screen::run(options),
        Options::Greeter => greeter::run(),
    };
    match result {
        // the process is in no login session: a success exit, which the server does not restart
        Ok(()) => ExitCode::SUCCESS,
        // a failure exit makes the server restart the process
        Err(e) => {
            diag::exit_error(&e);
            eprintln!("cupertino-login: {e}");
            ExitCode::FAILURE
        }
    }
}
