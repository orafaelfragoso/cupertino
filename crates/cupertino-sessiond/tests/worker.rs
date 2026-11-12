//! The worker's whole PAM transaction, run as the manager runs it (`cupertino-sessiond worker`, an empty environment, a
//! socketpair as standard input) under a private PAM policy (`cupertino-test-pam`): the conversation relayed to the
//! manager, each outcome the design names, the account check, the session's credentials, its opening and closing around
//! the leader, and the leader's environment. The policy's account is the test's own user under a name of its own, so
//! the leader runs without root; nothing reaches the machine's PAM stack, accounts, journal or buses. The worker's
//! diagnostic events go to a journal socket of the test's own.
#![cfg(feature = "app")]

use crate::diag;

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cupertino_diagnostic_events::journal;
use cupertino_login_ipc::{encode, Outcome, Secret};
use cupertino_sessiond::channel::Channel;
use cupertino_sessiond::worker_protocol::{FromWorker, Mode, SessionClass, ToWorker, MAX_FRAME};
use cupertino_test_pam::{Code, Pam, Prompt, Script, PASSWORD};
use diag::Journal;
use zeroize::Zeroizing;

/// The policy's account; no password database of the machine has it.
const LOGIN: &str = "cupertino-test-worker";
/// The session the policy's `pam_systemd` stand-in registers.
const SESSION_ID: &str = "c42";
/// How long the test waits for a message from the worker.
const WAIT: Duration = Duration::from_secs(30);

/// A home, the leader's output and the worker's standard error.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("home")).unwrap();
        Fixture { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The policy `script` with [`LOGIN`] living in [`Fixture::home`].
    fn pam(&self, script: &Script) -> Pam {
        Pam::new(script).with_account(LOGIN, &self.home())
    }
}

/// A session that registers [`SESSION_ID`] when it opens, as `pam_systemd` does.
fn opening() -> Script {
    Script {
        session_env: vec![
            format!("XDG_SESSION_ID={SESSION_ID}"),
            "XDG_RUNTIME_DIR=/nonexistent/runtime".into(),
        ],
        ..Script::default()
    }
}

/// `Begin` for [`LOGIN`] on terminal 7, whose leader writes its environment, and whether it holds descriptor 3, to `out`
/// (the configured extra variable `OUT`), then exits with 7.
fn begin(class: SessionClass, mode: Mode, out: &Path) -> ToWorker {
    ToWorker::Begin {
        service: "cupertino-session".into(),
        login: LOGIN.into(),
        tty: "/dev/tty7".into(),
        vt: 7,
        class,
        mode,
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            "env > $OUT; [ -e /proc/$$/fd/3 ] && echo fd3 >> $OUT; exit 7".into(),
        ],
        environment: vec![format!("OUT={}", out.display())],
        path: "/usr/bin:/bin".into(),
        screen_reader: false,
        failures: 0,
    }
}

/// A worker started as the manager starts one, and the manager's end of its socket.
struct Worker {
    child: Child,
    channel: Channel,
    stderr: PathBuf,
    _journal: Journal,
}

impl Worker {
    fn start(pam: &Pam, fixture: &Fixture, begin: &ToWorker) -> Worker {
        let (ours, theirs) = UnixStream::pair().unwrap();
        ours.set_read_timeout(Some(WAIT)).unwrap();
        let stderr = fixture.file("stderr");
        let events = Journal::new();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cupertino-sessiond"));
        command
            .arg("worker")
            .env_clear()
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env(journal::SOCKET_ENV, events.path())
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&stderr).unwrap());
        pam.isolate(&mut command);
        let child = command.spawn().expect("the worker starts");
        let mut channel = Channel::new(ours, MAX_FRAME);
        channel
            .send_frame(&encode(begin, MAX_FRAME).unwrap())
            .unwrap();
        Worker {
            child,
            channel,
            stderr,
            _journal: events,
        }
    }

    fn receive(&mut self) -> FromWorker {
        match self.channel.receive::<FromWorker>() {
            Ok(message) => message,
            Err(e) => panic!(
                "no message from the worker ({e}); it said:\n{}",
                self.said()
            ),
        }
    }

    fn send(&mut self, message: &ToWorker) {
        self.channel
            .send_frame(&encode(message, MAX_FRAME).unwrap())
            .unwrap();
    }

    fn answer(&mut self, text: &str) {
        let secret = Secret::new(Zeroizing::new(text.to_owned())).unwrap();
        self.send(&ToWorker::Answer { secret });
    }

    /// The password prompt, answered with `text`.
    fn answer_prompt(&mut self, text: &str) {
        assert_eq!(
            self.prompt(),
            (false, "Password: ".to_owned()),
            "{}",
            self.said()
        );
        self.answer(text);
    }

    fn prompt(&mut self) -> (bool, String) {
        match self.receive() {
            FromWorker::Prompt { echo, text } => (echo, text),
            other => panic!(
                "expected a prompt, got {other:?}; the worker said:\n{}",
                self.said()
            ),
        }
    }

    fn decided(&mut self) -> Outcome {
        match self.receive() {
            FromWorker::Decided { outcome } => outcome,
            other => panic!(
                "expected the decision, got {other:?}; the worker said:\n{}",
                self.said()
            ),
        }
    }

    fn said(&self) -> String {
        std::fs::read_to_string(&self.stderr).unwrap_or_default()
    }

    /// The worker's exit status.
    fn exit(mut self) -> (i32, String) {
        drop(self.channel);
        let status = self.child.wait().unwrap();
        let said = std::fs::read_to_string(&self.stderr).unwrap_or_default();
        (status.code().expect("the worker exits"), said)
    }
}

/// The leader's environment and descriptors as it wrote them.
fn leader_output(path: &Path) -> Vec<String> {
    let mut lines: Vec<String> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

fn uid() -> u32 {
    // Safety: getuid has no preconditions.
    unsafe { libc::getuid() }
}

#[test]
fn an_accepted_account_gets_its_credentials_and_session_around_the_leader() {
    let fixture = Fixture::new();
    let pam = fixture.pam(&opening());
    let out = fixture.file("leader");
    let mut worker = Worker::start(&pam, &fixture, &begin(SessionClass::User, Mode::Open, &out));
    worker.answer_prompt(PASSWORD);
    assert_eq!(worker.decided(), Outcome::Accepted);
    match worker.receive() {
        FromWorker::Started {
            session_id, uid: u, ..
        } => {
            assert_eq!(session_id, SESSION_ID);
            assert_eq!(u, uid());
        }
        other => panic!("expected the session to start, got {other:?}"),
    }
    let (status, said) = worker.exit();
    assert_eq!(
        status, 7,
        "the worker exits with the leader's status: {said}"
    );

    // authentication and the account check on the session's terminal, then the credentials, the session, the
    // credentials again once it is open; after the leader the session closes and the credentials go
    assert_eq!(
        pam.calls(),
        [
            format!(
                "authenticate service=cupertino-session user={LOGIN} tty=/dev/tty7 flags=0x0 answer=right -> success"
            ),
            format!("account service=cupertino-session user={LOGIN} tty=/dev/tty7 flags=0x0 -> success"),
            format!("setcred service=cupertino-session user={LOGIN} tty=/dev/tty7 flags=0x2 -> success"),
            format!("open_session service=cupertino-session user={LOGIN} tty=/dev/tty7 flags=0x0 -> success"),
            format!("setcred service=cupertino-session user={LOGIN} tty=/dev/tty7 flags=0x8 -> success"),
            format!("close_session service=cupertino-session user={LOGIN} tty=/dev/tty7 flags=0x0 -> success"),
            format!("setcred service=cupertino-session user={LOGIN} tty=/dev/tty7 flags=0x4 -> success"),
        ]
    );

    // the leader's environment is the PAM environment, the login variables and the configured extra, and nothing of the
    // worker's own (the test shim's variables included)
    let home = fixture.home().display().to_string();
    let environment = leader_output(&out);
    for wanted in [
        format!("OUT={}", out.display()),
        format!("HOME={home}"),
        format!("LOGNAME={LOGIN}"),
        "PATH=/usr/bin:/bin".to_owned(),
        "SHELL=/bin/sh".to_owned(),
        format!("USER={LOGIN}"),
        "XDG_RUNTIME_DIR=/nonexistent/runtime".to_owned(),
        "XDG_SEAT=seat0".to_owned(),
        "XDG_SESSION_CLASS=user".to_owned(),
        format!("XDG_SESSION_ID={SESSION_ID}"),
        "XDG_SESSION_TYPE=wayland".to_owned(),
        "XDG_VTNR=7".to_owned(),
    ] {
        assert!(environment.contains(&wanted), "{wanted} in {environment:?}");
    }
    assert!(
        environment
            .iter()
            .all(|line| !line.starts_with("LD_PRELOAD=") && !line.starts_with("CUPERTINO_TEST_PAM")),
        "{environment:?}"
    );
    // a user session gets no screen reader socket
    assert!(!environment.contains(&"fd3".to_owned()), "{environment:?}");
}

#[test]
fn the_login_screens_session_gets_its_class_and_the_screen_readers_socket() {
    let fixture = Fixture::new();
    let pam = fixture.pam(&opening());
    let out = fixture.file("leader");
    let mut worker = Worker::start(
        &pam,
        &fixture,
        &begin(SessionClass::Greeter, Mode::Open, &out),
    );
    worker.answer_prompt(PASSWORD);
    assert_eq!(worker.decided(), Outcome::Accepted);
    assert!(matches!(worker.receive(), FromWorker::Started { .. }));
    let (status, said) = worker.exit();
    assert_eq!(status, 7, "{said}");
    let environment = leader_output(&out);
    assert!(environment.contains(&"XDG_SESSION_CLASS=greeter".to_owned()));
    assert!(environment.contains(&"fd3".to_owned()), "{environment:?}");
    assert_eq!(
        pam.steps().last().map(String::as_str),
        Some("setcred -> success"),
        "the session closed: {:?}",
        pam.steps()
    );
}

#[test]
fn a_user_with_a_session_already_is_only_checked() {
    let fixture = Fixture::new();
    let pam = fixture.pam(&opening());
    let out = fixture.file("leader");
    let mut worker = Worker::start(
        &pam,
        &fixture,
        &begin(SessionClass::User, Mode::Verify, &out),
    );
    worker.answer_prompt(PASSWORD);
    assert_eq!(worker.decided(), Outcome::Accepted);
    let (status, said) = worker.exit();
    assert_eq!(status, 0, "{said}");
    assert_eq!(
        pam.steps(),
        ["authenticate -> success", "account -> success"],
        "no credentials and no session for a switch"
    );
    assert!(!out.exists(), "no leader runs");
}

/// The outcome of one attempt with `answer` under `script`, the worker's exit status, and the steps that ran.
fn attempt(script: Script, answer: &str) -> (Outcome, i32, Vec<String>) {
    let fixture = Fixture::new();
    let pam = fixture.pam(&script);
    let out = fixture.file("leader");
    let mut worker = Worker::start(&pam, &fixture, &begin(SessionClass::User, Mode::Open, &out));
    worker.answer_prompt(answer);
    let outcome = worker.decided();
    if outcome == Outcome::Accepted {
        assert!(matches!(worker.receive(), FromWorker::Started { .. }));
    }
    let (status, said) = worker.exit();
    if outcome != Outcome::Accepted {
        assert_eq!(status, 0, "a refused attempt is a clean exit: {said}");
        assert!(!out.exists(), "a refused attempt runs no leader");
    }
    (outcome, status, pam.steps())
}

#[test]
fn a_wrong_password_and_an_unknown_account_are_both_denied_and_open_nothing() {
    let (outcome, _, steps) = attempt(opening(), "not the password");
    assert_eq!(outcome, Outcome::Denied);
    assert_eq!(steps, ["authenticate -> auth_err"]);

    let unknown = Script {
        users: Some(vec!["someone-else".into()]),
        ..opening()
    };
    let (outcome, _, steps) = attempt(unknown, PASSWORD);
    assert_eq!(outcome, Outcome::Denied);
    assert_eq!(steps, ["authenticate -> user_unknown"]);
}

#[test]
fn each_refusal_of_the_stack_has_the_outcome_the_design_names() {
    let cases: [(Script, Outcome, &[&str]); 7] = [
        (
            Script {
                authenticate: Some(Code::MaxTries),
                ..opening()
            },
            Outcome::Locked,
            &["authenticate -> maxtries"],
        ),
        (
            Script {
                account: Some(Code::AcctExpired),
                ..opening()
            },
            Outcome::Expired,
            &["authenticate -> success", "account -> acct_expired"],
        ),
        (
            Script {
                account: Some(Code::NewAuthtokReqd),
                ..opening()
            },
            Outcome::Expired,
            &["authenticate -> success", "account -> new_authtok_reqd"],
        ),
        (
            Script {
                account: Some(Code::PermDenied),
                ..opening()
            },
            Outcome::Refused,
            &["authenticate -> success", "account -> perm_denied"],
        ),
        (
            Script {
                account: Some(Code::UserUnknown),
                ..opening()
            },
            Outcome::Denied,
            &["authenticate -> success", "account -> user_unknown"],
        ),
        (
            Script {
                authenticate: Some(Code::AuthinfoUnavail),
                ..opening()
            },
            Outcome::Internal,
            &["authenticate -> authinfo_unavail"],
        ),
        (
            Script {
                authenticate: Some(Code::CredInsufficient),
                ..opening()
            },
            Outcome::Denied,
            &["authenticate -> cred_insufficient"],
        ),
    ];
    for (script, wanted, steps) in cases {
        let (outcome, _, ran) = attempt(script, PASSWORD);
        assert_eq!(
            (outcome, ran),
            (wanted, steps.iter().map(|s| s.to_string()).collect())
        );
    }
}

#[test]
fn a_lockout_the_stack_says_before_the_prompt_is_locked_and_the_message_reaches_the_manager() {
    let fixture = Fixture::new();
    let pam = fixture.pam(&Script {
        lockout: Some("The account is locked due to 3 failed logins.".into()),
        ..opening()
    });
    let out = fixture.file("leader");
    let mut worker = Worker::start(&pam, &fixture, &begin(SessionClass::User, Mode::Open, &out));
    match worker.receive() {
        FromWorker::Notice { error, text } => {
            assert!(error);
            assert_eq!(text, "The account is locked due to 3 failed logins.");
        }
        other => panic!("expected the lock-out message, got {other:?}"),
    }
    worker.answer_prompt(PASSWORD);
    assert_eq!(worker.decided(), Outcome::Locked);
    let (status, _) = worker.exit();
    assert_eq!(status, 0);
    assert_eq!(pam.steps(), ["authenticate -> auth_err"]);
}

#[test]
fn the_account_checks_message_reaches_the_manager_and_the_session_still_opens() {
    let fixture = Fixture::new();
    let pam = fixture.pam(&Script {
        account_info: Some("Your password will expire in 2 days.".into()),
        ..opening()
    });
    let out = fixture.file("leader");
    let mut worker = Worker::start(
        &pam,
        &fixture,
        &begin(SessionClass::User, Mode::Verify, &out),
    );
    worker.answer_prompt(PASSWORD);
    match worker.receive() {
        FromWorker::Notice { error, text } => {
            assert!(!error);
            assert_eq!(text, "Your password will expire in 2 days.");
        }
        other => panic!("expected the account's message, got {other:?}"),
    }
    assert_eq!(worker.decided(), Outcome::Accepted);
    assert_eq!(worker.exit().0, 0);
}

#[test]
fn a_cancelled_prompt_and_a_prompt_with_echo_are_conversation_errors_or_relayed() {
    // the manager cancels the attempt: the conversation fails and the attempt is denied
    let fixture = Fixture::new();
    let pam = fixture.pam(&opening());
    let out = fixture.file("leader");
    let mut worker = Worker::start(&pam, &fixture, &begin(SessionClass::User, Mode::Open, &out));
    assert!(!worker.prompt().0, "a hidden prompt");
    worker.send(&ToWorker::Cancel {});
    assert_eq!(worker.decided(), Outcome::Denied);
    assert_eq!(worker.exit().0, 0);
    assert_eq!(pam.steps(), ["authenticate -> conv_err"]);
    drop(pam);

    // a prompt with echo on reaches the manager as one
    let fixture = Fixture::new();
    let pam = fixture.pam(&Script {
        prompt: Prompt::Echo,
        ..opening()
    });
    let mut worker = Worker::start(
        &pam,
        &fixture,
        &begin(SessionClass::User, Mode::Verify, &out),
    );
    assert_eq!(worker.prompt(), (true, "Password: ".to_owned()));
    worker.answer(PASSWORD);
    assert_eq!(worker.decided(), Outcome::Accepted);
    assert_eq!(worker.exit().0, 0);
}

#[test]
fn a_yes_or_no_question_is_refused_without_reaching_the_manager() {
    let fixture = Fixture::new();
    let pam = fixture.pam(&Script {
        prompt: Prompt::Radio,
        ..opening()
    });
    let out = fixture.file("leader");
    let mut worker = Worker::start(&pam, &fixture, &begin(SessionClass::User, Mode::Open, &out));
    assert_eq!(worker.decided(), Outcome::Denied);
    assert_eq!(worker.exit().0, 0);
    assert_eq!(pam.steps(), ["authenticate -> conv_err"]);
}

#[test]
fn a_manager_gone_during_the_prompt_ends_the_attempt_with_nothing_opened() {
    let fixture = Fixture::new();
    let pam = fixture.pam(&opening());
    let out = fixture.file("leader");
    let mut worker = Worker::start(&pam, &fixture, &begin(SessionClass::User, Mode::Open, &out));
    assert!(!worker.prompt().0, "a hidden prompt");
    let (status, said) = worker.exit();
    assert_ne!(status, 0, "{said}");
    assert_eq!(pam.steps(), ["authenticate -> conv_err"]);
    assert!(!out.exists());
}

/// The worker's failure after an accepted attempt: its `Failed` reason, exit status and the steps that ran.
fn failure(script: Script) -> (String, i32, Vec<String>) {
    let fixture = Fixture::new();
    let pam = fixture.pam(&script);
    let out = fixture.file("leader");
    let mut worker = Worker::start(&pam, &fixture, &begin(SessionClass::User, Mode::Open, &out));
    worker.answer_prompt(PASSWORD);
    assert_eq!(worker.decided(), Outcome::Accepted);
    let reason = match worker.receive() {
        FromWorker::Failed { reason } => reason,
        other => panic!("expected the worker to fail, got {other:?}"),
    };
    let (status, said) = worker.exit();
    assert!(
        said.contains(&reason),
        "the journal line names the reason: {said}"
    );
    assert!(!out.exists(), "no leader runs");
    (reason, status, pam.steps())
}

#[test]
fn credentials_or_a_session_the_stack_refuses_end_the_worker_and_leave_nothing_established() {
    let (reason, status, steps) = failure(Script {
        setcred: Some(Code::CredErr),
        ..opening()
    });
    assert!(reason.starts_with("opening the session"), "{reason}");
    assert_ne!(status, 0);
    assert_eq!(
        steps,
        [
            "authenticate -> success",
            "account -> success",
            "setcred -> cred_err"
        ]
    );

    let (reason, status, steps) = failure(Script {
        open_session: Some(Code::SessionErr),
        ..opening()
    });
    assert!(reason.starts_with("opening the session"), "{reason}");
    assert_ne!(status, 0);
    // the credentials that were established are deleted again
    assert_eq!(
        steps,
        [
            "authenticate -> success",
            "account -> success",
            "setcred -> success",
            "open_session -> session_err",
            "setcred -> success",
        ]
    );

    // a stack without a login session (no `pam_systemd`): the open session is closed again
    let (reason, status, steps) = failure(Script::default());
    assert!(
        reason.contains("did not register a login session"),
        "{reason}"
    );
    assert_ne!(status, 0);
    assert_eq!(
        steps,
        [
            "authenticate -> success",
            "account -> success",
            "setcred -> success",
            "open_session -> success",
            "setcred -> success",
            "close_session -> success",
            "setcred -> success",
        ]
    );
}

#[test]
fn a_session_that_fails_to_close_is_reported() {
    let fixture = Fixture::new();
    let pam = fixture.pam(&Script {
        close_session: Some(Code::SessionErr),
        ..opening()
    });
    let out = fixture.file("leader");
    let mut worker = Worker::start(&pam, &fixture, &begin(SessionClass::User, Mode::Open, &out));
    worker.answer_prompt(PASSWORD);
    assert_eq!(worker.decided(), Outcome::Accepted);
    assert!(matches!(worker.receive(), FromWorker::Started { .. }));
    let (status, said) = worker.exit();
    assert_ne!(status, 0);
    assert!(said.contains("closing the session"), "{said}");
    assert!(out.exists(), "the leader ran");
}
