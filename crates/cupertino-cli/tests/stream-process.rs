//! The stream rules in real processes: a pipe its reader closes early, and SIGINT and SIGTERM. The `cupertino` binary
//! and the `stream_probe` example both run through `cupertino_cli::main_with`.

#[allow(dead_code)]
mod fixtures {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ci/test-fixtures.rs"
    ));
}

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};

fn cupertino() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cupertino"));
    command.env("CUPERTINO_ASSET_SEARCH_PATH", "");
    command
}

fn probe() -> Command {
    let mut command = Command::new(fixtures::artifact(
        "cupertino-cli",
        "example",
        "stream_probe",
    ));
    command.env("CUPERTINO_ASSET_SEARCH_PATH", "");
    command
}

/// A pipe whose read end is already closed: every write to the other end fails with `EPIPE`.
fn closed_pipe() -> OwnedFd {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors pipe2 writes.
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    // SAFETY: pipe2 returned two new descriptors this function owns.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    drop(read);
    write
}

fn stderr_of(child: Child) -> (Option<i32>, String) {
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn help_into_head_c_1_exits_0_without_stderr() {
    let mut cli = cupertino()
        .args(["--format", "jsonl", "help"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let head = Command::new("head")
        .args(["-c", "1"])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .stdin(cli.stdout.take().unwrap())
        .output()
        .unwrap();
    assert_eq!(head.stdout, b"{");
    assert_eq!(stderr_of(cli), (Some(0), String::new()));
}

#[test]
fn a_stdout_whose_reader_is_gone_exits_0_without_stderr_in_every_form() {
    for args in [
        &["help"][..],
        &["--format", "json", "help"],
        &["--format", "jsonl", "help"],
        &["--format", "jsonl", "--version"],
        &["--format", "jsonl", "nothing"],
    ] {
        let cli = cupertino()
            .args(args)
            .stdout(closed_pipe())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        assert_eq!(stderr_of(cli), (Some(0), String::new()), "{args:?}");
    }
}

#[test]
fn a_stream_whose_reader_goes_stops_and_exits_0() {
    for format in ["jsonl", "human"] {
        let child = probe()
            .args(["--format", format, "probe", "flood"])
            .stdout(closed_pipe())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        assert_eq!(stderr_of(child), (Some(0), String::new()), "{format}");
    }
    // a reader that takes one line, then goes
    let mut child = probe()
        .args(["--format", "jsonl", "probe", "flood"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let record: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(
        (&record["type"], &record["seq"]),
        (&Value::from("event"), &Value::from(0))
    );
    assert_eq!(stderr_of(child), (Some(0), String::new()));
}

/// Starts `probe <command>` in JSON lines, waits for its `started` event, sends `signal` and returns the remaining
/// records, the exit code and stderr.
fn signalled(command: &str, signal: i32) -> (Vec<Value>, Option<i32>, String) {
    let mut child = probe()
        .args(["--format", "jsonl", "probe", command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut started = String::new();
    stdout.read_line(&mut started).unwrap();
    assert!(started.contains("\"event\":\"started\""), "{started}");
    // SAFETY: kill only sends a signal to the child this test started and still owns.
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    let records = rest
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let (code, stderr) = stderr_of(child);
    (records, code, stderr)
}

#[test]
fn sigint_and_sigterm_end_a_follow_stream_with_reason_stopped_and_exit_0() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let (records, code, stderr) = signalled("follow", signal);
        assert_eq!((code, stderr.as_str()), (Some(0), ""), "signal {signal}");
        let end = records.last().unwrap();
        assert_eq!(records.len(), 2, "result and end: {records:?}");
        assert_eq!(
            (
                &end["type"],
                &end["ok"],
                &end["exit_code"],
                &end["reason"],
                &end["seq"]
            ),
            (
                &Value::from("end"),
                &Value::from(true),
                &Value::from(0),
                &Value::from("stopped"),
                &Value::from(2)
            )
        );
    }
}

#[test]
fn sigint_and_sigterm_interrupt_a_finite_stream_with_exit_6() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let (records, code, stderr) = signalled("finite", signal);
        assert_eq!((code, stderr.as_str()), (Some(6), ""), "signal {signal}");
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0]["error"]["code"], "interrupted");
    }
}

/// Starts `probe lines`, which reports for every stdin line whether a stop was asked, with `ignored` set to be
/// ignored as a parent may leave it; returns the child, its stdin and its stdout after the `started` event.
fn lines_probe(
    ignored: Option<i32>,
) -> (
    Child,
    std::process::ChildStdin,
    BufReader<std::process::ChildStdout>,
) {
    let mut command = probe();
    command
        .args(["--format", "jsonl", "probe", "lines"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(signal) = ignored {
        // SAFETY: the closure only calls signal(2), which is async-signal-safe, between fork and exec.
        unsafe {
            command.pre_exec(move || {
                libc::signal(signal, libc::SIG_IGN);
                Ok(())
            });
        }
    }
    let mut child = command.spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut started = String::new();
    stdout.read_line(&mut started).unwrap();
    assert!(started.contains("\"event\":\"started\""), "{started}");
    (child, stdin, stdout)
}

/// Sends `signal`, then a line; the probe's report of that line says whether the signal asked it to stop. A signal
/// sent before the line is handled before the probe's read of it returns.
fn signal_then_line(
    child: &Child,
    stdin: &mut std::process::ChildStdin,
    stdout: &mut BufReader<std::process::ChildStdout>,
    signal: i32,
) -> bool {
    // SAFETY: kill only sends a signal to the child this test started and still owns.
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    stdin.write_all(b"x\n").unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let record: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(record["event"], "line", "{line}");
    record["data"]["stop_requested"].as_bool().unwrap()
}

#[test]
fn a_signal_the_parent_ignored_stays_ignored() {
    let (child, mut stdin, mut stdout) = lines_probe(Some(libc::SIGINT));
    assert!(!signal_then_line(
        &child,
        &mut stdin,
        &mut stdout,
        libc::SIGINT
    ));
    assert!(signal_then_line(
        &child,
        &mut stdin,
        &mut stdout,
        libc::SIGTERM
    ));
    drop(stdin);
    let (code, stderr) = stderr_of(child);
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
}

#[test]
fn a_second_signal_of_either_kind_ends_a_stream_that_did_not_stop_at_the_first() {
    for (first, second) in [
        (libc::SIGINT, libc::SIGINT),
        (libc::SIGINT, libc::SIGTERM),
        (libc::SIGTERM, libc::SIGINT),
    ] {
        let (child, mut stdin, mut stdout) = lines_probe(None);
        assert!(signal_then_line(&child, &mut stdin, &mut stdout, first));
        // SAFETY: kill only sends a signal to the child this test started and still owns.
        assert_eq!(unsafe { libc::kill(child.id() as i32, second) }, 0);
        // The signal is pending before stdin closes, so a process it did not end would read the end of stdin and
        // exit 0 instead of waiting for a line forever. The default action, not an exit: the parent sees the signal.
        drop(stdin);
        let status = child.wait_with_output().unwrap().status;
        assert_eq!(
            status.signal(),
            Some(second),
            "{first} then {second}: {status:?}"
        );
    }
}

#[test]
fn a_second_signal_keeps_a_signal_the_parent_ignored_ignored() {
    let (child, mut stdin, mut stdout) = lines_probe(Some(libc::SIGTERM));
    assert!(signal_then_line(
        &child,
        &mut stdin,
        &mut stdout,
        libc::SIGINT
    ));
    assert!(signal_then_line(
        &child,
        &mut stdin,
        &mut stdout,
        libc::SIGTERM
    ));
    drop(stdin);
    let (code, stderr) = stderr_of(child);
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
}

#[test]
fn a_full_stdout_is_exit_6_with_one_stderr_line_in_human_form_only() {
    for (args, stderr) in [
        (
            &["help"][..],
            "cupertino: cannot write the output: No space left on device (os error 28)\n",
        ),
        (&["--format", "json", "help"], ""),
        (&["--format", "jsonl", "help"], ""),
    ] {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let cli = cupertino()
            .args(args)
            .stdout(full)
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        assert_eq!(stderr_of(cli), (Some(6), stderr.to_string()), "{args:?}");
    }
}

#[test]
fn a_program_a_stream_starts_gets_the_default_sigpipe_and_stop_signals() {
    let out = probe()
        .args(["--format", "jsonl", "probe", "child"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let first: Value = serde_json::from_str(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let ignored = u64::from_str_radix(first["data"]["sig_ign"].as_str().unwrap(), 16).unwrap();
    for signal in [libc::SIGPIPE, libc::SIGINT, libc::SIGTERM] {
        assert_eq!(
            ignored & (1 << (signal - 1)),
            0,
            "signal {signal} is ignored in the child: {ignored:x}"
        );
    }
}
