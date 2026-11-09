//! `--record` on the headless output: the file is whole frames, each drawn or a repeat of a drawn one, and as many as
//! the time the recording ran comes to (60 a second), also when the encoder reads slower than the server draws.

use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use cupertino_server::backend::headless;

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

/// What the server's log says a recording wrote: video frames, drawn frames and the frames its time came to.
struct Written {
    frames: u64,
    drawn: u64,
    due: u64,
}

/// A headless second of a `size` output recorded into `video`; `None` without an EGL device (unless
/// `CUPERTINO_EGL_REQUIRED=1`).
fn record(dir: &Path, video: &Path, size: (u64, u64)) -> Option<Written> {
    let output = Command::new(env!("CARGO_BIN_EXE_cupertino-server"))
        .args(["--headless", "--size", &format!("{}x{}", size.0, size.1)])
        .arg("--record")
        .arg(video)
        .args(["--run-for", "1"])
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_RUNTIME_DIR", dir)
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
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
    let written = |name| field(&log, "recording written", name);
    Some(Written {
        frames: written("frames"),
        drawn: written("drawn"),
        due: written("due"),
    })
}

#[test]
fn a_recording_is_whole_frames_as_many_as_its_time_comes_to() {
    let size = (64, 48);
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("video.rgba");
    let Some(Written { frames, drawn, due }) = record(dir.path(), &video, size) else {
        return;
    };
    let length = std::fs::metadata(&video).unwrap().len();
    assert_eq!(length, frames * size.0 * size.1 * 4);
    assert_eq!(frames, due);
    assert!(
        frames >= drawn && drawn >= 1,
        "frames={frames} drawn={drawn}"
    );
    assert!(frames >= 30, "{frames} frames for a second at 60 a second");
}

#[test]
fn a_slow_encoder_gets_repeats_for_the_frames_not_drawn() {
    // a frame of this size fills the writer's buffer, so the server waits on the reader
    let size: (u64, u64) = (1280, 800);
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("frames");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .unwrap();
    let path = fifo.clone();
    // an encoder that takes 50 ms a frame, 20 a second
    let encoder = std::thread::spawn(move || {
        let mut file = std::fs::File::open(path).unwrap();
        let mut frame = vec![0; (size.0 * size.1 * 4) as usize];
        let mut read = 0;
        loop {
            match file.read_exact(&mut frame) {
                Ok(()) => read += 1,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => return read,
                Err(e) => panic!("reading the frames: {e}"),
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let Some(Written { frames, drawn, due }) = record(dir.path(), &fifo, size) else {
        return;
    };
    assert_eq!(encoder.join().unwrap(), frames);
    assert_eq!(frames, due);
    assert!(drawn < frames, "frames={frames} drawn={drawn}");
    assert!(frames >= 30, "{frames} frames for a second at 60 a second");
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
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
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
