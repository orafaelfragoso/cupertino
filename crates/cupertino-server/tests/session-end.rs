//! A panic on the session's thread writes its reason, on one line, to the file `cupertino-session` names, and the
//! process ends within the teardown limit even when the unwinding hangs; the session command logs that reason and ends
//! the session within 5 s.

mod common;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::spawn::run_child_test;
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
