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
