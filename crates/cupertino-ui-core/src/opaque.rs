//! The parts of a window its own pixels cover whole: its opaque region (`wl_surface.set_opaque_region`), from the
//! paint tree. Every paint operation lays its colour over what lies under it, so a pixel under a solid fill that is
//! fully opaque and covers it whole stays opaque whatever is painted over it. The region is the large fills of that
//! kind: of a solid colour at full alpha, rectangles or the cross of a rounded rectangle without its corners, outside
//! any translucent group or group with effects, under clips that are rectangles (or the cross of a rounded one), placed
//! by a transform that only moves. The window's materials that lie under its content are cut out, since its own pixels
//! leave them to show through. Each rectangle is pulled in by a point and rounded inwards to whole points, so the pixels
//! at its edges, which a fill at a fractional place covers only in part, are left out at any scale.

use crate::platform::MaterialRegion;
use cupertino_geometry::{Rect, Size};
use cupertino_ui_render::{DisplayList, Paint, PaintNodeId, PaintOp, Shape, Transform};

/// The shortest side a rectangle of the region may have, in points: smaller fills are left out, and the operations of
/// a node whose bounds are smaller are not looked at, so a frame of thousands of rows and cells looks only at the few
/// panes that cover most of a window.
const LEAST_SIDE: f64 = 128.0;
/// How many rectangles a window's region holds at most: the largest, so a window says what covers most of it in a
/// region the compositor can test quickly.
const MOST_RECTS: usize = 16;
/// How far each rectangle is pulled in on every side, in points: at a scale of 1 or more that leaves out the device
/// pixels a fill at a fractional place covers only in part.
const EDGE_INSET: f64 = 1.0;

/// The opaque region of a window `size` points large that paints `list`, with `materials` under parts of it.
pub(crate) fn region(list: &DisplayList, size: Size, materials: &[MaterialRegion]) -> Vec<Rect> {
    let window = Rect::new(0.0, 0.0, size.width, size.height);
    let mut found = Vec::new();
    // the nodes above the one being looked at, each with the clip it leaves its descendants (in window points; `None`
    // for no clip) and whether its descendants may count at all
    let mut stack: Vec<(PaintNodeId, Option<Rect>, bool)> = Vec::new();
    let mut clips: Vec<Option<Rect>> = Vec::new();
    for node in &list.nodes {
        while stack
            .last()
            .is_some_and(|(id, ..)| Some(*id) != node.parent)
        {
            stack.pop();
        }
        let (clip, counts) = match stack.last() {
            Some((_, clip, counts)) => (*clip, *counts),
            None => (Some(window), true),
        };
        let moves = moves_only(&node.transform);
        let counts = counts
            && moves.is_some()
            && node.group.is_none()
            && node.bounds.width() >= LEAST_SIDE
            && node.bounds.height() >= LEAST_SIDE;
        let Some((dx, dy)) = moves.filter(|_| counts) else {
            stack.push((node.id, clip, false));
            continue;
        };
        let place = |r: Rect| r.translate(dx, dy);
        // the node's own operations, under its ancestors' clips and the clips they push themselves
        clips.clear();
        clips.push(clip);
        let mut groups = 0usize;
        for op in &node.ops {
            match op {
                PaintOp::PushClip(shape) => {
                    let inner = inside(shape).map(place);
                    let top = *clips.last().expect("the ancestors' clip stays");
                    clips.push(meet(top, inner));
                }
                PaintOp::PopClip if clips.len() > 1 => {
                    clips.pop();
                }
                PaintOp::PushLayer { .. } => groups += 1,
                PaintOp::PopLayer => groups = groups.saturating_sub(1),
                PaintOp::Fill {
                    shape,
                    paint: Paint::Solid(color),
                } if groups == 0 && color.a >= 1.0 => {
                    let top = *clips.last().expect("the ancestors' clip stays");
                    for r in cross(shape).into_iter().flatten() {
                        if let Some(r) = top.and_then(|c| c.intersection(&place(r))) {
                            if r.width() >= LEAST_SIDE && r.height() >= LEAST_SIDE {
                                found.push(r);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let own = node.clip.as_ref().map(|shape| inside(shape).map(place));
        let below = match own {
            Some(inner) => meet(clip, inner),
            None => clip,
        };
        stack.push((node.id, below, below.is_some()));
    }
    let mut out: Vec<Rect> = found.into_iter().filter_map(inset).collect();
    for m in materials.iter().filter(|m| !m.raised && m.alpha > 0.0) {
        out = out.into_iter().flat_map(|r| subtract(r, m.rect)).collect();
    }
    out.retain(|r| r.width() > 0.0 && r.height() > 0.0);
    // the largest first, and one held whole by a larger one adds nothing
    out.sort_by(|a, b| area(b).total_cmp(&area(a)));
    let mut kept: Vec<Rect> = Vec::new();
    for r in out {
        if kept.len() == MOST_RECTS {
            break;
        }
        if !kept.iter().any(|k| holds(k, &r)) {
            kept.push(r);
        }
    }
    kept
}

/// The move of a transform that only moves, or `None` for one that scales, turns or skews.
fn moves_only(t: &Transform) -> Option<(f64, f64)> {
    let [a, b, c, d, e, f] = t.0;
    ((a, b, c, d) == (1.0, 0.0, 0.0, 1.0) && e.is_finite() && f.is_finite()).then_some((e, f))
}

/// The rectangle a clip of `shape` shows whole: a rectangle itself, a rounded one less its corners on both axes (the
/// largest rectangle both bars of its cross hold), and none for a superellipse or a path.
fn inside(shape: &Shape) -> Option<Rect> {
    match shape {
        Shape::Rect(r) => Some(*r),
        Shape::RoundedRect { rect, radius } => {
            let r = corner(rect, *radius);
            inset_by(*rect, r, r)
        }
        Shape::Superellipse { .. } | Shape::Path { .. } => None,
    }
}

/// The rectangles a fill of `shape` covers whole: a rectangle, or the two bars of a rounded rectangle's cross; none
/// for a path or a fill too small to count.
fn cross(shape: &Shape) -> [Option<Rect>; 2] {
    match shape {
        Shape::Rect(r) => [Some(*r), None],
        Shape::RoundedRect { rect, radius }
            if rect.width() >= LEAST_SIDE && rect.height() >= LEAST_SIDE =>
        {
            let r = corner(rect, *radius);
            [inset_by(*rect, r, 0.0), inset_by(*rect, 0.0, r)]
        }
        _ => [None, None],
    }
}

/// The corner radius a rounded rectangle is drawn with: no more than half its shorter side.
fn corner(rect: &Rect, radius: f64) -> f64 {
    if radius.is_finite() {
        radius.clamp(0.0, rect.width().min(rect.height()) / 2.0)
    } else {
        0.0
    }
}

/// `r` pulled in by `x` on its left and right and by `y` on its top and bottom, if anything is left.
fn inset_by(r: Rect, x: f64, y: f64) -> Option<Rect> {
    let (w, h) = (r.width() - 2.0 * x, r.height() - 2.0 * y);
    (w > 0.0 && h > 0.0).then(|| Rect::new(r.min_x() + x, r.min_y() + y, w, h))
}

/// Two clips met: what both show, `None` standing for showing nothing.
fn meet(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    a.zip(b).and_then(|(a, b)| a.intersection(&b))
}

/// `r` pulled in by [`EDGE_INSET`] and rounded inwards to whole points.
fn inset(r: Rect) -> Option<Rect> {
    let (x0, y0) = (
        (r.min_x() + EDGE_INSET).ceil(),
        (r.min_y() + EDGE_INSET).ceil(),
    );
    let (x1, y1) = (
        (r.max_x() - EDGE_INSET).floor(),
        (r.max_y() - EDGE_INSET).floor(),
    );
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// What of `r` lies outside `hole`: up to four rectangles, rounded outwards from the hole to whole points.
fn subtract(r: Rect, hole: Rect) -> Vec<Rect> {
    let hole = Rect::new(
        hole.min_x().floor(),
        hole.min_y().floor(),
        hole.max_x().ceil() - hole.min_x().floor(),
        hole.max_y().ceil() - hole.min_y().floor(),
    );
    let Some(cut) = r.intersection(&hole) else {
        return vec![r];
    };
    let mut out = Vec::with_capacity(4);
    let mut push = |x0: f64, y0: f64, x1: f64, y1: f64| {
        if x1 > x0 && y1 > y0 {
            out.push(Rect::new(x0, y0, x1 - x0, y1 - y0));
        }
    };
    push(r.min_x(), r.min_y(), r.max_x(), cut.min_y());
    push(r.min_x(), cut.max_y(), r.max_x(), r.max_y());
    push(r.min_x(), cut.min_y(), cut.min_x(), cut.max_y());
    push(cut.max_x(), cut.min_y(), r.max_x(), cut.max_y());
    out
}

fn area(r: &Rect) -> f64 {
    r.width() * r.height()
}

/// Whether `outer` holds all of `inner`.
fn holds(outer: &Rect, inner: &Rect) -> bool {
    outer.min_x() <= inner.min_x()
        && outer.min_y() <= inner.min_y()
        && outer.max_x() >= inner.max_x()
        && outer.max_y() >= inner.max_y()
}
