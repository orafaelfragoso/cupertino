//! The workspace position of one output: finger-tracked while a swipe is in progress, otherwise
//! chased by the 60 Hz slide solver ([`cupertino_effects::space_slide`]).
//!
//! The solver works in `x` units (0 = first workspace, 1 = last); the position is kept in
//! workspace-index units, so the two are related by `span = max(n − 1, 1).

use cupertino_animation::AnimatedValue;
use cupertino_effects::space_slide::{SlideStart, SpaceSlideState, STEP_RATE};

/// A running solver slide, fixed at the moment it started.
#[derive(Clone, Copy, Debug)]
struct Slide {
    from: SpaceSlideState,
    start: f64,
    /// Snap target in `x` units.
    target: f64,
    /// Rest tolerance in `x` units.
    tolerance: f64,
    /// Workspace-index units per `x` unit.
    span: f64,
}

impl Slide {
    /// The state after `steps` whole solver steps, stopping early at rest.
    fn state_after(&self, steps: u32) -> SpaceSlideState {
        let mut state = self.from;
        for _ in 0..steps {
            if state.is_at_rest(self.target, self.tolerance) {
                break;
            }
            state = state.step(self.target);
        }
        state
    }

    /// Whole solver steps elapsed at `now` and the fraction of the step in progress.
    fn clock(&self, now: f64) -> (u32, f64) {
        let ticks = ((now - self.start) * STEP_RATE).max(0.0);
        (ticks.floor() as u32, ticks.fract())
    }

    fn is_at_rest(&self, now: f64) -> bool {
        self.state_after(self.clock(now).0)
            .is_at_rest(self.target, self.tolerance)
    }

    /// `x` at `now`, interpolated between solver steps so frames faster than 60 Hz stay smooth.
    fn x(&self, now: f64) -> f64 {
        let (steps, fraction) = self.clock(now);
        let state = self.state_after(steps);
        if state.is_at_rest(self.target, self.tolerance) {
            return self.target;
        }
        let next = state.step(self.target);
        state.x + (next.x - state.x) * fraction
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SlidePosition {
    /// The resting value and the finger-tracked value; unused for the value while `slide` runs.
    tracked: AnimatedValue<f64>,
    slide: Option<Slide>,
}

impl SlidePosition {
    pub(crate) fn new(index: f64) -> Self {
        Self {
            tracked: AnimatedValue::new(index),
            slide: None,
        }
    }

    pub(crate) fn value(&self, now: f64) -> f64 {
        self.slide
            .map_or_else(|| self.tracked.value(now), |s| s.x(now) * s.span)
    }

    /// Index units per second.
    pub(crate) fn velocity(&self, now: f64) -> f64 {
        match self.slide {
            Some(s) => s.state_after(s.clock(now).0).v * s.span,
            None => self.tracked.velocity(now),
        }
    }

    pub(crate) fn is_animating(&self, now: f64) -> bool {
        match self.slide {
            Some(s) => !s.is_at_rest(now),
            None => self.tracked.is_animating(now),
        }
    }

    pub(crate) fn set_immediately(&mut self, index: f64) {
        self.slide = None;
        self.tracked.set_immediately(index);
    }

    /// Follow the fingers: call on every input sample with the finger-mapped index.
    pub(crate) fn track(&mut self, index: f64, now: f64) {
        if let Some(s) = self.slide.take() {
            self.tracked.set_immediately(s.x(now) * s.span);
        }
        self.tracked.track(index, now);
    }

    /// Chase workspace `target` from the current position and velocity. `spaces` is the workspace
    /// count and `output_width` the output width in points (only the programmatic tolerance uses it).
    pub(crate) fn slide_to(
        &mut self,
        target: usize,
        spaces: usize,
        output_width: f64,
        start: SlideStart,
        now: f64,
    ) {
        let span = spaces.saturating_sub(1).max(1) as f64;
        let from = SpaceSlideState {
            x: self.value(now) / span,
            v: self.velocity(now) / span,
        };
        self.slide = Some(Slide {
            from,
            start: now,
            target: target as f64 / span,
            tolerance: start.tolerance(spaces, output_width),
            span,
        });
    }
}
