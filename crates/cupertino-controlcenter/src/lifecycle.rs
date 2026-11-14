//! The open / close state machine of the panel, with injected time (seconds on any monotonic clock).
//!
//! `hidden -> shown` on a toggle; `shown -> hiding` on a dismissal; `hiding -> hidden` after the fade-out (derived from the
//! observers). A toggle while hiding opens the panel again.
//!
//! The fades are the same whether the panel draws them frame by frame or the compositor runs them on its clock
//! ([`OPEN_CURVE`], [`CLOSE_CURVE`]).

use cupertino_animation::{CubicBezier, LayerCurve};

/// How long the panel takes to fade out: it falls away quickly at first and slows towards the end.
pub const FADE_SECONDS: f64 = 0.18;
/// Time constant of the fade-out's exponential fall, in seconds.
const FADE_TAU: f64 = 0.06;
/// How long the panel takes to fade in.
pub const OPEN_SECONDS: f64 = 0.23;
/// Rate of the fade-in's critically damped curve, in radians per second.
const OPEN_RATE: f64 = 28.0;

/// The fade-in as the compositor runs it: the critically damped rise of `OPEN_RATE`, a spring of damping ratio 1
/// whose undamped period is a turn at that rate. It follows `rise` to [`OPEN_SECONDS`] and then settles the last
/// hundredth smoothly instead of at once.
pub const OPEN_CURVE: LayerCurve = LayerCurve::Spring {
    response: std::f64::consts::TAU / OPEN_RATE,
    damping: 1.0,
};

/// The fade-out as the compositor runs it: `fall` over [`FADE_SECONDS`] as a cubic Bezier through its ends, fitted to
/// it within a two-thousandth everywhere.
pub const CLOSE_CURVE: LayerCurve = LayerCurve::Timed {
    duration: FADE_SECONDS,
    // control points in 256ths, as the wire carries them
    bezier: CubicBezier::new(0.234375, 0.7421875, 0.54296875, 0.92578125),
};

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
    fn a_toggle_opens_the_panel_fading_in_to_full_opacity() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(
            (lifecycle.phase(0.0), lifecycle.opacity(0.0)),
            (Phase::Hidden, 0.0)
        );
        lifecycle.toggle(10.0);
        assert_eq!(
            (lifecycle.phase(10.0), lifecycle.opacity(10.0)),
            (Phase::Shown, 0.0)
        );
        assert!(lifecycle.opening(10.1));
        let mid = lifecycle.opacity(10.05);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert!(lifecycle.opacity(10.1) > mid, "it rises");
        assert!(!lifecycle.opening(10.0 + OPEN_SECONDS));
        assert_eq!(lifecycle.opacity(10.0 + OPEN_SECONDS), 1.0);
        assert_eq!(
            (lifecycle.phase(500.0), lifecycle.opacity(500.0)),
            (Phase::Shown, 1.0)
        );
    }

    #[test]
    fn under_reduce_motion_the_panel_appears_and_goes_at_once() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.set_reduce_motion(true);
        lifecycle.toggle(1.0);
        assert_eq!(lifecycle.opacity(1.0), 1.0);
        assert!(!lifecycle.opening(1.0));
        lifecycle.dismiss(2.0);
        assert_eq!(
            (lifecycle.phase(2.0), lifecycle.opacity(2.0)),
            (Phase::Hidden, 0.0)
        );
    }

    #[test]
    fn dismissing_fades_out_quickly_at_first_and_ends_after_180_milliseconds() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.toggle(0.0);
        lifecycle.dismiss(1.0);
        assert_eq!(
            (lifecycle.phase(1.0), lifecycle.opacity(1.0)),
            (Phase::Hiding, 1.0)
        );
        let (first, second) = (
            1.0 - lifecycle.opacity(1.06),
            lifecycle.opacity(1.06) - lifecycle.opacity(1.12),
        );
        assert!(first > 2.0 * second, "{first} then {second}");
        assert!(
            (lifecycle.opacity(1.06)
                - ((-1.0f64).exp() - (-3.0f64).exp()) / (1.0 - (-3.0f64).exp()))
            .abs()
                < 1e-9
        );
        assert_eq!(lifecycle.phase(1.1799), Phase::Hiding);
        assert_eq!(
            (lifecycle.phase(1.1801), lifecycle.opacity(1.1801)),
            (Phase::Hidden, 0.0)
        );
    }

    #[test]
    fn the_compositor_runs_the_same_fades() {
        use cupertino_animation::LayerProperty;
        let mut open = LayerProperty::new(0.0);
        open.animate(1.0, OPEN_CURVE, 0.0, 0.0);
        let mut close = LayerProperty::new(1.0);
        close.animate(0.0, CLOSE_CURVE, 0.0, 0.0);
        for ms in 0..=230 {
            let t = f64::from(ms) / 1000.0;
            let t_open = t.min(OPEN_SECONDS - 1e-9);
            assert!(
                (open.value(t_open) - rise(t_open)).abs() < 1e-6,
                "open at {ms} ms"
            );
            assert!(
                (close.value(t) - fall(t)).abs() < 5e-4,
                "close at {ms} ms: {} against {}",
                close.value(t),
                fall(t)
            );
        }
        assert!(1.0 - open.value(OPEN_SECONDS) < 0.013, "the last hundredth");
        assert!(!close.is_animating(FADE_SECONDS));
    }

    #[test]
    fn the_time_to_the_end_of_the_fade_counts_down() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(lifecycle.until_hidden(0.0), None);
        lifecycle.toggle(0.0);
        assert_eq!(lifecycle.until_hidden(0.0), None);
        lifecycle.dismiss(2.0);
        assert!((lifecycle.until_hidden(2.1).expect("hiding") - 0.08).abs() < 1e-9);
        assert_eq!(lifecycle.until_hidden(2.3), None);
    }

    #[test]
    fn a_toggle_while_shown_dismisses_and_a_toggle_while_hiding_reopens_from_where_the_fade_was() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.toggle(0.0);
        lifecycle.toggle(1.0);
        assert_eq!(lifecycle.phase(1.1), Phase::Hiding);
        let left = lifecycle.opacity(1.1);
        lifecycle.toggle(1.1);
        assert_eq!(
            (lifecycle.phase(1.1), lifecycle.opacity(1.1)),
            (Phase::Shown, left)
        );
        assert_eq!(lifecycle.opacity(1.1 + OPEN_SECONDS + 1e-6), 1.0);
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
