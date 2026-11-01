//! Starting a Cupertino binary from a test with a cleared environment.
//!
//! A package build compiles the installed theme directory into every binary (`CUPERTINO_THEME_DIR`), and that directory
//! does not exist while the tests run. A binary started with a cleared environment finds the theme files only through
//! `CUPERTINO_SPEC_DIR`, so every such spawn starts from [`cleared`], which always sets it.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The theme files of the checkout.
pub fn spec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/theme")
}

/// `binary` with an empty environment but `PATH`, `HOME` (the folder `home`) and `CUPERTINO_SPEC_DIR`; the caller adds
/// the rest.
pub fn cleared(binary: impl AsRef<OsStr>, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("CUPERTINO_SPEC_DIR", spec_dir());
    command
}

/// Runs the test `name` of this test binary in a child with `env` set, and returns how the child ended and when. A
/// child that still runs after `limit` is killed and the calling test fails.
pub fn run_child_test(
    name: &str,
    env: &[(&str, &OsStr)],
    limit: Duration,
) -> (ExitStatus, Duration) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    let started = Instant::now();
    let mut child = command.spawn().unwrap();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return (status, started.elapsed());
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child test {name} still ran after {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
