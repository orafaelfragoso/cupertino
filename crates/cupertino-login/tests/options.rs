//! The command line of `cupertino-login` (`cupertino_login::options`): the lock screen's options, and `--greeter` alone.
#![cfg(feature = "app")]

use cupertino_login::options::{Options, SessionOptions};
use std::time::Duration;

fn parse(args: &[&str]) -> Result<Options, String> {
    Options::parse(args.iter().map(|a| a.to_string()))
}

#[test]
fn the_lock_screen_takes_its_idle_time_and_hint_threshold() {
    assert_eq!(
        parse(&[]),
        Ok(Options::Session(SessionOptions {
            idle_timeout: None,
            retries_until_hint: 0,
        }))
    );
    assert_eq!(
        parse(&["--idle-timeout", "300", "--retries-until-hint", "3"]),
        Ok(Options::Session(SessionOptions {
            idle_timeout: Some(Duration::from_secs(300)),
            retries_until_hint: 3,
        }))
    );
}

#[test]
fn the_login_screen_takes_no_option() {
    assert_eq!(parse(&["--greeter"]), Ok(Options::Greeter));
    assert!(parse(&["--greeter", "--idle-timeout", "5"]).is_err());
}

#[test]
fn a_wrong_argument_is_refused() {
    for args in [
        &["--idle-timeout"][..],
        &["--idle-timeout", "0"],
        &["--idle-timeout", "soon"],
        &["--retries-until-hint", "-1"],
        &["--power-button-sleeps-system"],
    ] {
        assert!(parse(args).is_err(), "{args:?}");
    }
}
