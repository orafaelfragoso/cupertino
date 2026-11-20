//! The open / close state machine of the panel, with injected time (seconds on any monotonic clock).
//!
//! `hidden -> shown` on a toggle; `shown -> hiding` on a dismissal; `hiding -> hidden` once the fade-out has run. A toggle
//! while hiding opens the panel again.
//!
//! The compositor runs the fades on its clock, on the design catalogue's `SpringPreset::CONTROL_PANEL_OPEN` and
//! `Timing::CONTROL_PANEL_CLOSE` ([`open`], [`close`]); the lifecycle follows the same curves, so it knows how far a fade
//! has come when it turns and when the card's window may go. With Reduce Motion the panel appears and goes at once.

use cupertino_kit::motion::{SpringPreset, Timing};

/// The card's fade-in: the catalogue's critically damped rise of the control panel.
pub fn open() -> Timing {
    SpringPreset::CONTROL_PANEL_OPEN.into()
}

/// The card's fade-out: the catalogue's fall of the control panel, quick at first and slowing towards the end.
pub fn close() -> Timing {
    Timing::CONTROL_PANEL_CLOSE
}

/// How far `timing` has come `t` seconds after it started, 0 to 1; 1 from its end on.
fn progress(timing: Timing, t: f64) -> f64 {
    if t <= 0.0 {
        0.0
    } else if t >= timing.duration() {
        1.0
    } else {
        timing.value(t).clamp(0.0, 1.0)
    }
}

/// Where the panel is in its lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Hidden,
    Shown,
    Hiding,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Hidden,
    /// Opened at this time, from this opacity.
    Shown(f64, f64),
    /// Dismissed at this time, from this opacity.
    Hiding(f64, f64),
}

/// The lifetime of the panel. It fades in without moving and fades out; with Reduce Motion it appears and goes at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lifecycle {
    state: State,
    reduce_motion: bool,
}

impl Lifecycle {
    /// A hidden panel.
    pub fn new() -> Self {
        Self {
            state: State::Hidden,
            reduce_motion: false,
        }
    }

    /// With Reduce Motion the panel opens and closes at once, without a fade.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.reduce_motion = on;
    }

    fn fade_out_seconds(&self) -> f64 {
        if self.reduce_motion {
            0.0
        } else {
            close().duration()
        }
    }

    /// The phase at `now`.
    pub fn phase(&self, now: f64) -> Phase {
        match self.state {
            State::Hidden => Phase::Hidden,
            State::Shown(..) => Phase::Shown,
            State::Hiding(since, _) if now - since >= self.fade_out_seconds() => Phase::Hidden,
            State::Hiding(..) => Phase::Hiding,
        }
    }

    /// Opacity of the card at `now` as the compositor fades it: rising while it opens, 1 while shown, falling to 0
    /// while hiding. A reversal continues from where the fade was.
    pub fn opacity(&self, now: f64) -> f64 {
        match self.state {
            State::Hidden => 0.0,
            State::Shown(..) if self.reduce_motion => 1.0,
            State::Shown(since, from) => from + (1.0 - from) * progress(open(), now - since),
            State::Hiding(..) if self.reduce_motion => 0.0,
            State::Hiding(since, from) => from * (1.0 - progress(close(), now - since)),
        }
    }

    /// Seconds from `now` until the fade-out ends, while hiding.
    pub fn until_hidden(&self, now: f64) -> Option<f64> {
        match self.state {
            State::Hiding(since, _) if self.phase(now) == Phase::Hiding => {
                Some(since + self.fade_out_seconds() - now)
            }
            _ => None,
        }
    }

    /// Whether the panel is still fading in at `now`.
    pub fn opening(&self, now: f64) -> bool {
        matches!(self.state, State::Shown(since, _) if !self.reduce_motion && now - since < open().duration())
    }

    /// Open the panel when it is hidden or hiding, dismiss it when it is shown.
    pub fn toggle(&mut self, now: f64) {
        match self.phase(now) {
            Phase::Shown => self.dismiss(now),
            Phase::Hidden | Phase::Hiding => self.state = State::Shown(now, self.opacity(now)),
        }
    }

    /// Hide the panel at once, without the fade: its surfaces are gone with their output.
    pub fn hide(&mut self) {
        self.state = State::Hidden;
    }

    /// Start the fade-out of a shown panel; a hidden or hiding panel is left as it is.
    pub fn dismiss(&mut self, now: f64) {
        if self.phase(now) == Phase::Shown {
            self.state = State::Hiding(now, self.opacity(now));
        }
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}
