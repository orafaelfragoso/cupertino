use crate::{PathVerb, Shape, Transform};
use cupertino_geometry::{Point, Rect};
use tiny_skia::{Path, PathBuilder};

const KAPPA: f64 = 0.552_284_749_830_793_4;

/// A rectangle of physical pixels, `x1` and `y1` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Bounds {
    pub(crate) fn of_size(width: u32, height: u32) -> Bounds {
        Bounds {
            x0: 0,
            y0: 0,
            x1: width as i32,
            y1: height as i32,
        }
    }

    /// The pixels a logical rectangle touches, antialias fringe included.
    pub(crate) fn of_rect(rect: &Rect, scale: f64) -> Bounds {
        let cut = |v: f64| v.clamp(i32::MIN as f64 / 2.0, i32::MAX as f64 / 2.0);
        Bounds {
            x0: cut((rect.min_x() * scale).floor()) as i32 - 1,
            y0: cut((rect.min_y() * scale).floor()) as i32 - 1,
            x1: cut((rect.max_x() * scale).ceil()) as i32 + 1,
            y1: cut((rect.max_y() * scale).ceil()) as i32 + 1,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub(crate) fn width(&self) -> u32 {
        (self.x1 - self.x0).max(0) as u32
    }

    pub(crate) fn height(&self) -> u32 {
        (self.y1 - self.y0).max(0) as u32
    }

    pub(crate) fn intersect(&self, other: &Bounds) -> Bounds {
        Bounds {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }

    pub(crate) fn touches(&self, other: &Bounds) -> bool {
        !self.intersect(other).is_empty()
    }

    pub(crate) fn union(&self, other: &Bounds) -> Bounds {
        Bounds {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    pub(crate) fn outset(&self, by: i32) -> Bounds {
        Bounds {
            x0: self.x0.saturating_sub(by),
            y0: self.y0.saturating_sub(by),
            x1: self.x1.saturating_add(by),
            y1: self.y1.saturating_add(by),
        }
    }

    pub(crate) fn offset(&self, dx: i32, dy: i32) -> Bounds {
        Bounds {
            x0: self.x0.saturating_add(dx),
            y0: self.y0.saturating_add(dy),
            x1: self.x1.saturating_add(dx),
            y1: self.y1.saturating_add(dy),
        }
    }
}

/// Merges overlapping or touching rectangles until none remain, then orders the rest by row and column.
pub(crate) fn merge(mut rects: Vec<Bounds>) -> Vec<Bounds> {
    rects.retain(|r| !r.is_empty());
    loop {
        let pair = (0..rects.len()).find_map(|i| {
            (i + 1..rects.len())
                .find(|&j| rects[i].outset(1).touches(&rects[j]))
                .map(|j| (i, j))
        });
        let Some((i, j)) = pair else { break };
        let other = rects.swap_remove(j);
        rects[i] = rects[i].union(&other);
    }
    rects.sort_by_key(|r| (r.y0, r.x0, r.y1, r.x1));
    rects
}

pub(crate) fn transform(t: &Transform) -> tiny_skia::Transform {
    let [a, b, c, d, e, f] = t.0;
    tiny_skia::Transform::from_row(a as f32, b as f32, c as f32, d as f32, e as f32, f as f32)
}

/// Maps node coordinates to the pixels of a target whose top-left is at `origin`.
pub(crate) fn to_target(node: &Transform, scale: f64, origin: (i32, i32)) -> tiny_skia::Transform {
    tiny_skia::Transform::from_translate(-origin.0 as f32, -origin.1 as f32)
        .pre_concat(tiny_skia::Transform::from_scale(scale as f32, scale as f32))
        .pre_concat(transform(node))
}

pub(crate) fn path(shape: &Shape) -> Option<Path> {
    let mut b = PathBuilder::new();
    match shape {
        Shape::Rect(r) => {
            b.move_to(r.min_x() as f32, r.min_y() as f32);
            b.line_to(r.max_x() as f32, r.min_y() as f32);
            b.line_to(r.max_x() as f32, r.max_y() as f32);
            b.line_to(r.min_x() as f32, r.max_y() as f32);
            b.close();
        }
        Shape::RoundedRect { rect, radius } => rounded(&mut b, rect, *radius),
        Shape::Path { verbs, .. } => {
            for verb in verbs {
                match *verb {
                    PathVerb::MoveTo(p) => b.move_to(p.x as f32, p.y as f32),
                    PathVerb::LineTo(p) => b.line_to(p.x as f32, p.y as f32),
                    PathVerb::QuadTo(c, p) => {
                        b.quad_to(c.x as f32, c.y as f32, p.x as f32, p.y as f32)
                    }
                    PathVerb::CubicTo(c1, c2, p) => b.cubic_to(
                        c1.x as f32,
                        c1.y as f32,
                        c2.x as f32,
                        c2.y as f32,
                        p.x as f32,
                        p.y as f32,
                    ),
                    PathVerb::Close => b.close(),
                }
            }
        }
    }
    b.finish()
}

pub(crate) fn fill_rule(shape: &Shape) -> tiny_skia::FillRule {
    match shape {
        Shape::Path {
            fill_rule: crate::FillRule::EvenOdd,
            ..
        } => tiny_skia::FillRule::EvenOdd,
        _ => tiny_skia::FillRule::Winding,
    }
}

fn rounded(b: &mut PathBuilder, rect: &Rect, radius: f64) {
    let r = radius.clamp(0.0, rect.width().min(rect.height()) / 2.0);
    let (l, t, rt, bt) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let k = r * (1.0 - KAPPA);
    let p = |x: f64, y: f64| Point { x, y };
    b.move_to((l + r) as f32, t as f32);
    b.line_to((rt - r) as f32, t as f32);
    cubic(b, p(rt - k, t), p(rt, t + k), p(rt, t + r));
    b.line_to(rt as f32, (bt - r) as f32);
    cubic(b, p(rt, bt - k), p(rt - k, bt), p(rt - r, bt));
    b.line_to((l + r) as f32, bt as f32);
    cubic(b, p(l + k, bt), p(l, bt - k), p(l, bt - r));
    b.line_to(l as f32, (t + r) as f32);
    cubic(b, p(l, t + k), p(l + k, t), p(l + r, t));
    b.close();
}

fn cubic(b: &mut PathBuilder, c1: Point, c2: Point, to: Point) {
    b.cubic_to(
        c1.x as f32,
        c1.y as f32,
        c2.x as f32,
        c2.y as f32,
        to.x as f32,
        to.y as f32,
    );
}

/// The pixels a clip shape under `node` can let through; empty for a shape with no area.
pub(crate) fn clip_bounds(shape: &Shape, node: &Transform, scale: f64) -> Bounds {
    let empty = Bounds {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };
    let Some(path) = path(shape) else {
        return empty;
    };
    let Some(moved) = path.transform(to_target(node, scale, (0, 0))) else {
        return empty;
    };
    let b = moved.bounds();
    Bounds {
        x0: b.left().floor() as i32 - 1,
        y0: b.top().floor() as i32 - 1,
        x1: b.right().ceil() as i32 + 1,
        y1: b.bottom().ceil() as i32 + 1,
    }
}

/// Whether the clip `shape` under `node` lets every pixel of `area` through whole (its mask is fully opaque there), so
/// it cuts nothing of what is drawn in `area`. Only rectangles and rounded rectangles under a transform that only
/// scales and moves are judged; for a rounded rectangle the area must stay clear of the corners' squares. A pixel
/// is counted whole only one pixel inside the outline, which leaves room for the rounding of the rasteriser.
pub(crate) fn clip_holds(shape: &Shape, node: &Transform, scale: f64, area: &Bounds) -> bool {
    let [a, b, c, d, e, f] = node.0;
    if b != 0.0 || c != 0.0 || a <= 0.0 || d <= 0.0 || area.is_empty() {
        return false;
    }
    let (rect, radius) = match shape {
        Shape::Rect(rect) => (rect, 0.0),
        Shape::RoundedRect { rect, radius } => (
            rect,
            radius.clamp(0.0, rect.width().min(rect.height()) / 2.0),
        ),
        Shape::Path { .. } => return false,
    };
    if !(rect.width() > 0.0 && rect.height() > 0.0) {
        return false;
    }
    let (sx, sy) = (a * scale, d * scale);
    let (x0, x1) = (
        (rect.min_x() * a + e) * scale,
        (rect.max_x() * a + e) * scale,
    );
    let (y0, y1) = (
        (rect.min_y() * d + f) * scale,
        (rect.max_y() * d + f) * scale,
    );
    // the whole pixels between `lo` and `hi`, one pixel in from each
    let inside = |lo: f64, hi: f64, from: i32, to: i32| {
        f64::from(from) >= lo.ceil() + 1.0 && f64::from(to) <= hi.floor() - 1.0
    };
    let (rx, ry) = (radius * sx, radius * sy);
    let across = inside(x0, x1, area.x0, area.x1);
    let down = inside(y0, y1, area.y0, area.y1);
    // the rectangle less its corners is a cross: a band the full height between the corners, or the full width
    (across && inside(y0 + ry, y1 - ry, area.y0, area.y1))
        || (down && inside(x0 + rx, x1 - rx, area.x0, area.x1))
}

/// The pixels a node's own operations can reach, before effects; empty for a node with no ops that paint.
pub(crate) fn own_extent(node: &crate::PaintNode, scale: f64) -> Bounds {
    use crate::PaintOp;
    let none = Bounds {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };
    let [a, b, c, d, e, f] = node.transform.0;
    let stretch = a.abs().max(b.abs()).max(c.abs()).max(d.abs());
    let mut extent: Option<Bounds> = None;
    let mut add = |bounds: Bounds| {
        extent = Some(extent.map_or(bounds, |old| old.union(&bounds)));
    };
    for op in &node.ops {
        match op {
            PaintOp::Fill { shape, .. } => add(clip_bounds(shape, &node.transform, scale)),
            PaintOp::Stroke { shape, width, .. } => add(clip_bounds(shape, &node.transform, scale)
                .outset((width * scale * stretch / 2.0).ceil() as i32)),
            PaintOp::Image { dst, .. } => {
                add(clip_bounds(&Shape::Rect(*dst), &node.transform, scale))
            }
            PaintOp::Glyphs { origin, run, .. } => {
                let em = (f64::from(run.size) * scale * stretch).ceil() as i32;
                for g in &run.glyphs {
                    let (lx, ly) = (origin.x + f64::from(g.x), origin.y + f64::from(g.y));
                    let px = ((a * lx + c * ly + e) * scale).floor() as i32;
                    let py = ((b * lx + d * ly + f) * scale).floor() as i32;
                    add(Bounds {
                        x0: px - em,
                        y0: py - 2 * em,
                        x1: px + 2 * em,
                        y1: py + em,
                    });
                }
            }
            _ => {}
        }
    }
    extent.unwrap_or(none)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds {
        Bounds { x0, y0, x1, y1 }
    }

    #[test]
    fn a_clip_holds_an_area_inside_its_whole_pixels_and_clear_of_its_corners() {
        let id = Transform::IDENTITY;
        let rect = Shape::Rect(Rect::new(10.0, 10.0, 100.0, 50.0));
        // at scale 2 the rectangle covers pixels 20..220 by 20..120; one pixel is kept in from its outline
        assert!(clip_holds(&rect, &id, 2.0, &px(21, 21, 219, 119)));
        assert!(!clip_holds(&rect, &id, 2.0, &px(20, 21, 219, 119)));
        assert!(!clip_holds(&rect, &id, 2.0, &px(21, 21, 220, 119)));
        // a rounded rectangle lets through a band between its corners, across or down, not a corner's square
        let rounded = Shape::RoundedRect {
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            radius: 20.0,
        };
        assert!(clip_holds(&rounded, &id, 1.0, &px(1, 22, 99, 78)));
        assert!(clip_holds(&rounded, &id, 1.0, &px(22, 1, 78, 99)));
        assert!(!clip_holds(&rounded, &id, 1.0, &px(1, 1, 30, 30)));
        // a moved and scaled node is judged where it lands
        let moved = Transform([2.0, 0.0, 0.0, 2.0, 50.0, 0.0]);
        assert!(clip_holds(&rect, &moved, 1.0, &px(71, 21, 249, 119)));
        assert!(!clip_holds(&rect, &moved, 1.0, &px(60, 21, 249, 119)));
        // turned nodes, paths and empty areas are never judged held
        let turned = Transform([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
        assert!(!clip_holds(&rect, &turned, 1.0, &px(21, 21, 22, 22)));
        let path = Shape::Path {
            verbs: Vec::new(),
            fill_rule: crate::FillRule::NonZero,
        };
        assert!(!clip_holds(&path, &id, 1.0, &px(21, 21, 22, 22)));
        assert!(!clip_holds(&rect, &id, 1.0, &px(30, 30, 30, 30)));
    }
}
