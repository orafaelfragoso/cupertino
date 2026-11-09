//! The motion of a scroll view's content: a drag, the kinetic motion a flick leaves, the stretch past an edge and the
//! return to it. A pure model of the time it is given: every query takes the time and answers from the state alone,
//! so a trace of inputs and times always gives the same motion.
//!
//! Positions are offsets from the content's start along each axis, in points; `0..=max` is the scrollable range.
//! [`ScrollMotion::offset`] is the logical position (inside the range), [`ScrollMotion::stretch`] how far the content
//! is drawn past an edge and [`ScrollMotion::velocity`] its speed: three separate things.
//!
//! One owner of kinetic motion per input path. A finger on a touchpad or a screen drags ([`ScrollMotion::drag`]) and
//! its lift ([`ScrollMotion::release`]) may leave kinetic motion here. A wheel, the keyboard and continuous sources
//! step ([`ScrollMotion::step`]): their movement is complete as it arrives, so it never gets inertia of its own.
//!
//! - **Drag.** The content follows the finger one for one inside the range. Past an edge the finger's travel `x`
//!   beyond it shows as a stretch of `d·(1 − 1/(1 + r·x/d))`, with `d` the viewport's length along the axis and `r`
//!   [`EDGE_RESISTANCE`]: the content gives a little at first and never more than the viewport's length. Moving back
//!   unwinds the stretch along the same curve; the movement that reaches the edge stops there, and the next one moves
//!   the content.
//! - **Release.** The finger's speed over its last [`VELOCITY_WINDOW`] (none when it rested for [`RELEASE_PAUSE`]
//!   before lifting) launches kinetic motion that slows exponentially with [`MOMENTUM_TIME_CONSTANT`]; under
//!   [`VELOCITY_WINDOW`] nothing coasts. Kinetic motion that reaches an edge carries [`BOUNCE_CARRY`] of its speed past
//!   it and returns.
//! - **Return.** A stretch returns to its edge as `(x₀ + c·v·t)·e^(−t/τ)`, with `τ` [`RETURN_TIME_CONSTANT`] and `c·v`
//!   the speed carried past the edge (none for a release at rest): a stretch of 17 points is within half a point of
//!   the edge about 0.28 s after release. The return happens with Reduce Motion as well; only the kinetic motion stops.
//! - **New input.** A new drag catches the content where it is drawn: kinetic motion stops, and a returning stretch
//!   is held and dragged on from where it is. A step stops the motion of the axes it moves.
//! - **Axes.** With [`AxisPolicy::Predominant`] a drag moves only the axis its first movement moved most, until it
//!   ends; [`AxisPolicy::Free`] moves both. An axis whose content fits its viewport does not move.

use cupertino_geometry::{Point, Size};

/// How much of a pull past an edge shows as a stretch at first: the stretch's slope at the edge.
pub const EDGE_RESISTANCE: f64 = 0.07;
/// Time constant of a stretch's return to its edge, in seconds.
pub const RETURN_TIME_CONSTANT: f64 = 0.08;
/// Share of the speed at an edge that carries a stretch on past it before it returns.
pub const BOUNCE_CARRY: f64 = 0.31;
/// Time constant of kinetic motion: its speed falls to 1/e in this time, in seconds (a speed loss of 0.2 % a
/// millisecond).
pub const MOMENTUM_TIME_CONSTANT: f64 = 0.4995;
/// A release slower than this, in points per second, leaves no kinetic motion.
pub const MIN_FLING_SPEED: f64 = 50.0;
/// The finger's movement over this long before its last movement gives its speed at release, in seconds.
pub const VELOCITY_WINDOW: f64 = 0.1;
/// A finger that rested this long before lifting leaves no kinetic motion, in seconds.
pub const RELEASE_PAUSE: f64 = 0.05;
/// Motion this close to where it comes to rest has settled, in points.
pub const SETTLED: f64 = 0.25;

/// Which axes one drag moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AxisPolicy {
    /// The axis the drag's first movement moved most, until the drag ends: a mostly vertical document does not
    /// drift sideways.
    #[default]
    Predominant,
    /// Both axes, as on a canvas.
    Free,
}

/// What an axis is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Rest,
    Dragging,
    /// Kinetic motion after a release, inside the range.
    Coasting,
    /// A stretch going back to its edge.
    Returning,
}

/// The stretch `d·(1 − 1/(1 + r·x/d))` for a pull of `x ≥ 0` past an edge.
fn stretch_of(pull: f64, length: f64) -> f64 {
    if length <= 0.0 {
        return 0.0;
    }
    length * (1.0 - 1.0 / (1.0 + EDGE_RESISTANCE * pull / length))
}

/// The pull that shows as `stretch`; the inverse of [`stretch_of`].
fn pull_of(stretch: f64, length: f64) -> f64 {
    if length <= 0.0 || stretch <= 0.0 {
        return 0.0;
    }
    let y = (stretch / length).min(1.0 - 1e-9);
    length * (1.0 / (1.0 - y) - 1.0) / EDGE_RESISTANCE
}

/// The stretch's slope for a pull of `x`: how fast it grows as the finger moves.
fn stretch_slope(pull: f64, length: f64) -> f64 {
    if length <= 0.0 {
        return 0.0;
    }
    let q = 1.0 + EDGE_RESISTANCE * pull / length;
    EDGE_RESISTANCE / (q * q)
}

/// A return from `x0` points past an edge, moving outward at `speed` (points per second) as it starts.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Return {
    x0: f64,
    /// The carried speed `c·v`, outward.
    carried: f64,
}

impl Return {
    fn new(x0: f64, outward_speed: f64) -> Return {
        Return {
            x0,
            carried: BOUNCE_CARRY * outward_speed.max(0.0),
        }
    }

    fn distance(&self, t: f64) -> f64 {
        (self.x0 + self.carried * t) * (-t / RETURN_TIME_CONSTANT).exp()
    }

    /// Outward speed.
    fn speed(&self, t: f64) -> f64 {
        (self.carried - (self.x0 + self.carried * t) / RETURN_TIME_CONSTANT)
            * (-t / RETURN_TIME_CONSTANT).exp()
    }

    /// When the stretch is farthest out; it only shrinks from then on.
    fn peak(&self) -> f64 {
        if self.carried <= 0.0 {
            0.0
        } else {
            (RETURN_TIME_CONSTANT - self.x0 / self.carried).max(0.0)
        }
    }

    fn settled(&self, t: f64) -> bool {
        t >= self.peak() && self.distance(t) < SETTLED
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Motion {
    Rest(f64),
    Drag {
        /// Where the finger has taken the content, past an edge included: the pull beyond it.
        raw: f64,
        /// `(time, raw)` of the drag's recent movements, oldest first.
        samples: Vec<(f64, f64)>,
    },
    Coast {
        t0: f64,
        from: f64,
        v0: f64,
        /// When the motion reaches `edge` and with what speed, if it does.
        crossing: Option<(f64, f64, f64)>,
    },
    Spring {
        t0: f64,
        edge: f64,
        /// +1 past the end, −1 before the start.
        sign: f64,
        motion: Return,
    },
}

/// One axis of [`ScrollMotion`].
#[derive(Clone, Debug, PartialEq)]
struct AxisMotion {
    max: f64,
    /// The viewport's length along the axis, which bounds the stretch.
    length: f64,
    motion: Motion,
}

impl AxisMotion {
    fn new(at: f64, max: f64, length: f64) -> AxisMotion {
        let max = max.max(0.0);
        AxisMotion {
            max,
            length,
            motion: Motion::Rest(at.clamp(0.0, max)),
        }
    }

    fn movable(&self) -> bool {
        self.max > 0.0
    }

    /// The raw drag position shown: inside the range as it is, past an edge as a stretch.
    fn shown_raw(&self, raw: f64) -> f64 {
        if raw < 0.0 {
            -stretch_of(-raw, self.length)
        } else if raw > self.max {
            self.max + stretch_of(raw - self.max, self.length)
        } else {
            raw
        }
    }

    /// The raw drag position that shows as `shown`.
    fn raw_of(&self, shown: f64) -> f64 {
        if shown < 0.0 {
            -pull_of(-shown, self.length)
        } else if shown > self.max {
            self.max + pull_of(shown - self.max, self.length)
        } else {
            shown
        }
    }

    fn shown(&self, now: f64) -> f64 {
        match &self.motion {
            Motion::Rest(at) => *at,
            Motion::Drag { raw, .. } => self.shown_raw(*raw),
            Motion::Coast {
                t0,
                from,
                v0,
                crossing,
            } => {
                let t = (now - t0).max(0.0);
                match *crossing {
                    Some((tc, edge, vc)) if t >= tc => {
                        let sign = vc.signum();
                        let r = Return::new(0.0, vc.abs());
                        let tt = t - tc;
                        if r.settled(tt) {
                            edge
                        } else {
                            edge + sign * r.distance(tt)
                        }
                    }
                    _ if coast_settled(*v0, t) => from + v0 * MOMENTUM_TIME_CONSTANT,
                    _ => {
                        from + v0
                            * MOMENTUM_TIME_CONSTANT
                            * (1.0 - (-t / MOMENTUM_TIME_CONSTANT).exp())
                    }
                }
            }
            Motion::Spring {
                t0,
                edge,
                sign,
                motion,
            } => {
                let t = (now - t0).max(0.0);
                if motion.settled(t) {
                    *edge
                } else {
                    edge + sign * motion.distance(t)
                }
            }
        }
    }

    fn velocity(&self, now: f64) -> f64 {
        match &self.motion {
            Motion::Rest(_) => 0.0,
            Motion::Drag { samples, .. } => drag_velocity(samples, now),
            Motion::Coast {
                t0, v0, crossing, ..
            } => {
                let t = (now - t0).max(0.0);
                match *crossing {
                    Some((tc, _, vc)) if t >= tc => {
                        let r = Return::new(0.0, vc.abs());
                        if r.settled(t - tc) {
                            0.0
                        } else {
                            vc.signum() * r.speed(t - tc)
                        }
                    }
                    _ if coast_settled(*v0, t) => 0.0,
                    _ => v0 * (-t / MOMENTUM_TIME_CONSTANT).exp(),
                }
            }
            Motion::Spring {
                t0, sign, motion, ..
            } => {
                let t = (now - t0).max(0.0);
                if motion.settled(t) {
                    0.0
                } else {
                    sign * motion.speed(t)
                }
            }
        }
    }

    fn phase(&self, now: f64) -> Phase {
        match &self.motion {
            Motion::Rest(_) => Phase::Rest,
            Motion::Drag { .. } => Phase::Dragging,
            Motion::Coast {
                t0, v0, crossing, ..
            } => {
                let t = (now - t0).max(0.0);
                match *crossing {
                    Some((tc, _, vc)) if t >= tc => {
                        if Return::new(0.0, vc.abs()).settled(t - tc) {
                            Phase::Rest
                        } else {
                            Phase::Returning
                        }
                    }
                    _ if coast_settled(*v0, t) => Phase::Rest,
                    _ => Phase::Coasting,
                }
            }
            Motion::Spring { t0, motion, .. } => {
                if motion.settled((now - t0).max(0.0)) {
                    Phase::Rest
                } else {
                    Phase::Returning
                }
            }
        }
    }

    /// A drag starts: the content is caught where it is drawn.
    fn begin(&mut self, now: f64) {
        if matches!(self.motion, Motion::Drag { .. }) {
            return;
        }
        let shown = self.shown(now);
        let raw = self.raw_of(shown);
        self.motion = Motion::Drag {
            raw,
            samples: vec![(now, raw)],
        };
    }

    fn drag(&mut self, delta: f64, now: f64) {
        self.begin(now);
        let max = self.max;
        let Motion::Drag { raw, samples } = &mut self.motion else {
            unreachable!("begin starts a drag");
        };
        let mut next = *raw + delta;
        // moving back from past an edge stops at the edge
        if *raw < 0.0 && next > 0.0 {
            next = 0.0;
        } else if *raw > max && next < max {
            next = max;
        }
        *raw = next;
        // one sample per time: the catch and a movement at the same time are one position
        if samples.last().is_some_and(|(t, _)| *t == now) {
            samples.pop();
        }
        samples.push((now, next));
        samples.retain(|(t, _)| now - t <= 2.0 * VELOCITY_WINDOW);
    }

    /// The finger lifted at `now`; `inertia` false stops kinetic motion (Reduce Motion), not the return.
    fn release(&mut self, now: f64, inertia: bool) {
        let Motion::Drag { raw, samples } = &self.motion else {
            return;
        };
        let raw = *raw;
        let velocity = match samples.last() {
            Some((last, _)) if now - last <= RELEASE_PAUSE => drag_velocity(samples, *last),
            _ => 0.0,
        };
        if raw < 0.0 || raw > self.max {
            // past an edge: back to it, a flick outward carried on a little first
            let (edge, sign, pull) = if raw < 0.0 {
                (0.0, -1.0, -raw)
            } else {
                (self.max, 1.0, raw - self.max)
            };
            let outward = sign * velocity * stretch_slope(pull, self.length);
            self.spring(now, edge, sign, stretch_of(pull, self.length), outward);
        } else if inertia && velocity.abs() >= MIN_FLING_SPEED {
            self.coast(now, raw, velocity);
        } else {
            self.motion = Motion::Rest(raw);
        }
    }

    /// The drag ended without a release: no kinetic motion, a stretch returns.
    fn cancel(&mut self, now: f64) {
        if matches!(self.motion, Motion::Drag { .. }) {
            self.release_still(now);
        }
    }

    fn release_still(&mut self, now: f64) {
        if let Motion::Drag { samples, .. } = &mut self.motion {
            samples.clear();
        }
        self.release(now, false);
    }

    fn spring(&mut self, now: f64, edge: f64, sign: f64, x0: f64, outward: f64) {
        let motion = Return::new(x0, outward);
        self.motion = if motion.settled(0.0) {
            Motion::Rest(edge)
        } else {
            Motion::Spring {
                t0: now,
                edge,
                sign,
                motion,
            }
        };
    }

    fn coast(&mut self, now: f64, from: f64, v0: f64) {
        let rest = from + v0 * MOMENTUM_TIME_CONSTANT;
        let edge = if rest < 0.0 {
            Some(0.0)
        } else if rest > self.max {
            Some(self.max)
        } else {
            None
        };
        let crossing = edge.map(|edge| {
            let tc = -MOMENTUM_TIME_CONSTANT
                * (1.0 - (edge - from) / (v0 * MOMENTUM_TIME_CONSTANT)).ln();
            (
                tc.max(0.0),
                edge,
                v0 - (edge - from) / MOMENTUM_TIME_CONSTANT,
            )
        });
        self.motion = Motion::Coast {
            t0: now,
            from,
            v0,
            crossing,
        };
    }

    /// A step of `delta` (a wheel, a key): the content moves at once, inside the range, and stops there.
    fn step(&mut self, delta: f64, now: f64) {
        let at = self.shown(now).clamp(0.0, self.max);
        self.motion = Motion::Rest((at + delta).clamp(0.0, self.max));
    }

    fn jump(&mut self, to: f64) {
        self.motion = Motion::Rest(to.clamp(0.0, self.max));
    }

    /// The range or the viewport changed. Content at rest stays inside the new range at once; moving content goes
    /// on from where it is drawn under the new range.
    fn set_bounds(&mut self, max: f64, length: f64, now: f64) {
        let max = max.max(0.0);
        if (max, length) == (self.max, self.length) {
            return;
        }
        let shown = self.shown(now);
        let velocity = self.velocity(now);
        let phase = self.phase(now);
        self.max = max;
        self.length = length;
        match (phase, &mut self.motion) {
            (Phase::Dragging, Motion::Drag { raw, samples }) => {
                // keep what the finger did; only where it shows changes
                let _ = (raw, samples);
            }
            (Phase::Coasting, _) => self.coast(now, shown.clamp(0.0, max), velocity),
            (Phase::Returning, _) if shown < 0.0 || shown > max => {
                let (edge, sign) = if shown < 0.0 { (0.0, -1.0) } else { (max, 1.0) };
                self.spring(now, edge, sign, (shown - edge).abs(), 0.0);
            }
            _ => self.motion = Motion::Rest(shown.clamp(0.0, max)),
        }
    }
}

/// Whether kinetic motion launched at `v0` has, `t` after its launch, less than [`SETTLED`] left to travel: it is
/// drawn at its rest from then on.
fn coast_settled(v0: f64, t: f64) -> bool {
    (v0 * MOMENTUM_TIME_CONSTANT * (-t / MOMENTUM_TIME_CONSTANT).exp()).abs() < SETTLED
}

/// Least-squares slope of `samples` over the [`VELOCITY_WINDOW`] that ends at `at`, in points per second.
fn drag_velocity(samples: &[(f64, f64)], at: f64) -> f64 {
    let recent: Vec<(f64, f64)> = samples
        .iter()
        .copied()
        .filter(|(t, _)| at - t <= VELOCITY_WINDOW + 1e-9 && *t <= at + 1e-9)
        .collect();
    if recent.len() < 2 {
        return 0.0;
    }
    let n = recent.len() as f64;
    let (mt, mx) = recent
        .iter()
        .fold((0.0, 0.0), |(a, b), (t, x)| (a + t / n, b + x / n));
    let (num, den) = recent.iter().fold((0.0, 0.0), |(num, den), (t, x)| {
        (num + (t - mt) * (x - mx), den + (t - mt) * (t - mt))
    });
    if den <= 1e-12 {
        0.0
    } else {
        num / den
    }
}

/// The two axes of a scroll view's content in motion, x then y.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollMotion {
    axes: [AxisMotion; 2],
    /// Whether the view scrolls along each axis.
    scrolls: [bool; 2],
    policy: AxisPolicy,
    /// The axes the drag in progress moves, decided by its first movement.
    locked: Option<[bool; 2]>,
}

impl ScrollMotion {
    /// Content at rest at `offset`, scrolling along the axes of `scrolls` (x, y) within `max` with a viewport of
    /// `viewport`.
    pub fn new(offset: Point, max: Point, viewport: Size, scrolls: (bool, bool)) -> ScrollMotion {
        ScrollMotion {
            axes: [
                AxisMotion::new(offset.x, max.x, viewport.width),
                AxisMotion::new(offset.y, max.y, viewport.height),
            ],
            scrolls: [scrolls.0, scrolls.1],
            policy: AxisPolicy::default(),
            locked: None,
        }
    }

    pub fn with_policy(mut self, policy: AxisPolicy) -> ScrollMotion {
        self.policy = policy;
        self
    }

    pub fn policy(&self) -> AxisPolicy {
        self.policy
    }

    /// Where the content is drawn: the offset with the stretch.
    pub fn shown(&self, now: f64) -> Point {
        Point::new(self.axes[0].shown(now), self.axes[1].shown(now))
    }

    /// The logical offset, inside the range.
    pub fn offset(&self, now: f64) -> Point {
        let shown = self.shown(now);
        Point::new(
            shown.x.clamp(0.0, self.axes[0].max),
            shown.y.clamp(0.0, self.axes[1].max),
        )
    }

    /// How far the content is drawn past its edges: negative before the start, positive past the end.
    pub fn stretch(&self, now: f64) -> Point {
        let (shown, offset) = (self.shown(now), self.offset(now));
        Point::new(shown.x - offset.x, shown.y - offset.y)
    }

    /// In points per second.
    pub fn velocity(&self, now: f64) -> Point {
        Point::new(self.axes[0].velocity(now), self.axes[1].velocity(now))
    }

    /// What each axis is doing, x then y.
    pub fn phases(&self, now: f64) -> [Phase; 2] {
        [self.axes[0].phase(now), self.axes[1].phase(now)]
    }

    /// Whether the content still moves on its own: kinetic motion or a return.
    pub fn is_animating(&self, now: f64) -> bool {
        self.phases(now)
            .iter()
            .any(|p| matches!(p, Phase::Coasting | Phase::Returning))
    }

    pub fn is_dragging(&self) -> bool {
        self.axes
            .iter()
            .any(|a| matches!(a.motion, Motion::Drag { .. }))
    }

    /// Whether moving by `delta` would move the content along an axis it scrolls: toward the end with room before
    /// it, or toward the start with room behind.
    pub fn can_move(&self, delta: Point, now: f64) -> bool {
        let shown = self.shown(now);
        let room = |i: usize, d: f64, at: f64| {
            let a = &self.axes[i];
            self.scrolls[i] && a.movable() && ((d > 0.0 && at < a.max) || (d < 0.0 && at > 0.0))
        };
        room(0, delta.x, shown.x) || room(1, delta.y, shown.y)
    }

    /// A finger moved the content by `delta` (toward the end is positive). The first movement of a drag catches
    /// the content and decides its axes.
    pub fn drag(&mut self, delta: Point, now: f64) {
        let moves = match self.locked {
            Some(moves) => moves,
            None => {
                let can = [
                    self.scrolls[0] && self.axes[0].movable(),
                    self.scrolls[1] && self.axes[1].movable(),
                ];
                let moves = match self.policy {
                    AxisPolicy::Predominant if can == [true, true] => {
                        if delta.x.abs() > delta.y.abs() {
                            [true, false]
                        } else if delta.y.abs() > delta.x.abs() {
                            [false, true]
                        } else {
                            [true, true]
                        }
                    }
                    _ => can,
                };
                for (axis, on) in self.axes.iter_mut().zip(can) {
                    if on {
                        axis.begin(now);
                    }
                }
                self.locked = Some(moves);
                moves
            }
        };
        for (i, d) in [delta.x, delta.y].into_iter().enumerate() {
            if moves[i] {
                self.axes[i].drag(d, now);
            }
        }
    }

    /// The finger lifted: kinetic motion from its speed when `inertia` (off under Reduce Motion), and a stretch
    /// returns either way.
    pub fn release(&mut self, now: f64, inertia: bool) {
        self.locked = None;
        for a in &mut self.axes {
            a.release(now, inertia);
        }
    }

    /// The drag ended without a release (the gesture was cancelled): no kinetic motion; a stretch returns.
    pub fn cancel(&mut self, now: f64) {
        self.locked = None;
        for a in &mut self.axes {
            a.cancel(now);
        }
    }

    /// A wheel, key or continuous scroll moved the content by `delta`: at once, inside the range, without inertia.
    /// The motion of each axis it moves stops.
    pub fn step(&mut self, delta: Point, now: f64) {
        for (i, d) in [delta.x, delta.y].into_iter().enumerate() {
            if d != 0.0 && self.scrolls[i] {
                self.axes[i].step(d, now);
            }
        }
    }

    /// Put the content at rest at `offset` (a key, a reveal, the model), inside the range.
    pub fn jump_to(&mut self, offset: Point) {
        self.locked = None;
        self.axes[0].jump(offset.x);
        self.axes[1].jump(offset.y);
    }

    /// The scrollable range or the viewport changed.
    pub fn set_bounds(&mut self, max: Point, viewport: Size, now: f64) {
        self.axes[0].set_bounds(max.x, viewport.width, now);
        self.axes[1].set_bounds(max.y, viewport.height, now);
    }
}

#[cfg(test)]
mod tests;
