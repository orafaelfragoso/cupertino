//! The server binary records how it ended in its diagnostic store (`$XDG_STATE_HOME/cupertino/events`):
//! `server.session_end` with the exit status, and the error's text when it ended with one. Its crashes are in
//! `session-end.rs`, beside the session's own record of them.

use crate::common;

use std::path::Path;
use std::process::Output;

use common::spawn::cleared;
use cupertino_diagnostic_events::Severity;
use cupertino_server::backend::headless;

/// The server started with `args` in a cleared environment whose home, state and runtime directories are in `dir`.
fn server(dir: &Path, args: &[&str]) -> Output {
    let runtime = dir.join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(
        &runtime,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    cleared(env!("CARGO_BIN_EXE_cupertino-server"), &dir.join("home"))
        .args(args)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("CUPERTINO_NO_DBUS", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("the server starts")
}

fn store(dir: &Path) -> std::path::PathBuf {
    dir.join("state/cupertino/events")
}

#[test]
fn a_server_that_ends_with_an_error_records_its_status_and_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = server(
        dir.path(),
        &["--headless", "--appearance", "no-such-appearance"],
    );
    // as before: the error on standard error, and status 1
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("unknown appearance \"no-such-appearance\""),
        "{stderr}"
    );
    let ends = common::events::stored(&store(dir.path()), "server.session_end");
    assert_eq!(ends.len(), 1, "{ends:?}");
    let end = &ends[0];
    assert_eq!(end.severity, Severity::Error);
    assert_eq!(end.component, "cupertino-server");
    assert_eq!(end.message, "the server ended");
    assert_eq!(end.fields.get("status").unwrap(), 1);
    let error = end.fields.get("error").unwrap().as_str().unwrap();
    assert!(
        error.contains("unknown appearance \"no-such-appearance\""),
        "{error}"
    );
    assert_eq!(end.fields.get("backend").unwrap(), "headless");
    assert!(end.fields.get("renderer").is_none(), "no renderer was made");
    assert!(common::events::stored(&store(dir.path()), "server.panic").is_empty());
}

#[test]
fn a_server_that_ends_normally_records_status_zero_with_its_backend_and_renderer() {
    let dir = tempfile::tempdir().unwrap();
    let output = server(
        dir.path(),
        &["--headless", "--size", "64x48", "--run-for", "0.5"],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success()
        && stderr.contains(headless::NO_USABLE_DEVICE)
        && std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() != Ok("1")
    {
        eprintln!("SKIPPED: no EGL device; set CUPERTINO_EGL_REQUIRED=1 to fail instead");
        return;
    }
    assert!(output.status.success(), "{}:\n{stderr}", output.status);
    let ends = common::events::stored(&store(dir.path()), "server.session_end");
    assert_eq!(ends.len(), 1, "{ends:?}");
    let end = &ends[0];
    assert_eq!(end.severity, Severity::Info);
    assert_eq!(end.fields.get("status").unwrap(), 0);
    assert!(end.fields.get("error").is_none());
    assert_eq!(end.fields.get("backend").unwrap(), "headless");
    assert_eq!(end.fields.get("renderer").unwrap(), "gles");
}
