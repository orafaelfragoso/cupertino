//! The open/close transition of the overview: a progress from 0 (desktop) to 1 (overview) that a
//! gesture drives 1:1 and a spring completes. The display server owns it (it is bound to input and
//! the frame clock); the dock process draws the overview's chrome around it.
//!
//! The spring is critically damped with a response of 0.25 s (2.0 s while the slow-motion modifier is held); a
//! trackpad release faster than 5 progress per second settles on a response 0.6 times as long. Approximate: the swipe
//! distance, the rubber band and the velocity projection.

use cupertino_animation::{AnimatedValue, Curve, SpringParams};

use crate::config::OverviewConfig;

/// Progress-driven open/close transition.
#[derive(Clone, Debug)]
pub struct Transition {
    pub progress: AnimatedValue<f64>,
    swipe_distance: f64,
    rubber_band: f64,
    projection_s: f64,
    spring: SpringParams,
    slow_spring: SpringParams,
    slow_motion: bool,
    fast_release_speed: f64,
    fast_release_scale: f64,
}

impl Transition {
    pub fn new(cfg: &OverviewConfig) -> Self {
        Self {
            progress: AnimatedValue::new(0.0),
            swipe_distance: cfg.swipe_distance,
            rubber_band: cfg.rubber_band,
            projection_s: cfg.projection_s,
            spring: cfg.spring,
            slow_spring: cfg.slow_motion_spring(),
            slow_motion: false,
            fast_release_speed: cfg.fast_release_speed,
            fast_release_scale: cfg.fast_release_scale,
        }
    }

    /// Gesture-driven progress (0 = desktop, 1 = overview), rubber-banded beyond the ends.
    pub fn gesture_update(&mut self, cumulative_delta: f64, start_progress: f64, now: f64) {
        let raw = start_progress + cumulative_delta / self.swipe_distance;
        let k = self.rubber_band;
        let p = if raw > 1.0 {
            1.0 + (raw - 1.0) * k
        } else if raw < 0.0 {
            raw * k
        } else {
            raw
        };
        self.progress.track(p, now);
    }

    /// Decide the target from position and velocity (projected end point) and spring to it; true
    /// when the overview opens. A release faster than the fast-release speed settles on the shorter spring.
    pub fn gesture_end(&mut self, now: f64) -> bool {
        let p = self.progress.value(now);
        let v = self.progress.velocity(now);
        let open = p + v * self.projection_s >= 0.5;
        let spring = self.release_spring(v);
        self.progress
            .release(if open { 1.0 } else { 0.0 }, spring, now);
        open
    }

    /// The spring a release at `velocity` (progress per second) settles on: the open/close one, or one with the
    /// same damping and its response scaled by the fast-release scale when the release is faster than the
    /// fast-release speed.
    pub fn release_spring(&self, velocity: f64) -> SpringParams {
        let spring = self.spring();
        if velocity.abs() <= self.fast_release_speed {
            return spring;
        }
        let zeta = spring.damping / (2.0 * (spring.stiffness * spring.mass).sqrt());
        SpringParams::from_ratio(spring.perceptual_duration() * self.fast_release_scale, zeta)
    }

    pub fn toggle(&mut self, open: bool, now: f64) {
        self.progress.animate_to(
            if open { 1.0 } else { 0.0 },
            Curve::Spring(self.spring()),
            now,
        );
    }

    /// The slow-motion modifier is held: transitions that start from now on use the slow duration.
    pub fn set_slow_motion(&mut self, on: bool) {
        self.slow_motion = on;
    }

    pub fn slow_motion(&self) -> bool {
        self.slow_motion
    }

    /// Open/close spring: the configured one, or the slow-motion one while the modifier is held.
    pub fn spring(&self) -> SpringParams {
        if self.slow_motion {
            self.slow_spring
        } else {
            self.spring
        }
    }

    pub fn is_open(&self) -> bool {
        self.progress.target() >= 0.5
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.progress.is_animating(now)
    }

    /// Progress at `now` (can leave [0, 1] while rubber-banding).
    pub fn progress_at(&self, now: f64) -> f64 {
        self.progress.value(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toggle_settles_in_the_verified_time_and_slow_motion_is_longer() {
        let settle = |slow: bool| {
            let mut t = Transition::new(&OverviewConfig::default());
            t.set_slow_motion(slow);
            t.toggle(true, 0.0);
            let mut now = 0.0;
            while t.is_animating(now) && now < 30.0 {
                now += 0.01;
            }
            assert!(t.is_open());
            now
        };
        let (fast, slow) = (settle(false), settle(true));
        assert!(fast < 1.0, "{fast}");
        assert!(slow > 4.0 * fast, "{slow} vs {fast}");
    }

    #[test]
    fn a_fast_flick_opens_and_a_slow_short_drag_cancels() {
        let mut t = Transition::new(&OverviewConfig::default());
        for i in 0..6 {
            t.gesture_update(i as f64 * 15.0, 0.0, i as f64 * 0.008);
        }
        assert!(t.gesture_end(0.048));
        let mut t = Transition::new(&OverviewConfig::default());
        for i in 0..30 {
            t.gesture_update(i as f64 * 2.0, 0.0, i as f64 * 0.016);
        }
        assert!(!t.gesture_end(0.48));
        assert!(!t.is_open());
    }

    #[test]
    fn a_fast_release_settles_on_a_spring_six_tenths_as_long() {
        let t = Transition::new(&OverviewConfig::default());
        let slow = t.release_spring(4.0);
        assert!((slow.perceptual_duration() - 0.25).abs() < 1e-9);
        let fast = t.release_spring(-6.0);
        assert!((fast.perceptual_duration() - 0.15).abs() < 1e-9);
        assert!(fast.bounce().abs() < 1e-9, "still critically damped");
    }

    #[test]
    fn progress_rubber_bands_beyond_the_ends() {
        let mut t = Transition::new(&OverviewConfig::default());
        t.gesture_update(600.0, 0.0, 0.0);
        assert!((t.progress_at(0.0) - 1.15).abs() < 1e-9);
        t.gesture_update(-300.0, 0.0, 0.1);
        assert!((t.progress_at(0.1) + 0.15).abs() < 1e-9);
    }
}
