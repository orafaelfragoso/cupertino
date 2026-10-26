//! Magnetic window edges: while a window is moved or resized, its edges stick to guide edges
//! (other windows and the usable screen rectangle). Pure and time-explicit: feed every pointer
//! sample with its timestamp to [`Magnet::step`].
//! ,
//! per window edge against each parallel guide edge:
//! * a free edge snaps in a step that reaches or crosses the guide when its distance before the
//!   step was below `snap_distance` (30 pt), its speed is at most `snap_velocity` (400 pt/s) and
//!   the window overlaps the guide along the other axis, within `overlap_margin` (1 pt);
//! * a snapped edge lets go when its speed exceeds `unsnap_velocity` (400 pt/s), when the pointer
//!   has pulled farther than `35 + 105·(θ/60°)²` (140 pt from 60° on) since the snap, θ being the
//!   angle between the drag heading and the edge normal, or at once when the heading is within 15°
//!   of the normal and points away from the guide;
//! * facing edges of two windows stay `window_gap` (1 pt) apart.
//!
//! the edge speed is the pointer velocity along the edge normal; the pull is the
//! pointer's straight-line travel since the snap; "away from the guide" is towards the window's
//! own body; after an unsnap the axis may snap again only once that edge has travelled
//! `resnap_distance` (55 pt) from the guide or its velocity along the axis changed sign
//! (the resnap check of); the screen rectangle offers its edges from the inside only.

use crate::config::MagnetConfig;
use crate::geometry::{Edges, Point, Rect};

/// A rectangle whose edges attract window edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Guide {
    /// Another window: a window edge snaps to the facing edge (keeping the window gap) and to the
    /// aligned edge.
    Window(Rect),
    /// The visible piece `rect` of a partly covered window ([`crate::pieces`]): like a window, but
    /// only along `edges`, the sides that are edges of the window itself.
    Piece { rect: Rect, edges: Edges },
    /// The usable screen rectangle: a window edge snaps to the screen edge on the same side.
    Screen(Rect),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn pos(self, r: &Rect) -> f64 {
        match self {
            Side::Left => r.x,
            Side::Right => r.right(),
            Side::Top => r.y,
            Side::Bottom => r.bottom(),
        }
    }

    fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }

    /// +1 when the window's body lies at larger coordinates than this edge.
    fn inward(self) -> f64 {
        match self {
            Side::Left | Side::Top => 1.0,
            Side::Right | Side::Bottom => -1.0,
        }
    }

    fn horizontal(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }

    /// Extent of `r` along the axis parallel to this edge.
    fn span(self, r: &Rect) -> (f64, f64) {
        if self.horizontal() {
            (r.y, r.bottom())
        } else {
            (r.x, r.right())
        }
    }

    fn active(self, e: Edges) -> bool {
        match self {
            Side::Left => e.left,
            Side::Right => e.right,
            Side::Top => e.top,
            Side::Bottom => e.bottom,
        }
    }
}

/// A guide edge: position along the edge normal and extent along the edge.
#[derive(Clone, Copy, Debug)]
struct Line {
    pos: f64,
    span: (f64, f64),
}

/// Guide edges a window edge on `side` can snap to.
fn lines(guides: &[Guide], side: Side, gap: f64) -> Vec<Line> {
    let mut out = Vec::new();
    for g in guides {
        match g {
            Guide::Window(r) => {
                out.push(Line { pos: side.opposite().pos(r) + side.inward() * gap, span: side.span(r) });
                out.push(Line { pos: side.pos(r), span: side.span(r) });
            }
            Guide::Piece { rect: r, edges } => {
                if side.opposite().active(*edges) {
                    out.push(Line { pos: side.opposite().pos(r) + side.inward() * gap, span: side.span(r) });
                }
                if side.active(*edges) {
                    out.push(Line { pos: side.pos(r), span: side.span(r) });
                }
            }
            Guide::Screen(r) => out.push(Line { pos: side.pos(r), span: side.span(r) }),
        }
    }
    out
}

fn overlaps(window: (f64, f64), line: (f64, f64), margin: f64) -> bool {
    window.0 <= line.1 + margin && line.0 - margin <= window.1
}

/// Pull needed to unsnap at heading angle `theta` (degrees from the edge normal).
pub fn unsnap_distance(cfg: &MagnetConfig, theta: f64) -> f64 {
    if theta < cfg.unsnap_max_angle {
        cfg.unsnap_distance_min + (cfg.unsnap_distance_max - cfg.unsnap_distance_min) * (theta / cfg.unsnap_max_angle).powi(2)
    } else {
        cfg.unsnap_distance_max
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Snap {
    side: Side,
    line: f64,
    /// Pointer position when the edge snapped.
    pointer: Point,
}

/// Blocks re-snapping on an axis right after an unsnap.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Guard {
    side: Side,
    line: f64,
    velocity: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Axis {
    snap: Option<Snap>,
    guard: Option<Guard>,
}

/// Result of one pointer sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagnetStep {
    /// Frame after snapping.
    pub frame: Rect,
    /// An edge snapped or unsnapped in this step, so the window jumped relative to the pointer.
    pub jumped: bool,
}

/// Snapping state of one move or resize interaction.
#[derive(Clone, Debug)]
pub struct Magnet {
    pointer: Point,
    raw: Rect,
    time: f64,
    x: Axis,
    y: Axis,
}

impl Magnet {
    /// Start tracking at `pointer` with the window at `raw` (no edge snapped).
    pub fn new(pointer: Point, raw: Rect, now: f64) -> Self {
        Self { pointer, raw, time: now, x: Axis::default(), y: Axis::default() }
    }

    /// Sides currently held by a guide.
    pub fn snapped(&self) -> Edges {
        let mut e = Edges::NONE;
        for s in [self.x.snap, self.y.snap].into_iter().flatten() {
            match s.side {
                Side::Left => e.left = true,
                Side::Right => e.right = true,
                Side::Top => e.top = true,
                Side::Bottom => e.bottom = true,
            }
        }
        e
    }

    /// Advance to the pointer sample at `now`. `raw` is the frame the pointer alone asks for;
    /// `sides` are the edges that move with the pointer (all four for a move, the dragged ones for
    /// a resize). With both edges of an axis moving the window is translated, otherwise the moving
    /// edge is placed on the guide. `may_snap = false` keeps free edges free (a catch-up animation
    /// is running); `enabled = false` releases everything (snapping is switched off or Alt is held).
    /// Samples with the same timestamp count as infinitely fast.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        cfg: &MagnetConfig,
        pointer: Point,
        raw: Rect,
        sides: Edges,
        guides: &[Guide],
        enabled: bool,
        may_snap: bool,
        now: f64,
    ) -> MagnetStep {
        let (prev_raw, prev_pointer, dt) = (self.raw, self.pointer, now - self.time);
        self.pointer = pointer;
        self.raw = raw;
        self.time = now;
        let was = self.snapped();
        if !enabled {
            self.x = Axis::default();
            self.y = Axis::default();
            return MagnetStep { frame: raw, jumped: was.any() };
        }
        let (dx, dy) = (pointer.x - prev_pointer.x, pointer.y - prev_pointer.y);
        let velocity = |d: f64| if dt > 0.0 { d / dt } else if d == 0.0 { 0.0 } else { d.signum() * f64::INFINITY };
        let (vx, vy) = (velocity(dx), velocity(dy));

        let mut frame = raw;
        for (axis, sides_of_axis, d_axis, d_perp, v) in [
            (&mut self.x, [Side::Left, Side::Right], dx, dy, vx),
            (&mut self.y, [Side::Top, Side::Bottom], dy, dx, vy),
        ] {
            if let Some(s) = axis.snap {
                let present = lines(guides, s.side, cfg.window_gap).iter().any(|l| l.pos == s.line);
                let moved = d_axis != 0.0 || d_perp != 0.0;
                let theta = d_perp.abs().atan2(d_axis.abs()).to_degrees();
                let pull = ((pointer.x - s.pointer.x).powi(2) + (pointer.y - s.pointer.y).powi(2)).sqrt();
                let release = v.abs() > cfg.unsnap_velocity
                    || (moved && pull > unsnap_distance(cfg, theta))
                    || (cfg.instant_unsnap && moved && theta <= cfg.instant_unsnap_angle && d_axis * s.side.inward() > 0.0);
                if !present {
                    axis.snap = None;
                } else if release {
                    axis.snap = None;
                    axis.guard = Some(Guard { side: s.side, line: s.line, velocity: v });
                }
            } else if may_snap {
                if let Some(g) = axis.guard {
                    let travelled = (g.side.pos(&raw) - g.line).abs();
                    if travelled >= cfg.resnap_distance || (v != 0.0 && v.signum() != g.velocity.signum()) {
                        axis.guard = None;
                    }
                }
                if axis.guard.is_none() && v.abs() <= cfg.snap_velocity {
                    let mut best: Option<(f64, Snap)> = None;
                    for side in sides_of_axis.into_iter().filter(|s| s.active(sides)) {
                        let (e0, e1) = (side.pos(&prev_raw), side.pos(&raw));
                        for l in lines(guides, side, cfg.window_gap) {
                            let (d0, d1) = (e0 - l.pos, e1 - l.pos);
                            let crossed = d0 != 0.0 && (d1 == 0.0 || d0.signum() != d1.signum());
                            if crossed
                                && d0.abs() < cfg.snap_distance
                                && overlaps(side.span(&frame), l.span, cfg.overlap_margin)
                                && best.is_none_or(|(d, _)| d0.abs() < d)
                            {
                                best = Some((d0.abs(), Snap { side, line: l.pos, pointer }));
                            }
                        }
                    }
                    axis.snap = best.map(|(_, s)| s);
                }
            }
            if let Some(s) = axis.snap {
                frame = place(frame, s.side, s.line, sides_of_axis.iter().all(|x| x.active(sides)));
            }
        }
        MagnetStep { frame, jumped: self.snapped() != was }
    }
}

/// Put edge `side` of `r` on `line`: translate when both edges of the axis move, otherwise move
/// only that edge.
fn place(r: Rect, side: Side, line: f64, translate: bool) -> Rect {
    let d = line - side.pos(&r);
    match (side, translate) {
        (Side::Left | Side::Right, true) => r.translate(d, 0.0),
        (Side::Top | Side::Bottom, true) => r.translate(0.0, d),
        (Side::Left, false) => Rect::new(line, r.y, r.w - d, r.h),
        (Side::Right, false) => Rect::new(r.x, r.y, r.w + d, r.h),
        (Side::Top, false) => Rect::new(r.x, line, r.w, r.h - d),
        (Side::Bottom, false) => Rect::new(r.x, r.y, r.w, r.h + d),
    }
}
