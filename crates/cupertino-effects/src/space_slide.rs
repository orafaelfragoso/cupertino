//! The workspace slide solver: a damped chase of the snap target stepped at a fixed 60 Hz, with a
//! rubber band outside the first and last workspace.
//!
//! The offset `x` is in workspace units: 0 is the first workspace, 1 the last. One gap between
//! neighbouring workspaces is `output_width + 64` pt, so one point is
//! `1 / ((n − 1)·(output_width + 64))` in `x`. The functions are pure; the caller owns the clock
//! and pushes `x` to the compositor after every [`SpaceSlideState::step`].

/// Space between neighbouring workspaces beyond the output width: 64 pt.
pub const SPACE_GAP: f64 = 64.0;
/// Solver rate: 60 steps per second.
pub const STEP_RATE: f64 = 60.0;
/// Pull of the target on the velocity per step: 2.
const PULL: f64 = 2.0;
/// Velocity kept from one step to the next: 0.695.
const VELOCITY_KEPT: f64 = 0.695;
/// Velocity kept while outside `[0, 1]: 0.85.
const RUBBER_BAND_KEPT: f64 = 0.85;
/// Pull back towards the nearest end while outside `[0, 1]: 0.8.
const RUBBER_BAND_PULL: f64 = 0.8;
/// The slide rests below this speed, in `x` units per second.
const REST_SPEED: f64 = 0.01;
/// Rest tolerance after a gesture release, in `x` units.
const RELEASE_TOLERANCE: f64 = 0.001;

/// What started the slide; it selects the rest tolerance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlideStart {
    /// A programmatic switch: rest within 1 pt of the target.
    Programmatic,
    /// The release of a swipe: rest within 0.001 of the target in `x` units.
    Release,
}

impl SlideStart {
    /// Rest tolerance in `x` units for `spaces` workspaces on an output `output_width` pt wide.
    pub fn tolerance(self, spaces: usize, output_width: f64) -> f64 {
        match self {
            SlideStart::Programmatic => point_in_x(spaces, output_width),
            SlideStart::Release => RELEASE_TOLERANCE,
        }
    }
}

/// One point in `x` units; `spaces − 1` counts as 1 for a single workspace.
pub fn point_in_x(spaces: usize, output_width: f64) -> f64 {
    1.0 / (spaces.saturating_sub(1).max(1) as f64 * (output_width + SPACE_GAP))
}

/// Offset and velocity of the slide, in `x` units and `x` units per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpaceSlideState {
    pub x: f64,
    pub v: f64,
}

impl SpaceSlideState {
    pub fn at_rest(x: f64) -> Self {
        Self { x, v: 0.0 }
    }

    /// The state one 60 Hz step later while chasing `target` (in `[0, 1]`).
    pub fn step(self, target: f64) -> Self {
        let mut v = PULL * (target - self.x) + VELOCITY_KEPT * self.v;
        let x = self.x + v / STEP_RATE;
        if x < 0.0 {
            v = RUBBER_BAND_KEPT * v + RUBBER_BAND_PULL * -x;
        } else if x > 1.0 {
            v = RUBBER_BAND_KEPT * v - RUBBER_BAND_PULL * (x - 1.0);
        }
        Self { x, v }
    }

    /// Whether the slide has come to rest on `target` within `tolerance` (in `x` units).
    pub fn is_at_rest(self, target: f64, tolerance: f64) -> bool {
        self.v.abs() < REST_SPEED && (self.x - target).abs() < tolerance
    }
}

/// Seconds the slide takes to rest on `target` (in `[0, 1]`) starting from `start`, counted in
/// whole steps.
pub fn settle_time(start: SpaceSlideState, target: f64, tolerance: f64) -> f64 {
    let mut state = start;
    let mut steps = 0_u32;
    while !state.is_at_rest(target, tolerance) {
        state = state.step(target);
        steps += 1;
    }
    f64::from(steps) / STEP_RATE
}
