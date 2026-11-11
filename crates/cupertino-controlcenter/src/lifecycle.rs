//! The open / close state machine of the panel, with injected time (seconds on any monotonic clock).
//!
//! `hidden -> shown` on a toggle; `shown -> hiding` on a dismissal; `hiding -> hidden` after the fade-out (derived from the
//! observers). A toggle while hiding opens the panel again.

/// How long the panel takes to fade out: it falls away quickly at first and slows towards the end.
pub const FADE_SECONDS: f64 = 0.18;
/// Time constant of the fade-out's exponential fall, in seconds.
const FADE_TAU: f64 = 0.06;
/// How long the panel takes to fade in.
pub const OPEN_SECONDS: f64 = 0.23;
/// Rate of the fade-in's critically damped curve, in radians per second.
const OPEN_RATE: f64 = 28.0;

/// How far a critically damped rise of [`OPEN_RATE`] has come after `t` seconds, 0 to 1; 1 from [`OPEN_SECONDS`] on.
fn rise(t: f64) -> f64 {
    if t <= 0.0 {
        0.0
    } else if t >= OPEN_SECONDS {
        1.0
    } else {
        let wt = OPEN_RATE * t;
        1.0 - (1.0 + wt) * (-wt).exp()
    }
}

/// What is left of a fade-out `t` seconds after it began: an exponential fall stretched so it reaches 0 at
/// [`FADE_SECONDS`].
fn fall(t: f64) -> f64 {
    if t <= 0.0 {
        return 1.0;
    }
    if t >= FADE_SECONDS {
        return 0.0;
    }
    let end = (-FADE_SECONDS / FADE_TAU).exp();
    (((-t / FADE_TAU).exp() - end) / (1.0 - end)).clamp(0.0, 1.0)
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
            FADE_SECONDS
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

    /// Opacity of the card and its material at `now`: rising while it opens, 1 while shown, falling to 0 while hiding.
    /// A reversal continues from where the fade was.
    pub fn opacity(&self, now: f64) -> f64 {
        match self.state {
            State::Hidden => 0.0,
            State::Shown(..) if self.reduce_motion => 1.0,
            State::Shown(since, from) => from + (1.0 - from) * rise(now - since),
            State::Hiding(since, from) => {
                if self.reduce_motion {
                    0.0
                } else {
                    from * fall(now - since)
                }
            }
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
        matches!(self.state, State::Shown(since, _) if !self.reduce_motion && now - since < OPEN_SECONDS)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toggle_opens_the_panel_at_full_opacity() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(
            (lifecycle.phase(0.0), lifecycle.opacity(0.0)),
            (Phase::Hidden, 0.0)
        );
        lifecycle.toggle(10.0);
        assert_eq!(
            (lifecycle.phase(10.0), lifecycle.opacity(10.0)),
            (Phase::Shown, 1.0)
        );
        assert_eq!(
            (lifecycle.phase(500.0), lifecycle.opacity(500.0)),
            (Phase::Shown, 1.0)
        );
    }

    #[test]
    fn dismissing_fades_out_linearly_over_238_milliseconds() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.toggle(0.0);
        lifecycle.dismiss(1.0);
        assert_eq!(
            (lifecycle.phase(1.0), lifecycle.opacity(1.0)),
            (Phase::Hiding, 1.0)
        );
        assert_eq!(lifecycle.opacity(1.119), 0.5);
        assert_eq!(lifecycle.phase(1.2379), Phase::Hiding);
        assert_eq!(
            (lifecycle.phase(1.238), lifecycle.opacity(1.238)),
            (Phase::Hidden, 0.0)
        );
    }

    #[test]
    fn the_time_to_the_end_of_the_fade_counts_down() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(lifecycle.until_hidden(0.0), None);
        lifecycle.toggle(0.0);
        assert_eq!(lifecycle.until_hidden(0.0), None);
        lifecycle.dismiss(2.0);
        assert!((lifecycle.until_hidden(2.1).expect("hiding") - 0.138).abs() < 1e-9);
        assert_eq!(lifecycle.until_hidden(2.3), None);
    }

    #[test]
    fn a_toggle_while_shown_dismisses_and_a_toggle_while_hiding_reopens() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.toggle(0.0);
        lifecycle.toggle(1.0);
        assert_eq!(lifecycle.phase(1.1), Phase::Hiding);
        lifecycle.toggle(1.1);
        assert_eq!(
            (lifecycle.phase(1.1), lifecycle.opacity(1.1)),
            (Phase::Shown, 1.0)
        );
    }

    #[test]
    fn hiding_skips_the_fade_from_any_phase_and_a_toggle_opens_again() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.toggle(0.0);
        lifecycle.hide();
        assert_eq!(
            (lifecycle.phase(0.0), lifecycle.opacity(0.0)),
            (Phase::Hidden, 0.0)
        );
        lifecycle.toggle(1.0);
        lifecycle.dismiss(2.0);
        lifecycle.hide();
        assert_eq!(lifecycle.phase(2.0), Phase::Hidden);
        assert_eq!(lifecycle.until_hidden(2.0), None);
        lifecycle.toggle(3.0);
        assert_eq!(lifecycle.phase(3.0), Phase::Shown);
    }

    #[test]
    fn dismissing_a_hidden_or_hiding_panel_changes_nothing() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.dismiss(0.0);
        assert_eq!(lifecycle.phase(0.0), Phase::Hidden);
        lifecycle.toggle(1.0);
        lifecycle.dismiss(2.0);
        lifecycle.dismiss(2.1);
        assert_eq!(lifecycle.phase(2.239), Phase::Hidden);
    }
}
