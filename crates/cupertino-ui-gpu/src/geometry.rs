//! Where things land on the window's pixels, computed as the CPU renderer computes it: pixel bounds, transforms in
//! the rasterizer's `f32`, the path of a shape, a node's own extent and how far its layers' effects reach, and a
//! rectangle or rounded rectangle placed for its coverage. Both renderers must size layers and cut what they draw at
//! the same pixels, so these follow the CPU renderer's definitions exactly.

use cupertino_geometry::{Point, Rect};
use cupertino_raster_math::blur::sigma_for_radius;
use cupertino_ui_render::{Effect, PaintNode, PaintOp, PathVerb, Shape, Transform};
use tiny_skia::{Path, PathBuilder};

const KAPPA: f64 = 0.552_284_749_830_793_4;

/// Coordinates past this many pixels are not placed; the shape draws nothing.
const FAR: f32 = 1.0e7;

/// Largest deviation, in pixels, a layer's blur or shadow may have.
const MAX_SIGMA_PX: f32 = 4096.0;

/// A rectangle of physical pixels, `x1` and `y1` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Bounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Bounds {
    pub(crate) const EMPTY: Bounds = Bounds {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };

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

/// The inverse of `ts` as `[a, b, c, d, e, f]`, or `None` when it cannot be inverted.
pub(crate) fn inverse(ts: tiny_skia::Transform) -> Option<[f32; 6]> {
    let (a, b, c, d, e, f) = (ts.sx, ts.ky, ts.kx, ts.sy, ts.tx, ts.ty);
    let det = a * d - b * c;
    if !(det.abs() > 1e-12 && det.is_finite()) {
        return None;
    }
    let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
    Some([ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f)])
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
            fill_rule: cupertino_ui_render::FillRule::EvenOdd,
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

/// The pixels a placed path can touch: its bounds, a pixel out.
pub(crate) fn path_bounds(path: &Path, ts: tiny_skia::Transform) -> Bounds {
    let Some(moved) = path.clone().transform(ts) else {
        return Bounds::EMPTY;
    };
    let b = moved.bounds();
    Bounds {
        x0: b.left().floor() as i32 - 1,
        y0: b.top().floor() as i32 - 1,
        x1: b.right().ceil() as i32 + 1,
        y1: b.bottom().ceil() as i32 + 1,
    }
}

/// The pixels a clip shape under `node` can let through; empty for a shape with no area.
pub(crate) fn clip_bounds(shape: &Shape, node: &Transform, scale: f64) -> Bounds {
    match path(shape) {
        Some(p) => path_bounds(&p, to_target(node, scale, (0, 0))),
        None => Bounds::EMPTY,
    }
}

/// The pixels a node's own operations can reach, before effects; empty for a node with no ops that paint.
pub(crate) fn own_extent(node: &PaintNode, scale: f64) -> Bounds {
    let [a, b, c, d, e, f] = node.transform.0;
    let stretch = a.abs().max(b.abs()).max(c.abs()).max(d.abs());
    let mut extent: Option<Bounds> = None;
    let mut add = |bounds: Bounds| {
        extent = Some(extent.map_or(bounds, |old| Bounds {
            x0: old.x0.min(bounds.x0),
            y0: old.y0.min(bounds.y0),
            x1: old.x1.max(bounds.x1),
            y1: old.y1.max(bounds.y1),
        }));
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
    extent.unwrap_or(Bounds::EMPTY)
}

/// The deviation, in pixels, of the blur a toolkit `radius` (points) names at `scale`; zero below the smallest
/// deviation the kernel blurs at all, and held so that a pyramid of it stays small.
pub(crate) fn sigma_px(radius: f64, scale: f64) -> f32 {
    let sigma = (sigma_for_radius(radius as f32) as f64 * scale) as f32;
    if sigma.is_finite() && sigma >= 0.3 {
        sigma.min(MAX_SIGMA_PX)
    } else {
        0.0
    }
}

/// How far, in pixels, a blur of deviation `sigma` reaches: three deviations, and a texel of the last pyramid level
/// for the halvings and the resampling back to the layer.
fn reach(sigma: f32) -> i32 {
    if sigma <= 0.0 {
        return 0;
    }
    let mut var = sigma * sigma;
    let mut levels = 0;
    while var > 64.0 {
        var = (var - 0.25) / 4.0;
        levels += 1;
    }
    (3.0 * sigma).ceil() as i32 + (1 << levels)
}

/// How far, in pixels, an effect reaches beyond the content it is computed from.
pub(crate) fn support(effects: &[Effect], scale: f64) -> i32 {
    effects
        .iter()
        .map(|effect| match *effect {
            Effect::Blur { radius } => reach(sigma_px(radius, scale)),
            Effect::DropShadow { offset, blur, .. } | Effect::InnerShadow { offset, blur, .. } => {
                let shift = (offset.x.abs().max(offset.y.abs()) * scale)
                    .ceil()
                    .min(f64::from(1 << 20)) as i32;
                shift + reach(sigma_px(blur, scale))
            }
        })
        .fold(0, i32::saturating_add)
}

/// How far a node's layers' effects reach around its own operations.
pub(crate) fn node_support(node: &PaintNode, scale: f64) -> i32 {
    node.ops
        .iter()
        .map(|op| match op {
            PaintOp::PushLayer { effects, .. } => support(effects, scale),
            _ => 0,
        })
        .fold(0, i32::saturating_add)
}

/// A rectangle or rounded rectangle (or its stroke) placed on the window's pixels, for the coverage programs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Outline {
    /// Window pixel position to the shape's units, relative to its centre.
    pub inv: [f32; 6],
    pub half: [f32; 2],
    /// Corner radius in the shape's units; zero for square corners.
    pub corner: f32,
    /// Window pixels per unit of the shape.
    pub px: f32,
    /// Stroke width in the shape's units; `None` for a fill.
    pub stroke: Option<f32>,
    /// The window pixels the shape can touch.
    pub bounds: Bounds,
}

impl Outline {
    /// `shape` placed by `ts` (shape units to window pixels); `stroke` its width for a stroke. `None` for a path or a
    /// shape with no area.
    pub(crate) fn new(
        shape: &Shape,
        ts: tiny_skia::Transform,
        stroke: Option<f64>,
    ) -> Option<Outline> {
        let (rect, radius) = match shape {
            Shape::Rect(r) => (r, 0.0),
            Shape::RoundedRect { rect, radius } => (rect, *radius),
            Shape::Path { .. } => return None,
        };
        let (w, h) = (rect.width(), rect.height());
        if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
            return None;
        }
        let corner = if radius.is_finite() {
            radius.clamp(0.0, w.min(h) / 2.0)
        } else {
            0.0
        };
        let stroke = match stroke {
            Some(s) if !(s > 0.0 && s.is_finite()) => return None,
            s => s.map(|s| s as f32),
        };
        let (cx, cy) = (
            (rect.min_x() + w / 2.0) as f32,
            (rect.min_y() + h / 2.0) as f32,
        );
        let (a, b, c, d, e, f) = (ts.sx, ts.ky, ts.kx, ts.sy, ts.tx, ts.ty);
        let det = a * d - b * c;
        if !(det.abs() > 1e-12 && det.is_finite() && e.is_finite() && f.is_finite()) {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        let ie = -(ia * e + ic * f) - cx;
        let if_ = -(ib * e + id * f) - cy;
        let half = [(w / 2.0) as f32, (h / 2.0) as f32];
        let grow = stroke.map_or(0.0, |s| s / 2.0);
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let (x, y) = (cx + sx * (half[0] + grow), cy + sy * (half[1] + grow));
            let (wx, wy) = (a * x + c * y + e, b * x + d * y + f);
            lo = [lo[0].min(wx), lo[1].min(wy)];
            hi = [hi[0].max(wx), hi[1].max(wy)];
        }
        if ![lo[0], lo[1], hi[0], hi[1]].iter().all(|v| v.abs() <= FAR) {
            return None;
        }
        Some(Outline {
            inv: [ia, ib, ic, id, ie, if_],
            half,
            corner: corner as f32,
            px: det.abs().sqrt(),
            stroke,
            bounds: Bounds {
                x0: lo[0].floor() as i32 - 1,
                y0: lo[1].floor() as i32 - 1,
                x1: hi[0].ceil() as i32 + 1,
                y1: hi[1].ceil() as i32 + 1,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placed_rectangle_maps_its_window_pixels_to_its_centred_units() {
        let shape = Shape::Rect(Rect::new(10.0, 20.0, 30.0, 40.0));
        let o = Outline::new(&shape, tiny_skia::Transform::from_scale(2.0, 2.0), None).unwrap();
        assert_eq!(o.half, [15.0, 20.0]);
        assert_eq!(o.px, 2.0);
        // window pixel (50, 80) is the shape's point (25, 40), its centre
        let [a, b, c, d, e, f] = o.inv;
        assert_eq!(
            (a * 50.0 + c * 80.0 + e, b * 50.0 + d * 80.0 + f),
            (0.0, 0.0)
        );
        assert_eq!(
            o.bounds,
            Bounds {
                x0: 19,
                y0: 39,
                x1: 81,
                y1: 121
            }
        );
        assert!(Outline::new(&shape, tiny_skia::Transform::from_scale(0.0, 1.0), None).is_none());
    }

    #[test]
    fn effects_reach_three_deviations_and_a_texel_of_the_last_level() {
        assert_eq!(support(&[Effect::Blur { radius: 0.0 }], 1.0), 0);
        // radius 8 at scale 2: deviation 8 pixels, variance 64, no halving
        assert_eq!(support(&[Effect::Blur { radius: 8.0 }], 2.0), 24 + 1);
        let shadow = Effect::DropShadow {
            offset: Point { x: 0.0, y: 3.0 },
            blur: 6.0,
            color: cupertino_theme::Color::new(0.0, 0.0, 0.0, 0.35),
        };
        assert_eq!(support(&[shadow], 1.0), 3 + 9 + 1);
    }
}
