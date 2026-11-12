//! Every shell process and app records its own end in its diagnostic store (`$XDG_STATE_HOME/cupertino/events`): a
//! panic as `<area>.panic`, and a failure that ends it as `<area>.exit_error`, beside the line it prints. The real
//! binaries run with a cleared environment, no Wayland display and no session bus, so each fails at its start; the panic
//! is the one `CUPERTINO_DIAG_TEST_PANIC` asks for right after the emitter is installed.

use crate::common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::spawn::{cleared, wait_after};
use cupertino_diagnostic_events::{Record, Severity, TEST_PANIC_ENV};

/// A binary, the area of its events, and the arguments that make it fail at its start without touching a device.
struct Process {
    binary: &'static str,
    area: &'static str,
    args: &'static [&'static str],
}

const PROCESSES: [Process; 11] = [
    Process {
        binary: "cupertino-dock",
        area: "dock",
        args: &[],
    },
    Process {
        binary: "cupertino-menubar",
        area: "menubar",
        args: &[],
    },
    Process {
        binary: "cupertino-launcher",
        area: "launcher",
        args: &[],
    },
    Process {
        binary: "cupertino-notifications",
        area: "notifications",
        args: &[],
    },
    Process {
        binary: "cupertino-controlcenter",
        area: "controlcenter",
        args: &["--fake-backends"],
    },
    Process {
        binary: "cupertino-osd",
        area: "osd",
        args: &["--fake-backends"],
    },
    Process {
        binary: "cupertino-screenshot",
        area: "screenshot",
        args: &["--mode", "screen"],
    },
    Process {
        binary: "cupertino-windowmanager",
        area: "windowmanager",
        args: &[],
    },
    Process {
        binary: "cupertino-login",
        area: "login_ui",
        args: &[],
    },
    Process {
        binary: "cupertino-files",
        area: "files",
        args: &[],
    },
    Process {
        binary: "cupertino-settings",
        area: "settings",
        args: &[],
    },
];

struct Ended {
    code: Option<i32>,
    stderr: String,
    records: Vec<Record>,
}

/// `process` started in `dir` with `env` added; how it ended and what its store holds.
fn run(process: &Process, dir: &Path, env: &[(&str, &str)]) -> Ended {
    run_with(process, process.args, dir, |command| {
        for (key, value) in env {
            command.env(key, value);
        }
    })
}

/// `process` started in `dir` with `args`, and `adjust` applied to the command; how it ended and what its store holds.
fn run_with(
    process: &Process,
    args: &[&str],
    dir: &Path,
    adjust: impl FnOnce(&mut std::process::Command),
) -> Ended {
    let binary = common::fixtures::artifact(process.binary, "bin", process.binary);
    let runtime = dir.join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(
        &runtime,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let mut command = cleared(&binary, &dir.join("home"));
    command
        .args(args)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/dev/null/bus")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    adjust(&mut command);
    let mut child = command
        .spawn()
        .unwrap_or_else(|e| panic!("{}: {e}", binary.display()));
    let mut pipe = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        text
    });
    let (status, _) = wait_after(child, || true, common::HANG_GUARD);
    let stderr = reader.join().unwrap();
    let records = cupertino_diagnostic_events::store::read(&store(dir))
        .map(|contents| {
            assert_eq!(
                (contents.corrupt, contents.partial),
                (0, 0),
                "{}",
                process.binary
            );
            contents
                .records
                .into_iter()
                .map(|stored| stored.record)
                .collect()
        })
        .unwrap_or_default();
    Ended {
        code: status.code(),
        stderr,
        records,
    }
}

fn store(dir: &Path) -> PathBuf {
    dir.join("state/cupertino/events")
}

#[test]
fn a_shell_process_or_app_that_panics_records_the_panic_and_still_prints_it() {
    for process in &PROCESSES {
        let dir = tempfile::tempdir().unwrap();
        let ended = run(process, dir.path(), &[(TEST_PANIC_ENV, process.area)]);
        let name = process.binary;
        assert_eq!(ended.code, Some(101), "{name}: {}", ended.stderr);
        // the hook that was there before still prints the panic
        assert!(
            ended.stderr.contains("panicked at"),
            "{name}: {}",
            ended.stderr
        );
        assert_eq!(ended.records.len(), 1, "{name}: {:?}", ended.records);
        let record = &ended.records[0];
        assert_eq!(record.id, format!("{}.panic", process.area), "{name}");
        assert_eq!(record.severity, Severity::Fatal, "{name}");
        assert_eq!(record.component, name);
        assert!(
            record
                .fields
                .get("panic")
                .and_then(|m| m.as_str())
                .is_some_and(|m| m.contains(TEST_PANIC_ENV)),
            "{name}: {:?}",
            record.fields
        );
        assert_eq!(
            record.fields.get("thread").and_then(|t| t.as_str()),
            Some("main"),
            "{name}"
        );
    }
}

#[test]
fn a_shell_process_or_app_that_cannot_start_records_its_error_beside_the_line_it_prints() {
    for process in &PROCESSES {
        let dir = tempfile::tempdir().unwrap();
        let ended = run(process, dir.path(), &[]);
        let name = process.binary;
        assert_eq!(ended.code, Some(1), "{name}: {}", ended.stderr);
        let line = ended
            .stderr
            .lines()
            .rev()
            .find(|line| line.starts_with(&format!("{name}: ")))
            .unwrap_or_else(|| panic!("{name} printed no error: {}", ended.stderr));
        let exits: Vec<&Record> = ended
            .records
            .iter()
            .filter(|r| r.id == format!("{}.exit_error", process.area))
            .collect();
        assert_eq!(exits.len(), 1, "{name}: {:?}", ended.records);
        let record = exits[0];
        assert_eq!(record.severity, Severity::Error, "{name}");
        assert_eq!(record.component, name);
        let error = record
            .fields
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or_else(|| panic!("{name}: {record:?}"));
        assert!(!error.is_empty(), "{name}");
        // the printed line holds the error as it is; the event has it as the scrubber left it
        if record.redacted.is_empty() {
            assert!(
                line.ends_with(error),
                "{name}: {line:?} does not end with {error:?}"
            );
        } else {
            assert_eq!(record.redacted, ["error"], "{name}");
            assert!(!line.ends_with(error), "{name}: {line:?}");
        }
        assert!(
            ended.records.iter().all(|r| !r.id.ends_with(".panic")),
            "{name}: {:?}",
            ended.records
        );
    }
}

/// A wrong command line ends each process as before, with its usage on standard error and the failure status, and is
/// recorded as `<area>.exit_error` with the problem.
#[test]
fn a_shell_process_or_app_given_a_wrong_argument_records_it_beside_its_usage() {
    for process in &PROCESSES {
        let dir = tempfile::tempdir().unwrap();
        let ended = run_with(process, &["--no-such-option"], dir.path(), |_| {});
        let name = process.binary;
        assert_eq!(ended.code, Some(1), "{name}: {}", ended.stderr);
        assert!(
            ended.stderr.contains(&format!("usage: {name}")),
            "{name}: {}",
            ended.stderr
        );
        assert!(
            ended.stderr.starts_with(&format!("{name}: ")),
            "{name}: {}",
            ended.stderr
        );
        assert_eq!(ended.records.len(), 1, "{name}: {:?}", ended.records);
        let record = &ended.records[0];
        assert_eq!(record.id, format!("{}.exit_error", process.area), "{name}");
        assert_eq!(record.severity, Severity::Error, "{name}");
        let error = record.fields.get("error").and_then(|e| e.as_str()).unwrap();
        assert!(error.contains("--no-such-option"), "{name}: {error}");
        // the problem is the first line printed, after the program's name
        let first = ended.stderr.lines().next().unwrap();
        assert_eq!(first, format!("{name}: {error}"), "{name}");
    }
}

/// Settings started with an activation token that is not UTF-8 ends as before and records why.
#[test]
fn settings_given_an_activation_token_that_is_not_text_records_it() {
    use std::os::unix::ffi::OsStrExt;
    let process = PROCESSES
        .iter()
        .find(|p| p.binary == "cupertino-settings")
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ended = run_with(process, process.args, dir.path(), |command| {
        command.env(
            "XDG_ACTIVATION_TOKEN",
            std::ffi::OsStr::from_bytes(b"token-\xff"),
        );
    });
    assert_eq!(ended.code, Some(1), "{}", ended.stderr);
    assert_eq!(
        ended.stderr.trim_end(),
        "cupertino-settings: activation token is not valid UTF-8"
    );
    assert_eq!(ended.records.len(), 1, "{:?}", ended.records);
    assert_eq!(ended.records[0].id, "settings.exit_error");
    assert_eq!(
        ended.records[0].fields.get("error").unwrap(),
        "activation token is not valid UTF-8"
    );
}
