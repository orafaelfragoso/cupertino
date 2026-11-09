//! `cupertino-assets check` says on stderr which command replaces it, and still checks the pack.

use std::process::Command;

#[test]
fn check_names_the_command_that_replaces_it() {
    let pack = tempfile::tempdir().unwrap();
    std::fs::write(
        pack.path().join("manifest.json"),
        r#"{"version": 2, "name": "empty"}"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_cupertino-assets"))
        .arg("check")
        .arg(pack.path())
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "cupertino-assets: use `cupertino appearance assets check`; this command will be removed in a later release\n"
    );
    assert!(!out.stdout.is_empty(), "the report still follows");

    let usage = Command::new(env!("CARGO_BIN_EXE_cupertino-assets"))
        .arg("list")
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&usage.stderr),
        "usage: cupertino-assets check <dir>\n",
        "a wrong command gets the usage alone"
    );
}
