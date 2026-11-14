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

/// The most separate rectangles a frame repaints. Every rectangle walks the whole display list, so a damage of many
/// small pieces (each item of a scrolled grid, say) costs the list's length times their number; past this many, the
/// pieces closest together are repainted as one rectangle around both.
pub(crate) const MAX_DAMAGE_RECTS: usize = 16;

/// The most rectangles the touching pass pairs up, each against each. Past this many, the rectangles are first gathered
/// into the cells of a [`GATHER_CELLS`]-square grid over them, one rectangle around each cell's pieces.
const MAX_PAIRED: usize = 256;

/// The most rectangles the closest-pair pass starts from, each step trying every pair. Past this many once touching
/// pieces are merged, they are gathered into a [`GATHER_CELLS_CLOSE`]-square grid first.
const MAX_CLOSE_PAIRED: usize = 64;

/// The columns (and rows) of the grid a damage of more than [`MAX_PAIRED`] rectangles is gathered into.
const GATHER_CELLS: i64 = 16;

/// The columns (and rows) of the grid a damage of more than [`MAX_CLOSE_PAIRED`] rectangles is gathered into.
const GATHER_CELLS_CLOSE: i64 = 8;

/// Merges overlapping or touching rectangles, then the pairs whose merge adds the least repainted area until at most
/// [`MAX_DAMAGE_RECTS`] remain, and orders them by row and column. The result covers every input pixel, and no two of
/// its rectangles touch. The cost stays bounded for any number of inputs: past [`MAX_PAIRED`] (and again past
/// [`MAX_CLOSE_PAIRED`]) the pieces are gathered by a coarse grid, one pass over them, before any pairing.
pub(crate) fn merge(mut rects: Vec<Bounds>) -> Vec<Bounds> {
    rects.retain(|r| !r.is_empty());
    if rects.len() > MAX_PAIRED {
        rects = gather(&rects, GATHER_CELLS);
    }
    merge_touching(&mut rects);
    if rects.len() > MAX_CLOSE_PAIRED {
        rects = gather(&rects, GATHER_CELLS_CLOSE);
        merge_touching(&mut rects);
    }
    while rects.len() > MAX_DAMAGE_RECTS {
        let mut best = (0, 1, u64::MAX);
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let added = added_area(&rects[i], &rects[j]);
                if added < best.2 {
                    best = (i, j, added);
                }
            }
        }
        let (i, j, _) = best;
        let other = rects.remove(j);
        rects[i] = rects[i].union(&other);
        absorb_touching(&mut rects, i);
    }
    rects.sort_by_key(|r| (r.y0, r.x0, r.y1, r.x1));
    rects
}

/// Whether two rectangles are repainted as one: they overlap, touch or lie a pixel apart.
fn joins(a: &Bounds, b: &Bounds) -> bool {
    a.outset(1).touches(b)
}

fn area(r: &Bounds) -> u64 {
    u64::from(r.width()) * u64::from(r.height())
}

/// The pixels a repaint of the one rectangle around `a` and `b` adds to repainting both.
fn added_area(a: &Bounds, b: &Bounds) -> u64 {
    let both = area(a) + area(b) - area(&a.intersect(b));
    area(&a.union(b)).saturating_sub(both)
}

/// Merges rectangles until no two of them join: each in turn grows over every one that joins it, so the ones before
/// it never join each other again.
fn merge_touching(rects: &mut Vec<Bounds>) {
    let mut i = 0;
    while i < rects.len() {
        i = absorb_touching(rects, i) + 1;
    }
}

/// Grows `rects[i]` over every rectangle that joins it, until none does, keeping the others in order; where it ends.
fn absorb_touching(rects: &mut Vec<Bounds>, mut i: usize) -> usize {
    let mut j = 0;
    while j < rects.len() {
        if j != i && joins(&rects[i], &rects[j]) {
            let other = rects.remove(j);
            if j < i {
                i -= 1;
            }
            rects[i] = rects[i].union(&other);
            // the grown rectangle may now join one already passed
            j = 0;
        } else {
            j += 1;
        }
    }
    i
}

/// One rectangle around the pieces whose centres fall in each cell of a `cells`-square grid over all of `rects`.
fn gather(rects: &[Bounds], cells: i64) -> Vec<Bounds> {
    let Some(all) = rects.iter().copied().reduce(|a, b| a.union(&b)) else {
        return Vec::new();
    };
    let (w, h) = (
        i64::from(all.width()).max(1),
        i64::from(all.height()).max(1),
    );
    let mut grid: Vec<Option<Bounds>> = vec![None; (cells * cells) as usize];
    for r in rects {
        let cx = (i64::from(r.x0) + i64::from(r.x1)) / 2 - i64::from(all.x0);
        let cy = (i64::from(r.y0) + i64::from(r.y1)) / 2 - i64::from(all.y0);
        let cell = ((cy * cells / h).clamp(0, cells - 1) * cells
            + (cx * cells / w).clamp(0, cells - 1)) as usize;
        grid[cell] = Some(grid[cell].map_or(*r, |c| c.union(r)));
    }
    grid.into_iter().flatten().collect()
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

/// The pixels the clip `shape` under `node` can let anything through, and the pieces of them it lets through whole: a
/// rectangle, or a rounded rectangle less its corners, under a transform that only scales and moves; `None` for any
/// other clip. An edge on a pixel edge cuts nothing on either side of it; one inside a pixel leaves that pixel and the
/// next out of the whole ones, as [`clip_holds`] counts them.
pub(crate) fn clip_cover(
    shape: &Shape,
    node: &Transform,
    scale: f64,
) -> Option<(Bounds, Vec<Bounds>)> {
    let [a, b, c, d, e, f] = node.0;
    if b != 0.0 || c != 0.0 || a <= 0.0 || d <= 0.0 {
        return None;
    }
    let (rect, radius) = match shape {
        Shape::Rect(rect) => (rect, 0.0),
        Shape::RoundedRect { rect, radius } => (
            rect,
            radius.clamp(0.0, rect.width().min(rect.height()) / 2.0),
        ),
        Shape::Path { .. } => return None,
    };
    let cut = |v: f64| v.clamp(-f64::from(1 << 28), f64::from(1 << 28));
    let on_edge = |v: f64| (v - v.round()).abs() < 1e-9;
    // per edge: the first or last pixel anything reaches, and the first or last one wholly inside
    let low = |v: f64| {
        let v = cut(v);
        if on_edge(v) {
            (v.round() as i32, v.round() as i32)
        } else {
            (v.floor() as i32, v.ceil() as i32 + 1)
        }
    };
    let high = |v: f64| {
        let v = cut(v);
        if on_edge(v) {
            (v.round() as i32, v.round() as i32)
        } else {
            (v.ceil() as i32, v.floor() as i32 - 1)
        }
    };
    let (rx0, ix0) = low((rect.min_x() * a + e) * scale);
    let (rx1, ix1) = high((rect.max_x() * a + e) * scale);
    let (ry0, iy0) = low((rect.min_y() * d + f) * scale);
    let (ry1, iy1) = high((rect.max_y() * d + f) * scale);
    let reach = Bounds {
        x0: rx0,
        y0: ry0,
        x1: rx1,
        y1: ry1,
    };
    let inner = Bounds {
        x0: ix0,
        y0: iy0,
        x1: ix1,
        y1: iy1,
    };
    if radius <= 0.0 {
        return Some((reach, vec![inner]));
    }
    // a rounded rectangle less the squares its corners need: a band across between them and the middle of the rows
    // beside them
    let (cx, cy) = (
        (radius * a * scale).ceil() as i32 + 1,
        (radius * d * scale).ceil() as i32 + 1,
    );
    let pieces = vec![
        Bounds {
            x0: inner.x0 + cx,
            x1: inner.x1 - cx,
            y1: inner.y0 + cy,
            ..inner
        },
        Bounds {
            y0: inner.y0 + cy,
            y1: inner.y1 - cy,
            ..inner
        },
        Bounds {
            x0: inner.x0 + cx,
            x1: inner.x1 - cx,
            y0: inner.y1 - cy,
            ..inner
        },
    ];
    Some((
        reach,
        pieces.into_iter().filter(|p| !p.is_empty()).collect(),
    ))
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

    /// `n` squares of 10 pixels, 20 apart in a row of 32: none touches another.
    fn grid(n: i32) -> Vec<Bounds> {
        (0..n)
            .map(|i| {
                let (x, y) = (i % 32 * 20, i / 32 * 20);
                px(x, y, x + 10, y + 10)
            })
            .collect()
    }

    #[test]
    fn touching_damage_merges_and_a_few_separate_pieces_stay_apart_in_reading_order() {
        assert_eq!(
            merge(vec![px(0, 0, 10, 10), px(10, 0, 20, 10), px(0, 50, 5, 55)]),
            vec![px(0, 0, 20, 10), px(0, 50, 5, 55)]
        );
        let few = grid(MAX_DAMAGE_RECTS as i32);
        assert_eq!(merge(few.clone()).len(), MAX_DAMAGE_RECTS);
        assert_eq!(
            merge(vec![px(5, 5, 5, 9)]),
            vec![],
            "an empty rectangle is no damage"
        );
    }

    /// Whether every pixel of `input` lies in one of `output`.
    fn covers(output: &[Bounds], input: &[Bounds]) -> bool {
        input.iter().filter(|r| !r.is_empty()).all(|r| {
            (r.y0..r.y1).all(|y| {
                (r.x0..r.x1).all(|x| {
                    output
                        .iter()
                        .any(|o| o.x0 <= x && x < o.x1 && o.y0 <= y && y < o.y1)
                })
            })
        })
    }

    fn pixels(rects: &[Bounds]) -> u64 {
        rects.iter().map(area).sum()
    }

    /// A made-up spread of `n` small rectangles, some overlapping, over a frame of 4000 by 2500 pixels.
    fn scattered(n: u32, seed: u32) -> Vec<Bounds> {
        let mut state = seed.wrapping_mul(2_654_435_761).max(1);
        let mut next = move |below: u32| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state % below) as i32
        };
        (0..n)
            .map(|_| {
                let (x, y) = (next(3900), next(2400));
                px(x, y, x + 2 + next(60), y + 2 + next(60))
            })
            .collect()
    }

    #[test]
    fn scattered_pieces_stay_separate_and_only_the_closest_merge_past_the_limit() {
        // seventeen pieces far apart: the two nearest merge, the other fifteen stay as they are
        let mut pieces: Vec<Bounds> = (0..15).map(|i| px(i * 200, 0, i * 200 + 10, 10)).collect();
        pieces.push(px(3000, 900, 3010, 910));
        pieces.push(px(3000, 915, 3010, 925));
        let merged = merge(pieces.clone());
        assert_eq!(merged.len(), MAX_DAMAGE_RECTS, "{merged:?}");
        assert!(merged.contains(&px(3000, 900, 3010, 925)), "{merged:?}");
        assert!(covers(&merged, &pieces));
        assert_eq!(
            pixels(&merged),
            pixels(&pieces) + 50,
            "only the gap between the two closest is added"
        );
    }

    #[test]
    fn a_scrolled_grid_of_separate_items_is_repainted_in_a_few_rows_not_one_rectangle_around_them()
    {
        // a scrolled grid damages each of its items: each rectangle would walk the whole display list again
        for n in [MAX_DAMAGE_RECTS as i32 + 1, 700] {
            let pieces = grid(n);
            let merged = merge(pieces.clone());
            assert!(
                merged.len() <= MAX_DAMAGE_RECTS,
                "{n} pieces: {}",
                merged.len()
            );
            assert!(covers(&merged, &pieces), "{n} pieces");
            let all = pieces.iter().copied().reduce(|a, b| a.union(&b)).unwrap();
            if n == MAX_DAMAGE_RECTS as i32 + 1 {
                // seventeen in a row, ten pixels apart: one gap is painted over, the rest stay apart
                assert_eq!(merged.len(), MAX_DAMAGE_RECTS);
                assert_eq!(pixels(&merged), pixels(&pieces) + 100);
            }
            assert!(
                pixels(&merged) < area(&all),
                "{n} pieces: no more than the rectangle around them"
            );
        }
    }

    #[test]
    fn the_rectangle_around_everything_comes_only_when_the_pieces_fill_it() {
        // pieces that tile a block are repainted as that block; pieces spread over it are not
        let tiles: Vec<Bounds> = (0..400)
            .map(|i| px(i % 20 * 10, i / 20 * 10, i % 20 * 10 + 10, i / 20 * 10 + 10))
            .collect();
        assert_eq!(merge(tiles), vec![px(0, 0, 200, 200)]);
        let pieces = scattered(40, 1);
        let all = pieces.iter().copied().reduce(|a, b| a.union(&b)).unwrap();
        let merged = merge(pieces);
        assert!(
            merged.len() > 1 && pixels(&merged) * 4 < area(&all),
            "{merged:?}"
        );
    }

    #[test]
    fn the_merged_damage_always_covers_every_piece_and_no_two_of_its_rectangles_touch() {
        for (n, seed) in [
            (0, 1),
            (1, 2),
            (5, 3),
            (17, 4),
            (63, 5),
            (65, 6),
            (255, 7),
            (257, 8),
            (700, 9),
            (3000, 10),
        ] {
            let pieces = scattered(n, seed);
            let merged = merge(pieces.clone());
            assert!(
                merged.len() <= MAX_DAMAGE_RECTS,
                "{n}: {} rectangles",
                merged.len()
            );
            assert!(covers(&merged, &pieces), "{n} pieces");
            for (i, a) in merged.iter().enumerate() {
                for b in &merged[i + 1..] {
                    assert!(!joins(a, b), "{n}: {a:?} touches {b:?}");
                }
            }
        }
    }

    #[test]
    fn many_pieces_merge_within_a_bounded_time() {
        // the frame budget: a damage of thousands of pieces must not cost more than a repaint would save
        let pieces = scattered(20_000, 11);
        let start = std::time::Instant::now();
        let merged = merge(pieces.clone());
        assert!(
            start.elapsed() < std::time::Duration::from_millis(200),
            "{:?}",
            start.elapsed()
        );
        assert!(covers(&merged, &pieces[..200]));
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
