//! Running one test of this binary in a process of its own: an emitter is one per process, and some tests read what a
//! process wrote to standard error.

#![allow(dead_code)]

use std::path::Path;
use std::process::{Command, Output};

/// Set in the child process: the test named runs its body.
pub const CHILD: &str = "DIAG_TEST_CHILD";

/// Whether this process is the child of [`child`].
pub fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

/// Runs the test `name` of this binary in a new process with `envs` and a state directory under `state`, and returns
/// what it did.
pub fn child(name: &str, state: &Path, envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .args(["--exact", name, "--nocapture", "--test-threads", "1"])
        .env(CHILD, "1")
        .env("XDG_STATE_HOME", state)
        .env("HOME", state.join("home"))
        .env_remove("XDG_SESSION_ID")
        .env_remove(cupertino_diagnostic_events::journal::SOCKET_ENV);
    for (k, v) in envs {
        command.env(k, v);
    }
    command.output().unwrap()
}

/// The child's standard error, as text.
pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
