//! Activation-token rule (control flow and constants).
//!
//! The design's token store has no expiry; the window server decides at use time,
//! against the target application's last activation and the last user event: a token
//! older than the target's last activation is dead, a newer one is honoured, and otherwise (equal,
//! or the target was never active) a key or button press more than [`USER_EVENT_SLACK`] after
//! the token denies it. The last user event is the last key-down or mouse-button-down.
//! Tokens are single use; that bookkeeping is the compositor's, which owns the tokens.

/// A user event this long after the token's creation denies it (100 ms,:
/// 100 000 000 ns on the nanosecond event clock).
pub const USER_EVENT_SLACK: f64 = 0.1;

/// Whether a token created at `token` activates an application last activated at
/// `last_activation`, the last user event having happened at `last_user_event` (all in seconds of
/// one clock).
pub fn token_accepted(token: f64, last_activation: Option<f64>, last_user_event: Option<f64>) -> bool {
    match last_activation {
        Some(latest) if token < latest => false,
        Some(latest) if token > latest => true,
        _ => !last_user_event.is_some_and(|event| event > token + USER_EVENT_SLACK),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_older_than_the_last_activation_is_dead_and_a_newer_one_wins() {
        assert!(!token_accepted(5.0, Some(6.0), None));
        assert!(token_accepted(7.0, Some(6.0), Some(100.0)), "newer than the last activation: no user-event test");
    }

    #[test]
    fn without_a_newer_activation_a_later_user_event_denies_after_100_ms() {
        assert!(token_accepted(5.0, None, None));
        assert!(token_accepted(5.0, None, Some(5.1)), "exactly the slack still passes");
        assert!(!token_accepted(5.0, None, Some(5.11)));
        assert!(!token_accepted(5.0, Some(5.0), Some(5.2)), "equal to the last activation runs the user-event test");
        assert!(token_accepted(5.0, None, Some(1.0)), "an earlier event does not count");
    }
}
