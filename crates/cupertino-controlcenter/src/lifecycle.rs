//! The open / close state machine of the panel, with injected time (seconds on any monotonic clock).
//!
//! `hidden -> shown` on a toggle; `shown -> hiding` on a dismissal; `hiding -> hidden` after the fade-out (derived from the
//! observers). A toggle while hiding opens the panel again.

/// Fade-out of the panel, linear (0.238 s, curve).
pub const FADE_SECONDS: f64 = 0.238;

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
    Shown,
    /// Dismissed at this time.
    Hiding(f64),
}

/// The lifetime of the panel. Opening is immediate: the design has no open animation for the panel itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lifecycle {
    state: State,
}

impl Lifecycle {
    /// A hidden panel.
    pub fn new() -> Self {
        Self {
            state: State::Hidden,
        }
    }

    /// The phase at `now`.
    pub fn phase(&self, now: f64) -> Phase {
        match self.state {
            State::Hidden => Phase::Hidden,
            State::Shown => Phase::Shown,
            State::Hiding(since) if now - since >= FADE_SECONDS => Phase::Hidden,
            State::Hiding(_) => Phase::Hiding,
        }
    }

    /// Opacity of the card and its material at `now`: 1 while shown, falling linearly to 0 while hiding.
    pub fn opacity(&self, now: f64) -> f64 {
        match self.state {
            State::Hidden => 0.0,
            State::Shown => 1.0,
            State::Hiding(since) => (1.0 - (now - since) / FADE_SECONDS).clamp(0.0, 1.0),
        }
    }

    /// Seconds from `now` until the fade-out ends, while hiding.
    pub fn until_hidden(&self, now: f64) -> Option<f64> {
        match self.state {
            State::Hiding(since) if self.phase(now) == Phase::Hiding => {
                Some(since + FADE_SECONDS - now)
            }
            _ => None,
        }
    }

    /// Open the panel when it is hidden or hiding, dismiss it when it is shown.
    pub fn toggle(&mut self, now: f64) {
        match self.phase(now) {
            Phase::Shown => self.dismiss(now),
            Phase::Hidden | Phase::Hiding => self.state = State::Shown,
        }
    }

    /// Hide the panel at once, without the fade: its surfaces are gone with their output.
    pub fn hide(&mut self) {
        self.state = State::Hidden;
    }

    /// Start the fade-out of a shown panel; a hidden or hiding panel is left as it is.
    pub fn dismiss(&mut self, now: f64) {
        if self.phase(now) == Phase::Shown {
            self.state = State::Hiding(now);
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
