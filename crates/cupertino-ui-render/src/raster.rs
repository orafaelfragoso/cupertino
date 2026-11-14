//! Coverage of rectangles and rounded rectangles, their strokes and clips, from the shared distance function
//! (`cupertino_raster_math::coverage`), and the fills that lay a paint through it.
//!
//! A shape is placed on the window's pixels by its node's transform; each pixel's coverage is that of its centre's
//! distance to the outline, computed in the shape's own units and turned into pixels by the transform's scale. A
//! pixel's coverage depends on nothing but its own position, so a shape painted into any scratch image, cut anywhere by
//! the damage, gives the same bytes. Strokes are centred on the outline, as the stroker draws a path's: the shared
//! stroke of the outline moved out by half the width (a rectangle's corners stay square, a rounded one's stay round).
//! Paths keep the path rasterizer's coverage.

use crate::effects::{div255, mul_px, over_px};
use crate::geometry::Bounds;
use crate::Shape;
use cupertino_raster_math::coverage::{coverage, rect_distance, rounded_distance, stroke_coverage};
use cupertino_raster_math::dither;
use cupertino_raster_math::gradient::{linear_t, radial_t, Stop, Stops, MAX_STOPS};
use tiny_skia::{Mask, PixmapMut};

/// Coordinates past this many pixels are not placed; the shape draws nothing.
const FAR: f32 = 1.0e7;

/// A rectangle or rounded rectangle (or its stroke) placed on the window's pixels.
#[derive(Clone, Debug)]
pub(crate) struct Outline {
    /// Window pixel position to the shape's units, relative to its centre.
    inv: [f32; 6],
    half: [f32; 2],
    /// Corner radius in the shape's units; zero for square corners.
    corner: f32,
    /// Window pixels per unit of the shape.
    px: f32,
    /// Stroke width in the shape's units; `None` for a fill.
    stroke: Option<f32>,
    /// The window pixels the shape can touch.
    pub(crate) bounds: Bounds,
}

impl Outline {
    /// `shape` placed by `ts` (shape units to window pixels); `stroke` its width for a stroke. `None` for a path or a
    /// shape with no area.
    pub(crate) fn new(shape: &Shape, ts: tiny_skia::Transform, stroke: Option<f64>) -> Option<Outline> {
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
        // the inverse of the linear part, then the translation and the centre
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

    /// The coverage of window pixel `(x, y)`, 0 to 255, and whether the pixel lies inside the part every pixel of
    /// which has the same coverage (255 for a fill, 0 inside a stroke's inner outline).
    #[inline]
    fn at(&self, x: i32, y: i32) -> (u8, bool) {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let [a, b, c, d, e, f] = self.inv;
        let p = [a * fx + c * fy + e, b * fx + d * fy + f];
        let to_byte = |v: f32| (v * 255.0 + 0.5) as u8;
        match self.stroke {
            None => {
                let cov = to_byte(coverage(self.distance(p, 0.0), self.px));
                (cov, cov == 255)
            }
            Some(width) => {
                let outer = self.distance(p, width / 2.0);
                let inner = coverage(outer + width, self.px);
                (to_byte(stroke_coverage(outer, width, self.px)), inner >= 1.0)
            }
        }
    }

    /// Distance from `p` to the outline moved out by `grow`.
    #[inline]
    fn distance(&self, p: [f32; 2], grow: f32) -> f32 {
        if self.corner > 0.0 {
            rounded_distance(p, self.half, self.corner, 2.0) - grow
        } else {
            rect_distance(p, [self.half[0] + grow, self.half[1] + grow])
        }
    }

    /// Hands every pixel of `area` (window pixels) less `holes` that the shape covers to `put` (`y`, `x0`, `x1`,
    /// coverage): one pixel at a time along the edges, and each run of pixels it covers fully at once; a stroke's
    /// hollow middle is passed over. A row meets a fill's fully covered part, and a stroke's hollow, in one run, both
    /// being convex: each row is read in from both ends until it reaches that part.
    pub(crate) fn scan(&self, area: &Bounds, holes: &[Bounds], mut put: impl FnMut(i32, i32, i32, u8)) {
        let area = area.intersect(&self.bounds);
        if area.is_empty() {
            return;
        }
        let mut pieces: Vec<(i32, i32)> = Vec::with_capacity(4);
        for y in area.y0..area.y1 {
            pieces.clear();
            pieces.push((area.x0, area.x1));
            for h in holes.iter().filter(|h| y >= h.y0 && y < h.y1) {
                let mut kept = Vec::with_capacity(pieces.len() + 1);
                for &(a, b) in &pieces {
                    if h.x1 <= a || h.x0 >= b {
                        kept.push((a, b));
                        continue;
                    }
                    if h.x0 > a {
                        kept.push((a, h.x0));
                    }
                    if h.x1 < b {
                        kept.push((h.x1, b));
                    }
                }
                pieces = kept;
            }
            for &(x0, x1) in &pieces {
                let mut left = None;
                for x in x0..x1 {
                    let (c, inside) = self.at(x, y);
                    if inside {
                        left = Some(x);
                        break;
                    }
                    if c > 0 {
                        put(y, x, x + 1, c);
                    }
                }
                let Some(left) = left else { continue };
                let mut right = x1 - 1;
                while right > left {
                    let (c, inside) = self.at(right, y);
                    if inside {
                        break;
                    }
                    if c > 0 {
                        put(y, right, right + 1, c);
                    }
                    right -= 1;
                }
                if self.stroke.is_none() {
                    put(y, left, right + 1, 255);
                }
            }
        }
    }

    /// Multiplies `mask` (whose top-left pixel is window pixel `origin`) by the shape's coverage: the clip of nested
    /// clips is the product of their coverage.
    pub(crate) fn clip(&self, mask: &mut Mask, origin: (i32, i32)) {
        let (w, h) = (mask.width() as i32, mask.height() as i32);
        let all = Bounds {
            x0: origin.0,
            y0: origin.1,
            x1: origin.0 + w,
            y1: origin.1 + h,
        };
        let inside = all.intersect(&self.bounds);
        let mut cover = vec![0u8; (w * h) as usize];
        let at = |x: i32, y: i32| ((y - origin.1) * w + (x - origin.0)) as usize;
        self.scan(&inside, &[], |y, x0, x1, c| cover[at(x0, y)..at(x1, y)].fill(c));
        for (m, c) in mask.data_mut().iter_mut().zip(&cover) {
            *m = match *c {
                255 => *m,
                0 => 0,
                c => div255(u32::from(*m) * u32::from(c)) as u8,
            };
        }
    }
}

/// What a fill lays through its coverage: one premultiplied colour, or a colour per pixel.
pub(crate) enum Shade<'a> {
    Solid([u8; 4]),
    /// Premultiplied pixels (bytes in memory order) of the window rectangle `area`, row after row.
    Pixels { data: &'a [u32], area: Bounds },
}

impl Shade<'_> {
    #[inline]
    fn at(&self, x: i32, y: i32) -> u32 {
        match self {
            Shade::Solid(c) => u32::from_le_bytes(*c),
            Shade::Pixels { data, area } => {
                data[((y - area.y0) * area.width() as i32 + (x - area.x0)) as usize]
            }
        }
    }
}

/// A gradient paint placed on the window's pixels: the shared rule (`cupertino_raster_math::gradient`), evaluated at
/// each pixel's centre in the node's coordinates. Offsets are held to 0 to 1 and made to increase, as they were given
/// in order; a single stop is that colour everywhere. More than [`MAX_STOPS`] stops are refused by the definition,
/// and such a paint draws nothing.
pub(crate) struct Gradient {
    stops: Stops,
    /// Window pixel position to node coordinates.
    inv: [f32; 6],
    kind: GradientKind,
}

enum GradientKind {
    Linear { start: [f32; 2], end: [f32; 2] },
    Radial { centre: [f32; 2], radius: f32 },
}

impl Gradient {
    /// `paint` placed by `window` (node coordinates to window pixels); `None` for a solid paint, a paint with no stops
    /// or more than [`MAX_STOPS`], or a transform that cannot be inverted.
    pub(crate) fn new(paint: &crate::Paint, window: tiny_skia::Transform) -> Option<Gradient> {
        let (given, kind) = match paint {
            crate::Paint::Solid(_) => return None,
            crate::Paint::Linear { start, end, stops } => (
                stops,
                GradientKind::Linear {
                    start: [start.x as f32, start.y as f32],
                    end: [end.x as f32, end.y as f32],
                },
            ),
            crate::Paint::Radial {
                center,
                radius,
                stops,
            } => (
                stops,
                GradientKind::Radial {
                    centre: [center.x as f32, center.y as f32],
                    radius: *radius as f32,
                },
            ),
        };
        if given.is_empty() || given.len() > MAX_STOPS {
            return None;
        }
        let mut last = 0.0f32;
        let mut stops: Vec<Stop> = given
            .iter()
            .map(|s| {
                last = s.offset.clamp(0.0, 1.0).max(last);
                let c = s.color;
                Stop {
                    offset: last,
                    color: [c.r, c.g, c.b, c.a].map(|v| v.clamp(0.0, 1.0)),
                }
            })
            .collect();
        if stops.len() == 1 {
            stops.push(stops[0]);
        }
        let stops = Stops::new(&stops).ok()?;
        let (a, b, c, d, e, f) = (window.sx, window.ky, window.kx, window.sy, window.tx, window.ty);
        let det = a * d - b * c;
        if !(det.abs() > 1e-12 && det.is_finite()) {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Some(Gradient {
            stops,
            inv: [ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f)],
            kind,
        })
    }

    /// The premultiplied colour at window pixel `(x, y)`.
    #[inline]
    fn at(&self, x: i32, y: i32) -> [f32; 4] {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let [a, b, c, d, e, f] = self.inv;
        let p = [a * fx + c * fy + e, b * fx + d * fy + f];
        let t = match self.kind {
            GradientKind::Linear { start, end } => linear_t(p, start, end),
            GradientKind::Radial { centre, radius } => radial_t(p, centre, radius),
        };
        self.stops.at(t)
    }

    /// The colour of every pixel of `area` (window pixels), row after row, in premultiplied bytes; with `dithered`
    /// its colour channels are dithered by position before they are stored (its alpha is rounded).
    pub(crate) fn shade(&self, area: &Bounds, dithered: bool) -> Vec<u32> {
        let mut out = Vec::with_capacity(area.width() as usize * area.height() as usize);
        for y in area.y0..area.y1 {
            for x in area.x0..area.x1 {
                let c = self.at(x, y);
                let alpha = dither::quantize(c[3]);
                let channel = |v: f32| {
                    let stored = if dithered {
                        dither::store(v, x as u32, y as u32)
                    } else {
                        dither::quantize(v)
                    };
                    // premultiplied: no channel above the alpha
                    stored.min(alpha)
                };
                out.push(u32::from_le_bytes([
                    channel(c[0]),
                    channel(c[1]),
                    channel(c[2]),
                    alpha,
                ]));
            }
        }
        out
    }
}

/// A colour premultiplied and in bytes, as fills lay it.
pub(crate) fn premultiplied(c: cupertino_theme::Color) -> Option<[u8; 4]> {
    if ![c.r, c.g, c.b, c.a].iter().all(|v| v.is_finite()) {
        return None;
    }
    Some(c.premultiplied().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
}

/// The premultiplied `color` laid over the pixel `below` at full coverage, as [`fill`] lays it.
pub(crate) fn laid(color: [u8; 4], below: [u8; 4]) -> [u8; 4] {
    if color[3] == 255 {
        return color;
    }
    over_px(u32::from_le_bytes(color), u32::from_le_bytes(below)).to_le_bytes()
}

/// Lays `shade` through the coverage of `outline` over the pixmap whose top-left pixel is window pixel `origin`,
/// inside `limit` (window pixels) less `holes`, scaled by `mask` (pixmap-sized) when there is one.
pub(crate) fn fill(
    pm: &mut PixmapMut<'_>,
    origin: (i32, i32),
    limit: &Bounds,
    outline: &Outline,
    shade: &Shade<'_>,
    mask: Option<&Mask>,
    holes: &[Bounds],
) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let target = Bounds {
        x0: origin.0,
        y0: origin.1,
        x1: origin.0 + w,
        y1: origin.1 + h,
    };
    let area = limit.intersect(&target);
    let stride = w as usize;
    let mask = mask.map(Mask::data);
    let data = pm.data_mut();
    let pixels = data.as_chunks_mut::<4>().0;
    let index = |x: i32, y: i32| (y - origin.1) as usize * stride + (x - origin.0) as usize;
    let lay = |px: &mut [u8; 4], s: u32, k: u32| {
        let s = if k == 255 { s } else { mul_px(s, k) };
        *px = if s >> 24 == 255 {
            s
        } else {
            over_px(s, u32::from_le_bytes(*px))
        }
        .to_le_bytes();
    };
    outline.scan(&area, holes, |y, x0, x1, c| {
        let (start, end) = (index(x0, y), index(x1, y));
        let row = &mut pixels[start..end];
        match (shade, mask) {
            (Shade::Solid(s), None) if c == 255 && s[3] == 255 => row.fill(*s),
            (Shade::Solid(s), None) if c == 255 => {
                let s = u32::from_le_bytes(*s);
                for p in row {
                    *p = over_px(s, u32::from_le_bytes(*p)).to_le_bytes();
                }
            }
            _ => {
                for (j, p) in row.iter_mut().enumerate() {
                    let k = match mask {
                        Some(m) => div255(u32::from(m[start + j]) * u32::from(c)),
                        None => u32::from(c),
                    };
                    if k > 0 {
                        lay(p, shade.at(x0 + j as i32, y), k);
                    }
                }
            }
        }
    });
}

/// Lays `shade` through `cover` alone (a pixmap-sized coverage mask, a path's and its clips' together) over the pixmap
/// whose top-left pixel is window pixel `origin`, inside `area` (window pixels).
pub(crate) fn fill_masked(
    pm: &mut PixmapMut<'_>,
    origin: (i32, i32),
    area: &Bounds,
    shade: &Shade<'_>,
    cover: &Mask,
) {
    let stride = pm.width() as usize;
    let pixels = pm.data_mut().as_chunks_mut::<4>().0;
    let cover = cover.data();
    for y in area.y0..area.y1 {
        for x in area.x0..area.x1 {
            let i = (y - origin.1) as usize * stride + (x - origin.0) as usize;
            let k = u32::from(cover[i]);
            if k == 0 {
                continue;
            }
            let s = shade.at(x, y);
            let s = if k == 255 { s } else { mul_px(s, k) };
            pixels[i] = over_px(s, u32::from_le_bytes(pixels[i])).to_le_bytes();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::Rect;
    use cupertino_raster_math::vectors;

    fn place(scale: f32) -> tiny_skia::Transform {
        tiny_skia::Transform::from_scale(scale, scale)
    }

    /// Every coverage vector, for the shapes the renderer draws (circular corners), at the pixel whose centre is the
    /// vector's point: the shape is moved so that point falls on a pixel centre.
    #[test]
    fn coverage_matches_the_shared_vectors() {
        let mut checked = 0;
        for row in vectors::rows(vectors::COVERAGE) {
            let [px, py, hx, hy, extent, exponent, pixels, _, want] =
                row[..].try_into().expect("nine columns");
            if exponent > 2.001 {
                continue;
            }
            let rect = Rect::new(-hx, -hy, 2.0 * hx, 2.0 * hy);
            let shape = Shape::RoundedRect {
                rect,
                radius: extent,
            };
            // the vector's point at the centre of pixel (100, 100)
            let s = pixels as f32;
            let ts = tiny_skia::Transform::from_row(
                s,
                0.0,
                0.0,
                s,
                100.5 - px as f32 * s,
                100.5 - py as f32 * s,
            );
            let outline = Outline::new(&shape, ts, None).expect("placed");
            let (got, _) = outline.at(100, 100);
            let want = (want * 255.0 + 0.5) as u8;
            assert!(
                got.abs_diff(want) <= 1,
                "{row:?}: coverage {got}, the definition {want}"
            );
            checked += 1;
        }
        assert!(checked >= 48, "{checked}");
    }

    #[test]
    fn a_stroke_is_centred_on_its_outline_and_its_middle_is_hollow() {
        let shape = Shape::Rect(Rect::new(10.0, 10.0, 20.0, 20.0));
        let outline = Outline::new(&shape, place(1.0), Some(2.0)).unwrap();
        // the stroke covers 9..11 along the left edge
        assert_eq!(outline.at(9, 20).0, 255);
        assert_eq!(outline.at(10, 20).0, 255);
        assert_eq!(outline.at(8, 20).0, 0);
        assert_eq!(outline.at(11, 20), (0, true));
        // square corners
        assert_eq!(outline.at(9, 9).0, 255);
        let mut covered = 0;
        outline.scan(&outline.bounds, &[], |_, x0, x1, _| covered += x1 - x0);
        // the ring of 22 x 22 less 18 x 18 pixels
        assert_eq!(covered, 22 * 22 - 18 * 18);
    }

    #[test]
    fn nested_clips_multiply_their_coverage() {
        let half = Shape::Rect(Rect::new(0.5, 0.0, 4.0, 4.0));
        let outline = Outline::new(&half, place(1.0), None).unwrap();
        let mut mask = Mask::new(4, 4).unwrap();
        mask.data_mut().fill(255);
        outline.clip(&mut mask, (0, 0));
        assert_eq!(mask.data()[0], 128);
        outline.clip(&mut mask, (0, 0));
        assert_eq!(mask.data()[0], 64);
        assert_eq!(mask.data()[1], 255);
    }

    #[test]
    fn a_turned_rectangle_is_covered_by_its_distance() {
        let shape = Shape::Rect(Rect::new(-10.0, -10.0, 20.0, 20.0));
        let turn = tiny_skia::Transform::from_rotate(45.0).post_translate(50.0, 50.0);
        let outline = Outline::new(&shape, turn, None).unwrap();
        assert_eq!(outline.at(50, 50).0, 255);
        // the corner of the turned square points straight up, 14.1 pixels over the centre
        assert_eq!(outline.at(49, 30).0, 0);
        let edge = outline.at(50, 36).0;
        assert!(edge > 0 && edge < 255, "{edge}");
    }
    #[test]
    fn gradients_match_the_shared_vectors() {
        use crate::{GradientStop, Paint};
        use cupertino_geometry::Point;
        let sets = vectors::gradient_stops();
        let mut checked = 0;
        for row in vectors::rows(vectors::GRADIENT) {
            let [g, t, r, gr, b, a] = row[..].try_into().expect("six columns");
            let stops: Vec<GradientStop> = sets[g as usize]
                .as_slice()
                .iter()
                .map(|s| GradientStop {
                    offset: s.offset,
                    color: cupertino_theme::Color::new(s.color[0], s.color[1], s.color[2], s.color[3]),
                })
                .collect();
            // a linear gradient 1000 pixels long placed so that the centre of pixel 0 lies at the vector's `t`
            let start = 0.5 - 1000.0 * t;
            let paint = Paint::Linear {
                start: Point::new(start, 0.0),
                end: Point::new(start + 1000.0, 0.0),
                stops,
            };
            let gradient = Gradient::new(&paint, tiny_skia::Transform::identity()).expect("placed");
            let at = gradient.at(0, 0);
            let want = [r, gr, b, a].map(|v| v as f32);
            for c in 0..4 {
                assert!((at[c] - want[c]).abs() < 1e-3, "{row:?}: {at:?}");
            }
            checked += 1;
        }
        assert!(checked > 20, "{checked}");
    }

    #[test]
    fn a_gradient_is_dithered_by_position_and_keeps_its_exact_values() {
        use crate::{GradientStop, Paint};
        use cupertino_geometry::Point;
        let grey = |v: f32| cupertino_theme::Color::new(v, v, v, 1.0);
        // a shallow ramp, from 100 to 101 over 64 pixels
        let paint = Paint::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(64.0, 0.0),
            stops: vec![
                GradientStop { offset: 0.0, color: grey(100.0 / 255.0) },
                GradientStop { offset: 1.0, color: grey(101.0 / 255.0) },
            ],
        };
        let g = Gradient::new(&paint, tiny_skia::Transform::identity()).unwrap();
        let area = Bounds { x0: 0, y0: 0, x1: 64, y1: 8 };
        let plain = g.shade(&area, false);
        let dithered = g.shade(&area, true);
        let red = |v: &u32| v.to_le_bytes()[0];
        // undithered, the ramp is one step at its middle; dithered, every column's mean follows the ramp
        assert_eq!(plain.iter().filter(|v| red(v) == 101).count(), 32 * 8);
        for x in [8usize, 24, 40, 56] {
            let column: u32 = (0..8).map(|y| u32::from(red(&dithered[y * 64 + x]))).sum();
            let want = 100.0 + (x as f32 + 0.5) / 64.0;
            assert!((column as f32 / 8.0 - want).abs() <= 0.25, "{x}: {column}");
        }
        // the alpha is never dithered, and every pixel is the definition's store of the colour at its centre
        for (i, v) in dithered.iter().enumerate() {
            let (x, y) = ((i % 64) as u32, (i / 64) as u32);
            let c = g.at(x as i32, y as i32);
            assert_eq!(v.to_le_bytes()[3], 255);
            assert_eq!(red(v), dither::store(c[0], x, y));
        }
        // more than eight stops are refused
        let many = Paint::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(1.0, 0.0),
            stops: vec![GradientStop { offset: 0.0, color: grey(0.0) }; 9],
        };
        assert!(Gradient::new(&many, tiny_skia::Transform::identity()).is_none());
    }
}
