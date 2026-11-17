//! Screen-edge dwell: reveal an auto-hidden surface (dock, fullscreen menu bar) when the pointer
//! rests against an output edge, and hide it again once the pointer has left it.
//!
//! The dock's reveal delay is as designed (0.15 s). The fullscreen menu bar's
//! reveal and hide delays are unknown: [`EdgeConfig::menu_bar`] uses the dock value as a
//! placeholder and every field is configurable. The edge thickness is approximate.

use crate::geom::{Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub struct EdgeConfig {
    pub edge: Edge,
    /// Distance from the output edge that counts as touching it. approximate.
    pub thickness: f64,
    /// Seconds the pointer must rest on the edge before the reveal.
    pub reveal_delay: f64,
    /// Seconds the pointer must stay outside the hold region before the hide.
    pub hide_delay: f64,
}

impl EdgeConfig {
    /// Dock on a bottom edge: reveal delay, hide delay approximate.
    pub fn dock(edge: Edge) -> Self {
        Self {
            edge,
            thickness: 1.0,
            reveal_delay: 0.15,
            hide_delay: 0.0,
        }
    }

    /// Fullscreen menu bar on the top edge: delays unknown placeholders.
    pub fn menu_bar() -> Self {
        Self {
            edge: Edge::Top,
            thickness: 1.0,
            reveal_delay: 0.15,
            hide_delay: 0.3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeEvent {
    Reveal { output: usize },
    Hide { output: usize },
}

#[derive(Clone, Copy, Debug)]
enum State {
    Hidden,
    Dwelling {
        output: usize,
        since: f64,
    },
    Shown {
        output: usize,
        outside_since: Option<f64>,
    },
}

pub struct EdgeReveal {
    pub cfg: EdgeConfig,
    state: State,
    hold: Option<Rect>,
}

impl EdgeReveal {
    pub fn new(cfg: EdgeConfig) -> Self {
        Self {
            cfg,
            state: State::Hidden,
            hold: None,
        }
    }

    /// Region that keeps the surface shown while the pointer is inside (the revealed surface).
    pub fn set_hold_region(&mut self, region: Option<Rect>) {
        self.hold = region;
    }

    pub fn is_shown(&self) -> bool {
        matches!(self.state, State::Shown { .. })
    }

    fn on_edge(&self, p: Point, outputs: &[Rect]) -> Option<usize> {
        let t = self.cfg.thickness;
        outputs.iter().enumerate().find_map(|(i, r)| {
            if !r.contains(p) {
                return None;
            }
            let (touching, probe) = match self.cfg.edge {
                Edge::Top => (p.y - r.y < t, Point::new(p.x, r.y - 0.5)),
                Edge::Bottom => (r.bottom() - p.y <= t, Point::new(p.x, r.bottom() + 0.5)),
                Edge::Left => (p.x - r.x < t, Point::new(r.x - 0.5, p.y)),
                Edge::Right => (r.right() - p.x <= t, Point::new(r.right() + 0.5, p.y)),
            };
            (touching && !outputs.iter().any(|o| o.contains(probe))).then_some(i)
        })
    }

    pub fn update(&mut self, pointer: Point, outputs: &[Rect], now: f64) -> Option<EdgeEvent> {
        let on_edge = self.on_edge(pointer, outputs);
        let in_hold = self.hold.is_some_and(|h| h.contains(pointer));
        match self.state {
            State::Hidden => {
                if let Some(output) = on_edge {
                    self.state = State::Dwelling { output, since: now };
                    return self.tick(now);
                }
            }
            State::Dwelling { output, .. } => match on_edge {
                Some(o) if o == output => return self.tick(now),
                Some(o) => {
                    self.state = State::Dwelling {
                        output: o,
                        since: now,
                    }
                }
                None => self.state = State::Hidden,
            },
            State::Shown {
                output,
                outside_since,
            } => {
                let inside = on_edge == Some(output) || in_hold;
                self.state = State::Shown {
                    output,
                    outside_since: if inside {
                        None
                    } else {
                        outside_since.or(Some(now))
                    },
                };
                return self.tick(now);
            }
        }
        None
    }

    pub fn tick(&mut self, now: f64) -> Option<EdgeEvent> {
        match self.state {
            State::Dwelling { output, since } if now - since >= self.cfg.reveal_delay - 1e-9 => {
                self.state = State::Shown {
                    output,
                    outside_since: None,
                };
                Some(EdgeEvent::Reveal { output })
            }
            State::Shown {
                output,
                outside_since: Some(s),
            } if now - s >= self.cfg.hide_delay - 1e-9 => {
                self.state = State::Hidden;
                Some(EdgeEvent::Hide { output })
            }
            _ => None,
        }
    }

    pub fn next_deadline(&self) -> Option<f64> {
        match self.state {
            State::Dwelling { since, .. } => Some(since + self.cfg.reveal_delay),
            State::Shown {
                outside_since: Some(s),
                ..
            } => Some(s + self.cfg.hide_delay),
            _ => None,
        }
    }
}
