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
            Some(prev) if dt <= BLEND_WINDOW_S && v.is_sign_negative() == prev.is_sign_negative() => BLEND * v + (1.0 - BLEND) * prev,
            _ => v,
        };
        self.velocity = (Some(blend((p.x - last_p.x) / dt, self.velocity.0)), Some(blend((p.y - last_p.y) / dt, self.velocity.1)));
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
    let slopes: Vec<f64> = points.windows(2).map(|w| (w[1].y - w[0].y) / (w[1].x - w[0].x)).collect();
    let mean = slopes.iter().sum::<f64>() / slopes.len() as f64;
    let variance: f64 = slopes.iter().map(|s| (s - mean).powi(2)).sum();
    let n = POINTS as f64;
    let centroid = Point::new(points.iter().map(|p| p.x).sum::<f64>() / n, points.iter().map(|p| p.y).sum::<f64>() / n);
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
    let line = variance * SLOPE_VARIANCE_SCALE <= MAX_SLOPE_VARIANCE && outside > MIN_LINE_POINTS_OUTSIDE;
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
        Self { filter: VelocityFilter::default(), points: [Point::default(); POINTS], reversals: 0, last_reversal: None, triggered: false, multiplier: 1.0 }
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
        if self.last_reversal.is_some_and(|last| t > last + REVERSAL_GAP_S) {
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
        if self.last_reversal.is_some_and(|last| now > last + DECAY_AFTER_S) {
            self.reversals = (f64::from(self.reversals) * DECAY) as u32;
        }
        self.multiplier = (f64::from(self.reversals) * SCALE_PER_REVERSAL).clamp(1.0, SHAKE_SCALE_MAX);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Shake horizontally between `x0` and `x1` at height `y` in `step` pt steps every 10 ms,
    /// starting at `t0`, for `strokes` one-way strokes. Returns the time after the last motion and
    /// whether any motion reported a shake.
    fn shake_line(d: &mut ShakeDetector, x0: f64, x1: f64, y: f64, step: f64, strokes: u32, t0: f64) -> (f64, bool) {
        let (mut t, mut x, mut dir, mut any) = (t0, x0, 1.0, false);
        for _ in 0..strokes {
            let target = if dir > 0.0 { x1 } else { x0 };
            while (target - x) * dir > 0.0 {
                x += step * dir;
                t += 0.01;
                any |= d.motion(Point::new(x, y), t);
            }
            dir = -dir;
        }
        (t, any)
    }

    #[test]
    fn a_horizontal_shake_grows_the_cursor_five_times() {
        let mut d = ShakeDetector::new();
        let (t, shook) = shake_line(&mut d, 100.0, 400.0, 300.0, 50.0, 12, 1.0);
        assert!(shook, "ten reversals of a line shake trigger");
        assert_eq!(d.multiplier(), SHAKE_SCALE_MAX);
        // still shaking: the count stays high and the multiplier at its maximum
        assert_eq!(d.evaluate(t + 0.05), SHAKE_SCALE_MAX);
    }

    #[test]
    fn the_multiplier_decays_frame_by_frame_once_the_shaking_stops() {
        let mut d = ShakeDetector::new();
        let (t, shook) = shake_line(&mut d, 100.0, 400.0, 300.0, 50.0, 11, 1.0);
        assert!(shook);
        let n = d.reversals;
        assert_eq!(n, 10, "the shake triggered on its tenth reversal");
        // the reversal was the last event that turned; evaluations within 0.1 s keep the count
        let last = d.last_reversal.unwrap();
        assert_eq!(d.evaluate(last + 0.09), 5.0);
        // past 0.1 s each evaluation decays: 10, 8, 6, 4, 3, 2, 1, 0 → 5, 4, 3, 2, 1.5, 1, 1, 1
        let mut scales = Vec::new();
        let mut now = last.max(t) + 0.1;
        for _ in 0..8 {
            now += SHAKE_EVALUATION_INTERVAL_S;
            scales.push(d.evaluate(now));
        }
        assert_eq!(scales, [4.0, 3.0, 2.0, 1.5, 1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn release_returns_to_the_normal_size_and_a_new_shake_triggers_again() {
        let mut d = ShakeDetector::new();
        assert!(shake_line(&mut d, 100.0, 400.0, 300.0, 50.0, 11, 1.0).1);
        d.release();
        assert_eq!(d.multiplier(), 1.0);
        assert!(shake_line(&mut d, 100.0, 400.0, 300.0, 50.0, 11, 2.0).1, "the next shake is recognised");
    }

    #[test]
    fn slow_back_and_forth_motion_is_not_a_shake() {
        // more than 0.2 s between reversals: every turn starts the detector over
        let mut d = ShakeDetector::new();
        let (mut t, mut x) = (1.0, 100.0);
        for i in 0..400 {
            let dir = if (i / 30) % 2 == 0 { 1.0 } else { -1.0 };
            x += 10.0 * dir;
            t += 0.01;
            assert!(!d.motion(Point::new(x, 300.0), t), "turned at {t}");
        }
        assert_eq!(d.multiplier(), 1.0);
    }

    #[test]
    fn a_small_jitter_is_not_a_shake() {
        // reversals inside the 32 pt radius pass neither test
        let mut d = ShakeDetector::new();
        let (_, shook) = shake_line(&mut d, 300.0, 320.0, 300.0, 5.0, 30, 1.0);
        assert!(!shook);
        assert_eq!(d.multiplier(), 1.0);
    }

    #[test]
    fn a_circle_through_all_four_quadrants_is_a_shake() {
        // ten reversal points on a circle of radius 100 around (500, 500), visited in order
        let points: [Point; POINTS] = std::array::from_fn(|i| {
            let a = i as f64 * std::f64::consts::TAU / 8.0 + 0.3;
            Point::new(500.0 + 100.0 * a.cos(), 500.0 + 100.0 * a.sin())
        });
        assert!(is_shake(&points));
        let mut inside = points;
        inside[0] = Point::new(500.0, 500.0);
        // one point at the centre: not a circle; the slopes vary too much for a line
        assert!(!is_shake(&inside));
    }

    #[test]
    fn quadrants_follow_the_truncated_sign() {
        assert_eq!([quadrant(5.0, 5.0), quadrant(-5.0, 5.0), quadrant(-5.0, -5.0), quadrant(5.0, -5.0)], [1, 2, 3, 4]);
        assert_eq!(quadrant(-0.5, -0.9), 1, "values above -1 truncate to 0");
    }

    #[test]
    fn the_filter_blends_same_sign_velocities_and_drops_close_samples() {
        let mut f = VelocityFilter::default();
        f.add(Point::new(0.0, 0.0), 1.0);
        assert_eq!(f.velocity, (None, None), "one sample has no velocity");
        f.add(Point::new(10.0, 0.0), 1.01);
        let v = f.velocity.0.unwrap();
        assert!((v - 1000.0).abs() < 1e-9, "{v}");
        f.add(Point::new(20.0, 0.0), 1.0105);
        assert!((f.velocity.0.unwrap() - 1000.0).abs() < 1e-9, "a sample 0.5 ms later is dropped");
        f.add(Point::new(20.0, 0.0), 1.03);
        // 10 pt over the 20 ms since the accepted sample: 0.2 · 500 + 0.8 · 1000
        assert!((f.velocity.0.unwrap() - 900.0).abs() < 1e-9, "{:?}", f.velocity);
        f.add(Point::new(10.0, 0.0), 1.04);
        assert!((f.velocity.0.unwrap() + 1000.0).abs() < 1e-9, "a sign change takes the new velocity");
    }

    #[test]
    fn the_user_scale_is_clamped_to_the_settings_range() {
        assert_eq!([clamp_cursor_scale(0.5), clamp_cursor_scale(2.5), clamp_cursor_scale(9.0)], [1.0, 2.5, 4.0]);
    }
}
