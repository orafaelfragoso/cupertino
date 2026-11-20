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
        PowerButton {
            sleeps_system,
            state: State::Up,
        }
    }

    /// The key went down: a hold starts, whatever came before.
    pub fn press(&mut self, now: f64) {
        self.state = State::Down(now);
    }

    /// The key went up. A release without a press (the press happened while the lock screen was up) does nothing.
    pub fn release(&mut self, now: f64) -> Option<Effect> {
        let state = std::mem::replace(&mut self.state, State::Up);
        let State::Down(since) = state else {
            return None;
        };
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
