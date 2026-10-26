//! Interruptible, retargetable property animations driven by absolute presentation timestamps.
//!
//! This layer is an independent design (approximate behaviour): the layer framework's
//! additive / retargeting semantics for interrupted animations were not analysed. Design goals:
//! * time is always an absolute timestamp (seconds, e.g. from the DRM page-flip clock), so the
//!   same curve is sampled identically at 60/120/144 Hz or with dropped frames;
//! * retargeting preserves position and velocity (C¹ continuity), which is the behaviour users
//!   perceive on the design when a window animation is interrupted;
//! * gesture-driven progress maps finger movement 1:1 and hands off to a spring using the release
//!   velocity.

use crate::spring::{Spring, SpringParams};
use crate::timing::CubicBezier;

pub trait Animatable: Copy {
    fn lerp(a: Self, b: Self, t: f64) -> Self;
    fn sub(a: Self, b: Self) -> Self;
    fn scale(a: Self, s: f64) -> Self;
    fn norm(a: Self) -> f64;
}

impl Animatable for f64 {
    fn lerp(a: Self, b: Self, t: f64) -> Self { a + (b - a) * t }
    fn sub(a: Self, b: Self) -> Self { a - b }
    fn scale(a: Self, s: f64) -> Self { a * s }
    fn norm(a: Self) -> f64 { a.abs() }
}

impl<const N: usize> Animatable for [f64; N] {
    fn lerp(a: Self, b: Self, t: f64) -> Self { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) }
    fn sub(a: Self, b: Self) -> Self { std::array::from_fn(|i| a[i] - b[i]) }
    fn scale(a: Self, s: f64) -> Self { std::array::from_fn(|i| a[i] * s) }
    fn norm(a: Self) -> f64 { a.iter().map(|x| x * x).sum::<f64>().sqrt() }
}

#[derive(Clone, Copy, Debug)]
pub enum Curve {
    Timed { duration: f64, bezier: CubicBezier },
    Spring(SpringParams),
}

#[derive(Clone, Copy, Debug)]
enum Active<T> {
    Timed { from: T, to: T, start: f64, duration: f64, bezier: CubicBezier },
    /// Each component may have its own initial velocity in absolute units; we keep the per-axis
    /// velocity as a vector and solve one normalised spring per call (all axes share parameters).
    Spring { from: T, to: T, start: f64, spring: Spring, extra_vel: T },
    Gesture { value: T, last_t: f64, velocity: T },
}

#[derive(Clone, Debug)]
pub struct AnimatedValue<T: Animatable> {
    state: Option<Active<T>>,
    resting: T,
}

impl<T: Animatable> AnimatedValue<T> {
    pub fn new(v: T) -> Self { Self { state: None, resting: v } }

    pub fn is_animating(&self, now: f64) -> bool {
        match self.state {
            None => false,
            Some(Active::Timed { start, duration, .. }) => now < start + duration,
            Some(Active::Spring { start, spring, .. }) => now < start + spring.settling_duration(),
            Some(Active::Gesture { .. }) => true,
        }
    }

    pub fn value(&self, now: f64) -> T {
        match self.state {
            None => self.resting,
            Some(Active::Timed { from, to, start, duration, bezier }) => {
                let x = if duration <= 0.0 { 1.0 } else { ((now - start) / duration).clamp(0.0, 1.0) };
                T::lerp(from, to, bezier.eval(x))
            }
            Some(Active::Spring { from, to, start, spring, extra_vel }) => {
                let t = (now - start).max(0.0);
                let base = T::lerp(from, to, spring.value(t));
                // velocity component orthogonal to (to-from): decays with a zero-target spring
                let zero = Spring::new(SpringParams { initial_velocity: 0.0, ..spring.params });
                let decay = Self::impulse_response(&zero, t);
                let add = T::scale(extra_vel, decay);
                base.add_to(add)
            }
            Some(Active::Gesture { value, .. }) => value,
        }
    }

    /// Unit-impulse response of the spring (displacement per unit initial velocity).
    fn impulse_response(s: &Spring, t: f64) -> f64 {
        // x(t) for x(0)=0, x'(0)=1 of the same oscillator == p(t) of a spring with v0=1
        // minus p(t) with v0=0 (linearity).
        let a = Spring::new(SpringParams { initial_velocity: 1.0, ..s.params });
        a.value(t) - s.value(t)
    }

    pub fn velocity(&self, now: f64) -> T {
        if let Some(Active::Gesture { velocity, .. }) = self.state {
            return velocity;
        }
        // forward difference: animations are clamped before their start time
        let h = 1e-6;
        T::scale(T::sub(self.value(now + h), self.value(now)), 1.0 / h)
    }

    pub fn target(&self) -> T {
        match self.state {
            None => self.resting,
            Some(Active::Timed { to, .. }) | Some(Active::Spring { to, .. }) => to,
            Some(Active::Gesture { value, .. }) => value,
        }
    }

    /// Start (or retarget) towards `to`. Position and velocity at `now` are preserved.
    pub fn animate_to(&mut self, to: T, curve: Curve, now: f64) {
        let from = self.value(now);
        let vel = match self.state { Some(Active::Gesture { velocity, .. }) => velocity, _ => self.velocity(now) };
        self.state = Some(match curve {
            Curve::Timed { duration, bezier } => Active::Timed { from, to, start: now, duration, bezier },
            Curve::Spring(mut p) => {
                let dist = T::sub(to, from);
                let d = T::norm(dist);
                // project velocity onto the travel direction → fractional initial velocity (fractions of the distance per second)
                let (along, rest) = if d > 1e-9 {
                    let dir = T::scale(dist, 1.0 / d);
                    let proj = dot(vel, dir);
                    (proj / d, T::sub(vel, T::scale(dir, proj)))
                } else {
                    (0.0, vel)
                };
                p.initial_velocity = along;
                Active::Spring { from, to, start: now, spring: Spring::new(p), extra_vel: rest }
            }
        });
        self.resting = to;
    }

    pub fn set_immediately(&mut self, v: T) { self.state = None; self.resting = v; }

    /// Cancel and freeze at the current presentation value.
    pub fn cancel(&mut self, now: f64) { let v = self.value(now); self.set_immediately(v); }

    /// Gesture tracking: call on every input sample with the finger-mapped value.
    pub fn track(&mut self, v: T, now: f64) {
        let (vel, _) = match self.state {
            Some(Active::Gesture { value, last_t, velocity }) if now > last_t => {
                let inst = T::scale(T::sub(v, value), 1.0 / (now - last_t));
                // simple exponential smoothing of input velocity (approximate)
                (T::lerp(velocity, inst, 0.6), ())
            }
            Some(Active::Gesture { velocity, .. }) => (velocity, ()),
            _ => (T::scale(v, 0.0), ()),
        };
        self.state = Some(Active::Gesture { value: v, last_t: now, velocity: vel });
        self.resting = v;
    }

    /// Release a gesture: settle to `to` with a spring seeded by the measured velocity.
    pub fn release(&mut self, to: T, spring: SpringParams, now: f64) { self.animate_to(to, Curve::Spring(spring), now); }
}

trait AddTo { fn add_to(self, o: Self) -> Self; }
impl<T: Animatable> AddTo for T {
    fn add_to(self, o: Self) -> Self { T::sub(self, T::scale(o, -1.0)) }
}

fn dot<T: Animatable>(a: T, b: T) -> f64 {
    // (|a+b|² − |a−b|²)/4
    let s = T::norm(T::sub(a, T::scale(b, -1.0)));
    let d = T::norm(T::sub(a, b));
    (s * s - d * d) / 4.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retarget_preserves_c1() {
        let mut a = AnimatedValue::new(0.0f64);
        a.animate_to(100.0, Curve::Spring(SpringParams::default()), 0.0);
        let t = 0.12;
        let (p, v) = (a.value(t), a.velocity(t));
        a.animate_to(-50.0, Curve::Spring(SpringParams::from_perceptual(0.4, 0.1)), t);
        assert!((a.value(t) - p).abs() < 1e-9);
        assert!((a.velocity(t) - v).abs() / v.abs() < 1e-3, "{} vs {}", a.velocity(t), v);
        assert!((a.value(t + 10.0) + 50.0).abs() < 1e-3);
    }

    #[test]
    fn vector_retarget_keeps_orthogonal_velocity() {
        let mut a = AnimatedValue::new([0.0, 0.0]);
        a.animate_to([100.0, 0.0], Curve::Spring(SpringParams::default()), 0.0);
        let t = 0.1;
        let v = a.velocity(t);
        a.animate_to([100.0, 100.0], Curve::Spring(SpringParams::default()), t);
        let v2 = a.velocity(t);
        assert!((v[0] - v2[0]).abs() < 0.05 * v[0].abs() + 1e-6);
        let end = a.value(t + 10.0);
        assert!((end[0] - 100.0).abs() < 1e-3 && (end[1] - 100.0).abs() < 1e-3);
    }

    #[test]
    fn refresh_rate_independent() {
        let mut a = AnimatedValue::new(0.0f64);
        a.animate_to(1.0, Curve::Timed { duration: 0.25, bezier: CubicBezier::DEFAULT }, 1.0);
        // sampling at 60 Hz and 120 Hz hits identical values at shared timestamps
        for k in 0..15 {
            let t60 = 1.0 + k as f64 / 60.0;
            let t120 = 1.0 + (2 * k) as f64 / 120.0;
            assert_eq!(a.value(t60), a.value(t120));
        }
    }

    #[test]
    fn gesture_release_uses_velocity() {
        let mut a = AnimatedValue::new(0.0f64);
        for i in 0..10 {
            a.track(i as f64 * 10.0, i as f64 * 0.008); // 1250 pt/s
        }
        a.release(200.0, SpringParams::default(), 0.072);
        assert!(a.velocity(0.0721) > 500.0);
    }
}
