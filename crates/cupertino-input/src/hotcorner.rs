//! Hot corners: pointer dwell in an output corner triggers an action.
//!
//! The existence of four corner actions, a per-corner modifier requirement and a corner action
//! delay is as designed; the delay's default value is
//! unknown, so [`HotCornerConfig::dwell`] is a configurable placeholder. The corner hit region reaches
//! 4 pt from the corner along each edge.
//!
//! The recognizer must be fed pointer motion and modifier changes through [`HotCorners::update`]
//! and time through [`HotCorners::tick`] (use [`HotCorners::next_deadline`] to schedule a timer):
//! a resting pointer fires on the tick after the dwell. A corner fires once per visit; the pointer
//! must leave the corner (or release the required modifier and re-enter) to arm it again.

use crate::geom::{Modifiers, Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    fn index(self) -> usize {
        match self {
            Corner::TopLeft => 0,
            Corner::TopRight => 1,
            Corner::BottomLeft => 2,
            Corner::BottomRight => 3,
        }
    }

    fn point(self, r: Rect) -> Point {
        match self {
            Corner::TopLeft => Point::new(r.x, r.y),
            Corner::TopRight => Point::new(r.right(), r.y),
            Corner::BottomLeft => Point::new(r.x, r.bottom()),
            Corner::BottomRight => Point::new(r.right(), r.bottom()),
        }
    }

    fn outward(self) -> (f64, f64) {
        match self {
            Corner::TopLeft => (-1.0, -1.0),
            Corner::TopRight => (1.0, -1.0),
            Corner::BottomLeft => (-1.0, 1.0),
            Corner::BottomRight => (1.0, 1.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CornerAction {
    Overview,
    AppOverview,
    ShowDesktop,
    AppGrid,
    Launcher,
    ControlPanel,
    NotificationCenter,
    LockScreen,
    SleepDisplay,
    /// Launch a command.
    Exec(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CornerBinding {
    pub action: CornerAction,
    /// Modifiers that must be held (extra modifiers are allowed).
    pub modifiers: Modifiers,
}

impl CornerBinding {
    pub fn new(action: CornerAction) -> Self {
        Self {
            action,
            modifiers: Modifiers::NONE,
        }
    }

    pub fn with_modifiers(action: CornerAction, modifiers: Modifiers) -> Self {
        Self { action, modifiers }
    }
}

#[derive(Clone, Debug)]
pub struct HotCornerConfig {
    /// Per-corner binding in [`Corner::ALL`] order; `None` disables the corner. No corner is bound
    /// by default.
    pub bindings: [Option<CornerBinding>; 4],
    /// Distance from the corner (per axis) that counts as "in the corner" (4 pt).
    pub size: f64,
    /// Seconds the pointer must rest in the corner. unknown placeholder.
    pub dwell: f64,
}

impl Default for HotCornerConfig {
    fn default() -> Self {
        Self {
            bindings: [None, None, None, None],
            size: 4.0,
            dwell: 0.1,
        }
    }
}

impl HotCornerConfig {
    pub fn set(&mut self, corner: Corner, binding: Option<CornerBinding>) {
        self.bindings[corner.index()] = binding;
    }

    pub fn binding(&self, corner: Corner) -> Option<&CornerBinding> {
        self.bindings[corner.index()].as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CornerEvent {
    /// The pointer entered a bound corner (and holds the required modifiers).
    Armed { corner: Corner, output: usize },
    Triggered {
        corner: Corner,
        output: usize,
        action: CornerAction,
    },
    /// The pointer left the corner, or the required modifiers were released.
    Disarmed { corner: Corner, output: usize },
}

#[derive(Clone, Copy, Debug)]
struct Hover {
    corner: Corner,
    output: usize,
    since: Option<f64>,
    fired: bool,
}

pub struct HotCorners {
    pub cfg: HotCornerConfig,
    hover: Option<Hover>,
}

impl HotCorners {
    pub fn new(cfg: HotCornerConfig) -> Self {
        Self { cfg, hover: None }
    }

    /// Whether the corner `corner` of `outputs[index]` is a trap: the pointer cannot continue into
    /// another output past either edge.
    pub fn is_exposed(outputs: &[Rect], index: usize, corner: Corner) -> bool {
        let r = outputs[index];
        let c = corner.point(r);
        let (ox, oy) = corner.outward();
        let probe_h = Point::new(c.x + 0.5 * ox, c.y - 0.5 * oy);
        let probe_v = Point::new(c.x - 0.5 * ox, c.y + 0.5 * oy);
        !outputs
            .iter()
            .any(|o| o.contains(probe_h) || o.contains(probe_v))
    }

    fn locate(&self, pointer: Point, outputs: &[Rect]) -> Option<(usize, Corner)> {
        let s = self.cfg.size;
        for (i, r) in outputs.iter().enumerate() {
            if !r.contains(pointer) {
                continue;
            }
            for corner in Corner::ALL {
                let c = corner.point(*r);
                let (ox, oy) = corner.outward();
                let dx = if ox < 0.0 {
                    pointer.x - c.x
                } else {
                    c.x - pointer.x
                };
                let dy = if oy < 0.0 {
                    pointer.y - c.y
                } else {
                    c.y - pointer.y
                };
                if dx <= s
                    && dy <= s
                    && self.cfg.binding(corner).is_some()
                    && Self::is_exposed(outputs, i, corner)
                {
                    return Some((i, corner));
                }
            }
        }
        None
    }

    /// Feed the pointer position, the held modifiers and the output rectangles.
    pub fn update(
        &mut self,
        pointer: Point,
        modifiers: Modifiers,
        outputs: &[Rect],
        now: f64,
    ) -> Vec<CornerEvent> {
        let found = self.locate(pointer, outputs);
        let mut out = Vec::new();
        let same = matches!((found, self.hover), (Some((i, c)), Some(h)) if h.output == i && h.corner == c);
        if !same {
            if let Some(h) = self.hover.take() {
                if h.since.is_some() {
                    out.push(CornerEvent::Disarmed {
                        corner: h.corner,
                        output: h.output,
                    });
                }
            }
            if let Some((output, corner)) = found {
                self.hover = Some(Hover {
                    corner,
                    output,
                    since: None,
                    fired: false,
                });
            }
        }
        if let Some(h) = self.hover.as_mut() {
            let satisfied = self.cfg.bindings[h.corner.index()]
                .as_ref()
                .is_some_and(|b| modifiers.contains(b.modifiers));
            match (satisfied, h.since) {
                (true, None) => {
                    h.since = Some(now);
                    h.fired = false;
                    out.push(CornerEvent::Armed {
                        corner: h.corner,
                        output: h.output,
                    });
                }
                (false, Some(_)) => {
                    h.since = None;
                    out.push(CornerEvent::Disarmed {
                        corner: h.corner,
                        output: h.output,
                    });
                }
                _ => {}
            }
        }
        out.extend(self.tick(now));
        out
    }

    /// Fire the pending corner when the dwell has elapsed.
    pub fn tick(&mut self, now: f64) -> Vec<CornerEvent> {
        let mut out = Vec::new();
        if let Some(h) = self.hover.as_mut() {
            if let (Some(since), false) = (h.since, h.fired) {
                if now - since >= self.cfg.dwell - 1e-9 {
                    h.fired = true;
                    if let Some(b) = self.cfg.bindings[h.corner.index()].as_ref() {
                        out.push(CornerEvent::Triggered {
                            corner: h.corner,
                            output: h.output,
                            action: b.action.clone(),
                        });
                    }
                }
            }
        }
        out
    }

    /// Time at which [`Self::tick`] will fire, if a corner is waiting.
    pub fn next_deadline(&self) -> Option<f64> {
        self.hover.and_then(|h| match (h.since, h.fired) {
            (Some(s), false) => Some(s + self.cfg.dwell),
            _ => None,
        })
    }

    pub fn current(&self) -> Option<(usize, Corner)> {
        self.hover.map(|h| (h.output, h.corner))
    }
}
