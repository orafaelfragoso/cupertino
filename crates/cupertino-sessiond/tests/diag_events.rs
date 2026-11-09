//! The diagnostic events of the session manager, read from a journal socket of the test's own. The worker runs as the
//! manager starts it (`cupertino-sessiond worker`, an empty environment, a socketpair as standard input), in a user and
//! mount namespace of its own where `/etc/pam.d` is a directory of the test's own and `/dev/log` a socket of the
//! test's own: its PAM service is `pam_deny` (a refused password) or `pam_permit` (an account the password database
//! does not have, so the session cannot start), and nothing PAM logs reaches the system's journal. The manager runs
//! with a settings file that is missing, and once asked to panic. Each test also checks that what the manager sees,
//! the worker's log line and the exit status are as before, and that no event names the account.
#![cfg(feature = "app")]

mod diag;

use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::process::{Child, Command, Stdio};

use cupertino_diagnostic_events::{self as events, Severity};
use cupertino_login_ipc::{encode, Outcome};
use cupertino_sessiond::channel::Channel;
use cupertino_sessiond::worker_protocol::{FromWorker, Mode, SessionClass, ToWorker, MAX_FRAME};
use cupertino_test_bus::missing_tool;
use diag::Journal;

/// An account no password database has; it must never reach the journal.
const LOGIN: &str = "zelda-carpenter-7f3a";

const BINARY: &str = env!("CARGO_BIN_EXE_cupertino-sessiond");

/// The PAM service the worker is asked for.
const SERVICE: &str = "cupertino-diag-test";

/// Binds the test's `/etc/pam.d` and `/dev/log` in the namespace, then starts the worker.
const ENTER: &str =
    r#"mount --bind "$1" /etc/pam.d && mount --bind "$2" /dev/log && exec "$3" worker"#;

/// A PAM configuration of the test's own, and the socket that stands in for `/dev/log`.
struct Pam {
    dir: tempfile::TempDir,
    _log: UnixDatagram,
}

impl Pam {
    fn new(stack: &str) -> Pam {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("pam.d")).unwrap();
        std::fs::write(dir.path().join("pam.d").join(SERVICE), stack).unwrap();
        let log = UnixDatagram::bind(dir.path().join("log")).unwrap();
        Pam { dir, _log: log }
    }
}

/// Whether this system lets a user enter a user and mount namespace of its own.
fn namespaces() -> bool {
    let entered = Command::new("unshare")
        .args(["--user", "--map-root-user", "--mount", "true"])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !entered {
        missing_tool("unshare with user namespaces");
    }
    entered
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

/// A worker that reports to `journal` and authenticates with `pam`, and the manager's end of its socketpair.
fn worker(journal: &Journal, pam: &Pam) -> (Child, Channel) {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let child = Command::new("unshare")
        .args([
            "--user",
            "--map-root-user",
            "--mount",
            "sh",
            "-c",
            ENTER,
            "sh",
        ])
        .arg(pam.dir.path().join("pam.d"))
        .arg(pam.dir.path().join("log"))
        .arg(BINARY)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(events::journal::SOCKET_ENV, journal.path())
        .stdin(Stdio::from(OwnedFd::from(theirs)))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    (child, Channel::new(ours, MAX_FRAME))
}

#[test]
fn a_refused_password_is_recorded_with_its_service_pam_code_and_count_but_not_the_account() {
    if !namespaces() {
        return;
    }
    let journal = Journal::new();
    let pam = Pam::new("auth required pam_deny.so\naccount required pam_deny.so\n");
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
    if !namespaces() {
        return;
    }
    let journal = Journal::new();
    let pam = Pam::new(
        "auth required pam_permit.so\naccount required pam_permit.so\nsession required pam_permit.so\n",
    );
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
    let out = Command::new(BINARY)
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
    let out = Command::new(BINARY)
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
