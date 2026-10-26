//! Spring-loaded folders: opening a folder when a drag dwells over it.
//!
//! * The preference stores the delay in seconds (global domain); the design converts it to the
//!   slider ticks `(int)(seconds * 1002 + 167) clamped to 167..=1169, default 668 ticks
//!   which is 0.5 s.
//! * Spring-loading is enabled by default.
//! * Holding the pointer inside the same target restarts nothing; entering another target or
//!   leaving restarts / cancels (behaviour).

use crate::metrics::timing::{
    SPRING_DELAY_DEFAULT_TICKS, SPRING_TICKS_MAX, SPRING_TICKS_MIN, SPRING_TICKS_PER_SECOND,
};

pub fn ticks_from_seconds(seconds: f64) -> i32 {
    ((seconds * SPRING_TICKS_PER_SECOND + SPRING_TICKS_MIN as f64) as i32)
        .clamp(SPRING_TICKS_MIN, SPRING_TICKS_MAX)
}

/// Inverse of [`ticks_from_seconds`] (the design only shows the forward direction).
pub fn seconds_from_ticks(ticks: i32) -> f64 {
    (ticks.clamp(SPRING_TICKS_MIN, SPRING_TICKS_MAX) - SPRING_TICKS_MIN) as f64
        / SPRING_TICKS_PER_SECOND
}

pub fn default_delay() -> f64 {
    seconds_from_ticks(SPRING_DELAY_DEFAULT_TICKS)
}

#[derive(Clone, Debug)]
pub struct SpringLoad<T: PartialEq + Clone> {
    pub enabled: bool,
    pub delay: f64,
    target: Option<(T, f64)>,
    fired: bool,
}

impl<T: PartialEq + Clone> Default for SpringLoad<T> {
    fn default() -> Self {
        SpringLoad {
            enabled: true,
            delay: default_delay(),
            target: None,
            fired: false,
        }
    }
}

impl<T: PartialEq + Clone> SpringLoad<T> {
    /// The drag is over `target` (or over nothing) at time `now`.
    pub fn hover(&mut self, target: Option<T>, now: f64) {
        match (&self.target, target) {
            (Some((t, _)), Some(n)) if *t == n => {}
            (_, Some(n)) => {
                self.target = Some((n, now));
                self.fired = false;
            }
            (_, None) => {
                self.target = None;
                self.fired = false;
            }
        }
    }

    pub fn cancel(&mut self) {
        self.hover(None, 0.0);
    }

    /// Target to open now, at most once per dwell.
    pub fn poll(&mut self, now: f64) -> Option<T> {
        if !self.enabled || self.fired {
            return None;
        }
        match &self.target {
            Some((t, start)) if now - start >= self.delay => {
                self.fired = true;
                Some(t.clone())
            }
            _ => None,
        }
    }

    /// Progress 0..=1 of the dwell, for the pulsing highlight.
    pub fn progress(&self, now: f64) -> f64 {
        match &self.target {
            Some((_, start)) => ((now - start) / self.delay).clamp(0.0, 1.0),
            None => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_conversion_matches_the_measured_default() {
        assert_eq!(ticks_from_seconds(0.5), 668);
        assert_eq!(ticks_from_seconds(0.0), 167);
        assert_eq!(ticks_from_seconds(1.0), 1169);
        assert_eq!(ticks_from_seconds(5.0), 1169);
        assert!((default_delay() - 0.5).abs() < 1e-3);
        assert!((seconds_from_ticks(1169) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn opens_after_the_delay_once() {
        let mut s: SpringLoad<u32> = SpringLoad::default();
        s.hover(Some(7), 10.0);
        assert_eq!(s.poll(10.3), None);
        s.hover(Some(7), 10.4);
        assert_eq!(s.poll(10.51), Some(7));
        assert_eq!(s.poll(11.0), None);
    }

    #[test]
    fn moving_to_another_target_restarts() {
        let mut s: SpringLoad<u32> = SpringLoad::default();
        s.hover(Some(1), 0.0);
        s.hover(Some(2), 0.4);
        assert_eq!(s.poll(0.6), None);
        assert_eq!(s.poll(0.95), Some(2));
        s.hover(None, 1.0);
        s.hover(Some(2), 1.1);
        assert_eq!(s.poll(1.2), None);
    }

    #[test]
    fn disabled_never_fires() {
        let mut s: SpringLoad<u32> = SpringLoad {
            enabled: false,
            ..Default::default()
        };
        s.hover(Some(1), 0.0);
        assert_eq!(s.poll(100.0), None);
    }

    #[test]
    fn progress_is_clamped() {
        let mut s: SpringLoad<u32> = SpringLoad::default();
        assert_eq!(s.progress(5.0), 0.0);
        s.hover(Some(1), 0.0);
        assert!((s.progress(0.25) - 0.5).abs() < 0.01);
        assert_eq!(s.progress(9.0), 1.0);
    }
}
