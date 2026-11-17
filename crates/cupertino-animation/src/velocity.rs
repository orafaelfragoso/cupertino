//! Velocity estimation over a short sliding window of samples.
//!
//! The tracker keeps the samples of the last 0.1 s by default and reports the least-squares slope
//! of position over time across them. A sample earlier than the newest one clears the window.

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
        Self {
            window,
            samples: VecDeque::new(),
        }
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
