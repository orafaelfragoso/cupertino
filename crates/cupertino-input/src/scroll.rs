//! Kinetic scrolling with rubber-band overscroll for compositor UI (menus, the overview, the app
//! grid, notification lists).
//!
//! Built on `cupertino-animation`: the fling uses the default quartic momentum model or, when configured, the per-millisecond exponential one; the overshoot and return at
//! a bound uses the elastic motion `(x0 + v·t·0.31)·exp(−20t/1.6). Dragging
//! beyond a bound maps the raw finger offset through the elastic curve of
//! `cupertino_animation::RubberBand` (approximate: the filter used while dragging was not reduced
//! to a closed form).
//!
//! Every query is a pure function of the supplied absolute time; mutating calls take the time too.

use cupertino_animation::{ElasticMotion, Momentum, MomentumModel, QuarticMomentum, RubberBand};

use crate::velocity::VelocityTracker;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MomentumKind {
    /// Default model.
    Quartic,
    /// The per-millisecond alternative.
    Exponential,
}

#[derive(Clone, Debug)]
pub struct ScrollConfig {
    pub momentum: MomentumKind,
    pub elastic: ElasticMotion,
    /// Viewport length along the axis; scales the drag overscroll curve. approximate.
    pub dimension: f64,
    /// Stiffness coefficient of the drag overscroll curve. approximate.
    pub drag_coefficient: f64,
    pub velocity_window: f64,
    /// Release velocities below this (pt/s) do not start a fling. approximate.
    pub min_fling_velocity: f64,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            momentum: MomentumKind::Quartic,
            elastic: ElasticMotion::default(),
            dimension: 800.0,
            drag_coefficient: cupertino_animation::rubber_band::DEFAULT_COEFFICIENT,
            velocity_window: 0.1,
            min_fling_velocity: 50.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Crossing {
    time: f64,
    edge: f64,
    sign_out: f64,
    out_velocity: f64,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Idle {
        position: f64,
    },
    Dragging {
        raw: f64,
    },
    Coasting {
        t0: f64,
        model: MomentumModel,
        crossing: Option<Crossing>,
    },
    Elastic {
        t0: f64,
        edge: f64,
        sign_out: f64,
        overshoot: f64,
        out_velocity: f64,
    },
}

pub struct KineticScroller {
    pub cfg: ScrollConfig,
    min: f64,
    max: f64,
    state: State,
    velocity: VelocityTracker,
}

impl KineticScroller {
    pub fn new(cfg: ScrollConfig, min: f64, max: f64, position: f64) -> Self {
        let velocity = VelocityTracker::new(cfg.velocity_window);
        Self {
            cfg,
            min,
            max: max.max(min),
            state: State::Idle { position },
            velocity,
        }
    }

    pub fn bounds(&self) -> (f64, f64) {
        (self.min, self.max)
    }

    fn band(&self) -> RubberBand {
        RubberBand::with_coefficient(self.cfg.dimension, self.cfg.drag_coefficient)
    }

    /// Change the scrollable range; a position that ends up outside returns elastically.
    pub fn set_bounds(&mut self, min: f64, max: f64, now: f64) {
        let p = self.position(now);
        self.min = min;
        self.max = max.max(min);
        if matches!(self.state, State::Idle { .. }) {
            self.settle_from(p, 0.0, now);
        }
    }

    pub fn position(&self, now: f64) -> f64 {
        match self.state {
            State::Idle { position } => position,
            State::Dragging { raw } => self.band().apply(raw, self.min, self.max),
            State::Coasting {
                t0,
                model,
                crossing,
            } => {
                let t = (now - t0).max(0.0);
                match crossing {
                    Some(c) if t >= c.time => {
                        self.elastic_position(c.edge, c.sign_out, 0.0, c.out_velocity, t - c.time)
                    }
                    _ => model.position(t),
                }
            }
            State::Elastic {
                t0,
                edge,
                sign_out,
                overshoot,
                out_velocity,
            } => {
                self.elastic_position(edge, sign_out, overshoot, out_velocity, (now - t0).max(0.0))
            }
        }
    }

    fn elastic_position(
        &self,
        edge: f64,
        sign_out: f64,
        overshoot: f64,
        out_velocity: f64,
        t: f64,
    ) -> f64 {
        let e = &self.cfg.elastic;
        if t >= e.settle_time(overshoot, out_velocity) {
            return edge;
        }
        edge + sign_out * e.delta(overshoot, out_velocity, t)
    }

    /// Instantaneous velocity (pt/s); zero while idle, the finger velocity estimate while dragging.
    pub fn velocity(&self, now: f64) -> f64 {
        match self.state {
            State::Idle { .. } => 0.0,
            State::Dragging { .. } => self.velocity.velocity(now),
            State::Coasting {
                t0,
                model,
                crossing,
            } => {
                let t = (now - t0).max(0.0);
                match crossing {
                    Some(c) if t >= c.time => {
                        c.sign_out * self.cfg.elastic.velocity(0.0, c.out_velocity, t - c.time)
                    }
                    _ => model.velocity(t),
                }
            }
            State::Elastic {
                t0,
                sign_out,
                overshoot,
                out_velocity,
                ..
            } => {
                sign_out
                    * self
                        .cfg
                        .elastic
                        .velocity(overshoot, out_velocity, (now - t0).max(0.0))
            }
        }
    }

    pub fn is_dragging(&self) -> bool {
        matches!(self.state, State::Dragging { .. })
    }

    pub fn is_animating(&self, now: f64) -> bool {
        match self.state {
            State::Idle { .. } => false,
            State::Dragging { .. } => true,
            State::Coasting {
                t0,
                model,
                crossing,
            } => {
                let t = now - t0;
                match crossing {
                    Some(c) => t < c.time + self.cfg.elastic.settle_time(0.0, c.out_velocity),
                    None => t < model.duration(),
                }
            }
            State::Elastic {
                t0,
                overshoot,
                out_velocity,
                ..
            } => now - t0 < self.cfg.elastic.settle_time(overshoot, out_velocity),
        }
    }

    /// Time at which the current animation ends, if one is running.
    pub fn end_time(&self) -> Option<f64> {
        match self.state {
            State::Coasting {
                t0,
                model,
                crossing,
            } => Some(
                t0 + match crossing {
                    Some(c) => c.time + self.cfg.elastic.settle_time(0.0, c.out_velocity),
                    None => model.duration(),
                },
            ),
            State::Elastic {
                t0,
                overshoot,
                out_velocity,
                ..
            } => Some(t0 + self.cfg.elastic.settle_time(overshoot, out_velocity)),
            _ => None,
        }
    }

    /// Fingers touch down: stop any animation at the current position and start tracking.
    pub fn begin_drag(&mut self, now: f64) {
        let shown = self.position(now);
        let raw = if shown < self.min {
            self.min + self.band().inverse(shown - self.min)
        } else if shown > self.max {
            self.max + self.band().inverse(shown - self.max)
        } else {
            shown
        };
        self.velocity = VelocityTracker::new(self.cfg.velocity_window);
        self.velocity.push(now, raw);
        self.state = State::Dragging { raw };
    }

    /// Move the content by `delta` points (positive increases the position).
    pub fn drag(&mut self, delta: f64, now: f64) {
        if let State::Dragging { raw } = self.state {
            let raw = raw + delta;
            self.velocity.push(now, raw);
            self.state = State::Dragging { raw };
        }
    }

    /// Fingers lift: fling (or return from an overscroll). Returns the release velocity.
    pub fn end_drag(&mut self, now: f64) -> f64 {
        let State::Dragging { .. } = self.state else {
            return 0.0;
        };
        let shown = self.position(now);
        let v = self.velocity.velocity(now);
        if shown < self.min || shown > self.max {
            self.settle_from(shown, v, now);
        } else {
            self.fling(shown, v, now);
        }
        v
    }

    /// Wheel / key scrolling: jump the position (clamped to the bounds) without animation.
    pub fn jump_to(&mut self, position: f64) {
        self.state = State::Idle {
            position: position.clamp(self.min, self.max),
        };
    }

    /// Start a fling from the current position with the given velocity (pt/s).
    pub fn fling_with(&mut self, velocity: f64, now: f64) {
        let p = self.position(now);
        self.fling(p, velocity, now);
    }

    /// Stop at the current position (a resting touch); an overscrolled position returns to the edge.
    pub fn stop(&mut self, now: f64) {
        let p = self.position(now);
        self.settle_from(p, 0.0, now);
    }

    fn settle_from(&mut self, shown: f64, velocity: f64, now: f64) {
        if shown < self.min || shown > self.max {
            let (edge, sign_out) = if shown < self.min {
                (self.min, -1.0)
            } else {
                (self.max, 1.0)
            };
            self.state = State::Elastic {
                t0: now,
                edge,
                sign_out,
                overshoot: (shown - edge).abs(),
                out_velocity: velocity * sign_out,
            };
        } else {
            self.state = State::Idle { position: shown };
        }
    }

    fn fling(&mut self, position: f64, velocity: f64, now: f64) {
        if velocity.abs() < self.cfg.min_fling_velocity {
            self.settle_from(position, 0.0, now);
            return;
        }
        let model = match self.cfg.momentum {
            MomentumKind::Quartic => {
                MomentumModel::Quartic(QuarticMomentum::new(position, velocity))
            }
            MomentumKind::Exponential => {
                MomentumModel::Exponential(Momentum::new(position, velocity, f64::MIN, f64::MAX))
            }
        };
        let dest = model.destination();
        let (edge, sign_out) = if velocity > 0.0 {
            (self.max, 1.0)
        } else {
            (self.min, -1.0)
        };
        let beyond = (dest - edge) * sign_out > 0.0;
        let crossing = beyond.then(|| {
            let time = Self::time_to_reach(&model, edge, sign_out);
            Crossing {
                time,
                edge,
                sign_out,
                out_velocity: model.velocity(time).abs(),
            }
        });
        self.state = State::Coasting {
            t0: now,
            model,
            crossing,
        };
    }

    fn time_to_reach(model: &MomentumModel, edge: f64, sign_out: f64) -> f64 {
        match model {
            MomentumModel::Quartic(q) => q.time_to_reach(edge).unwrap_or(0.0),
            _ => {
                let (mut lo, mut hi) = (0.0, model.duration().max(1e-6));
                for _ in 0..60 {
                    let mid = 0.5 * (lo + hi);
                    if (model.position(mid) - edge) * sign_out >= 0.0 {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                hi
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scroller() -> KineticScroller {
        KineticScroller::new(ScrollConfig::default(), 0.0, 2000.0, 500.0)
    }

    fn drag_with_velocity(s: &mut KineticScroller, v: f64, start: f64) -> f64 {
        s.begin_drag(start);
        let mut t = start;
        for _ in 0..8 {
            t += 0.01;
            s.drag(v * 0.01, t);
        }
        s.end_drag(t);
        t
    }

    #[test]
    fn fling_inside_bounds_ends_at_the_momentum_destination() {
        let mut s = scroller();
        let t = drag_with_velocity(&mut s, 2000.0, 0.0);
        let start = s.position(t);
        let q = QuarticMomentum::new(start, s.velocity(t));
        assert!(s.is_animating(t + 0.1));
        let end = s.end_time().unwrap();
        assert!(
            (s.position(end + 0.01) - q.destination()).abs() < 2.0,
            "{} vs {}",
            s.position(end + 0.01),
            q.destination()
        );
        assert!(!s.is_animating(end + 0.01));
        assert!(s.position(end + 5.0) > start);
    }

    #[test]
    fn position_is_continuous_and_monotonic_while_coasting() {
        let mut s = scroller();
        let t = drag_with_velocity(&mut s, 1500.0, 0.0);
        let mut last = s.position(t);
        let mut x = t;
        while x < t + 3.0 {
            x += 0.004;
            let p = s.position(x);
            assert!(p >= last - 1e-9 && p - last < 40.0);
            last = p;
        }
    }

    #[test]
    fn fling_past_the_bound_overshoots_then_returns() {
        let mut s = KineticScroller::new(ScrollConfig::default(), 0.0, 300.0, 250.0);
        s.fling_with(4000.0, 0.0);
        let mut peak: f64 = 0.0;
        let mut t = 0.0;
        while t < 4.0 {
            t += 0.005;
            peak = peak.max(s.position(t));
        }
        assert!(peak > 300.0, "overshoots the bound: {peak}");
        assert!(peak < 300.0 + 400.0, "bounded overshoot: {peak}");
        assert_eq!(s.position(10.0), 300.0, "settles exactly on the edge");
        assert!(!s.is_animating(10.0));
    }

    #[test]
    fn fling_towards_the_lower_bound_mirrors() {
        let mut s = KineticScroller::new(ScrollConfig::default(), 0.0, 300.0, 50.0);
        s.fling_with(-4000.0, 0.0);
        let min = (0..800)
            .map(|i| s.position(i as f64 * 0.005))
            .fold(f64::MAX, f64::min);
        assert!(min < 0.0);
        assert_eq!(s.position(10.0), 0.0);
    }

    #[test]
    fn drag_beyond_a_bound_is_rubber_banded() {
        let mut s = KineticScroller::new(ScrollConfig::default(), 0.0, 1000.0, 1000.0);
        s.begin_drag(0.0);
        s.drag(200.0, 0.01);
        let shown = s.position(0.01) - 1000.0;
        assert!(shown > 0.0 && shown < 200.0, "{shown}");
        s.drag(2000.0, 0.02);
        assert!(
            s.position(0.02) - 1000.0 < s.cfg.dimension,
            "bounded by the viewport"
        );
        s.drag(-2200.0, 0.03);
        assert!((s.position(0.03) - 1000.0).abs() < 1e-9, "back at the edge");
    }

    #[test]
    fn release_while_overscrolled_returns_to_the_edge_smoothly() {
        let mut s = KineticScroller::new(ScrollConfig::default(), 0.0, 1000.0, 1000.0);
        s.begin_drag(0.0);
        s.drag(300.0, 0.5);
        s.end_drag(1.0);
        let start = s.position(1.0);
        assert!(start > 1000.0);
        let mut last = start;
        for i in 1..200 {
            let p = s.position(1.0 + i as f64 * 0.01);
            assert!(p <= last + 1e-9 && p >= 1000.0, "monotonic return");
            last = p;
        }
        assert_eq!(s.position(5.0), 1000.0);
        assert!(s.end_time().unwrap() < 3.0);
    }

    #[test]
    fn slow_release_does_not_fling() {
        let mut s = scroller();
        s.begin_drag(0.0);
        s.drag(10.0, 0.01);
        s.drag(0.0, 0.5);
        s.end_drag(0.6);
        assert!(!s.is_animating(0.6));
        assert_eq!(s.position(0.6), 510.0);
    }

    #[test]
    fn touching_stops_a_fling_where_it_is() {
        let mut s = scroller();
        s.fling_with(3000.0, 0.0);
        let p = s.position(0.2);
        s.begin_drag(0.2);
        assert!((s.position(0.2) - p).abs() < 1e-9);
        s.end_drag(0.3);
        assert!((s.position(0.3) - p).abs() < 1e-9);
        assert!(!s.is_animating(0.3));
        s.fling_with(3000.0, 1.0);
        s.stop(1.2);
        assert!(!s.is_animating(1.3));
    }

    #[test]
    fn begin_drag_during_overscroll_keeps_the_shown_position() {
        let mut s = KineticScroller::new(ScrollConfig::default(), 0.0, 100.0, 100.0);
        s.fling_with(5000.0, 0.0);
        let shown = s.position(0.5);
        assert!(shown > 100.0);
        s.begin_drag(0.5);
        assert!((s.position(0.5) - shown).abs() < 1e-6);
    }

    #[test]
    fn exponential_model_can_be_selected() {
        let cfg = ScrollConfig {
            momentum: MomentumKind::Exponential,
            ..Default::default()
        };
        let mut s = KineticScroller::new(cfg, -1e6, 1e6, 0.0);
        s.fling_with(2000.0, 0.0);
        assert!(s.end_time().unwrap() > 1.0);
        assert!((s.position(60.0) - 998.0).abs() < 1.0);
        let mut q = KineticScroller::new(ScrollConfig::default(), -1e6, 1e6, 0.0);
        q.fling_with(2000.0, 0.0);
        assert!(
            q.position(60.0) > 0.0 && (q.position(60.0) - s.position(60.0)).abs() > 1.0,
            "models differ"
        );
    }

    #[test]
    fn exponential_overshoot_uses_the_same_elastic_return() {
        let cfg = ScrollConfig {
            momentum: MomentumKind::Exponential,
            ..Default::default()
        };
        let mut s = KineticScroller::new(cfg, 0.0, 100.0, 90.0);
        s.fling_with(5000.0, 0.0);
        assert!((0..400).any(|i| s.position(i as f64 * 0.01) > 100.0));
        assert_eq!(s.position(20.0), 100.0);
    }

    #[test]
    fn bounds_change_returns_an_overscrolled_position() {
        let mut s = KineticScroller::new(ScrollConfig::default(), 0.0, 1000.0, 900.0);
        s.set_bounds(0.0, 500.0, 0.0);
        assert!(s.is_animating(0.0) || s.position(0.0) == 500.0 || s.position(0.0) > 500.0);
        assert_eq!(s.position(10.0), 500.0);
    }

    #[test]
    fn jump_clamps_and_idle_is_stable() {
        let mut s = scroller();
        s.jump_to(5000.0);
        assert_eq!(s.position(0.0), 2000.0);
        assert!(!s.is_animating(0.0));
        assert_eq!(s.velocity(0.0), 0.0);
        assert_eq!(s.end_time(), None);
        assert_eq!(s.bounds(), (0.0, 2000.0));
    }

    #[test]
    fn zero_velocity_end_drag_is_safe() {
        let mut s = scroller();
        assert_eq!(s.end_drag(0.0), 0.0, "not dragging");
        s.begin_drag(0.0);
        assert_eq!(s.end_drag(0.0), 0.0);
        assert_eq!(s.position(0.0), 500.0);
    }
}
