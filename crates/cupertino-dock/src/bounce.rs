//! Launch and attention bounce animations.
//!
//! With [`BounceModel::Verified`] (default): a launch bounce is the continuous
//! parabola `y = 1 − (s/P)²` with `P = 355 ms` (a 710 ms cycle, 0 at the start, peak at 355 ms)
//! capped at 120 s; attention bounces are the three-hop sequence 500/326/212 ms with amplitudes
//! 1/0.5/0.25, each hop a parabola `1 − ((r − L/2)/(L/2))²`, repeating every 2000 ms. An
//! informational request plays the sequence once, a critical one loops until cleared.
//! [`BounceModel::Parametric`] keeps the generic arcs (approximate). Stopping lets the running
//! arc finish so the tile never snaps.

use crate::config::{BounceConfig, BounceModel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BounceKind {
    /// Repeats until the app is running (or the timeout).
    Launch,
    /// Plays the attention sequence once.
    Informational,
    /// Repeats the attention sequence until cleared.
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounce {
    pub kind: BounceKind,
    pub start: f64,
    stop_at: Option<f64>,
}

fn arc(u: f64) -> f64 {
    4.0 * u * (1.0 - u)
}

impl Bounce {
    pub fn new(kind: BounceKind, start: f64) -> Self {
        Self {
            kind,
            start,
            stop_at: None,
        }
    }

    fn natural_end(&self, cfg: &BounceConfig) -> Option<f64> {
        match (self.kind, cfg.model) {
            (BounceKind::Launch, _) => Some(self.start + cfg.launch_timeout),
            (BounceKind::Informational, BounceModel::Verified) => {
                Some(self.start + cfg.attention_sequence_s())
            }
            (BounceKind::Informational, BounceModel::Parametric) => {
                Some(self.start + cfg.informational_arcs as f64 * cfg.period)
            }
            (BounceKind::Critical, _) => None,
        }
    }

    fn end(&self, cfg: &BounceConfig) -> Option<f64> {
        match (self.natural_end(cfg), self.stop_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Hop containing `r` seconds into the cycle: `(index, hop start, hop length)` in seconds.
    fn hop_at(cfg: &BounceConfig, r: f64) -> Option<(usize, f64, f64)> {
        let mut at = 0.0;
        for (i, h) in cfg.attention_hops.iter().enumerate() {
            let len = h.length_ms / 1000.0;
            if r < at + len {
                return Some((i, at, len));
            }
            at += len;
        }
        None
    }

    /// Request the bounce to finish: the running arc (launch) or hop (attention) completes first.
    pub fn stop(&mut self, cfg: &BounceConfig, now: f64) {
        let t = (now - self.start).max(0.0);
        let at = match (self.kind, cfg.model) {
            (BounceKind::Launch, _) | (_, BounceModel::Parametric) => {
                // a stop at the very start still lets the first cycle run
                let arcs = (t / cfg.period).ceil().max(1.0);
                self.start + arcs * cfg.period
            }
            (_, BounceModel::Verified) => {
                let cycle = cfg.attention_cycle.max(1e-6);
                let base = (t / cycle).floor() * cycle;
                match Self::hop_at(cfg, t - base) {
                    Some((_, s, l)) => self.start + base + s + l,
                    None => self.start + t,
                }
            }
        };
        self.stop_at = Some(match self.stop_at {
            Some(s) => s.min(at),
            None => at,
        });
    }

    /// A stop was requested: the running arc or hop is the last one.
    pub fn is_stopping(&self) -> bool {
        self.stop_at.is_some()
    }

    pub fn is_active(&self, cfg: &BounceConfig, now: f64) -> bool {
        now >= self.start && self.end(cfg).is_none_or(|e| now < e)
    }

    /// Normalised bounce curve in `[0, 1]` at `now`: 0 at rest, 1 at the peak of the highest hop.
    /// Includes the hop amplitude (verified model) or decay (parametric model), but not the lift.
    pub fn shape(&self, cfg: &BounceConfig, now: f64) -> f64 {
        if !self.is_active(cfg, now) {
            return 0.0;
        }
        let t = now - self.start;
        match (self.kind, cfg.model) {
            (BounceKind::Launch, BounceModel::Verified) => arc((t / cfg.period).fract()),
            (_, BounceModel::Verified) => {
                let r = t % cfg.attention_cycle.max(1e-6);
                match Self::hop_at(cfg, r) {
                    Some((i, s, l)) => {
                        let x = (r - s - l / 2.0) / (l / 2.0);
                        cfg.attention_hops[i].amplitude * (1.0 - x * x)
                    }
                    None => 0.0,
                }
            }
            (kind, BounceModel::Parametric) => {
                let (index, u) = match kind {
                    BounceKind::Critical => {
                        let burst_arcs = (cfg.secondary_duration / cfg.period).round().max(1.0);
                        let burst = burst_arcs * cfg.period;
                        let cycle = burst + cfg.secondary_interval.max(0.0);
                        let c = t % cycle;
                        if c >= burst {
                            return 0.0;
                        }
                        ((c / cfg.period).floor(), (c / cfg.period).fract())
                    }
                    _ => ((t / cfg.period).floor(), (t / cfg.period).fract()),
                };
                let decay = if kind == BounceKind::Informational {
                    cfg.decay.powf(index)
                } else {
                    1.0
                };
                decay * arc(u)
            }
        }
    }

    /// Peak lift in points for a tile of length `len`.
    pub fn peak_lift_pt(&self, cfg: &BounceConfig, len: f64) -> f64 {
        match cfg.model {
            BounceModel::Parametric => cfg.height * len,
            BounceModel::Verified => match self.kind {
                BounceKind::Launch => cfg.launch_lift.pt(len),
                _ => cfg.attention_lift.pt(len),
            },
        }
    }

    /// Lift in points at `now` for a tile of length `len` (up for a bottom dock).
    pub fn lift_pt(&self, cfg: &BounceConfig, len: f64, now: f64) -> f64 {
        self.shape(cfg, now) * self.peak_lift_pt(cfg, len)
    }

    /// Lift as a fraction of the tile size at time `now`, for a tile of length `cfg.tile_len`.
    pub fn offset(&self, cfg: &BounceConfig, now: f64) -> f64 {
        let len = cfg.tile_len.max(1e-9);
        self.lift_pt(cfg, len, now) / len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_shape() {
        let cfg = BounceConfig::parametric();
        let b = Bounce::new(BounceKind::Launch, 1.0);
        assert_eq!(b.offset(&cfg, 0.5), 0.0);
        assert_eq!(b.offset(&cfg, 1.0), 0.0);
        assert!((b.offset(&cfg, 1.0 + cfg.period / 2.0) - cfg.height).abs() < 1e-12);
        assert!(b.offset(&cfg, 1.0 + cfg.period).abs() < 1e-12);
        assert!(b.offset(&cfg, 1.0 + 3.5 * cfg.period) > 0.99 * cfg.height);
    }

    #[test]
    fn launch_stop_finishes_current_arc() {
        let cfg = BounceConfig::parametric();
        let mut b = Bounce::new(BounceKind::Launch, 0.0);
        let now = 1.0 * cfg.period + 0.1;
        b.stop(&cfg, now);
        assert!(b.is_active(&cfg, now + 0.3));
        assert!(b.offset(&cfg, now + 0.3) > 0.0);
        assert!(!b.is_active(&cfg, 2.0 * cfg.period + 1e-9));
        assert_eq!(b.offset(&cfg, 5.0), 0.0);
    }

    #[test]
    fn launch_times_out() {
        let cfg = BounceConfig {
            launch_timeout: 2.0,
            ..BounceConfig::parametric()
        };
        let b = Bounce::new(BounceKind::Launch, 0.0);
        assert!(b.is_active(&cfg, 1.9));
        assert!(!b.is_active(&cfg, 2.0));
    }

    #[test]
    fn informational_has_fixed_arc_count_and_decay() {
        let cfg = BounceConfig {
            informational_arcs: 3,
            decay: 0.5,
            ..BounceConfig::parametric()
        };
        let b = Bounce::new(BounceKind::Informational, 0.0);
        let peak = |k: f64| b.offset(&cfg, (k + 0.5) * cfg.period);
        assert!((peak(0.0) - cfg.height).abs() < 1e-12);
        assert!((peak(1.0) - cfg.height * 0.5).abs() < 1e-12);
        assert!((peak(2.0) - cfg.height * 0.25).abs() < 1e-12);
        assert!(!b.is_active(&cfg, 3.0 * cfg.period));
    }

    #[test]
    fn critical_repeats_in_bursts_until_stopped() {
        let cfg = BounceConfig {
            period: 1.0,
            secondary_duration: 2.0,
            secondary_interval: 3.0,
            ..BounceConfig::parametric()
        };
        let mut b = Bounce::new(BounceKind::Critical, 0.0);
        assert!(b.offset(&cfg, 0.5) > 0.0);
        assert!(b.offset(&cfg, 1.5) > 0.0);
        assert_eq!(b.offset(&cfg, 3.0), 0.0, "pause");
        assert!(b.offset(&cfg, 5.5) > 0.0, "second burst");
        assert!(b.is_active(&cfg, 1000.0));
        b.stop(&cfg, 5.2);
        assert!(!b.is_active(&cfg, 6.0 + 1e-9));
    }

    #[test]
    fn launch_parabola_matches_recovered_curve() {
        let cfg = BounceConfig::default();
        let b = Bounce::new(BounceKind::Launch, 0.0);
        assert_eq!(b.shape(&cfg, 0.0), 0.0);
        assert!((b.shape(&cfg, 0.355) - 1.0).abs() < 1e-12, "peak at 355 ms");
        assert!(b.shape(&cfg, 0.709).abs() < 1e-2);
        for k in 0..710 {
            let t = k as f64 / 1000.0;
            let s = if t + 0.355 >= 0.71 {
                t + 0.355 - 0.71
            } else {
                t + 0.355
            };
            let s = if s > 0.355 { s - 0.71 } else { s };
            let expected = 1.0 - (s / 0.355) * (s / 0.355);
            assert!((b.shape(&cfg, t) - expected).abs() < 1e-9, "t={t}");
        }
        assert!(b.is_active(&cfg, 119.9));
        assert!(!b.is_active(&cfg, 120.0), "capped at 120 s");
    }

    #[test]
    fn launch_lift_formula() {
        let cfg = BounceConfig::default();
        let b = Bounce::new(BounceKind::Launch, 0.0);
        assert!((b.peak_lift_pt(&cfg, 64.0) - 17.9).abs() < 1e-9);
        assert!((b.lift_pt(&cfg, 64.0, 0.355) - 17.9).abs() < 1e-9);
        assert!((b.offset(&cfg, 0.355) * cfg.tile_len - 17.9).abs() < 1e-9);
    }

    #[test]
    fn attention_hops_match_recovered_sequence() {
        let cfg = BounceConfig::default();
        let b = Bounce::new(BounceKind::Critical, 0.0);
        assert!((b.shape(&cfg, 0.250) - 1.0).abs() < 1e-12);
        assert!((b.shape(&cfg, 0.500 + 0.163) - 0.5).abs() < 1e-12);
        assert!((b.shape(&cfg, 0.826 + 0.106) - 0.25).abs() < 1e-12);
        assert_eq!(
            b.shape(&cfg, 0.5),
            0.0,
            "hops touch the ground between each other"
        );
        assert_eq!(b.shape(&cfg, 1.5), 0.0, "rest until 2000 ms");
        assert!(
            (b.shape(&cfg, 2.250) - 1.0).abs() < 1e-12,
            "repeats every 2000 ms"
        );
        assert!(b.is_active(&cfg, 1000.0));
        assert!((cfg.attention_sequence_s() - 1.038).abs() < 1e-12);
        assert!((b.peak_lift_pt(&cfg, 64.0) - 65.0).abs() < 1e-12, "len + 1");
    }

    #[test]
    fn informational_attention_plays_once() {
        let cfg = BounceConfig::default();
        let b = Bounce::new(BounceKind::Informational, 0.0);
        assert!(b.is_active(&cfg, 1.0));
        assert!(!b.is_active(&cfg, 1.039));
        assert_eq!(b.shape(&cfg, 2.25), 0.0);
    }

    #[test]
    fn verified_stop_finishes_the_running_hop() {
        let cfg = BounceConfig::default();
        let mut b = Bounce::new(BounceKind::Critical, 0.0);
        b.stop(&cfg, 0.6);
        assert!(b.is_active(&cfg, 0.8));
        assert!(!b.is_active(&cfg, 0.826 + 1e-9));
        let mut r = Bounce::new(BounceKind::Critical, 0.0);
        r.stop(&cfg, 1.5);
        assert!(
            !r.is_active(&cfg, 1.5 + 1e-9),
            "stopping at rest is immediate"
        );
        let mut l = Bounce::new(BounceKind::Launch, 0.0);
        l.stop(&cfg, 0.75);
        assert!(l.is_active(&cfg, 1.41));
        assert!(
            !l.is_active(&cfg, 1.42 + 1e-9),
            "rounded up to the end of the 710 ms cycle"
        );
    }

    #[test]
    fn shapes_stay_in_unit_range() {
        let cfg = BounceConfig::default();
        for kind in [
            BounceKind::Launch,
            BounceKind::Informational,
            BounceKind::Critical,
        ] {
            let b = Bounce::new(kind, 0.3);
            for k in 0..6000 {
                let s = b.shape(&cfg, k as f64 / 1000.0);
                assert!((0.0..=1.0 + 1e-12).contains(&s) && s.is_finite());
            }
        }
    }
}
