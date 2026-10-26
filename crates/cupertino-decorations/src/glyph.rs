//! Glyph rasterisation after the design's CPU text path (§3, §4, §10.1, §11;
//! register): the outline is dilated ("stem darkening") by edge translation,
//! then scan-converted to 8-bit coverage.
//!
//! * Dilation (provisional): every glyph draw sets the antialiasing style 0x400 with a pair that depends on the
//!   fill colour, so the smoothing-level table does not draw text: d = min((0.0058 + 0.01 · Yq) · s, 0.3)
//!   device pixels on both axes, s the font size in device pixels, Yq the fill colour's linear luminance quantised to
//!   quarters. Text the design draws through its declarative toolkit's renderer floors s to a power of 1.25 first
//!   ([`declarative_size`]).
//! * Geometry: every edge is translated by (sign(edge dy) · d, −sign(edge dx) · d + d) in device
//!   axes with y up, for counter-clockwise outer contours: left and right edges move out by d, bottom edges stay, top
//!   edges rise 2 · d. Curves are flattened first and each flattened segment is translated by its own direction;
//!   the translated segments are joined end to start (the join is not known, approximate).
//! * Coverage: the exact area of the polygon in each pixel (signed-area accumulation), so a translated edge that
//!   covers a fraction f of a pixel adds f; the level table is an identity ramp. Overlapping contours of a
//!   variable font add and clamp to 1.

use swash::zeno::{Point, Verb};

/// Whether glyphs are dilated: on while font smoothing is allowed, the default; off only when the
/// smoothing preference holds a value other than 1, 2 or 3 (no such key exists here yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Smoothing {
    Off,
    On,
}

/// The largest dilation, device pixels.
const DILATION_CAP: f32 = 0.3;

/// The dilation pair's base and luminance term (arithmetic, provisional).
const DILATION_BASE: f32 = 0.0058;
const DILATION_PER_LUMINANCE: f32 = 0.01;

/// The fill colour's luminance as the dilation reads it: linear sRGB, Rec. 709 weights, alpha ignored, rounded half away
/// from zero to a quarter. `rgb` is the sRGB-encoded colour, 0 to 1.
pub fn quantized_luminance(rgb: [f32; 3]) -> f32 {
    let linear = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let y = 0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2]);
    (4.0 * y).round() / 4.0
}

/// The size the declarative toolkit's renderer dilates with: `s` floored to a power of 1.25,
/// s' = 1.25^floor(log_1.25 s) (formula, that s is the font's device pixel size is there).
pub fn declarative_size(s: f32) -> f32 {
    const STEP: f32 = 1.25;
    STEP.powf((s.ln() / STEP.ln()).floor())
}

impl Smoothing {
    /// The dilation d (both axes) in device pixels of text `s` device pixels per em in a fill whose quantised luminance
    /// is `yq`.
    pub fn dilation(self, s: f32, yq: f32) -> f32 {
        match self {
            Smoothing::Off => 0.0,
            Smoothing::On => ((DILATION_BASE + DILATION_PER_LUMINANCE * yq) * s).min(DILATION_CAP),
        }
    }
}

/// Subpixel positions per pixel of a glyph origin in x and y at pixel size `px`:
/// x = min(5, ceil(1 / (0.03 · px))), y = min(5, ceil(1 / (0.12 · px))), the default quantisation with both
/// preference limits at 5.
pub fn quantization(px: f32) -> (u8, u8) {
    let levels = |k: f32| (1.0 / (k * px)).ceil().clamp(1.0, 5.0) as u8;
    (levels(0.03), levels(0.12))
}

/// 8-bit coverage of one glyph. `left` and `top` place it: pixel (0, 0) is `left` pixels right of the origin and
/// `top` pixels above the baseline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// The flattening tolerance of curves, device pixels (ours, approximate; below what 8-bit coverage resolves).
const TOLERANCE: f32 = 0.02;

/// The contours of the path `points` / `verbs` as closed polygons in device pixels, y up, shifted by `offset`.
fn polygons(points: &[Point], verbs: &[Verb], offset: (f32, f32)) -> Vec<Vec<Point>> {
    let shift = |p: Point| Point::new(p.x + offset.0, p.y + offset.1);
    let mut contours: Vec<Vec<Point>> = Vec::new();
    let mut i = 0;
    for verb in verbs {
        match verb {
            Verb::MoveTo => {
                contours.push(vec![shift(points[i])]);
                i += 1;
            }
            Verb::LineTo => {
                contours.last_mut().expect("an outline starts with a move").push(shift(points[i]));
                i += 1;
            }
            Verb::QuadTo => {
                let c = contours.last_mut().expect("an outline starts with a move");
                let (p0, p1, p2) = (*c.last().expect("a contour has its start"), shift(points[i]), shift(points[i + 1]));
                let dd = (p0 - p1 * 2.0 + p2).length();
                let n = ((dd / (8.0 * TOLERANCE)).sqrt().ceil() as usize).clamp(1, 64);
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    let u = 1.0 - t;
                    c.push(p0 * (u * u) + p1 * (2.0 * u * t) + p2 * (t * t));
                }
                i += 2;
            }
            Verb::CurveTo => {
                let c = contours.last_mut().expect("an outline starts with a move");
                let (p0, p1, p2, p3) = (*c.last().expect("a contour has its start"), shift(points[i]), shift(points[i + 1]), shift(points[i + 2]));
                let dd = (p0 - p1 * 2.0 + p2).length().max((p1 - p2 * 2.0 + p3).length());
                let n = ((3.0 * dd / (4.0 * TOLERANCE)).sqrt().ceil() as usize).clamp(1, 64);
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    let u = 1.0 - t;
                    c.push(p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t));
                }
                i += 3;
            }
            Verb::Close => {}
        }
    }
    contours.retain(|c| c.len() >= 3);
    contours
}

/// Twice the signed area of the polygons, y up: positive when the outer contours run counter-clockwise.
fn signed_area(contours: &[Vec<Point>]) -> f32 {
    contours.iter().map(|c| (0..c.len()).map(|k| {
        let (a, b) = (c[k], c[(k + 1) % c.len()]);
        a.x * b.y - b.x * a.y
    }).sum::<f32>()).sum()
}

/// The edges of `contours` after the dilation `(dx, dy) of: each edge translated by its own direction, the
/// translated edges joined end to start. `orientation` is +1 for counter-clockwise outer contours, −1 for clockwise
/// ones (TrueType), so that the symmetric part always grows the ink; the +dy shift is upward either way.
fn dilated_edges(contours: &[Vec<Point>], (dx, dy): (f32, f32), orientation: f32) -> Vec<(Point, Point)> {
    let sign = |v: f32| if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 };
    let mut edges = Vec::new();
    for c in contours {
        let moved: Vec<(Point, Point)> = (0..c.len())
            .map(|k| {
                let (a, b) = (c[k], c[(k + 1) % c.len()]);
                let t = Point::new(orientation * sign(b.y - a.y) * dx, -orientation * sign(b.x - a.x) * dy + dy);
                (a + t, b + t)
            })
            .filter(|(a, b)| a != b)
            .collect();
        for k in 0..moved.len() {
            let (edge, next) = (moved[k], moved[(k + 1) % moved.len()]);
            edges.push(edge);
            if edge.1 != next.0 {
                edges.push((edge.1, next.0));
            }
        }
    }
    edges
}

/// Rasterise the outline `points` / `verbs` (pixels, y up, origin on the baseline; a scaled glyph outline) at the
/// subpixel `offset` with the dilation `dilation`. `None` for an empty outline.
pub fn rasterize(points: &[Point], verbs: &[Verb], offset: (f32, f32), dilation: (f32, f32)) -> Option<Mask> {
    let contours = polygons(points, verbs, offset);
    if contours.is_empty() {
        return None;
    }
    let orientation = if signed_area(&contours) >= 0.0 { 1.0 } else { -1.0 };
    let edges = dilated_edges(&contours, dilation, orientation);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (a, b) in &edges {
        for p in [a, b] {
            (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
        }
    }
    let (left, top) = (x0.floor() as i32, y1.ceil() as i32);
    let (width, height) = ((x1.ceil() as i32 - left).max(1) as usize, (top - y0.floor() as i32).max(1) as usize);
    // two spare columns: a segment adds to the pixel right of the last one it touches
    let stride = width + 2;
    let mut acc = vec![0.0f32; stride * height];
    for (a, b) in edges {
        // raster space: x from the left column, y down from the top row
        let to_raster = |p: Point| (p.x - left as f32, top as f32 - p.y);
        line(&mut acc, stride, height, to_raster(a), to_raster(b));
    }
    let mut data = vec![0u8; width * height];
    for row in 0..height {
        let mut sum = 0.0;
        for col in 0..width {
            sum += acc[row * stride + col];
            data[row * width + col] = (sum.abs().min(1.0) * 255.0).round() as u8;
        }
    }
    Some(Mask { left, top, width: width as u32, height: height as u32, data })
}

/// Add the signed area of the segment `p0`–`p1` (raster space) to the accumulation buffer `acc`: after a running sum
/// along each row the buffer holds the coverage of the closed polygon (the signed-area method of font-rs).
fn line(acc: &mut [f32], stride: usize, height: usize, p0: (f32, f32), p1: (f32, f32)) {
    if p0.1 == p1.1 {
        return;
    }
    let (dir, p0, p1) = if p0.1 < p1.1 { (1.0, p0, p1) } else { (-1.0, p1, p0) };
    let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
    let mut x = p0.0;
    let y_start = p0.1.max(0.0);
    if p0.1 < 0.0 {
        x -= p0.1 * dxdy;
    }
    let mut add = |i: isize, v: f32| {
        if i >= 0 && (i as usize) < acc.len() {
            acc[i as usize] += v;
        }
    };
    for y in (y_start as usize)..height.min(p1.1.ceil() as usize) {
        let row = (y * stride) as isize;
        let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
        let x_next = x + dxdy * dy;
        let d = dy * dir;
        let (xa, xb) = if x < x_next { (x, x_next) } else { (x_next, x) };
        let xa_floor = xa.floor();
        let xa_i = xa_floor as isize;
        let xb_ceil = xb.ceil();
        let xb_i = xb_ceil as isize;
        if xb_i <= xa_i + 1 {
            let xm = 0.5 * (x + x_next) - xa_floor;
            add(row + xa_i, d - d * xm);
            add(row + xa_i + 1, d * xm);
        } else {
            let s = (xb - xa).recip();
            let xa_f = xa - xa_floor;
            let a0 = 0.5 * s * (1.0 - xa_f) * (1.0 - xa_f);
            let xb_f = xb - xb_ceil + 1.0;
            let am = 0.5 * s * xb_f * xb_f;
            add(row + xa_i, d * a0);
            if xb_i == xa_i + 2 {
                add(row + xa_i + 1, d * (1.0 - a0 - am));
            } else {
                let a1 = s * (1.5 - xa_f);
                add(row + xa_i + 1, d * (a1 - a0));
                for xi in xa_i + 2..xb_i - 1 {
                    add(row + xi, d * s);
                }
                let a2 = a1 + (xb_i - xa_i - 3) as f32 * s;
                add(row + xb_i - 1, d * (1.0 - a2 - am));
            }
            add(row + xb_i, d * am);
        }
        x = x_next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Add a square contour from (x0, y0) to (x1, y1), y up, counter-clockwise when `ccw`.
    fn square(path: &mut (Vec<Point>, Vec<Verb>), [x0, y0, x1, y1]: [f32; 4], ccw: bool) {
        let mut corners = vec![Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)];
        if !ccw {
            corners.reverse();
        }
        path.0.extend(corners);
        path.1.extend([Verb::MoveTo, Verb::LineTo, Verb::LineTo, Verb::LineTo, Verb::Close]);
    }

    fn raster(path: &(Vec<Point>, Vec<Verb>), dilation: (f32, f32)) -> Mask {
        rasterize(&path.0, &path.1, (0.0, 0.0), dilation).expect("a non-empty path")
    }

    /// The coverage of the pixel whose lower-left corner is at (x, y), y up; 0 outside the mask.
    fn at(m: &Mask, x: i32, y: i32) -> u8 {
        let (col, row) = (x - m.left, m.top - 1 - y);
        if col < 0 || row < 0 || col >= m.width as i32 || row >= m.height as i32 {
            return 0;
        }
        m.data[(row * m.width as i32 + col) as usize]
    }

    #[test]
    fn the_dilation_follows_the_fill_luminance_and_its_cap() {
        // worked values: 13 pt at 2x black 0.1508, white 0.3 (cap), 13 pt at 1x black 0.0754, 11 pt at 2x 0.1276
        let black = quantized_luminance([0.0, 0.0, 0.0]);
        let white = quantized_luminance([1.0, 1.0, 1.0]);
        assert_eq!((black, white), (0.0, 1.0));
        for (s, yq, d) in [(26.0, black, 0.1508), (26.0, white, 0.3), (13.0, black, 0.0754), (22.0, black, 0.1276)] {
            assert!((Smoothing::On.dilation(s, yq) - d).abs() < 1e-5, "{s} px, Yq {yq}: {}", Smoothing::On.dilation(s, yq));
        }
        assert_eq!(Smoothing::Off.dilation(26.0, white), 0.0);
        // the declarative renderer's 26 px becomes 1.25^14 = 22.74, so dark text gets 0.1319
        let s = declarative_size(26.0);
        assert!((s - 22.737).abs() < 1e-3, "{s}");
        assert!((Smoothing::On.dilation(s, black) - 0.1319).abs() < 1e-4);
        assert!((declarative_size(25.0) - 22.737).abs() < 1e-3 && (declarative_size(28.5) - 28.422).abs() < 1e-3, "floors, never rounds up");
        // greys: sRGB 0.5 is linear 0.214, 4Y = 0.856, so Yq = 0.25; sRGB 0.735 is linear 0.4995, 4Y = 1.998, so 0.5
        assert_eq!(quantized_luminance([0.5, 0.5, 0.5]), 0.25);
        assert!((Smoothing::On.dilation(26.0, quantized_luminance([0.5, 0.5, 0.5])) - 0.2158).abs() < 1e-5, "sRGB 0.5 grey at 26 px");
        // near-black ends below linear 0.125: sRGB 0.38 is linear 0.1195, so Yq = 0 and 0.1508 at 26 px
        assert_eq!(quantized_luminance([0.38, 0.38, 0.38]), 0.0);
        assert_eq!(quantized_luminance([0.735, 0.735, 0.735]), 0.5);
        // the accent blue (0, 122, 255): Y = 0.7152 · 0.1946 + 0.0722 = 0.2114, 4Y = 0.846, so Yq = 0.25
        assert_eq!(quantized_luminance([0.0, 122.0 / 255.0, 1.0]), 0.25);
        assert!((Smoothing::On.dilation(26.0, 0.25) - 0.2158).abs() < 1e-5);
    }

    #[test]
    fn origin_quantisation_follows_the_size_table() {
        // 6-8 px (5, 2), 9-11 (4, 1), 12-16 (3, 1), 17-24 (2, 1), one position from 34 px
        for (px, q) in [(6.0, (5, 2)), (8.0, (5, 2)), (9.0, (4, 1)), (11.0, (4, 1)), (12.0, (3, 1)), (13.0, (3, 1)), (16.0, (3, 1)), (17.0, (2, 1)), (24.0, (2, 1)), (26.0, (2, 1)), (34.0, (1, 1))] {
            assert_eq!(quantization(px), q, "{px} px");
        }
    }

    #[test]
    fn a_square_grows_sideways_and_upward_by_the_dilation() {
        // left and right edges out by dx, bottom edge in place, top edge 2 dy higher
        let (dx, dy) = (0.1573, 0.196625);
        for ccw in [true, false] {
            let mut path = (Vec::new(), Vec::new());
            square(&mut path, [2.0, 2.0, 8.0, 8.0], ccw);
            let m = raster(&path, (dx, dy));
            assert_eq!(at(&m, 1, 4), 40, "left column: dx = 0.1573 of the pixel ({ccw})");
            assert_eq!(at(&m, 8, 4), 40, "right column");
            assert_eq!(at(&m, 4, 8), 100, "row above the top: 2 dy = 0.3933 ({ccw})");
            assert_eq!(at(&m, 4, 1), 0, "nothing below the bottom edge");
            assert_eq!(at(&m, 4, 4), 255);
            assert_eq!(at(&m, 1, 8), 12, "corner, under the join: 1.5 dx dy = 0.0464");
        }
        let mut path = (Vec::new(), Vec::new());
        square(&mut path, [2.0, 2.0, 8.0, 8.0], true);
        let plain = raster(&path, (0.0, 0.0));
        assert_eq!((plain.left, plain.top, plain.width, plain.height), (2, 8, 6, 6), "no dilation: the square's own pixels");
        assert!(plain.data.iter().all(|&c| c == 255));
    }

    #[test]
    fn a_hole_shrinks_by_the_dilation() {
        // a frame: outer square counter-clockwise, inner square clockwise; the ink grows into the hole
        let mut path = (Vec::new(), Vec::new());
        square(&mut path, [0.0, 0.0, 10.0, 10.0], true);
        square(&mut path, [3.0, 3.0, 7.0, 7.0], false);
        let m = raster(&path, (0.3, 0.3));
        assert_eq!(at(&m, 3, 5), 77, "the hole's left column: 0.3 inked");
        assert_eq!(at(&m, 5, 6), 0, "the hole's top edge rises by 2 dy: its top row stays clear");
        assert_eq!(at(&m, 5, 3), 153, "the hole's bottom edge rises by 2 dy = 0.6");
    }
}
