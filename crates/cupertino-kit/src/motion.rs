//! Motion by name. Every easing, spring and timed motion is an entry of the design catalogue ([`Easing`],
//! [`SpringPreset`], [`Timing`]); none is built from curve or spring numbers here, and a program that needs a new
//! motion adds it to the catalogue. A duration may go with a named easing ([`Easing::over`]); a spring never takes a
//! number.

use cupertino_animation::AnimatedValue;

pub use cupertino_animation::Animatable;
pub use cupertino_theme::animation::{Easing, SpringPreset, Timing};

/// A value that moves to a target and keeps its velocity when the target changes. Time is an absolute timestamp in
/// seconds, so the same motion is sampled alike at any frame rate.
#[derive(Clone, Debug)]
pub struct Animated<T: Animatable>(AnimatedValue<T>);

impl<T: Animatable> Animated<T> {
    /// A value resting at `value`.
    pub fn new(value: T) -> Self {
        Animated(AnimatedValue::new(value))
    }

    /// The value shown at `now`.
    pub fn value(&self, now: f64) -> T {
        self.0.value(now)
    }

    /// The velocity shown at `now`, in units per second.
    pub fn velocity(&self, now: f64) -> T {
        self.0.velocity(now)
    }

    /// The value it moves towards, or rests at.
    pub fn target(&self) -> T {
        self.0.target()
    }

    /// Whether it moves at `now`.
    pub fn is_animating(&self, now: f64) -> bool {
        self.0.is_animating(now)
    }

    /// Jump to `value` and rest there.
    pub fn set_immediately(&mut self, value: T) {
        self.0.set_immediately(value)
    }

    /// Stop at the value shown at `now`.
    pub fn cancel(&mut self, now: f64) {
        self.0.cancel(now)
    }

    /// Follow a gesture: called with each input sample's value, it measures the velocity a release starts with.
    pub fn track(&mut self, value: T, now: f64) {
        self.0.track(value, now)
    }

    /// Move to `to` along `timing` from `now`, keeping the value and the velocity shown then.
    pub fn animate_to(&mut self, to: T, timing: Timing, now: f64) {
        self.0.animate_to(to, timing.curve(), now)
    }

    /// End a gesture: settle to `to` on `spring`, starting with the velocity the gesture had.
    pub fn release(&mut self, to: T, spring: SpringPreset, now: f64) {
        self.0.release(to, spring.params(), now)
    }
}
