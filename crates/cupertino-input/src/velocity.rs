//! Velocity estimation over a short sliding window of samples.
//!
//! approximate: the window length (0.1 s) is a design choice; the design's own estimator was
//! not analysed.

use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct VelocityTracker {
    window: f64,
    samples: VecDeque<(f64, f64)>,
}

impl Default for VelocityTracker {
    fn default() -> Self {
        Self::new(0.1)
    }
}

impl VelocityTracker {
    pub fn new(window: f64) -> Self {
        Self { window, samples: VecDeque::new() }
    }

    pub fn reset(&mut self) {
        self.samples.clear();
    }

    /// Record the absolute position `value` at time `t` (seconds).
    pub fn push(&mut self, t: f64, value: f64) {
        if self.samples.back().is_some_and(|s| t < s.0) {
            self.samples.clear();
        }
        self.samples.push_back((t, value));
        while self.samples.front().is_some_and(|s| t - s.0 > self.window) {
            self.samples.pop_front();
        }
    }

    /// Least-squares slope over the window (units per second); zero with fewer than two samples
    /// or when the newest sample is older than the window relative to `now`.
    pub fn velocity(&self, now: f64) -> f64 {
        let n = self.samples.len();
        if n < 2 || self.samples.back().is_none_or(|s| now - s.0 > self.window) {
            return 0.0;
        }
        let t0 = self.samples[0].0;
        let (mut st, mut sv, mut stt, mut stv) = (0.0, 0.0, 0.0, 0.0);
        for &(t, v) in &self.samples {
            let x = t - t0;
            st += x;
            sv += v;
            stt += x * x;
            stv += x * v;
        }
        let n = n as f64;
        let denom = n * stt - st * st;
        if denom.abs() < 1e-12 {
            return 0.0;
        }
        (n * stv - st * sv) / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_speed_is_recovered() {
        let mut v = VelocityTracker::default();
        for i in 0..10 {
            v.push(i as f64 * 0.01, i as f64 * 0.01 * 500.0);
        }
        assert!((v.velocity(0.09) - 500.0).abs() < 1e-6);
    }

    #[test]
    fn old_samples_fall_out_of_the_window() {
        let mut v = VelocityTracker::default();
        for i in 0..10 {
            v.push(i as f64 * 0.01, i as f64 * 10.0);
        }
        for i in 10..30 {
            v.push(i as f64 * 0.01, 90.0);
        }
        assert!(v.velocity(0.29).abs() < 1e-6);
    }

    #[test]
    fn stale_or_single_sample_is_zero() {
        let mut v = VelocityTracker::default();
        assert_eq!(v.velocity(0.0), 0.0);
        v.push(0.0, 0.0);
        assert_eq!(v.velocity(0.0), 0.0);
        v.push(0.01, 5.0);
        assert!(v.velocity(0.02) > 0.0);
        assert_eq!(v.velocity(1.0), 0.0, "stale");
    }

    #[test]
    fn time_going_backwards_resets() {
        let mut v = VelocityTracker::default();
        v.push(1.0, 0.0);
        v.push(1.01, 10.0);
        v.push(0.5, 0.0);
        assert_eq!(v.velocity(0.5), 0.0);
    }

    #[test]
    fn reset_clears() {
        let mut v = VelocityTracker::default();
        v.push(0.0, 0.0);
        v.push(0.01, 1.0);
        v.reset();
        assert_eq!(v.velocity(0.01), 0.0);
    }
}
