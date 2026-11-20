//! The `cupertino-shell` program refuses what it is not started with, before it looks for its connection (D18).

use std::process::Command;

/// The status for a usage error.
const EXIT_USAGE: i32 = 2;

fn run(args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cupertino-shell"))
        .args(args)
        .env_remove("WAYLAND_SOCKET")
        // its diagnostic events go to the test's own directory, never the real home
        .env("HOME", env!("CARGO_TARGET_TMPDIR"))
        .env("XDG_STATE_HOME", env!("CARGO_TARGET_TMPDIR"))
        .output()
        .expect("cupertino-shell runs");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn an_unknown_argument_or_component_is_a_usage_error() {
    for args in [
        &["--verbose"][..],
        &["--only"],
        &["--only", "screensaver"],
        &["--only", "osd", "--only", "dock"],
    ] {
        let (status, stderr) = run(args);
        assert_eq!(status, Some(EXIT_USAGE), "{args:?}: {stderr}");
        assert!(
            stderr.contains("usage: cupertino-shell [--only COMPONENT]"),
            "{stderr}"
        );
    }
}

#[test]
fn a_known_component_passes_to_the_connection() {
    // no WAYLAND_SOCKET: the host fails at its connection, not at its arguments
    let (status, stderr) = run(&["--only", "osd"]);
    assert_eq!(status, Some(1), "{stderr}");
}
