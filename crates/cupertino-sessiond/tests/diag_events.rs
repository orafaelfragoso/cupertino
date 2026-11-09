//! The diagnostic events of the session manager, read from a journal socket of the test's own. The worker runs as the
//! manager starts it (`cupertino-sessiond worker`, an empty environment, a socketpair as standard input), under the test
//! PAM shim (`cupertino-test-pam`): its policy refuses the password, or accepts an account the password database does
//! not have, so the session cannot start, and nothing PAM does reaches the machine's PAM stack or journal. The manager
//! runs under the shim too, with a settings file that is missing, and once asked to panic. Each test also checks that what the manager sees,
//! the worker's log line and the exit status are as before, and that no event names the account.
#![cfg(feature = "app")]

mod diag;

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};

use cupertino_diagnostic_events::{self as events, Severity};
use cupertino_login_ipc::{encode, Outcome};
use cupertino_sessiond::channel::Channel;
use cupertino_sessiond::worker_protocol::{FromWorker, Mode, SessionClass, ToWorker, MAX_FRAME};
use cupertino_test_pam::{Code, Pam, PamIsolated, Script};
use diag::Journal;

/// An account no password database has; it must never reach the journal.
const LOGIN: &str = "zelda-carpenter-7f3a";

const BINARY: &str = env!("CARGO_BIN_EXE_cupertino-sessiond");

/// The PAM service the worker is asked for.
const SERVICE: &str = "cupertino-diag-test";

/// A policy that asks for no password and gives each step `authenticate`'s result, else success.
fn policy(authenticate: Option<Code>) -> Pam {
    Pam::new(&Script {
        password: None,
        authenticate,
        ..Script::default()
    })
}

fn begin(failures: u64) -> ToWorker {
    ToWorker::Begin {
        service: SERVICE.to_owned(),
        login: LOGIN.to_owned(),
        tty: "/dev/tty63".to_owned(),
        vt: 63,
        class: SessionClass::User,
        mode: Mode::Open,
        command: vec!["/bin/true".to_owned()],
        environment: Vec::new(),
        path: "/usr/bin".to_owned(),
        screen_reader: false,
        failures,
    }
}

/// A worker that reports to `journal` and authenticates under `pam`, and the manager's end of its socketpair.
fn worker(journal: &Journal, pam: &Pam) -> (Child, Channel) {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let child = Command::new(BINARY)
        .arg("worker")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(events::journal::SOCKET_ENV, journal.path())
        .pam_isolated(pam)
        .stdin(Stdio::from(OwnedFd::from(theirs)))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    (child, Channel::new(ours, MAX_FRAME))
}

#[test]
fn a_refused_password_is_recorded_with_its_service_pam_code_and_count_but_not_the_account() {
    let journal = Journal::new();
    let pam = policy(Some(Code::AuthErr));
    let (child, mut channel) = worker(&journal, &pam);
    channel
        .send_frame(&encode(&begin(2), MAX_FRAME).unwrap())
        .unwrap();
    // the manager hears the same decision as before, and the worker ends as before
    let decided = channel.receive::<FromWorker>().unwrap();
    assert!(
        matches!(
            decided,
            FromWorker::Decided {
                outcome: Outcome::Denied
            }
        ),
        "{decided:?}"
    );
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cupertino-sessiond: worker: Authenticate refused (denied): "),
        "{stderr}"
    );

    let event = journal.one("login.auth.failed");
    event.check("login.auth.failed", Severity::Warn, &[LOGIN, "zelda"]);
    assert_eq!(event.field("SYSLOG_IDENTIFIER"), "cupertino-sessiond");
    assert_eq!(event.text("service"), SERVICE);
    assert_eq!(event.text("stage"), "authenticate");
    assert_eq!(event.text("error"), "AUTH_ERR");
    assert_eq!(event.text("outcome"), "denied");
    assert_eq!(event.record.fields.get("failures").unwrap(), 3);
    assert!(event.record.fields.get("login").is_none());
}

#[test]
fn a_session_that_cannot_start_is_recorded_beside_the_same_report_to_the_manager() {
    let journal = Journal::new();
    let pam = policy(None);
    let (child, mut channel) = worker(&journal, &pam);
    channel
        .send_frame(&encode(&begin(0), MAX_FRAME).unwrap())
        .unwrap();
    let decided = channel.receive::<FromWorker>().unwrap();
    assert!(
        matches!(
            decided,
            FromWorker::Decided {
                outcome: Outcome::Accepted
            }
        ),
        "{decided:?}"
    );
    let failed = channel.receive::<FromWorker>().unwrap();
    let FromWorker::Failed { reason } = failed else {
        panic!("expected Failed, got {failed:?}");
    };
    assert_eq!(
        reason,
        "the authenticated account has no password database entry"
    );
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "cupertino-sessiond: worker: the authenticated account has no password database entry"
        ),
        "{stderr}"
    );

    let event = journal.one("login.session.start_failed");
    event.check(
        "login.session.start_failed",
        Severity::Error,
        &[LOGIN, "zelda"],
    );
    assert_eq!(event.text("stage"), "worker");
    assert_eq!(event.text("service"), SERVICE);
    assert_eq!(
        event.text("error"),
        "the authenticated account has no password database entry"
    );
    assert!(
        journal.read().is_empty(),
        "an accepted password is no failed authentication"
    );
}

#[test]
fn a_manager_without_its_settings_records_its_exit_beside_the_same_message_and_status() {
    let journal = Journal::new();
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("login.conf");
    let pam = policy(None);
    let out = Command::new(BINARY)
        .pam_isolated(&pam)
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("CUPERTINO_LOGIN_CONF", &missing)
        .env(events::journal::SOCKET_ENV, journal.path())
        .env_remove(events::TEST_PANIC_ENV)
        .env_remove("STATE_DIRECTORY")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr,
        format!(
            "cupertino-sessiond: {0}: {0}: No such file or directory (os error 2)\n",
            missing.display()
        )
    );
    let event = journal.one("login.exit_error");
    event.check(
        "login.exit_error",
        Severity::Error,
        &[missing.to_str().unwrap()],
    );
    assert!(
        event
            .text("error")
            .ends_with(": No such file or directory (os error 2)"),
        "{}",
        event.text("error")
    );
}

#[test]
fn a_panic_is_recorded_and_the_process_ends_with_the_panic_status() {
    let journal = Journal::new();
    let pam = policy(None);
    let out = Command::new(BINARY)
        .pam_isolated(&pam)
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(events::TEST_PANIC_ENV, "login")
        .env(events::journal::SOCKET_ENV, journal.path())
        .env_remove("STATE_DIRECTORY")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(101));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("panicked"), "{stderr}");
    let event = journal.one("login.panic");
    event.check("login.panic", Severity::Fatal, &[]);
    assert_eq!(event.field("SYSLOG_IDENTIFIER"), "cupertino-sessiond");
    assert_eq!(
        event.text("panic"),
        "CUPERTINO_DIAG_TEST_PANIC asked this process to panic"
    );
}
