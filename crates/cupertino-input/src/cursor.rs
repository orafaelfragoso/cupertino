//! Cursor size and shake to locate: the accessibility cursor scale and the pointer-shake detector
//! that grows the cursor while the pointer is shaken.
//!
//! the
//! user scale is clamped to 1…4 by the settings layer and the display server accepts any scale
//! below 16; the detector records reversals of the smoothed pointer velocity in a 10-slot table,
//! recognises a circular or a back-and-forth shake from them, and its multiplier
//! `clamp(0.5·n, 1, 5)` decays by `n ← trunc(0.8·n) once the shaking stops. The multiplier applies
//! on top of the user scale (the product is).
//!
//! Time is in seconds, positions in points (the global pointer space).

use crate::geom::Point;

/// Smallest cursor scale the settings offer.
pub const CURSOR_SCALE_MIN: f64 = 1.0;
/// Largest cursor scale the settings offer.
pub const CURSOR_SCALE_MAX: f64 = 4.0;
/// Exclusive upper bound of a cursor scale the display server accepts, and the cap of the shake
/// multiplier.
pub const CURSOR_SCALE_LIMIT: f64 = 16.0;

/// The user cursor scale as the settings layer stores and reads it: clamped to
/// [`CURSOR_SCALE_MIN`]…[`CURSOR_SCALE_MAX`]. Panics on NaN.
pub fn clamp_cursor_scale(scale: f64) -> f64 {
    assert!(!scale.is_nan(), "cursor scale is NaN");
    scale.clamp(CURSOR_SCALE_MIN, CURSOR_SCALE_MAX)
}

/// Reversals the detector collects before it tests for a shake.
const POINTS: usize = 10;
/// A motion this long after the last reversal starts the detector over, s.
const REVERSAL_GAP_S: f64 = 0.2;
/// Squared distance from the centroid a point must reach to count as outside, pt² (32 pt,
/// ).
const RADIUS_SQUARED: f64 = 1024.0;
/// Quadrant steps in one rotation direction a circular shake needs more than.
const MIN_ROTATION_STEPS: u32 = 3;
/// Scale applied to the slope variance before the line test.
const SLOPE_VARIANCE_SCALE: f64 = 0.125;
/// Largest scaled slope variance of a line shake.
const MAX_SLOPE_VARIANCE: f64 = 0.2;
/// Points a line shake needs outside the radius, more than.
const MIN_LINE_POINTS_OUTSIDE: u32 = 4;
/// Multiplier per counted reversal.
const SCALE_PER_REVERSAL: f64 = 0.5;
/// Largest shake multiplier.
pub const SHAKE_SCALE_MAX: f64 = 5.0;
/// Time after the last reversal from which each evaluation decays the count, s.
const DECAY_AFTER_S: f64 = 0.1;
/// Decay factor of the reversal count.
const DECAY: f64 = 0.8;
/// Interval between evaluations while the cursor is grown, s.
pub const SHAKE_EVALUATION_INTERVAL_S: f64 = 1.0 / 60.0;
/// Samples closer together than this are dropped by the velocity filter, s.
const MIN_SAMPLE_DT_S: f64 = 0.001;
/// Samples at most this far apart are blended with the previous velocity, s.
const BLEND_WINDOW_S: f64 = 0.2;
/// Weight of the new velocity in the blend.
const BLEND: f64 = 0.2;

/// The detector's velocity filter: per-axis velocity, blended with the previous one while both
/// have the same sign and the samples are close in time. The design's
/// per-axis turn accumulators are not modelled: nothing in the detector reads them.
#[derive(Clone, Debug, Default, PartialEq)]
struct VelocityFilter {
    /// Time and position of the last accepted sample.
    last: Option<(f64, Point)>,
    /// Smoothed velocity, pt/s per axis; `None` (the design's NaN) until two samples arrived.
    velocity: (Option<f64>, Option<f64>),
}

impl VelocityFilter {
    fn add(&mut self, p: Point, t: f64) {
        let Some((last_t, last_p)) = self.last else {
            self.last = Some((t, p));
            return;
        };
        let dt = t - last_t;
        if dt <= MIN_SAMPLE_DT_S {
            return;
        }
        let blend = |v: f64, prev: Option<f64>| match prev {
            Some(prev)
                if dt <= BLEND_WINDOW_S && v.is_sign_negative() == prev.is_sign_negative() =>
            {
                BLEND * v + (1.0 - BLEND) * prev
            }
            _ => v,
        };
        self.velocity = (
            Some(blend((p.x - last_p.x) / dt, self.velocity.0)),
            Some(blend((p.y - last_p.y) / dt, self.velocity.1)),
        );
        self.last = Some((t, p));
    }
}

/// Whether the smoothed velocity reversed on one axis: both values non-zero, opposite signs (a
/// missing velocity reads as 0, as the design's NaN does).
fn reversed(before: Option<f64>, after: Option<f64>) -> bool {
    match (before.unwrap_or(0.0), after.unwrap_or(0.0)) {
        (0.0, _) | (_, 0.0) => false,
        (b, a) => b.is_sign_negative() != a.is_sign_negative(),
    }
}

/// Quadrant of `(dx, dy)` around the centroid: 1 `(+, +)`, 2 `(−, +)`, 3 `(−, −)`, 4 `(+, −).
/// A component counts as negative only from −1 down: the design tests the sign of the
/// truncated integer.
fn quadrant(dx: f64, dy: f64) -> u32 {
    match (dx.trunc() < 0.0, dy.trunc() < 0.0) {
        (false, false) => 1,
        (true, false) => 2,
        (true, true) => 3,
        (false, true) => 4,
    }
}

/// The gesture test over the ten reversal points: a circular shake (every point outside the
/// radius and more than three quadrant steps in one direction) or a line shake (low variance of
/// the nine consecutive slopes and more than four points outside the radius).,.
fn is_shake(points: &[Point; POINTS]) -> bool {
    let slopes: Vec<f64> = points
        .windows(2)
        .map(|w| (w[1].y - w[0].y) / (w[1].x - w[0].x))
        .collect();
    let mean = slopes.iter().sum::<f64>() / slopes.len() as f64;
    let variance: f64 = slopes.iter().map(|s| (s - mean).powi(2)).sum();
    let n = POINTS as f64;
    let centroid = Point::new(
        points.iter().map(|p| p.x).sum::<f64>() / n,
        points.iter().map(|p| p.y).sum::<f64>() / n,
    );
    let (mut outside, mut down, mut up, mut previous) = (0, 1, 1, None);
    for p in points {
        let (dx, dy) = (p.x - centroid.x, p.y - centroid.y);
        if dx * dx + dy * dy >= RADIUS_SQUARED {
            outside += 1;
        }
        let q = quadrant(dx, dy);
        if let Some(prev) = previous {
            if q + 1 == prev || (prev == 1 && q == 4) {
                down += 1;
            } else if q == prev + 1 || (prev == 4 && q == 1) {
                up += 1;
            }
        }
        previous = Some(q);
    }
    let circle = outside == POINTS as u32 && (down > MIN_ROTATION_STEPS || up > MIN_ROTATION_STEPS);
    let line =
        variance * SLOPE_VARIANCE_SCALE <= MAX_SLOPE_VARIANCE && outside > MIN_LINE_POINTS_OUTSIDE;
    circle || line
}

/// Shake-to-locate detector of one pointer. Feed every pointer motion to [`motion`](Self::motion);
/// once it reports a shake, call [`evaluate`](Self::evaluate) every
/// [`SHAKE_EVALUATION_INTERVAL_S`] for as long as it returns more than 1.
#[derive(Clone, Debug, PartialEq)]
pub struct ShakeDetector {
    filter: VelocityFilter,
    /// Positions of the first ten reversals since the last reset.
    points: [Point; POINTS],
    /// Reversals counted since the last reset; it keeps counting past ten and decays after.
    reversals: u32,
    last_reversal: Option<f64>,
    /// A shake was recognised and the count has not been reset since.
    triggered: bool,
    multiplier: f64,
}

impl Default for ShakeDetector {
    fn default() -> Self {
        Self {
            filter: VelocityFilter::default(),
            points: [Point::default(); POINTS],
            reversals: 0,
            last_reversal: None,
            triggered: false,
            multiplier: 1.0,
        }
    }
}

impl ShakeDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the pointer position `p` at time `t`. True when this motion completed a shake: the
    /// multiplier has grown and the caller starts the evaluations.
    pub fn motion(&mut self, p: Point, t: f64) -> bool {
        let before = self.filter.velocity;
        self.filter.add(p, t);
        let after = self.filter.velocity;
        if self
            .last_reversal
            .is_some_and(|last| t > last + REVERSAL_GAP_S)
        {
            self.reset();
        } else if reversed(before.0, after.0) || reversed(before.1, after.1) {
            if let Some(slot) = self.points.get_mut(self.reversals as usize) {
                *slot = p;
            }
            self.reversals += 1;
            self.last_reversal = Some(t);
        }
        if self.triggered || self.reversals < POINTS as u32 {
            return false;
        }
        if is_shake(&self.points) {
            self.triggered = true;
            self.evaluate(t);
            true
        } else {
            self.reversals = 0;
            false
        }
    }

    /// Recompute the multiplier at time `now`: the count decays when the last reversal is more
    /// than 0.1 s old, and the multiplier is `clamp(0.5·n, 1, 5).
    pub fn evaluate(&mut self, now: f64) -> f64 {
        if self
            .last_reversal
            .is_some_and(|last| now > last + DECAY_AFTER_S)
        {
            self.reversals = (f64::from(self.reversals) * DECAY) as u32;
        }
        self.multiplier =
            (f64::from(self.reversals) * SCALE_PER_REVERSAL).clamp(1.0, SHAKE_SCALE_MAX);
        self.multiplier
    }

    /// Return to the normal size at once and forget the shake, for a caller that cannot run the
    /// evaluations.
    pub fn release(&mut self) {
        self.reset();
        self.multiplier = 1.0;
    }

    /// The current multiplier, 1 when the cursor has its normal size.
    pub fn multiplier(&self) -> f64 {
        self.multiplier
    }

    /// Forget the reversals (the design also clears the ten points); the multiplier keeps its
    /// value until the next [`evaluate`](Self::evaluate).
    fn reset(&mut self) {
        self.points = [Point::default(); POINTS];
        self.reversals = 0;
        self.last_reversal = None;
        self.triggered = false;
    }
}
