//! An attempt on the lock screen's state machine (`cupertino_login::model`), as the lock screen and the login screen
//! drive it: a wrong password shakes the field and keeps the lock, an error keeps it and says why, and only an
//! accepted attempt (or the session manager's proof of a fast user switch) unlocks.

use cupertino_login::model::{
    AuthOutcome, LockScreen, NoAttempt, Phase, HIDE_AFTER, SHAKE_DURATION,
};

/// A lock screen at rest at time 0, its interface shown.
fn locked() -> LockScreen {
    let mut screen = LockScreen::new(0.0, HIDE_AFTER);
    screen.settle_interface();
    screen
}

#[test]
fn a_wrong_password_shakes_the_field_and_keeps_the_lock() {
    let mut screen = locked();
    let password = screen.submit(1.0, "hunter2").expect("a password to check");
    assert_eq!(password.as_str(), "hunter2");
    assert!(screen.checking(), "the attempt is checked");
    assert!(!screen.editable(), "nothing is typed while it is checked");
    let field = screen.field();

    screen.finish_auth(1.5, AuthOutcome::Denied).unwrap();
    assert_eq!(screen.phase(), Phase::Failed { since: 1.5 });
    assert!(screen.failure_outline(), "the field shows the failure");
    assert!(screen.denied_said(), "and says the password was wrong");
    let offsets: Vec<f64> = (1..20)
        .map(|i| screen.shake(1.5 + SHAKE_DURATION * f64::from(i) / 20.0))
        .collect();
    assert!(
        offsets.iter().any(|x| *x > 1.0) && offsets.iter().any(|x| *x < -1.0),
        "the field shakes both ways: {offsets:?}"
    );
    assert!(screen.editable(), "a retry is typed during the shake");

    let after = 1.5 + SHAKE_DURATION + 0.01;
    screen.tick(after);
    assert_eq!(screen.phase(), Phase::Locked, "the lock stays");
    assert_eq!(screen.shake(after), 0.0, "the shake is over");
    assert_eq!(
        screen.field(),
        field,
        "the field emptied at the submission stays empty"
    );
    for later in [2.0, 10.0, 60.0] {
        screen.tick(later);
        assert_ne!(
            screen.phase(),
            Phase::Unlocked,
            "a wrong password never unlocks"
        );
    }
}

#[test]
fn reduce_motion_shows_the_failure_without_the_shake() {
    let mut screen = locked();
    screen.set_reduce_motion(true);
    screen.submit(1.0, "wrong").unwrap();
    screen.finish_auth(1.2, AuthOutcome::Denied).unwrap();
    assert!(screen.failure_outline());
    assert_eq!(screen.shake(1.3), 0.0);
}

#[test]
fn an_error_keeps_the_lock_and_says_why() {
    let mut screen = locked();
    screen.submit(1.0, "pw").unwrap();
    screen.notify("Account locked for 10 minutes.".into());
    screen
        .finish_auth(1.4, AuthOutcome::Error("This account is locked.".into()))
        .unwrap();
    assert_eq!(screen.phase(), Phase::Locked);
    assert_eq!(screen.notice(), Some("This account is locked."));
    assert!(!screen.failure_outline(), "it is no wrong password");
    assert_eq!(screen.shake(1.5), 0.0);
}

#[test]
fn only_an_accepted_attempt_unlocks() {
    let mut screen = locked();
    assert_eq!(
        screen.finish_auth(1.0, AuthOutcome::Success),
        Err(NoAttempt),
        "an answer without an attempt is refused"
    );
    assert_eq!(
        screen.submit(1.0, ""),
        None,
        "an empty field submits nothing"
    );
    screen.submit(1.0, "pw").unwrap();
    assert_eq!(screen.submit(1.1, "again"), None, "one attempt at a time");
    screen.finish_auth(2.0, AuthOutcome::Success).unwrap();
    assert_eq!(screen.phase(), Phase::Unlocking);
    screen.tick(10.0);
    assert_eq!(screen.phase(), Phase::Unlocked);
}

#[test]
fn the_session_manager_unlocks_a_fast_user_switch_from_any_phase() {
    let mut screen = locked();
    screen.submit(1.0, "pw").unwrap();
    screen.unlock_by_switch(1.5);
    assert_eq!(screen.phase(), Phase::Unlocking);
    screen.tick(10.0);
    assert_eq!(screen.phase(), Phase::Unlocked);
}
