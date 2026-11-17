//! Dragging a window into the top of the screen opens the overview: once the pointer that moves the window reaches
//! the output's top edge, the motion that keeps pushing up past it, within [`TopZoneConfig::max_angle`] of straight up,
//! is summed; when the sum reaches [`TopZoneConfig::distance`] the overview opens. Moving down, sideways past the
//! angle, or away from the edge starts the sum again.

/// Where the top zone fires.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TopZoneConfig {
    /// How far the pointer pushes up past the top edge before the overview opens (64 pt).
    pub distance: f64,
    /// The largest angle from straight up a push counts at, in radians (π/16).
    pub max_angle: f64,
    /// The pointer is at the top edge while it is less than this below it (1 pt).
    pub edge: f64,
}

impl Default for TopZoneConfig {
    fn default() -> Self {
        Self {
            distance: 64.0,
            max_angle: std::f64::consts::PI / 16.0,
            edge: 1.0,
        }
    }
}

/// The recognizer of one window drag.
#[derive(Clone, Debug, Default)]
pub struct TopZone {
    pub cfg: TopZoneConfig,
    pushed: f64,
    fired: bool,
}

impl TopZone {
    pub fn new(cfg: TopZoneConfig) -> Self {
        Self {
            cfg,
            pushed: 0.0,
            fired: false,
        }
    }

    /// The window drag moved the pointer by `(dx, dy)` (points, y down) and left it `below_top` points below its
    /// output's top edge. True once, when the push past the edge reaches the distance; again only after a reset.
    pub fn motion(&mut self, below_top: f64, dx: f64, dy: f64) -> bool {
        if below_top >= self.cfg.edge {
            self.pushed = 0.0;
            self.fired = false;
            return false;
        }
        let up = -dy;
        if up <= 0.0 {
            if dx != 0.0 || dy > 0.0 {
                self.pushed = 0.0;
            }
            return false;
        }
        if dx.abs().atan2(up) > self.cfg.max_angle {
            self.pushed = 0.0;
            return false;
        }
        self.pushed += dx.hypot(dy);
        if self.pushed >= self.cfg.distance && !self.fired {
            self.fired = true;
            return true;
        }
        false
    }

    /// The drag ended or started anew.
    pub fn reset(&mut self) {
        self.pushed = 0.0;
        self.fired = false;
    }
}
