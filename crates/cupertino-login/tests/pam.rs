//! The lock screen's PAM attempts ([`Authenticator`]) against a private policy (`cupertino-test-pam`): each test runs the
//! attempts in a copy of this test binary that loads the test shim, so its transactions start from the policy and
//! never from `/etc/pam.d`, and reads back the events the authenticator gave and the steps the policy recorded. The
//! account is a name of the test's own; nothing reaches the machine's PAM stack, accounts or journal.
#![cfg(feature = "app")]

use std::sync::mpsc;
use std::time::Duration;

use cupertino_login::auth::{Authenticator, Event, SERVICE};
use cupertino_login::model::password;
use cupertino_test_pam::{Code, Pam, Prompt, Script, CHILD_VARIABLE, PASSWORD};

/// The account the attempts are for.
const LOGIN: &str = "cupertino-test-lock";
/// The child's name for this file's runner.
const CHILD: &str = "attempts_in_a_process_with_the_test_shim";

/// Run by the tests below in a process with the shim: one attempt for each line of the case, each event printed as
/// `EVENT <debug>`.
#[test]
#[ignore = "started by the other tests of this file, in a process with the test shim"]
fn attempts_in_a_process_with_the_test_shim() {
    cupertino_test_pam::require_loaded();
    let case = std::env::var(CHILD_VARIABLE).expect("the case");
    let (sender, events) = mpsc::channel();
    let authenticator = Authenticator::spawn(LOGIN.into(), move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    for answer in case.split('\n') {
        authenticator.attempt(password(answer));
        loop {
            let event = events
                .recv_timeout(Duration::from_secs(30))
                .expect("the attempt ends");
            println!("EVENT {event:?}");
            if matches!(event, Event::Outcome(_)) {
                break;
            }
        }
    }
}

/// The events of the attempts with `answers` under `script`, and the steps the policy recorded.
fn attempts(script: &Script, answers: &[&str]) -> (Vec<String>, Vec<String>) {
    let pam = Pam::new(script);
    let output = pam
        .child(CHILD, &answers.join("\n"))
        .output()
        .expect("the test binary starts again");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the attempts failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = stdout
        .lines()
        // libtest writes `test NAME ... ` before the child's first line, without a line break
        .filter_map(|line| line.split_once("EVENT ").map(|(_, event)| event.to_owned()))
        .collect();
    let calls = pam.calls();
    for call in &calls {
        assert!(
            call.contains(&format!(" service={SERVICE} user={LOGIN} ")),
            "the lock screen's service and account: {call}"
        );
    }
    (events, pam.steps())
}

#[test]
fn the_right_password_is_a_success_after_the_account_check() {
    let (events, steps) = attempts(&Script::default(), &[PASSWORD]);
    assert_eq!(events, ["Outcome(Success)"]);
    assert_eq!(steps, ["authenticate -> success", "account -> success"]);
}

#[test]
fn a_wrong_password_is_denied_and_the_next_attempt_is_a_transaction_of_its_own() {
    let (events, steps) = attempts(&Script::default(), &["wrong", PASSWORD]);
    assert_eq!(events, ["Outcome(Denied)", "Outcome(Success)"]);
    assert_eq!(
        steps,
        [
            "authenticate -> auth_err",
            "authenticate -> success",
            "account -> success"
        ]
    );
}

#[test]
fn too_many_tries_is_denied_and_an_unknown_account_is_an_error() {
    let (events, _) = attempts(
        &Script {
            authenticate: Some(Code::MaxTries),
            ..Script::default()
        },
        &[PASSWORD],
    );
    assert_eq!(events, ["Outcome(Denied)"]);

    let (events, steps) = attempts(
        &Script {
            users: Some(vec!["someone-else".into()]),
            ..Script::default()
        },
        &[PASSWORD],
    );
    assert_eq!(steps, ["authenticate -> user_unknown"]);
    assert_eq!(events.len(), 1);
    assert!(
        events[0].starts_with("Outcome(Error(\"authentication failed to run: "),
        "{events:?}"
    );
}

#[test]
fn an_expired_or_locked_account_does_not_unlock_with_the_right_password() {
    for code in [Code::AcctExpired, Code::NewAuthtokReqd, Code::PermDenied] {
        let (events, steps) = attempts(
            &Script {
                account: Some(code),
                ..Script::default()
            },
            &[PASSWORD],
        );
        assert_eq!(
            steps,
            [
                "authenticate -> success".to_owned(),
                format!("account -> {}", code.name())
            ]
        );
        assert_eq!(events.len(), 1);
        assert!(
            events[0].starts_with("Outcome(Error(\"account not available: "),
            "{events:?}"
        );
    }
}

#[test]
fn the_stacks_messages_reach_the_screen_in_order_before_the_outcome() {
    let (events, _) = attempts(
        &Script {
            lockout: Some("The account is locked due to 3 failed logins.".into()),
            ..Script::default()
        },
        &[PASSWORD],
    );
    assert_eq!(
        events,
        [
            "Notice(\"The account is locked due to 3 failed logins.\")",
            "Outcome(Denied)"
        ]
    );

    let (events, _) = attempts(
        &Script {
            auth_info: Some("Welcome back.".into()),
            account_info: Some("Your password will expire in 2 days.".into()),
            ..Script::default()
        },
        &[PASSWORD],
    );
    assert_eq!(
        events,
        [
            "Notice(\"Welcome back.\")",
            "Notice(\"Your password will expire in 2 days.\")",
            "Outcome(Success)"
        ]
    );
}

#[test]
fn a_prompt_the_lock_screen_does_not_answer_fails_the_attempt() {
    for prompt in [Prompt::Echo, Prompt::Radio] {
        let (events, steps) = attempts(
            &Script {
                prompt,
                ..Script::default()
            },
            &[PASSWORD],
        );
        assert_eq!(steps, ["authenticate -> conv_err"], "{prompt:?}");
        assert_eq!(events.len(), 1);
        assert!(
            events[0].starts_with("Outcome(Error(\"authentication failed to run: "),
            "{prompt:?}: {events:?}"
        );
    }
}

#[test]
fn a_stack_that_fails_to_run_is_an_error_not_a_denial() {
    let (events, _) = attempts(
        &Script {
            authenticate: Some(Code::AuthinfoUnavail),
            ..Script::default()
        },
        &[PASSWORD],
    );
    assert_eq!(events.len(), 1);
    assert!(
        events[0].starts_with("Outcome(Error(\"authentication failed to run: "),
        "{events:?}"
    );
}
