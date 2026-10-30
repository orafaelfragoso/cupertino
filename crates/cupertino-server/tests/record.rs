//! `--record` on the headless output: the file is whole frames, as many as the server says it wrote, each of them
//! drawn or a repeat of a drawn one.

use std::path::Path;
use std::process::Command;

use cupertino_server::backend::headless;

/// The output's size in pixels: small, so a second of frames is a small file.
const SIZE: (u64, u64) = (64, 48);

/// The value of `field=` on the line of the log that has `message`.
fn field(log: &str, message: &str, field: &str) -> u64 {
    let line = log
        .lines()
        .find(|l| l.contains(message))
        .unwrap_or_else(|| panic!("no {message:?} in the log:\n{log}"));
    let value = line
        .split_whitespace()
        .find_map(|word| word.strip_prefix(&format!("{field}=")))
        .unwrap_or_else(|| panic!("no {field}= in {line:?}"));
    value.parse().unwrap()
}

/// The server's log of a headless second recorded into `video`, `None` without an EGL device (unless
/// `CUPERTINO_EGL_REQUIRED=1`).
fn record(dir: &Path, video: &Path) -> Option<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_cupertino-server"))
        .args(["--headless", "--size", &format!("{}x{}", SIZE.0, SIZE.1)])
        .arg("--record")
        .arg(video)
        .args(["--run-for", "1"])
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_RUNTIME_DIR", dir)
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .output()
        .expect("the server starts");
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success()
        && log.contains(headless::NO_USABLE_DEVICE)
        && std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() != Ok("1")
    {
        eprintln!("SKIPPED: no EGL device; set CUPERTINO_EGL_REQUIRED=1 to fail instead");
        return None;
    }
    assert!(output.status.success(), "{}:\n{log}", output.status);
    Some(log)
}

#[test]
fn a_recording_is_whole_frames_as_many_as_the_server_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("video.rgba");
    let Some(log) = record(dir.path(), &video) else {
        return;
    };
    let frames = field(&log, "recording written", "frames");
    let drawn = field(&log, "recording written", "drawn");
    let length = std::fs::metadata(&video).unwrap().len();
    assert_eq!(length, frames * SIZE.0 * SIZE.1 * 4);
    assert!(
        frames >= drawn && drawn >= 1,
        "frames={frames} drawn={drawn}"
    );
}

#[test]
fn a_fifo_nobody_reads_ends_the_server_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("frames");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cupertino-server"))
        .args(["--headless", "--size", "64x48", "--run-for", "30"])
        .arg("--record")
        .arg(&fifo)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("NO_COLOR", "1")
        .output()
        .expect("the server starts");
    let log = String::from_utf8_lossy(&output.stderr);
    if log.contains(headless::NO_USABLE_DEVICE)
        && std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() != Ok("1")
    {
        eprintln!("SKIPPED: no EGL device; set CUPERTINO_EGL_REQUIRED=1 to fail instead");
        return;
    }
    assert!(!output.status.success(), "{log}");
    let expected = format!("{}: no reader opened it within 10s", fifo.display());
    assert!(
        log.contains(&expected),
        "no {expected:?} in the log:\n{log}"
    );
}
