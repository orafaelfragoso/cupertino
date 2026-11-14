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
}
