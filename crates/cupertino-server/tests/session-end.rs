//! A panic on the session's thread writes its reason, on one line, to the file `cupertino-session` names, and the
//! process ends within the teardown limit even when the unwinding hangs; the session command logs that reason and ends
//! the session within 5 s. The server itself, crashed with `--crash` (a panic, then an abort), leaves its backtrace in
//! its log and the reason in the session log, and the session ends within 5 s of the crash. The crash is also in the
//! server's diagnostic store: `server.panic` after `server.session_end` for a panic, `server.fatal_signal` for a signal.

use crate::common;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::spawn::{run_child_test, run_child_test_after, spec_dir, wait_after};
use cupertino_diagnostic_events::{store, Record, Severity};
use cupertino_server::backend::headless;
use cupertino_server::logging::{self, SESSION_END_ENV, TEARDOWN_LIMIT};

const CHILD: &str = "CUPERTINO_TEST_SESSION_END_CHILD";

/// A child slower to start than its limit is timed from the moment it marks, not from its spawn: a loaded machine
/// starting a child slowly fails no test.
#[test]
fn a_child_is_timed_from_its_mark_not_from_a_slow_start() {
    const SLOW: &str = "CUPERTINO_TEST_SLOW_START_CHILD";
    if let Some(mark) = std::env::var_os(SLOW) {
        std::thread::sleep(Duration::from_millis(1500));
        std::fs::write(mark, "").unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mark = dir.path().join("mark");
    let (status, took) = run_child_test_after(
        "a_child_is_timed_from_its_mark_not_from_a_slow_start",
        &[(SLOW, mark.as_os_str())],
        || mark.exists(),
        Duration::from_secs(1),
    );
    assert!(status.success());
    assert!(took < Duration::from_secs(1), "{took:?} after the mark");
}

struct Hang;

impl Drop for Hang {
    fn drop(&mut self) {
        loop {
            std::thread::park();
        }
    }
}

#[test]
fn a_panic_is_recorded_for_the_session_and_a_hung_teardown_is_cut_short() {
    if std::env::var_os(CHILD).is_some() {
        logging::init(None);
        logging::record_session_end(|key| std::env::var_os(key));
        let _hang = Hang;
        panic!("the output\nvanished");
    }
    let dir = tempfile::tempdir().unwrap();
    let end = dir.path().join("session-end");
    let (status, took) = run_child_test(
        "a_panic_is_recorded_for_the_session_and_a_hung_teardown_is_cut_short",
        &[(CHILD, OsStr::new("1")), (SESSION_END_ENV, end.as_os_str())],
        TEARDOWN_LIMIT + Duration::from_secs(3),
    );
    assert_eq!(status.code(), Some(101));
    assert!(took >= TEARDOWN_LIMIT, "ended after {took:?}");
    let reason = std::fs::read_to_string(&end).unwrap();
    assert!(
        reason.starts_with("panic: panicked at crates/cupertino-server/tests/session-end.rs:")
            || reason.starts_with("panic: panicked at tests/session-end.rs:"),
        "{reason:?}"
    );
    assert!(reason.ends_with(": the output vanished\n"), "{reason:?}");
}

struct AbortOnDrop;

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        std::process::abort();
    }
}

#[test]
fn an_abort_in_the_teardown_of_a_panic_keeps_the_panic_as_the_reason() {
    if std::env::var_os(CHILD).is_some() {
        logging::init(None);
        logging::record_session_end(|key| std::env::var_os(key));
        let _abort = AbortOnDrop;
        panic!("the first failure");
    }
    let dir = tempfile::tempdir().unwrap();
    let end = dir.path().join("session-end");
    let (status, _) = run_child_test(
        "an_abort_in_the_teardown_of_a_panic_keeps_the_panic_as_the_reason",
        &[(CHILD, OsStr::new("1")), (SESSION_END_ENV, end.as_os_str())],
        TEARDOWN_LIMIT + Duration::from_secs(3),
    );
    assert_eq!(
        std::os::unix::process::ExitStatusExt::signal(&status),
        Some(libc::SIGABRT)
    );
    let reason = std::fs::read_to_string(&end).unwrap();
    assert!(reason.ends_with(": the first failure\n"), "{reason:?}");
}

#[test]
fn the_session_command_logs_why_the_compositor_panicked_and_ends_within_five_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let server = dir.path().join("server");
    std::fs::write(
        &server,
        "#!/bin/sh\nexec \"$TEST_EXE\" --exact a_panic_is_recorded_for_the_session_and_a_hung_teardown_is_cut_short \
         --nocapture >/dev/null 2>&1\n",
    )
    .unwrap();
    std::fs::set_permissions(&server, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let session = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/files/usr/bin/cupertino-session");
    let child = Command::new("sh")
        .arg(session)
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir.path())
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("CUPERTINO_SESSION_SERVER", &server)
        .env("TEST_EXE", std::env::current_exe().unwrap())
        .env(CHILD, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let end = dir.path().join("state/cupertino/session-end");
    let (status, _) = wait_after(child, || end.exists(), Duration::from_secs(5));
    assert_eq!(status.code(), Some(101));
    let log = std::fs::read_to_string(dir.path().join("state/cupertino/session.log")).unwrap();
    let end = log.lines().last().unwrap();
    assert!(
        end.contains("the compositor ended with status 101 after "),
        "{log}"
    );
    assert!(end.contains(" s: panic: panicked at "), "{log}");
    assert!(end.ends_with(": the output vanished"), "{log}");
    assert!(!dir.path().join("state/cupertino/session-end").exists());
}

/// When `--crash` ends the server, in seconds after its start.
const CRASH_AT: f64 = 1.0;

struct Crashed {
    status: std::process::ExitStatus,
    /// From the crash, which writes the session-end reason first, to the end of the session.
    after_the_crash: Duration,
    session_log: String,
    server_log: String,
    /// What the server wrote to its diagnostic store, oldest first.
    events: Vec<Record>,
}

impl Crashed {
    /// The events of `id`.
    fn events(&self, id: &str) -> Vec<&Record> {
        self.events.iter().filter(|r| r.id == id).collect()
    }

    /// The ids of the events that record how the server ended, in the order they were written.
    fn ends(&self) -> Vec<&str> {
        self.events
            .iter()
            .map(|r| r.id.as_str())
            .filter(|id| ["server.session_end", "server.panic", "server.fatal_signal"].contains(id))
            .collect()
    }
}

/// The session command running the headless server with `--crash CRASH_AT:how`; `None` without an EGL device (unless
/// `CUPERTINO_EGL_REQUIRED=1`).
fn crash_the_session(how: &str) -> Option<Crashed> {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state/cupertino");
    let server = dir.path().join("server");
    // the session command asks for the hardware; this server stands in for it offscreen
    std::fs::write(
        &server,
        format!(
            "#!/bin/sh\nexec \"$REAL_SERVER\" --headless --size 64x48 --run-for 30 --crash {CRASH_AT}:{how}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&server, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let runtime = dir.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(
        &runtime,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let session = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/files/usr/bin/cupertino-session");
    let child = Command::new("sh")
        .arg(session)
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir.path())
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("CUPERTINO_SPEC_DIR", spec_dir())
        .env("CUPERTINO_SESSION_SERVER", &server)
        .env("REAL_SERVER", env!("CARGO_BIN_EXE_cupertino-server"))
        .env(logging::LOG_FILE_ENV, state.join("server.log"))
        .env("NO_COLOR", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let end = state.join("session-end");
    let (status, after_the_crash) = wait_after(child, || end.exists(), Duration::from_secs(20));
    let session_log = std::fs::read_to_string(state.join("session.log")).unwrap();
    let server_log = std::fs::read_to_string(state.join("server.log")).unwrap_or_default();
    let events = store::read(&state.join("events"))
        .map(|contents| contents.records.into_iter().map(|s| s.record).collect())
        .unwrap_or_default();
    if session_log.contains(headless::NO_USABLE_DEVICE)
        && std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() != Ok("1")
    {
        eprintln!("SKIPPED: no EGL device; set CUPERTINO_EGL_REQUIRED=1 to fail instead");
        return None;
    }
    assert!(!state.join("session-end").exists());
    Some(Crashed {
        status,
        after_the_crash,
        session_log,
        server_log,
        events,
    })
}

#[test]
fn a_panic_of_the_server_leaves_its_backtrace_and_ends_the_session_within_five_seconds() {
    let Some(crashed) = crash_the_session("panic") else {
        return;
    };
    assert_eq!(crashed.status.code(), Some(101), "{}", crashed.session_log);
    let end = crashed.session_log.lines().last().unwrap();
    assert!(
        end.contains("the compositor ended with status 101 after "),
        "{end}"
    );
    assert!(end.contains(": panic: panicked at "), "{end}");
    assert!(end.ends_with(": the crash asked for with --crash"), "{end}");
    let log = &crashed.server_log;
    assert!(log.contains("panic: panicked at "), "{log}");
    assert!(log.contains("backtrace:"), "{log}");
    assert!(
        log.contains("cupertino_server::cli::schedule_script"),
        "the backtrace names the frames:\n{log}"
    );
    // the server's hook runs first (the session's end), then the hook it chained to records the panic
    assert_eq!(
        crashed.ends(),
        ["server.session_end", "server.panic"],
        "{:?}",
        crashed.events
    );
    let panic = crashed.events("server.panic")[0];
    assert_eq!(panic.severity, Severity::Fatal);
    assert_eq!(panic.component, "cupertino-server");
    assert_eq!(
        panic.fields.get("panic").unwrap(),
        "the crash asked for with --crash"
    );
    let location = panic.fields.get("location").unwrap().as_str().unwrap();
    assert!(location.starts_with("cli.rs:"), "{location}");
    assert_eq!(panic.fields.get("backend").unwrap(), "headless");
    assert_eq!(panic.fields.get("renderer").unwrap(), "gles");
    let end = crashed.events("server.session_end")[0];
    assert_eq!(end.severity, Severity::Error);
    assert_eq!(end.fields.get("status").unwrap(), 101);
    let error = end.fields.get("error").unwrap().as_str().unwrap();
    assert!(error.starts_with("panic: panicked at "), "{error}");
    assert!(
        error.ends_with("the crash asked for with --crash"),
        "{error}"
    );
    let after = crashed.after_the_crash;
    eprintln!("the session ended {after:?} after the panic");
    assert!(after < Duration::from_secs(5), "{after:?}");
}

#[test]
fn an_abort_of_the_server_leaves_its_backtrace_and_ends_the_session_within_five_seconds() {
    let Some(crashed) = crash_the_session("abort") else {
        return;
    };
    assert_eq!(
        crashed.status.code(),
        Some(128 + libc::SIGABRT),
        "{}",
        crashed.session_log
    );
    let end = crashed.session_log.lines().last().unwrap();
    assert!(
        end.contains("the compositor ended with status 134 after "),
        "{end}"
    );
    assert!(end.ends_with(": the server was ended by SIGABRT"), "{end}");
    let log = &crashed.server_log;
    assert!(log.contains("fatal signal SIGABRT; backtrace:"), "{log}");
    assert!(
        log.contains("cupertino_server::cli::schedule_script"),
        "the backtrace names the frames:\n{log}"
    );
    assert_fatal_signal(&crashed, "SIGABRT");
    let after = crashed.after_the_crash;
    eprintln!("the session ended {after:?} after the abort");
    assert!(after < Duration::from_secs(5), "{after:?}");
}

#[test]
fn a_stack_overflow_of_the_server_leaves_its_backtrace_and_ends_the_session_within_five_seconds() {
    let Some(crashed) = crash_the_session("overflow") else {
        return;
    };
    assert_eq!(
        crashed.status.code(),
        Some(128 + libc::SIGSEGV),
        "{}",
        crashed.session_log
    );
    let end = crashed.session_log.lines().last().unwrap();
    assert!(end.ends_with(": the server was ended by SIGSEGV"), "{end}");
    let log = &crashed.server_log;
    assert!(log.contains("fatal signal SIGSEGV; backtrace:"), "{log}");
    assert!(
        log.contains("cupertino_server::cli::overflow"),
        "the backtrace names the frames"
    );
    assert_fatal_signal(&crashed, "SIGSEGV");
    let after = crashed.after_the_crash;
    eprintln!("the session ended {after:?} after the overflow");
    assert!(after < Duration::from_secs(5), "{after:?}");
}

/// The server ended by `signal` wrote one `server.fatal_signal` line from its handler, and nothing else recorded the end.
fn assert_fatal_signal(crashed: &Crashed, signal: &str) {
    assert_eq!(
        crashed.ends(),
        ["server.fatal_signal"],
        "{:?}",
        crashed.events
    );
    let fatal = crashed.events("server.fatal_signal")[0];
    assert_eq!(fatal.severity, Severity::Fatal);
    assert_eq!(fatal.message, "the server was ended by a fatal signal");
    assert_eq!(fatal.component, "cupertino-server");
    assert_eq!(fatal.fields.get("signal").unwrap(), signal);
    // the backend is known before the line is made ready
    assert_eq!(fatal.fields.get("backend").unwrap(), "headless");
    let written = cupertino_diagnostic_events::time::parse(&fatal.time).unwrap();
    let age = std::time::SystemTime::now()
        .duration_since(written)
        .unwrap();
    assert!(
        age < Duration::from_secs(120),
        "written at the crash: {}",
        fatal.time
    );
}
