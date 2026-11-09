//! Starting a Cupertino binary from a test with a cleared environment.
//!
//! A package build compiles the installed theme directory into every binary (`CUPERTINO_THEME_DIR`), and that directory
//! does not exist while the tests run. A binary started with a cleared environment finds the theme files only through
//! `CUPERTINO_SPEC_DIR`, so every such spawn starts from [`cleared`], which always sets it.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The theme files of the checkout.
pub fn spec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/theme")
}

/// `binary` with an empty environment but `PATH`, `HOME` (the folder `home`), `CUPERTINO_SPEC_DIR` and the empty asset
/// search path (`CUPERTINO_ASSET_SEARCH_PATH`), so it draws with the built-in assets whatever packs the machine has
/// installed; the caller adds the rest.
pub fn cleared(binary: impl AsRef<OsStr>, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("CUPERTINO_SPEC_DIR", spec_dir())
        .env("CUPERTINO_ASSET_SEARCH_PATH", "");
    command
}

/// How long a child of [`wait_after`] may take to get to what is timed. Its own limit is for a child that hangs
/// on the way; how fast a loaded machine starts it is not what the tests measure.
const SETUP_LIMIT: Duration = Duration::from_secs(60);

/// Runs the test `name` of this test binary in a child with `env` set, and returns how the child ended and when. A
/// child that still runs after `limit` is killed and the calling test fails.
pub fn run_child_test(
    name: &str,
    env: &[(&str, &OsStr)],
    limit: Duration,
) -> (ExitStatus, Duration) {
    run_child_test_after(name, env, || true, limit)
}

/// [`run_child_test`], timed by [`wait_after`].
pub fn run_child_test_after(
    name: &str,
    env: &[(&str, &OsStr)],
    started: impl Fn() -> bool,
    limit: Duration,
) -> (ExitStatus, Duration) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    wait_after(command.spawn().unwrap(), started, limit)
}

/// Waits for `child`, timed from the first moment `started` holds, not from the spawn: how the child ended and how long
/// after `started`. A child still running `limit` after it, or [`SETUP_LIMIT`] after the call without it, is killed and
/// the calling test fails.
pub fn wait_after(
    mut child: Child,
    started: impl Fn() -> bool,
    limit: Duration,
) -> (ExitStatus, Duration) {
    let spawned = Instant::now();
    let mut since = None;
    loop {
        if since.is_none() && started() {
            since = Some(Instant::now());
        }
        if let Some(status) = child.try_wait().unwrap() {
            return (status, since.unwrap_or(spawned).elapsed());
        }
        let late = match since {
            Some(at) => (at.elapsed() > limit).then(|| format!("{limit:?} after it started")),
            None => (spawned.elapsed() > SETUP_LIMIT)
                .then(|| format!("{SETUP_LIMIT:?} without starting")),
        };
        if let Some(late) = late {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child still ran {late}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
