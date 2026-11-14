//! The span path for a node that would be painted through a scratch image. When every clip above it is an
//! axis-aligned rectangle and its large fills are solid rectangles or rounded rectangles, the pixels that those fills
//! and the clips cover fully, and that none of its other operations (smaller fills, strokes, images, glyphs) reach,
//! hold one known colour in that scratch image. Those pixels are written as rows (copied when the colour is opaque,
//! blended with the same integer source-over as the scratch image's composite otherwise), and only the rest goes
//! through the scratch image, with those pixels cut out of the fills so the rasterizer skips them. Both give the
//! same bytes.

use crate::geometry::{self, Bounds};
use crate::raster;
use crate::{Paint, PaintNode, PaintOp, Shape};
use tiny_skia::{PixmapMut, Point};

/// The farthest edge, in target pixels, the span path trusts its arithmetic for; a node reaching past it keeps its
/// scratch image.
const FAR: f32 = 1_048_576.0;

/// The narrowest run of fully covered pixels worth writing as rows; a narrower one stays with the scratch image.
const MIN_SIDE: u32 = 4;

/// What a node's scratch image holds over the pixels its fills cover fully.
#[derive(Debug)]
pub(crate) struct Solid {
    /// Disjoint rectangles of target pixels, in window pixels, whose scratch pixel is `color`.
    pub core: Vec<Bounds>,
    /// The pixels outside which the node's operations leave the scratch image transparent, in window pixels.
    pub reach: Bounds,
    /// Premultiplied RGBA.
    pub color: [u8; 4],
    /// The operations, by index, that cover all of `core`: the fills `core` is cut out of.
    pub covering: Vec<bool>,
    /// For each glyph run among the operations, by index, the window pixels its rasters cover.
    pub glyphs: Vec<Option<Bounds>>,
}

/// The solid core of `node` when it is painted into a scratch image whose top-left pixel is `origin` and which covers
/// `area` (window pixels), under the clips of `chain`. `glyphs` holds, for each glyph run among the operations, the
/// window pixels its rasters cover. `None` when an operation, clip or transform is not one the span path handles, or
/// when no pixel is covered fully.
pub(crate) fn solid(
    node: &PaintNode,
    chain: &[&PaintNode],
    scale: f64,
    origin: (i32, i32),
    area: &Bounds,
    glyphs: &[Option<Bounds>],
) -> Option<Solid> {
    let ts = geometry::to_target(&node.transform, scale, origin);
    if !axis_aligned(&ts) {
        return None;
    }
    // the scratch image's pixel where the fills cover it fully, laid as the fills lay it
    let mut pixel = [0u8; 4];
    let [a, b, c, d, _, _] = node.transform.0;
    let stretch = a.abs().max(b.abs()).max(c.abs()).max(d.abs());
    let mut core = vec![*area];
    let mut reach: Option<Bounds> = None;
    let mut covering = vec![false; node.ops.len()];
    let mut clipped = 0;
    for (i, op) in node.ops.iter().enumerate() {
        // what the operation can touch, when it is a detail kept out of the core
        let outer = match op {
            PaintOp::Fill {
                shape: shape @ (Shape::Rect(r) | Shape::RoundedRect { rect: r, .. }),
                paint: Paint::Solid(color),
            } => {
                let (Some(_), Some(color)) = (
                    raster::Outline::new(shape, ts, None),
                    raster::premultiplied(*color),
                ) else {
                    // draws nothing
                    continue;
                };
                let outer = exterior(&ts, r)?.offset(origin.0, origin.1);
                let inner: Vec<Bounds> = match shape {
                    Shape::RoundedRect { rect, radius } => interior_rounded(&ts, rect, *radius),
                    _ => interior_rect(&ts, r.min_x(), r.min_y(), r.max_x(), r.max_y())
                        .into_iter()
                        .collect(),
                };
                let inner: Vec<Bounds> =
                    inner.iter().map(|b| b.offset(origin.0, origin.1)).collect();
                let within = intersect_all(&core, &inner);
                let beside = subtract_all(&core, &outer);
                reach = Some(reach.map_or(outer, |r| r.union(&outer)));
                // a fill under a clip of its own, or one that covers less than it hides, is a detail
                if clipped == 0 && pixels(&within) >= pixels(&beside) {
                    core = within;
                    covering[i] = true;
                    pixel = raster::laid(color, pixel);
                } else {
                    core = beside;
                }
                continue;
            }
            PaintOp::Fill { shape, .. } => geometry::clip_bounds(shape, &node.transform, scale),
            PaintOp::Stroke { shape, width, .. } => {
                geometry::clip_bounds(shape, &node.transform, scale)
                    .outset((width * scale * stretch).ceil() as i32 * 2 + 2)
            }
            PaintOp::Image { dst, .. } => {
                geometry::clip_bounds(&Shape::Rect(*dst), &node.transform, scale)
            }
            PaintOp::Glyphs { .. } => glyphs.get(i).copied().flatten()?,
            PaintOp::PushClip(_) => {
                clipped += 1;
                continue;
            }
            PaintOp::PopClip => {
                clipped -= 1;
                continue;
            }
            PaintOp::PushLayer { .. } | PaintOp::PopLayer => return None,
        };
        if outer.is_empty() {
            continue;
        }
        core = subtract_all(&core, &outer);
        reach = Some(reach.map_or(outer, |r| r.union(&outer)));
    }
    for owner in chain {
        let Some(clip) = &owner.clip else { continue };
        let Shape::Rect(r) = clip else { return None };
        let ts = geometry::to_target(&owner.transform, scale, origin);
        if !axis_aligned(&ts) {
            return None;
        }
        let inner = interior_rect(&ts, r.min_x(), r.min_y(), r.max_x(), r.max_y())?;
        core = intersect_all(&core, &[inner.offset(origin.0, origin.1)]);
        // nothing shows outside the clip
        let outer = exterior(&ts, r)?.offset(origin.0, origin.1);
        reach = reach.map(|r| r.intersect(&outer));
    }
    core.retain(|r| r.width() >= MIN_SIDE && r.height() >= MIN_SIDE);
    if core.is_empty() || !covering.contains(&true) {
        return None;
    }
    Some(Solid {
        core,
        reach: reach?.intersect(area),
        color: pixel,
        covering,
        glyphs: glyphs.to_vec(),
    })
}

fn pixels(rects: &[Bounds]) -> u64 {
    rects
        .iter()
        .map(|r| u64::from(r.width()) * u64::from(r.height()))
        .sum()
}

/// Every rectangle of `rects` less `cut`, as disjoint rectangles.
fn subtract_all(rects: &[Bounds], cut: &Bounds) -> Vec<Bounds> {
    rects
        .iter()
        .flat_map(|r| subtract(*r, std::slice::from_ref(cut)))
        .collect()
}

/// A transform that only scales by positive factors and translates.
/// Whether `ts` only moves and scales, by the same amount both ways: under a stretch the coverage of a pixel wholly
/// inside an edge can fall short of full, the distance being measured in the shape's units.
fn axis_aligned(ts: &tiny_skia::Transform) -> bool {
    ts.kx == 0.0 && ts.ky == 0.0 && ts.sx > 0.0 && ts.sx == ts.sy
}

/// Where `ts` puts the point, computed as the rasterizer computes it.
fn map(ts: &tiny_skia::Transform, x: f32, y: f32) -> Point {
    let mut p = [Point::from_xy(x, y)];
    ts.map_points(&mut p);
    p[0]
}

/// The pixels between two mapped edges that the anti-aliased rasterizer covers fully: from the first whole pixel at
/// or after `lo` to the last whole pixel ending at or before `hi`. `None` past [`FAR`].
fn span(lo: f32, hi: f32) -> Option<(i32, i32)> {
    if !(lo.abs() <= FAR && hi.abs() <= FAR) {
        return None;
    }
    let cut = |v: f32| v.clamp(i32::MIN as f32 / 2.0, i32::MAX as f32 / 2.0) as i32;
    let (a, b) = (cut(lo.ceil()), cut(hi.floor()));
    (a < b).then_some((a, b))
}

/// Every pixel the rasterizer can touch filling a rectangle, or a rounded rectangle, with these outer edges, in target
/// pixels: it places an edge at or inside where `ts` puts it, never past it.
fn exterior(ts: &tiny_skia::Transform, r: &cupertino_geometry::Rect) -> Option<Bounds> {
    let lo = map(ts, r.min_x() as f32, r.min_y() as f32);
    let hi = map(ts, r.max_x() as f32, r.max_y() as f32);
    if ![lo.x, lo.y, hi.x, hi.y].iter().all(|v| v.abs() <= FAR) {
        return None;
    }
    Some(Bounds {
        x0: lo.x.floor() as i32,
        y0: lo.y.floor() as i32,
        x1: hi.x.ceil() as i32,
        y1: hi.y.ceil() as i32,
    })
}

/// The pixels fully inside the rectangle whose path has these corners, in target pixels.
fn interior_rect(ts: &tiny_skia::Transform, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<Bounds> {
    let lo = map(ts, x0 as f32, y0 as f32);
    let hi = map(ts, x1 as f32, y1 as f32);
    let (xa, xb) = span(lo.x, hi.x)?;
    let (ya, yb) = span(lo.y, hi.y)?;
    Some(Bounds {
        x0: xa,
        y0: ya,
        x1: xb,
        y1: yb,
    })
}

/// The pixels fully inside a rounded rectangle, as disjoint rectangles in target pixels: the band between the corners'
/// arcs at full width, and above and below it the parts between the arcs.
fn interior_rounded(
    ts: &tiny_skia::Transform,
    rect: &cupertino_geometry::Rect,
    radius: f64,
) -> Vec<Bounds> {
    // the same corner points the path is built from
    let r = radius.clamp(0.0, rect.width().min(rect.height()) / 2.0);
    let (l, t, rt, bt) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let outer = (map(ts, l as f32, t as f32), map(ts, rt as f32, bt as f32));
    let inner = (
        map(ts, (l + r) as f32, (t + r) as f32),
        map(ts, (rt - r) as f32, (bt - r) as f32),
    );
    let mut out = Vec::with_capacity(3);
    let (Some((xa, xb)), Some((ya, yb))) = (span(outer.0.x, outer.1.x), span(outer.0.y, outer.1.y))
    else {
        return out;
    };
    // where the straight edges start, past the arcs; never outside the rectangle
    let ceil = |v: f32, lo: i32, hi: i32| (v.ceil() as i64).clamp(lo.into(), hi.into()) as i32;
    let floor = |v: f32, lo: i32, hi: i32| (v.floor() as i64).clamp(lo.into(), hi.into()) as i32;
    let (band_top, band_bottom) = (ceil(inner.0.y, ya, yb), floor(inner.1.y, ya, yb));
    let (cap_left, cap_right) = (ceil(inner.0.x, xa, xb), floor(inner.1.x, xa, xb));
    let mut push = |b: Bounds| {
        if !b.is_empty() {
            out.push(b);
        }
    };
    let band_bottom = band_bottom.max(band_top);
    push(Bounds {
        x0: xa,
        y0: band_top,
        x1: xb,
        y1: band_bottom,
    });
    push(Bounds {
        x0: cap_left,
        y0: ya,
        x1: cap_right,
        y1: band_top,
    });
    push(Bounds {
        x0: cap_left,
        y0: band_bottom,
        x1: cap_right,
        y1: yb,
    });
    out
}

/// Every non-empty intersection of a rectangle of `a` with one of `b`; disjoint when each list is.
fn intersect_all(a: &[Bounds], b: &[Bounds]) -> Vec<Bounds> {
    a.iter()
        .flat_map(|x| b.iter().map(move |y| x.intersect(y)))
        .filter(|r| !r.is_empty())
        .collect()
}

/// `from` less every rectangle of `cut`, as disjoint rectangles.
pub(crate) fn subtract(from: Bounds, cut: &[Bounds]) -> Vec<Bounds> {
    let mut left = vec![from];
    for c in cut {
        let mut next = Vec::with_capacity(left.len() + 4);
        for r in left {
            let i = r.intersect(c);
            if i.is_empty() {
                next.push(r);
                continue;
            }
            // the rows above and below the cut at full width, then the columns beside it
            let parts = [
                Bounds { y1: i.y0, ..r },
                Bounds { y0: i.y1, ..r },
                Bounds {
                    x1: i.x0,
                    y0: i.y0,
                    y1: i.y1,
                    ..r
                },
                Bounds {
                    x0: i.x1,
                    y0: i.y0,
                    y1: i.y1,
                    ..r
                },
            ];
            next.extend(parts.into_iter().filter(|p| !p.is_empty()));
        }
        left = next;
    }
    left
}

/// Sets every pixel of `rect` (pixmap pixels) to `color`, row by row.
fn fill(pm: &mut PixmapMut<'_>, rect: &Bounds, color: [u8; 4]) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let r = rect.intersect(&Bounds {
        x0: 0,
        y0: 0,
        x1: w,
        y1: h,
    });
    if r.is_empty() {
        return;
    }
    let stride = w as usize * 4;
    let (x0, x1) = (r.x0 as usize * 4, r.x1 as usize * 4);
    let data = pm.data_mut();
    if color.iter().all(|&c| c == color[0]) {
        for y in r.y0..r.y1 {
            let row = y as usize * stride;
            data[row + x0..row + x1].fill(color[0]);
        }
        return;
    }
    let mut line = Vec::with_capacity(x1 - x0);
    for _ in r.x0..r.x1 {
        line.extend_from_slice(&color);
    }
    for y in r.y0..r.y1 {
        let row = y as usize * stride;
        data[row + x0..row + x1].copy_from_slice(&line);
    }
}

/// Lays `color` over `rect` (pixmap pixels) with the integer source-over a scratch image's composite uses: rows are
/// copied when the colour is opaque.
pub(crate) fn cover(pm: &mut PixmapMut<'_>, rect: &Bounds, color: [u8; 4]) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let r = rect.intersect(&Bounds {
        x0: 0,
        y0: 0,
        x1: w,
        y1: h,
    });
    if r.is_empty() || color == [0; 4] {
        return;
    }
    if color[3] == 255 {
        return fill(pm, &r, color);
    }
    let stride = w as usize * 4;
    let (x0, x1) = (r.x0 as usize * 4, r.x1 as usize * 4);
    let data = pm.data_mut();
    // 16-bit lanes: d * keep stays within them, and so does the rounding division by 255
    let keep = 255 - u16::from(color[3]);
    let mut pattern = [0u16; 16];
    for (i, p) in pattern.iter_mut().enumerate() {
        *p = u16::from(color[i % 4]);
    }
    let blend = |s: u16, d: &mut u8| {
        let v = u16::from(*d) * keep + 128;
        *d = (s + ((v + (v >> 8)) >> 8)).min(255) as u8;
    };
    for y in r.y0..r.y1 {
        let row = y as usize * stride;
        let line = &mut data[row + x0..row + x1];
        let (blocks, tail) = line.as_chunks_mut::<16>();
        for block in blocks {
            for (d, &s) in block.iter_mut().zip(&pattern) {
                blend(s, d);
            }
        }
        for (d, &s) in tail.iter_mut().zip(&pattern) {
            blend(s, d);
        }
    }
}

/// Span writes to the frame not made yet, as disjoint rectangles of window pixels in the order they were asked for, so
/// that layers of one colour each over the same pixels collapse into one write: an opaque layer replaces what is
/// pending under it, and a translucent one over a pending opaque colour becomes that blend, computed once. Whatever
/// else paints a part of the frame first writes what is pending there ([`Pending::flush`]).
#[derive(Debug, Default)]
pub(crate) struct Pending {
    /// Each rectangle, its colour and whether the colour replaces the pixels (`true`) or is laid over them.
    entries: Vec<(Bounds, [u8; 4], bool)>,
    /// The pixmap is all zero where nothing has painted yet, so a pending clear to transparent is not written.
    zero: bool,
}

impl Pending {
    /// Nothing pending, over a pixmap that is all zero (`zero`), or holds pixels that a clear must write over.
    pub(crate) fn over_zero(zero: bool) -> Self {
        Self {
            entries: Vec::new(),
            zero,
        }
    }

    /// Writes one pending rectangle, unless it clears pixels known to be zero already.
    fn write(&self, pm: &mut PixmapMut<'_>, rect: &Bounds, color: [u8; 4], replaces: bool) {
        // a clear entry covers only pixels nothing has painted (what paints there writes the entry first)
        if !(self.zero && replaces && color == [0; 4]) {
            write(pm, rect, color, replaces);
        }
    }

    /// Sets `rect` to `color` (the frame being cleared).
    pub(crate) fn set(&mut self, rect: Bounds, color: [u8; 4]) {
        self.add(rect, color, true);
    }

    /// Lays `color` over `rect` with the integer source-over of [`cover`].
    pub(crate) fn over(&mut self, pm: &mut PixmapMut<'_>, rect: Bounds, color: [u8; 4]) {
        if color == [0; 4] || rect.is_empty() {
            return;
        }
        if color[3] == 255 {
            return self.add(rect, color, true);
        }
        let mut rest = vec![rect];
        let mut merged = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            let (a, below, replaces) = self.entries[i];
            let both = a.intersect(&rect);
            if both.is_empty() {
                i += 1;
                continue;
            }
            if replaces {
                // over a known colour the result is known
                merged.push((both, blend(color, below), true));
            } else {
                // two translucent layers: the lower one is written now, the upper one stays pending
                cover(pm, &both, below);
                merged.push((both, color, false));
            }
            rest = subtract_each(&rest, &both);
            self.entries.swap_remove(i);
            self.entries.extend(
                subtract(a, &[both])
                    .into_iter()
                    .map(|p| (p, below, replaces)),
            );
        }
        self.entries.extend(merged);
        self.entries
            .extend(rest.into_iter().map(|r| (r, color, false)));
    }

    fn add(&mut self, rect: Bounds, color: [u8; 4], replaces: bool) {
        if rect.is_empty() {
            return;
        }
        let mut kept = Vec::with_capacity(self.entries.len() + 4);
        for (a, c, r) in self.entries.drain(..) {
            kept.extend(subtract(a, &[rect]).into_iter().map(|p| (p, c, r)));
        }
        kept.push((rect, color, replaces));
        self.entries = kept;
    }

    /// Writes what is pending inside `region` (window pixels) to the pixmap whose top-left pixel is `origin`, before
    /// something else paints there.
    pub(crate) fn flush(&mut self, pm: &mut PixmapMut<'_>, origin: (i32, i32), region: &Bounds) {
        if !self.entries.iter().any(|(a, ..)| a.touches(region)) {
            return;
        }
        let mut kept = Vec::with_capacity(self.entries.len() + 4);
        for (a, c, replaces) in std::mem::take(&mut self.entries) {
            let both = a.intersect(region);
            if both.is_empty() {
                kept.push((a, c, replaces));
                continue;
            }
            self.write(pm, &both.offset(-origin.0, -origin.1), c, replaces);
            kept.extend(subtract(a, &[both]).into_iter().map(|p| (p, c, replaces)));
        }
        self.entries = kept;
    }

    /// Takes out of what is pending the parts of `region` (window pixels) that are set to one colour, each with its
    /// colour, and writes nothing there: the caller writes every pixel of them itself, from that colour (a kept layer
    /// laid over the background under it in one pass). Translucent layers pending there stay pending.
    pub(crate) fn take_set(&mut self, region: &Bounds) -> Vec<(Bounds, [u8; 4])> {
        let mut taken = Vec::new();
        if !self.entries.iter().any(|(a, _, r)| *r && a.touches(region)) {
            return taken;
        }
        let mut kept = Vec::with_capacity(self.entries.len() + 4);
        for (a, c, replaces) in std::mem::take(&mut self.entries) {
            let both = a.intersect(region);
            if !replaces || both.is_empty() {
                kept.push((a, c, replaces));
                continue;
            }
            taken.push((both, c));
            kept.extend(subtract(a, &[both]).into_iter().map(|p| (p, c, replaces)));
        }
        self.entries = kept;
        taken
    }

    /// Writes everything pending.
    pub(crate) fn flush_all(&mut self, pm: &mut PixmapMut<'_>, origin: (i32, i32)) {
        for (a, c, replaces) in std::mem::take(&mut self.entries) {
            self.write(pm, &a.offset(-origin.0, -origin.1), c, replaces);
        }
    }
}

fn write(pm: &mut PixmapMut<'_>, rect: &Bounds, color: [u8; 4], replaces: bool) {
    if replaces {
        fill(pm, rect, color);
    } else {
        cover(pm, rect, color);
    }
}

/// `color` laid over the pixel `below` as [`cover`] lays it.
fn blend(color: [u8; 4], below: [u8; 4]) -> [u8; 4] {
    let keep = 255 - u32::from(color[3]);
    let mut out = [0; 4];
    for i in 0..4 {
        let v = u32::from(below[i]) * keep + 128;
        out[i] = (u32::from(color[i]) + ((v + (v >> 8)) >> 8)).min(255) as u8;
    }
    out
}

fn subtract_each(rects: &[Bounds], cut: &Bounds) -> Vec<Bounds> {
    subtract_all(rects, cut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Pixmap;

    fn b(x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds {
        Bounds { x0, y0, x1, y1 }
    }

    fn area(rects: &[Bounds]) -> i64 {
        rects
            .iter()
            .map(|r| i64::from(r.width()) * i64::from(r.height()))
            .sum()
    }

    #[test]
    fn subtracting_leaves_disjoint_pieces_that_cover_the_rest() {
        let from = b(0, 0, 40, 30);
        let cut = [b(10, 5, 20, 25), b(15, 0, 50, 10), b(-5, 28, 3, 40)];
        let left = subtract(from, &cut);
        for (i, x) in left.iter().enumerate() {
            assert!(!x.is_empty());
            for y in &left[i + 1..] {
                assert!(!x.touches(y), "{x:?} overlaps {y:?}");
            }
            for c in &cut {
                assert!(!x.touches(c), "{x:?} keeps part of {c:?}");
            }
        }
        let mut covered = 0;
        for y in 0..30 {
            for x in 0..40 {
                let inside = |r: &Bounds| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1;
                if !cut.iter().any(inside) {
                    covered += 1;
                    assert!(left.iter().any(inside), "({x}, {y}) is lost");
                }
            }
        }
        assert_eq!(area(&left), covered);
    }

    #[test]
    fn a_rounded_rectangles_interior_leaves_out_its_corners() {
        let ts = tiny_skia::Transform::from_scale(2.0, 2.0);
        let rect = cupertino_geometry::Rect::new(1.0, 1.0, 40.0, 20.0);
        let inner = interior_rounded(&ts, &rect, 6.0);
        // the band between the arcs spans the full width, the caps stop where the arcs do
        assert_eq!(inner[0], b(2, 14, 82, 30));
        assert_eq!(inner[1], b(14, 2, 70, 14));
        assert_eq!(inner[2], b(14, 30, 70, 42));
        assert!(
            interior_rounded(&ts, &cupertino_geometry::Rect::new(0.0, 0.0, 0.2, 9.0), 2.0)
                .is_empty()
        );
    }

    #[test]
    fn a_stretched_node_keeps_its_scratch_image() {
        // across a stretch a pixel wholly inside an edge can be short of full coverage: the span path stays out
        let node = crate::PaintNode {
            layer: None,
            id: crate::PaintNodeId(1),
            parent: None,
            revision: crate::PaintRevision(1),
            transform: crate::Transform([1.1, 0.0, 0.0, 1.0, 0.0, 0.0]),
            clip: None,
            group: None,
            bounds: cupertino_geometry::Rect::new(0.0, 0.0, 40.0, 40.0),
            hit: cupertino_geometry::Rect::new(0.0, 0.0, 0.0, 0.0),
            ops: vec![PaintOp::Fill {
                shape: Shape::Rect(cupertino_geometry::Rect::new(0.0, 0.0, 30.0, 30.0)),
                paint: Paint::Solid(cupertino_theme::Color::new(0.2, 0.3, 0.4, 1.0)),
            }],
        };
        let area = b(0, 0, 80, 80);
        assert!(solid(&node, &[], 2.0, (0, 0), &area, &[None]).is_none());
        let even = crate::PaintNode {
            transform: crate::Transform([1.1, 0.0, 0.0, 1.1, 0.0, 0.0]),
            ..node
        };
        assert!(solid(&even, &[], 2.0, (0, 0), &area, &[None]).is_some());
    }

    #[test]
    fn a_rectangle_whose_edges_fall_inside_pixels_keeps_only_its_whole_pixels() {
        let ts = tiny_skia::Transform::from_row(1.5, 0.0, 0.0, 1.5, 0.25, 0.0);
        assert_eq!(
            interior_rect(&ts, 1.0, 2.0, 11.0, 4.0),
            Some(b(2, 3, 16, 6))
        );
        assert_eq!(interior_rect(&ts, 1.0, 2.0, 1.4, 4.0), None);
    }

    #[test]
    fn covering_blends_like_the_scratch_composite_and_copies_opaque_rows() {
        let mut pixmap = Pixmap::new(4, 3).unwrap();
        pixmap.data_mut().fill(200);
        let mut pm = pixmap.as_mut();
        cover(&mut pm, &b(1, 0, 3, 2), [10, 20, 30, 128]);
        cover(&mut pm, &b(0, 2, 9, 9), [1, 2, 3, 255]);
        let px = |x: usize, y: usize| {
            let at = (y * 4 + x) * 4;
            <[u8; 4]>::try_from(&pixmap.data()[at..at + 4]).unwrap()
        };
        let blended = 200u32 * 127 / 255;
        assert_eq!(px(0, 0), [200; 4]);
        for (x, y) in [(1, 0), (2, 1)] {
            let p = px(x, y);
            assert!(p[0].abs_diff((10 + blended) as u8) <= 1, "{p:?}");
        }
        assert_eq!(px(3, 1), [200; 4]);
        assert_eq!(px(0, 2), [1, 2, 3, 255]);
        assert_eq!(px(3, 2), [1, 2, 3, 255]);
    }

    #[test]
    fn writes_held_back_end_in_the_pixels_of_each_write_made_at_once() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n) as i32
        };
        for _ in 0..200 {
            let mut start = Pixmap::new(24, 16).unwrap();
            for (i, px) in start.data_mut().chunks_mut(4).enumerate() {
                let a = (i * 37 % 256) as u8;
                px.copy_from_slice(&[a / 3, a / 2, a / 4, a]);
            }
            let (mut held, mut direct) = (start.clone(), start.clone());
            let mut pending = Pending::default();
            for _ in 0..8 {
                let (x, y) = (next(24), next(16));
                let rect = b(x, y, x + 1 + next(12), y + 1 + next(10)).intersect(&b(0, 0, 24, 16));
                let alpha = [255, 0, 1, 90, 200][next(5) as usize];
                let color = [next(256), next(256), next(256)].map(|c| (c * alpha / 255) as u8);
                let color = [color[0], color[1], color[2], alpha as u8];
                match next(4) {
                    0 => {
                        pending.flush(&mut held.as_mut(), (0, 0), &rect);
                        cover(&mut held.as_mut(), &rect, color);
                        cover(&mut direct.as_mut(), &rect, color);
                    }
                    1 if alpha == 255 => {
                        pending.set(rect, [0; 4]);
                        fill(&mut direct.as_mut(), &rect, [0; 4]);
                    }
                    _ => {
                        pending.over(&mut held.as_mut(), rect, color);
                        cover(&mut direct.as_mut(), &rect, color);
                    }
                }
            }
            pending.flush_all(&mut held.as_mut(), (0, 0));
            assert!(
                held.data() == direct.data(),
                "the held writes differ from the direct ones"
            );
        }
    }

    /// A frame just allocated is all zero: its clear is not written, and what is laid over it gives the bytes of the
    /// same writes over a frame that was cleared by writing.
    #[test]
    fn over_a_zeroed_frame_the_clear_is_skipped_and_the_pixels_are_the_same() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n) as i32
        };
        let whole = b(0, 0, 24, 16);
        for _ in 0..200 {
            let (mut zeroed, mut written, mut direct) = (
                Pixmap::new(24, 16).unwrap(),
                Pixmap::new(24, 16).unwrap(),
                Pixmap::new(24, 16).unwrap(),
            );
            let mut skips = Pending::over_zero(true);
            let mut writes = Pending::over_zero(false);
            skips.set(whole, [0; 4]);
            writes.set(whole, [0; 4]);
            for _ in 0..8 {
                let (x, y) = (next(24), next(16));
                let rect = b(x, y, x + 1 + next(12), y + 1 + next(10)).intersect(&whole);
                let alpha = [255, 0, 1, 90, 200][next(5) as usize];
                let color = [next(256), next(256), next(256)].map(|c| (c * alpha / 255) as u8);
                let color = [color[0], color[1], color[2], alpha as u8];
                if next(3) == 0 {
                    for (pending, pixmap) in
                        [(&mut skips, &mut zeroed), (&mut writes, &mut written)]
                    {
                        pending.flush(&mut pixmap.as_mut(), (0, 0), &rect);
                        cover(&mut pixmap.as_mut(), &rect, color);
                    }
                } else {
                    skips.over(&mut zeroed.as_mut(), rect, color);
                    writes.over(&mut written.as_mut(), rect, color);
                }
                cover(&mut direct.as_mut(), &rect, color);
            }
            skips.flush_all(&mut zeroed.as_mut(), (0, 0));
            writes.flush_all(&mut written.as_mut(), (0, 0));
            assert!(
                zeroed.data() == written.data(),
                "skipping the clear changed pixels"
            );
            assert!(
                zeroed.data() == direct.data(),
                "the held writes differ from the direct ones"
            );
        }
        // the clear alone writes nothing: a pixel that is not zero (which a zeroed frame never has) stays as it was
        let mut canary = Pixmap::new(4, 4).unwrap();
        canary.data_mut()[..4].copy_from_slice(&[9, 9, 9, 9]);
        let mut pending = Pending::over_zero(true);
        pending.set(b(0, 0, 4, 4), [0; 4]);
        pending.flush_all(&mut canary.as_mut(), (0, 0));
        assert_eq!(&canary.data()[..4], &[9, 9, 9, 9]);
    }
}
