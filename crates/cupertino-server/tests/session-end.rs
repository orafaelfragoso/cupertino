//! A panic on the session's thread writes its reason, on one line, to the file `cupertino-session` names, and the
//! process ends within the teardown limit even when the unwinding hangs; the session command logs that reason and ends
//! the session within 5 s. The server itself, crashed with `--crash` (a panic, then an abort), leaves its backtrace in
//! its log and the reason in the session log, and the session ends within 5 s of the crash.

mod common;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::spawn::{run_child_test, spec_dir};
use cupertino_server::backend::headless;
use cupertino_server::logging::{self, SESSION_END_ENV, TEARDOWN_LIMIT};

const CHILD: &str = "CUPERTINO_TEST_SESSION_END_CHILD";

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
    let started = Instant::now();
    let mut child = Command::new("sh")
        .arg(session)
        .env_clear()
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
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            panic!("the session still ran after 5 s");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
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
    took: Duration,
    session_log: String,
    server_log: String,
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
    let started = Instant::now();
    let mut child = Command::new("sh")
        .arg(session)
        .env_clear()
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
    let limit = Duration::from_secs_f64(CRASH_AT) + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            panic!("the session still ran after {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let took = started.elapsed();
    let session_log = std::fs::read_to_string(state.join("session.log")).unwrap();
    let server_log = std::fs::read_to_string(state.join("server.log")).unwrap_or_default();
    if session_log.contains(headless::NO_USABLE_DEVICE)
        && std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() != Ok("1")
    {
        eprintln!("SKIPPED: no EGL device; set CUPERTINO_EGL_REQUIRED=1 to fail instead");
        return None;
    }
    assert!(!state.join("session-end").exists());
    Some(Crashed {
        status,
        took,
        session_log,
        server_log,
    })
}

/// The time from the crash to the end of the session, as an upper bound: the whole run less `CRASH_AT`, so it includes
/// the server's start.
fn after_the_crash(crashed: &Crashed) -> Duration {
    crashed
        .took
        .saturating_sub(Duration::from_secs_f64(CRASH_AT))
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
    let after = after_the_crash(&crashed);
    eprintln!("the session ended {after:?} after the panic (upper bound)");
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
    let after = after_the_crash(&crashed);
    eprintln!("the session ended {after:?} after the abort (upper bound)");
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
    let after = after_the_crash(&crashed);
    eprintln!("the session ended {after:?} after the overflow (upper bound)");
    assert!(after < Duration::from_secs(5), "{after:?}");
}
