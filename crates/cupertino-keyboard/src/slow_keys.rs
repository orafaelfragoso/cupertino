//! Slow keys: a key only counts once it has been held for the acceptance delay.
//!
//! The design keyboard filter keeps a copy of the key-down and arms a timer of `delay` ms:
//!
//! * the timer fires: the stored press is sent on (the key is accepted);
//! * the key goes up before the timer: the timer is cancelled and both the press and the release vanish;
//! * a *different* key goes down while one is pending: the pending press is cancelled and the new press is rejected
//!   too (nothing reaches the keyboard state); the same key going down again restarts the wait;
//! * the menu key (consumer usage 0x40) is exempt.
//!
//! The filter passes the release of a rejected key on (there is no stored copy to match it); the event system ignores
//! a release whose press it never saw, so the model drops it instead ([`SlowStep::Drop`], equivalent).
//! While slow keys are on, the caps-lock delay is skipped and the key repeat runs at
//! [`crate::repeat::SLOW_KEYS_REPEAT_MS`].
//!
//! Key codes are Linux evdev codes; times are milliseconds on any monotonic clock.

use std::collections::BTreeSet;

/// Acceptance delay until configured: in the preference defaults.
pub const DEFAULT_DELAY_MS: u32 = 250;

/// evdev `KEY_MENU`, the key of consumer usage 0x40 that slow keys never hold back.
const KEY_MENU: u32 = 139;

/// What happens to one key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlowStep {
    /// Pass the event on now.
    Deliver,
    /// The press is held back; [`SlowKeys::due`] releases it at [`SlowKeys::deadline`].
    Pending,
    /// The press was rejected because another key was pending; nothing is delivered.
    Rejected,
    /// The event vanishes (the release of a press that was never delivered).
    Drop,
}

/// Slow-keys filter state.
#[derive(Clone, Debug)]
pub struct SlowKeys {
    delay_ms: u32,
    /// The held-back key and the time its press is accepted.
    pending: Option<(u32, u64)>,
    /// Keys whose press was delivered and whose release is still to come.
    delivered: BTreeSet<u32>,
}

impl SlowKeys {
    pub fn new(delay_ms: u32) -> SlowKeys {
        SlowKeys { delay_ms, pending: None, delivered: BTreeSet::new() }
    }

    pub fn delay_ms(&self) -> u32 {
        self.delay_ms
    }

    /// A key went down at `now_ms`.
    pub fn press(&mut self, code: u32, now_ms: u64) -> SlowStep {
        if code == KEY_MENU {
            return SlowStep::Deliver;
        }
        match self.pending {
            Some((pending, _)) if pending != code => {
                self.pending = None;
                SlowStep::Rejected
            }
            _ => {
                self.pending = Some((code, now_ms + u64::from(self.delay_ms)));
                SlowStep::Pending
            }
        }
    }

    /// A key went up.
    pub fn release(&mut self, code: u32) -> SlowStep {
        if code == KEY_MENU {
            return SlowStep::Deliver;
        }
        if self.pending.is_some_and(|(pending, _)| pending == code) {
            self.pending = None;
            return SlowStep::Drop;
        }
        if self.delivered.remove(&code) {
            SlowStep::Deliver
        } else {
            SlowStep::Drop
        }
    }

    /// When the pending press is accepted, if one is pending.
    pub fn deadline(&self) -> Option<u64> {
        self.pending.map(|(_, at)| at)
    }

    /// The pending key whose delay has elapsed at `now_ms`: its press is to be delivered now.
    pub fn due(&mut self, now_ms: u64) -> Option<u32> {
        let (code, at) = self.pending?;
        if at > now_ms {
            return None;
        }
        self.pending = None;
        self.delivered.insert(code);
        Some(code)
    }

    /// Forget everything (slow keys switched off): presses already delivered keep their releases, so the caller
    /// passes releases through once the filter is off.
    pub fn reset(&mut self) {
        self.pending = None;
        self.delivered.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u32 = 30;
    const B: u32 = 48;

    #[test]
    fn a_held_key_is_accepted_after_the_delay_and_its_release_passes() {
        let mut s = SlowKeys::new(DEFAULT_DELAY_MS);
        assert_eq!(s.press(A, 1000), SlowStep::Pending);
        assert_eq!(s.deadline(), Some(1250));
        assert_eq!(s.due(1249), None);
        assert_eq!(s.due(1250), Some(A));
        assert_eq!(s.deadline(), None);
        assert_eq!(s.release(A), SlowStep::Deliver);
    }

    #[test]
    fn a_short_press_vanishes_completely() {
        let mut s = SlowKeys::new(250);
        assert_eq!(s.press(A, 0), SlowStep::Pending);
        assert_eq!(s.release(A), SlowStep::Drop);
        assert_eq!(s.due(1000), None);
    }

    #[test]
    fn a_second_key_while_one_is_pending_rejects_both() {
        let mut s = SlowKeys::new(250);
        s.press(A, 0);
        assert_eq!(s.press(B, 100), SlowStep::Rejected);
        assert_eq!(s.due(1000), None, "the pending key was cancelled");
        assert_eq!(s.release(B), SlowStep::Drop);
        assert_eq!(s.release(A), SlowStep::Drop);
    }

    #[test]
    fn the_same_key_again_restarts_the_wait() {
        let mut s = SlowKeys::new(250);
        s.press(A, 0);
        assert_eq!(s.press(A, 200), SlowStep::Pending);
        assert_eq!(s.due(300), None);
        assert_eq!(s.due(450), Some(A));
    }

    #[test]
    fn a_new_key_after_acceptance_waits_on_its_own() {
        let mut s = SlowKeys::new(250);
        s.press(A, 0);
        s.due(250);
        assert_eq!(s.press(B, 300), SlowStep::Pending);
        assert_eq!(s.release(A), SlowStep::Deliver, "the accepted key's release still passes");
        assert_eq!(s.due(550), Some(B));
    }

    #[test]
    fn the_menu_key_is_exempt() {
        let mut s = SlowKeys::new(250);
        assert_eq!(s.press(KEY_MENU, 0), SlowStep::Deliver);
        assert_eq!(s.release(KEY_MENU), SlowStep::Deliver);
        assert_eq!(s.deadline(), None);
    }
}
