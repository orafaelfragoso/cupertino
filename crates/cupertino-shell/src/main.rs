//! The `cupertino-shell` program: the shell host the server starts, `cupertino-shell` for the shared host and
//! `cupertino-shell --only NAME` for a separate one. See the README.

#![forbid(unsafe_code)]

use cupertino_diagnostic_events as diag;

fn main() {
    diag::init(diag::component!(), "shell", diag::Sink::UserStore);
    cupertino_shell::run(&cupertino_shell::COMPONENTS, std::env::args().skip(1))
}
