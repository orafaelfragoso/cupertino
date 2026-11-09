//! The command's exit codes: 0 once the checks ran, whatever they found, and 2 for a wrong argument.

use std::process::Command;

/// The command run in a session whose compositor is gone: nothing answers on its Wayland socket.
fn diagnose(args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let runtime = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cupertino-diagnose"))
        .args(args)
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("XDG_SESSION_ID", "cupertino-test-no-such-session")
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("WAYLAND_DISPLAY", "wayland-gone")
        .output()
        .unwrap();
    let json = serde_json::from_slice(&output.stdout).unwrap_or_default();
    (output.status.code(), json)
}

#[test]
fn a_failing_check_still_exits_0_and_a_wrong_argument_exits_2() {
    let (code, report) = diagnose(&["--json", "--area", "session"]);
    let wayland = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "session.wayland")
        .unwrap();
    assert_eq!(wayland["status"], "fail", "{report}");
    assert_eq!(code, Some(0));
    assert_eq!(diagnose(&["--area", "network"]).0, Some(2));
    assert_eq!(diagnose(&["--colour"]).0, Some(2));
}

/// The command's stdout and stderr, run as [`diagnose`] runs it.
fn diagnose_output(args: &[&str]) -> std::process::Output {
    let runtime = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_cupertino-diagnose"))
        .args(args)
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("XDG_SESSION_ID", "cupertino-test-no-such-session")
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("WAYLAND_DISPLAY", "wayland-gone")
        .output()
        .unwrap()
}

#[test]
fn the_text_report_names_the_command_that_replaces_it_and_the_json_does_not_change() {
    const NOTICE: &str =
        "cupertino-diagnose: use `cupertino diagnose`; this command will be removed in a later release\n";
    let text = diagnose_output(&["--area", "session"]);
    assert_eq!(text.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&text.stderr), NOTICE);
    assert!(String::from_utf8_lossy(&text.stdout).contains("session"));

    let json = diagnose_output(&["--json", "--area", "session"]);
    assert_eq!(json.status.code(), Some(0));
    assert_eq!(json.stderr, b"", "no notice beside the JSON");
    let stdout = String::from_utf8(json.stdout).unwrap();
    assert!(
        stdout.starts_with("{\n") && stdout.ends_with("\n}\n"),
        "{stdout}"
    );
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        report["checks"].is_array(),
        "the report alone, nothing before or after it"
    );
}
