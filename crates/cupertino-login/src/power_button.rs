//! What a press and release of the power button do in a logged-in session. Pure: the process feeds it the key's
//! press and release and the time, an absolute `f64` in seconds on its own clock, and carries out what it returns.
//!
//! A release within [`DEBOUNCE_SECONDS`] is ignored. A release after that sleeps the system when `PowerButtonSleepsSystem` is
//! on and is ignored when it is off. Holding to [`SHUT_DOWN_UI_SECONDS`] shows the power-button panel, whatever the
//! setting. The server drops auto-repeat, so every press starts a hold: a release that never arrived (a failed call, a
//! restart) leaves the next press working.

/// A release before this is a bounce or a brush (`debounce`).
pub const DEBOUNCE_SECONDS: f64 = 0.35;
/// How long the key is held until the panel shows (`shutdown-UI time`).
pub const SHUT_DOWN_UI_SECONDS: f64 = 1.5;

/// What the process must do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Put the computer to sleep.
    Sleep,
    /// Show the power-button panel (the process leaves it out when a panel or a log-out is already running).
    ShowPanel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Up,
    /// Pressed at this time and nothing done yet.
    Down(f64),
    /// Held; the panel was asked for, so the release does nothing.
    PanelShown,
}

/// The hold timing of one power key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PowerButton {
    /// `PowerButtonSleepsSystem` (default off).
    sleeps_system: bool,
    state: State,
}

impl PowerButton {
    pub fn new(sleeps_system: bool) -> Self {
        PowerButton { sleeps_system, state: State::Up }
    }

    /// The key went down: a hold starts, whatever came before.
    pub fn press(&mut self, now: f64) {
        self.state = State::Down(now);
    }

    /// The key went up. A release without a press (the press happened while the lock screen was up) does nothing.
    pub fn release(&mut self, now: f64) -> Option<Effect> {
        let state = std::mem::replace(&mut self.state, State::Up);
        let State::Down(since) = state else { return None };
        let held = now - since;
        if held >= SHUT_DOWN_UI_SECONDS {
            Some(Effect::ShowPanel)
        } else if held > DEBOUNCE_SECONDS && self.sleeps_system {
            Some(Effect::Sleep)
        } else {
            None
        }
    }

    /// Time passed while the key may be down: at [`SHUT_DOWN_UI_SECONDS`] the panel is asked for, once.
    pub fn tick(&mut self, now: f64) -> Option<Effect> {
        match self.state {
            State::Down(since) if now - since >= SHUT_DOWN_UI_SECONDS => {
                self.state = State::PanelShown;
                Some(Effect::ShowPanel)
            }
            State::Up | State::Down(_) | State::PanelShown => None,
        }
    }

    /// Seconds until [`PowerButton::tick`] has something to do; `None` while no hold is waiting.
    pub fn next_wakeup(&self, now: f64) -> Option<f64> {
        match self.state {
            State::Down(since) => Some((since + SHUT_DOWN_UI_SECONDS - now).max(0.0)),
            State::Up | State::PanelShown => None,
        }
    }

    /// Forget a key that is down: the lock screen swallows the press and the release.
    pub fn reset(&mut self) {
        self.state = State::Up;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Press at 10.0, release `held` seconds later, with a tick at the release's time first as the loop would.
    fn tap(sleeps_system: bool, held: f64) -> Option<Effect> {
        let mut button = PowerButton::new(sleeps_system);
        button.press(10.0);
        button.tick(10.0 + held).or_else(|| button.release(10.0 + held))
    }

    #[test]
    fn a_release_within_the_debounce_is_ignored_with_either_setting() {
        assert_eq!((tap(false, 0.2), tap(true, 0.2)), (None, None));
    }

    #[test]
    fn a_release_between_the_debounce_and_the_hold_sleeps_only_when_the_setting_is_on() {
        assert_eq!((tap(false, 0.5), tap(true, 0.5)), (None, Some(Effect::Sleep)));
        assert_eq!((tap(false, 1.49), tap(true, 1.49)), (None, Some(Effect::Sleep)));
    }

    #[test]
    fn holding_to_one_and_a_half_seconds_shows_the_panel_once_with_either_setting() {
        for sleeps_system in [false, true] {
            let mut button = PowerButton::new(sleeps_system);
            button.press(10.0);
            assert_eq!(button.tick(11.49), None);
            assert_eq!(button.tick(11.5), Some(Effect::ShowPanel));
            assert_eq!(button.tick(12.0), None);
            assert_eq!(button.release(12.5), None);
            assert_eq!(button.next_wakeup(12.5), None);
        }
    }

    #[test]
    fn a_late_loop_still_shows_the_panel_for_a_release_after_the_hold_time() {
        let mut button = PowerButton::new(false);
        button.press(10.0);
        assert_eq!(button.release(12.0), Some(Effect::ShowPanel));
    }

    #[test]
    fn a_press_after_a_lost_release_starts_a_new_hold() {
        let mut button = PowerButton::new(true);
        button.press(10.0);
        assert_eq!(button.tick(11.5), Some(Effect::ShowPanel));
        button.press(20.0);
        assert_eq!(button.tick(20.4), None);
        assert_eq!(button.release(20.5), Some(Effect::Sleep));
    }

    #[test]
    fn the_wakeup_counts_down_to_the_hold_time_while_the_key_is_down() {
        let mut button = PowerButton::new(false);
        assert_eq!(button.next_wakeup(10.0), None);
        button.press(10.0);
        assert_eq!((button.next_wakeup(10.0), button.next_wakeup(10.5), button.next_wakeup(13.0)), (Some(1.5), Some(1.0), Some(0.0)));
    }

    #[test]
    fn a_release_without_a_press_and_a_reset_key_do_nothing() {
        let mut button = PowerButton::new(true);
        assert_eq!(button.release(5.0), None);
        button.press(10.0);
        button.reset();
        assert_eq!((button.tick(12.0), button.release(12.0), button.next_wakeup(12.0)), (None, None, None));
    }
}
