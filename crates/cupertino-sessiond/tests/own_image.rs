//! A worker runs the manager's own program image: after a package upgrade replaced or removed the file the manager was
//! started from, the manager still starts workers, and they are its own code, not the new file's. This test program stands
//! in for the manager: a link to it is started, its file is removed or replaced by a script, and it starts a worker the way
//! the manager does ([`own_program`]).
#![cfg(feature = "app")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use cupertino_sessiond::manager::own_program;

/// What the process started by a test does: `remove` or `replace` its own file and start a worker, or (`worker`) say that
/// it ran.
const STEP: &str = "CUPERTINO_TEST_OWN_IMAGE_STEP";
/// The directory the worker writes what ran to.
const DIR: &str = "CUPERTINO_TEST_OWN_IMAGE_DIR";
/// The file in [`DIR`] that says what ran.
const RAN: &str = "ran";
/// What this program's worker writes to [`RAN`]; the script that replaces the file writes something else.
const OWN: &str = "the manager's image";

fn helper_name() -> String {
    cupertino_test_bus::test_path("manager_process")
}

/// Not a test of its own: the tests below start a link to this program to run it. Without [`STEP`] it does nothing.
#[test]
fn manager_process() {
    let Ok(step) = std::env::var(STEP) else {
        return;
    };
    let dir = PathBuf::from(std::env::var_os(DIR).expect("the directory"));
    let name = std::env::var("CUPERTINO_TEST_OWN_IMAGE_NAME").expect("the helper's name");
    match step.as_str() {
        "worker" => {
            std::fs::write(dir.join(RAN), OWN).expect("written");
            return;
        }
        "remove" | "replace" => {}
        other => panic!("unknown step {other}"),
    }
    let file = std::env::current_exe().expect("this program's file");
    if step == "remove" {
        std::fs::remove_file(&file).expect("removed");
    } else {
        // as a package manager does: the new file is written beside the old one and renamed over it
        let new = file.with_extension("new");
        std::fs::write(
            &new,
            format!("#!/bin/sh\necho replaced > \"${DIR}/{RAN}\"\n"),
        )
        .expect("the new file");
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).expect("mode");
        std::fs::rename(&new, &file).expect("renamed over");
    }
    let status = own_program()
        .args(["--exact", &name, "--nocapture"])
        .env_clear()
        .env(STEP, "worker")
        .env(DIR, &dir)
        .env("CUPERTINO_TEST_OWN_IMAGE_NAME", &name)
        .status()
        .expect("the worker starts");
    assert!(status.success(), "the worker failed: {status}");
}

/// Start a link to this program in a directory of its own that, as the manager, does `step` to its file and starts a
/// worker; what the worker wrote.
fn run_as_manager(step: &str) -> String {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("a directory");
    let program = dir.path().join("manager");
    link_or_copy(&std::env::current_exe().expect("this program"), &program);
    let name = helper_name();
    let output = Command::new(&program)
        .args(["--exact", &name, "--nocapture"])
        .env(STEP, step)
        .env(DIR, dir.path())
        .env("CUPERTINO_TEST_OWN_IMAGE_NAME", &name)
        // the stand-in reads no packs and keeps any diagnostic event to this directory
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("XDG_STATE_HOME", dir.path())
        .env("CUPERTINO_DIAG_JOURNAL_SOCKET", dir.path().join("journal"))
        .output()
        .expect("the manager starts");
    assert!(
        output.status.success(),
        "the manager failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout)
    );
    std::fs::read_to_string(dir.path().join(RAN))
        .expect("the worker ran")
        .trim()
        .to_owned()
}

/// A hard link of `from` at `to` (the same file system as the target directory), else a copy.
fn link_or_copy(from: &Path, to: &Path) {
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to).expect("copied");
    }
}

#[test]
fn a_manager_whose_file_was_removed_still_starts_its_own_worker() {
    assert_eq!(run_as_manager("remove"), OWN);
}

#[test]
fn a_manager_whose_file_was_replaced_starts_its_own_worker_not_the_new_program() {
    assert_eq!(run_as_manager("replace"), OWN);
}
