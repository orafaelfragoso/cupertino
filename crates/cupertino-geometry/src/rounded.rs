//! Rounded rectangles with circular or continuous ("squircle") corners.
//!
//! the design uses circular corners by default and a continuous curve where a
//! layer asks for it (layer archives).
//! The exact continuous curve is unknown. This module models it as a superellipse quadrant:
//!
//! * the corner occupies a square of side `extent = expansion · radius` on each edge
//!   (`expansion` ≈ 1.528, approximate, community-documented, configurable);
//! * inside that square the boundary is `u^n + v^n = 1` (normalised to the square), with the
//!   exponent `n` chosen so that the curve passes through the same diagonal point as a circular arc
//!   of the nominal radius (approximate). `expansion = 1` gives `n = 2`, a circle, exactly.
//!
//! For `n > 2` the curvature is zero where the corner meets the straight edge (G² continuity), which
//! is the defining property of the continuous corner.
//!
//! Three representations are provided and tested against each other: cubic [`Path`] segments,
//! a reference signed distance function ([`RoundedRect::sdf`]) and a closed-form first-order
//! approximation ([`RoundedRect::sdf_approx`]) that matches the shader implementation.

use crate::types::{Point, Rect};
use std::collections::HashMap;
use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2};
use std::sync::{Mutex, OnceLock};

/// Cubic Bézier handle length for a quarter circle of unit radius: 4/3·tan(π/8).
pub const CIRCLE_KAPPA: f64 = 0.552_284_749_830_793_4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CornerStyle {
    Circular,
    /// `expansion` = corner extent / nominal radius; values ≤ 1 degenerate to a circle.
    Continuous {
        expansion: f64,
    },
}

impl CornerStyle {
    /// approximate (community-documented value, to be verified at run time).
    pub const DEFAULT_EXPANSION: f64 = 1.528;

    pub const fn continuous() -> Self {
        CornerStyle::Continuous {
            expansion: Self::DEFAULT_EXPANSION,
        }
    }

    pub fn expansion(self) -> f64 {
        match self {
            CornerStyle::Circular => 1.0,
            CornerStyle::Continuous { expansion } => expansion.max(1.0),
        }
    }

    pub fn is_circular(self) -> bool {
        self.expansion() <= 1.0 + 1e-9
    }

    /// Superellipse exponent `n` (2 for a circle).
    pub fn exponent(self) -> f64 {
        let k = self.expansion();
        if k <= 1.0 + 1e-9 {
            return 2.0;
        }
        std::f64::consts::LN_2 / -(1.0 - (1.0 - FRAC_1_SQRT_2) / k).ln()
    }
}

/// Per-corner nominal radii.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Radii {
    pub top_left: f64,
    pub top_right: f64,
    pub bottom_right: f64,
    pub bottom_left: f64,
}

impl Radii {
    pub const fn new(top_left: f64, top_right: f64, bottom_right: f64, bottom_left: f64) -> Self {
        Self {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }

    pub const fn uniform(r: f64) -> Self {
        Self::new(r, r, r, r)
    }

    /// Only the top corners rounded (typical window shape with a square bottom edge).
    pub const fn top(r: f64) -> Self {
        Self::new(r, r, 0.0, 0.0)
    }

    pub const fn bottom(r: f64) -> Self {
        Self::new(0.0, 0.0, r, r)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundedRect {
    pub rect: Rect,
    pub radii: Radii,
    pub style: CornerStyle,
}

#[derive(Clone, Copy, Debug)]
pub struct PathOptions {
    /// Cubic segments per continuous corner (circular corners always use one).
    pub segments_per_corner: usize,
}

impl Default for PathOptions {
    fn default() -> Self {
        Self {
            segments_per_corner: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathSeg {
    MoveTo(Point),
    LineTo(Point),
    CubicTo(Point, Point, Point),
    Close,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub segments: Vec<PathSeg>,
}

impl Path {
    pub fn is_closed(&self) -> bool {
        matches!(self.segments.last(), Some(PathSeg::Close))
    }

    /// Start point of the first sub-path.
    pub fn start(&self) -> Option<Point> {
        match self.segments.first() {
            Some(PathSeg::MoveTo(p)) => Some(*p),
            _ => None,
        }
    }

    /// End point of the last drawing segment (before `Close`).
    pub fn end(&self) -> Option<Point> {
        self.segments.iter().rev().find_map(|s| match s {
            PathSeg::MoveTo(p) | PathSeg::LineTo(p) | PathSeg::CubicTo(_, _, p) => Some(*p),
            PathSeg::Close => None,
        })
    }

    pub fn cubic_count(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| matches!(s, PathSeg::CubicTo(..)))
            .count()
    }

    /// Polyline approximation with maximum deviation `tolerance`.
    pub fn flatten(&self, tolerance: f64) -> Vec<Point> {
        let mut out = Vec::new();
        let mut cur = Point::ZERO;
        for s in &self.segments {
            match *s {
                PathSeg::MoveTo(p) | PathSeg::LineTo(p) => {
                    out.push(p);
                    cur = p;
                }
                PathSeg::CubicTo(c1, c2, p) => {
                    let dd = (cur - c1 * 2.0 + c2)
                        .length()
                        .max((c1 - c2 * 2.0 + p).length());
                    let n =
                        ((0.75 * dd / tolerance.max(1e-9)).sqrt().ceil() as usize).clamp(1, 512);
                    for i in 1..=n {
                        out.push(cubic_at(cur, c1, c2, p, i as f64 / n as f64));
                    }
                    cur = p;
                }
                PathSeg::Close => {}
            }
        }
        out
    }

    pub fn bounds(&self) -> Rect {
        let pts = self.flatten(0.01);
        let Some(first) = pts.first() else {
            return Rect::ZERO;
        };
        let (mut l, mut t, mut r, mut b) = (first.x, first.y, first.x, first.y);
        for p in &pts {
            l = l.min(p.x);
            r = r.max(p.x);
            t = t.min(p.y);
            b = b.max(p.y);
        }
        Rect::from_ltrb(l, t, r, b)
    }
}

pub fn cubic_at(p0: Point, p1: Point, p2: Point, p3: Point, t: f64) -> Point {
    let m = 1.0 - t;
    p0 * (m * m * m) + p1 * (3.0 * m * m * t) + p2 * (3.0 * m * t * t) + p3 * (t * t * t)
}

type Pt = (f64, f64);
type Cubic = [Pt; 4];

fn superellipse_norm(u: f64, v: f64, n: f64) -> f64 {
    (u.max(0.0).powf(n) + v.max(0.0).powf(n)).powf(1.0 / n)
}

/// Point on the normalised quadrant at polar angle `phi` measured from the `u` axis.
fn quadrant_point(phi: f64, n: f64) -> Pt {
    let (s, c) = phi.sin_cos();
    let rho = (c.max(0.0).powf(n) + s.max(0.0).powf(n)).powf(-1.0 / n);
    (rho * c.max(0.0), rho * s.max(0.0))
}

/// Unit tangent in the direction of increasing `u` (decreasing `v`).
fn quadrant_tangent(p: Pt, n: f64) -> Pt {
    let (a, b) = (p.1.max(0.0).powf(n - 1.0), p.0.max(0.0).powf(n - 1.0));
    let l = a.hypot(b).max(1e-300);
    (a / l, -b / l)
}

fn cubic_pt(c: &Cubic, t: f64) -> Pt {
    let m = 1.0 - t;
    let (a, b, cc, d) = (m * m * m, 3.0 * m * m * t, 3.0 * m * t * t, t * t * t);
    (
        a * c[0].0 + b * c[1].0 + cc * c[2].0 + d * c[3].0,
        a * c[0].1 + b * c[1].1 + cc * c[2].1 + d * c[3].1,
    )
}

/// First-order distance of a normalised point to the unit superellipse (positive outside).
fn norm_error(p: Pt, n: f64) -> f64 {
    let g = superellipse_norm(p.0, p.1, n);
    if g < 1e-12 {
        return -1.0;
    }
    let grad = (p.0.max(0.0).powf(2.0 * n - 2.0) + p.1.max(0.0).powf(2.0 * n - 2.0)).sqrt()
        / g.powf(n - 1.0);
    (g - 1.0) / grad.max(1e-12)
}

fn fit_segment(p0: Pt, t0: Pt, p1: Pt, t1: Pt, n: f64) -> Cubic {
    let chord = (p1.0 - p0.0).hypot(p1.1 - p0.1);
    let build = |h0: f64, h1: f64| -> Cubic {
        [
            p0,
            (p0.0 + t0.0 * h0, p0.1 + t0.1 * h0),
            (p1.0 - t1.0 * h1, p1.1 - t1.1 * h1),
            p1,
        ]
    };
    let err = |c: &Cubic| -> f64 {
        (1..=11)
            .map(|i| norm_error(cubic_pt(c, i as f64 / 12.0), n).abs())
            .fold(0.0, f64::max)
    };
    let mut best = (chord / 3.0, chord / 3.0);
    let mut best_err = err(&build(best.0, best.1));
    let mut span = chord * 0.5;
    for _ in 0..6 {
        let center = best;
        for i in -4i32..=4 {
            for j in -4i32..=4 {
                let h0 = (center.0 + i as f64 * span / 4.0).max(1e-4 * chord);
                let h1 = (center.1 + j as f64 * span / 4.0).max(1e-4 * chord);
                let e = err(&build(h0, h1));
                if e < best_err {
                    best_err = e;
                    best = (h0, h1);
                }
            }
        }
        span *= 0.4;
    }
    build(best.0, best.1)
}

/// Normalised quadrant from `A = (0, 1)` to `B = (1, 0)` as cubic segments. The fit is cached per
/// `(expansion, segments)` because it is far more expensive than emitting the path.
fn quadrant_cubics(style: CornerStyle, segments: usize) -> Vec<Cubic> {
    if style.is_circular() {
        return vec![[
            (0.0, 1.0),
            (CIRCLE_KAPPA, 1.0),
            (1.0, CIRCLE_KAPPA),
            (1.0, 0.0),
        ]];
    }
    type Cache = Mutex<HashMap<(u64, usize), Vec<Cubic>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let key = (style.expansion().to_bits(), segments.max(1));
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return v.clone();
    }
    let v = fit_quadrant(style, segments);
    cache.lock().unwrap().insert(key, v.clone());
    v
}

fn fit_quadrant(style: CornerStyle, segments: usize) -> Vec<Cubic> {
    let n = style.exponent();
    let segs = segments.max(1);
    let pts: Vec<Pt> = (0..=segs)
        .map(|k| quadrant_point(FRAC_PI_2 * (1.0 - k as f64 / segs as f64), n))
        .collect();
    let mut tans: Vec<Pt> = pts.iter().map(|&p| quadrant_tangent(p, n)).collect();
    tans[0] = (1.0, 0.0);
    tans[segs] = (0.0, -1.0);
    (0..segs)
        .map(|k| fit_segment(pts[k], tans[k], pts[k + 1], tans[k + 1], n))
        .collect()
}

impl RoundedRect {
    pub fn new(rect: Rect, radius: f64, style: CornerStyle) -> Self {
        Self {
            rect,
            radii: Radii::uniform(radius),
            style,
        }
    }

    pub fn circular(rect: Rect, radius: f64) -> Self {
        Self::new(rect, radius, CornerStyle::Circular)
    }

    pub fn continuous(rect: Rect, radius: f64) -> Self {
        Self::new(rect, radius, CornerStyle::continuous())
    }

    pub fn with_radii(rect: Rect, radii: Radii, style: CornerStyle) -> Self {
        Self { rect, radii, style }
    }

    /// Corner extents (side of the square each corner occupies) after shrinking so that adjacent
    /// corners fit on every edge. Order: top-left, top-right, bottom-right, bottom-left.
    pub fn extents(&self) -> [f64; 4] {
        let k = self.style.expansion();
        let r = &self.radii;
        let mut e = [
            r.top_left.max(0.0) * k,
            r.top_right.max(0.0) * k,
            r.bottom_right.max(0.0) * k,
            r.bottom_left.max(0.0) * k,
        ];
        let (w, h) = (self.rect.width().max(0.0), self.rect.height().max(0.0));
        let mut f = 1.0f64;
        for (len, a, b) in [
            (w, e[0], e[1]),
            (h, e[1], e[2]),
            (w, e[3], e[2]),
            (h, e[0], e[3]),
        ] {
            if a + b > len && a + b > 0.0 {
                f = f.min(len / (a + b));
            }
        }
        e.iter_mut().for_each(|x| *x *= f);
        e
    }

    pub fn path(&self) -> Path {
        self.path_with(&PathOptions::default())
    }

    pub fn path_with(&self, opts: &PathOptions) -> Path {
        let [e_tl, e_tr, e_br, e_bl] = self.extents();
        let (l, t, r, b) = (
            self.rect.min_x(),
            self.rect.min_y(),
            self.rect.max_x(),
            self.rect.max_y(),
        );
        let quad = quadrant_cubics(self.style, opts.segments_per_corner);
        let mut segs = Vec::with_capacity(5 + 4 * quad.len());
        let mut cur = Point::new(l + e_tl, t);
        segs.push(PathSeg::MoveTo(cur));
        let line = |segs: &mut Vec<PathSeg>, cur: &mut Point, to: Point| {
            if cur.distance(to) > 1e-12 {
                segs.push(PathSeg::LineTo(to));
                *cur = to;
            }
        };
        // (centre, extent, sign of u, sign of v, reversed traversal)
        let corners = [
            (
                Point::new(r - e_tr, t + e_tr),
                e_tr,
                1.0,
                -1.0,
                false,
                Point::new(r - e_tr, t),
                Point::new(r, t + e_tr),
            ),
            (
                Point::new(r - e_br, b - e_br),
                e_br,
                1.0,
                1.0,
                true,
                Point::new(r, b - e_br),
                Point::new(r - e_br, b),
            ),
            (
                Point::new(l + e_bl, b - e_bl),
                e_bl,
                -1.0,
                1.0,
                false,
                Point::new(l + e_bl, b),
                Point::new(l, b - e_bl),
            ),
            (
                Point::new(l + e_tl, t + e_tl),
                e_tl,
                -1.0,
                -1.0,
                true,
                Point::new(l, t + e_tl),
                Point::new(l + e_tl, t),
            ),
        ];
        for (centre, e, su, sv, rev, enter, exit) in corners {
            line(&mut segs, &mut cur, enter);
            if e <= 0.0 {
                line(&mut segs, &mut cur, exit);
                continue;
            }
            let map = |p: Pt| Point::new(centre.x + e * su * p.0, centre.y + e * sv * p.1);
            let ordered: Vec<Cubic> = if rev {
                quad.iter()
                    .rev()
                    .map(|c| [c[3], c[2], c[1], c[0]])
                    .collect()
            } else {
                quad.clone()
            };
            for c in ordered {
                segs.push(PathSeg::CubicTo(map(c[1]), map(c[2]), map(c[3])));
            }
            cur = exit;
        }
        segs.push(PathSeg::Close);
        Path { segments: segs }
    }

    fn corner_for(&self, d: Point, ext: &[f64; 4]) -> f64 {
        match (d.x < 0.0, d.y < 0.0) {
            (true, true) => ext[0],
            (false, true) => ext[1],
            (false, false) => ext[2],
            (true, false) => ext[3],
        }
    }

    /// Reference signed distance (negative inside, positive outside) to the shape boundary.
    /// Exact for circular corners; for continuous corners the nearest point on the superellipse
    /// quadrant is found numerically (≈1e-9 accurate, slow).
    pub fn sdf(&self, p: Point) -> f64 {
        self.sdf_impl(p, false)
    }

    /// Closed-form first-order distance, identical to the shader implementation. Exact for circular
    /// corners and straight edges; for continuous corners it deviates from the true distance by well
    /// under 1 % within a few pixels of the boundary (see the unit tests).
    pub fn sdf_approx(&self, p: Point) -> f64 {
        self.sdf_impl(p, true)
    }

    fn sdf_impl(&self, p: Point, approx: bool) -> f64 {
        let ext = self.extents();
        let c = self.rect.center();
        let (hx, hy) = (self.rect.width() * 0.5, self.rect.height() * 0.5);
        let d = p - c;
        let e = self.corner_for(d, &ext);
        let qx = d.x.abs() - (hx - e);
        let qy = d.y.abs() - (hy - e);
        if qx <= 0.0 && qy <= 0.0 {
            return qx.max(qy) - e;
        }
        if qx <= 0.0 {
            return qy - e;
        }
        if qy <= 0.0 {
            return qx - e;
        }
        if e <= 0.0 {
            return qx.hypot(qy);
        }
        if self.style.is_circular() {
            return qx.hypot(qy) - e;
        }
        let n = self.style.exponent();
        if approx {
            corner_sdf_approx(qx, qy, e, n)
        } else {
            corner_sdf_exact(qx, qy, e, n)
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        self.sdf(p) <= 0.0
    }

    /// Anti-aliasing coverage in `[0, 1]` for a pixel centred at `p` with edge length `pixel`
    /// (`1/scale` in layout units): a linear ramp over one pixel centred on the boundary.
    pub fn coverage(&self, p: Point, pixel: f64) -> f64 {
        coverage_from_distance(self.sdf(p), pixel)
    }

    pub fn coverage_approx(&self, p: Point, pixel: f64) -> f64 {
        coverage_from_distance(self.sdf_approx(p), pixel)
    }
}

pub fn coverage_from_distance(d: f64, pixel: f64) -> f64 {
    (0.5 - d / pixel.max(1e-12)).clamp(0.0, 1.0)
}

/// `(G − 1) / |∇G|` with `G` the superellipse norm; clamped by the bounding-box distance so the
/// result stays finite and monotonic near the corner centre.
pub fn corner_sdf_approx(qx: f64, qy: f64, e: f64, n: f64) -> f64 {
    let (u, v) = ((qx / e).max(1e-9), (qy / e).max(1e-9));
    let g = (u.powf(n) + v.powf(n)).powf(1.0 / n);
    let grad = (u.powf(2.0 * n - 2.0) + v.powf(2.0 * n - 2.0)).sqrt() / g.powf(n - 1.0);
    let d = (g - 1.0) / grad * e;
    d.max(qx.max(qy) - e)
}

pub fn corner_sdf_exact(qx: f64, qy: f64, e: f64, n: f64) -> f64 {
    let (u, v) = (qx / e, qy / e);
    let dist = |phi: f64| {
        let p = quadrant_point(phi, n);
        (p.0 - u).hypot(p.1 - v)
    };
    const N: usize = 24;
    let (mut best_i, mut best_d) = (0usize, f64::INFINITY);
    for i in 0..=N {
        let d = dist(FRAC_PI_2 * i as f64 / N as f64);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    let step = FRAC_PI_2 / N as f64;
    let mut lo = ((best_i as f64 - 1.0) * step).max(0.0);
    let mut hi = ((best_i as f64 + 1.0) * step).min(FRAC_PI_2);
    let gr = 0.618_033_988_749_894_9;
    let (mut a, mut b) = (hi - gr * (hi - lo), lo + gr * (hi - lo));
    let (mut fa, mut fb) = (dist(a), dist(b));
    for _ in 0..40 {
        if fa < fb {
            hi = b;
            b = a;
            fb = fa;
            a = hi - gr * (hi - lo);
            fa = dist(a);
        } else {
            lo = a;
            a = b;
            fa = fb;
            b = lo + gr * (hi - lo);
            fb = dist(b);
        }
    }
    let d = best_d.min(fa).min(fb);
    let inside = superellipse_norm(u, v, n) < 1.0;
    if inside {
        -d * e
    } else {
        d * e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rr(style: CornerStyle) -> RoundedRect {
        RoundedRect::new(Rect::new(10.0, 20.0, 200.0, 120.0), 24.0, style)
    }

    fn styles() -> [CornerStyle; 3] {
        [
            CornerStyle::Circular,
            CornerStyle::continuous(),
            CornerStyle::Continuous { expansion: 1.2 },
        ]
    }

    #[test]
    fn exponent_is_two_for_circle_and_larger_for_smoothing() {
        assert!((CornerStyle::Circular.exponent() - 2.0).abs() < 1e-12);
        assert!((CornerStyle::Continuous { expansion: 1.0 }.exponent() - 2.0).abs() < 1e-12);
        let n = CornerStyle::continuous().exponent();
        assert!(n > 3.0 && n < 3.5, "n = {n}");
        assert!(CornerStyle::Continuous { expansion: 1.2 }.exponent() < n);
    }

    #[test]
    fn sdf_sign_inside_outside() {
        for s in styles() {
            let r = rr(s);
            assert!(r.sdf(r.rect.center()) < 0.0);
            assert!(r.sdf(Point::new(-50.0, 0.0)) > 0.0);
            assert!(
                r.sdf(Point::new(10.0, 20.0)) > 0.0,
                "sharp corner point is outside a rounded rect"
            );
            assert!(r.sdf(Point::new(110.0, 20.0)).abs() < 1e-9);
            assert!((r.sdf(Point::new(110.0, 30.0)) + 10.0).abs() < 1e-9);
            assert!((r.sdf(Point::new(110.0, 10.0)) - 10.0).abs() < 1e-9);
        }
    }

    #[test]
    fn circular_matches_closed_form() {
        let r = RoundedRect::circular(Rect::new(0.0, 0.0, 100.0, 100.0), 20.0);
        let p = Point::new(2.0, 2.0);
        let expect = ((20.0f64 - 2.0).hypot(20.0 - 2.0)) - 20.0;
        assert!((r.sdf(p) - expect).abs() < 1e-12);
        assert!((r.sdf(p) - r.sdf_approx(p)).abs() < 1e-12);
    }

    #[test]
    fn continuous_with_unit_expansion_equals_circular() {
        let a = RoundedRect::circular(Rect::new(0.0, 0.0, 80.0, 60.0), 12.0);
        let b = RoundedRect::new(
            Rect::new(0.0, 0.0, 80.0, 60.0),
            12.0,
            CornerStyle::Continuous { expansion: 1.0 },
        );
        for i in 0..40 {
            let p = Point::new(i as f64 * 2.1 - 3.0, i as f64 * 1.7 - 2.0);
            assert!((a.sdf(p) - b.sdf(p)).abs() < 1e-12);
        }
        assert_eq!(a.path().segments.len(), b.path().segments.len());
    }

    #[test]
    fn sdf_is_lipschitz_continuous() {
        for s in styles() {
            let r = rr(s);
            let mut worst: f64 = 0.0;
            for i in 0..90 {
                for j in 0..60 {
                    let p = Point::new(i as f64 * 2.6 - 10.0, j as f64 * 2.6 + 5.0);
                    let q = Point::new(p.x + 0.37, p.y + 0.21);
                    let (a, b) = (r.sdf(p), r.sdf(q));
                    worst = worst.max((a - b).abs() / p.distance(q));
                }
            }
            assert!(worst <= 1.0 + 1e-6, "{s:?} lipschitz {worst}");
        }
    }

    #[test]
    fn approx_is_continuous_and_close_near_boundary() {
        let r = rr(CornerStyle::continuous());
        let mut worst_err: f64 = 0.0;
        let mut worst_lip: f64 = 0.0;
        for i in 0..400 {
            for j in 0..300 {
                let p = Point::new(i as f64 * 0.5 - 5.0, j as f64 * 0.5 + 15.0);
                let (a, x) = (r.sdf(p), r.sdf_approx(p));
                if a.abs() < 3.0 {
                    worst_err = worst_err.max((a - x).abs());
                }
                let q = Point::new(p.x + 0.25, p.y + 0.25);
                worst_lip =
                    worst_lip.max((r.sdf_approx(p) - r.sdf_approx(q)).abs() / p.distance(q));
            }
        }
        assert!(
            worst_err < 0.05,
            "approximation error {worst_err} px within 3 px of the edge"
        );
        assert!(worst_lip < 1.35, "approx lipschitz {worst_lip}");
    }

    #[test]
    fn approx_is_zero_on_boundary() {
        for s in styles() {
            let r = rr(s);
            for p in r.path().flatten(0.001) {
                assert!(
                    r.sdf_approx(p).abs() < 0.01,
                    "{s:?} {p:?} → {}",
                    r.sdf_approx(p)
                );
            }
        }
    }

    #[test]
    fn path_closes() {
        for s in styles() {
            let r = rr(s);
            let p = r.path();
            assert!(p.is_closed());
            let (a, b) = (p.start().unwrap(), p.end().unwrap());
            assert!(a.distance(b) < 1e-9, "{s:?}: start {a:?} end {b:?}");
        }
    }

    #[test]
    fn path_is_continuous_between_segments() {
        let p = rr(CornerStyle::continuous()).path();
        let mut cur = None;
        for s in &p.segments {
            match *s {
                PathSeg::MoveTo(q) => cur = Some(q),
                PathSeg::LineTo(q) => {
                    assert!(cur.is_some());
                    cur = Some(q);
                }
                PathSeg::CubicTo(_, _, q) => {
                    assert!(cur.is_some());
                    cur = Some(q);
                }
                PathSeg::Close => {}
            }
        }
        assert_eq!(p.cubic_count(), 16);
        assert_eq!(rr(CornerStyle::Circular).path().cubic_count(), 4);
    }

    #[test]
    fn path_lies_on_zero_level_set() {
        for s in styles() {
            let r = rr(s);
            let tol = match s {
                CornerStyle::Circular => 4e-4 * 24.0,
                _ => 2e-3,
            };
            for p in r.path().flatten(0.0005) {
                let d = r.sdf(p).abs();
                assert!(d < tol, "{s:?} point {p:?} distance {d}");
            }
        }
    }

    #[test]
    fn path_bounds_equal_rect() {
        for s in styles() {
            let r = rr(s);
            let b = r.path().bounds();
            assert!((b.min_x() - 10.0).abs() < 1e-6 && (b.max_x() - 210.0).abs() < 1e-6);
            assert!((b.min_y() - 20.0).abs() < 1e-6 && (b.max_y() - 140.0).abs() < 1e-6);
        }
    }

    #[test]
    fn symmetric_under_mirroring() {
        for s in styles() {
            let r = rr(s);
            let c = r.rect.center();
            for i in 0..30 {
                let p = Point::new(c.x + i as f64 * 3.3 - 50.0, c.y + i as f64 * 2.2 - 50.0);
                let mx = Point::new(2.0 * c.x - p.x, p.y);
                let my = Point::new(p.x, 2.0 * c.y - p.y);
                let mxy = Point::new(2.0 * c.x - p.x, 2.0 * c.y - p.y);
                let d = r.sdf(p);
                assert!((d - r.sdf(mx)).abs() < 1e-9);
                assert!((d - r.sdf(my)).abs() < 1e-9);
                assert!((d - r.sdf(mxy)).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn path_symmetric_under_mirroring() {
        let r = RoundedRect::continuous(Rect::new(0.0, 0.0, 100.0, 100.0), 20.0);
        let pts = r.path().flatten(0.01);
        for p in &pts {
            let m = Point::new(100.0 - p.x, p.y);
            let d = r.sdf(m).abs();
            assert!(d < 2e-3);
        }
    }

    #[test]
    fn coverage_ramp() {
        let r = RoundedRect::circular(Rect::new(0.0, 0.0, 100.0, 100.0), 10.0);
        assert_eq!(r.coverage(Point::new(50.0, 50.0), 1.0), 1.0);
        assert_eq!(r.coverage(Point::new(-5.0, 50.0), 1.0), 0.0);
        assert!((r.coverage(Point::new(0.0, 50.0), 1.0) - 0.5).abs() < 1e-12);
        assert!((r.coverage(Point::new(0.25, 50.0), 1.0) - 0.75).abs() < 1e-12);
        assert!((r.coverage(Point::new(50.0, 100.0 + 0.5), 1.0)).abs() < 1e-12);
        let c = RoundedRect::continuous(Rect::new(0.0, 0.0, 100.0, 100.0), 20.0);
        assert!(
            c.coverage(Point::new(1.0, 1.0), 1.0) < 0.05,
            "pixel at the sharp corner is mostly uncovered"
        );
    }

    #[test]
    fn coverage_area_matches_polygon_area() {
        let r = RoundedRect::continuous(Rect::new(0.0, 0.0, 64.0, 64.0), 14.0);
        let n = 256;
        let mut area = 0.0;
        for j in 0..n {
            for i in 0..n {
                let p = Point::new(
                    (i as f64 + 0.5) * 64.0 / n as f64,
                    (j as f64 + 0.5) * 64.0 / n as f64,
                );
                area += r.coverage(p, 64.0 / n as f64);
            }
        }
        area *= (64.0 / n as f64).powi(2);
        let poly = r.path().flatten(0.001);
        let mut a2 = 0.0;
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            a2 += p.x * q.y - q.x * p.y;
        }
        assert!(
            (area - a2.abs() * 0.5).abs() < 0.5,
            "raster {area} vs polygon {}",
            a2.abs() * 0.5
        );
    }

    #[test]
    fn continuous_corner_extent_and_diagonal() {
        let c = Rect::new(0.0, 0.0, 100.0, 100.0);
        let circ = RoundedRect::circular(c, 20.0);
        let cont = RoundedRect::continuous(c, 20.0);
        let on_edge = Point::new(1.528 * 20.0, 0.0);
        assert!(cont.sdf(on_edge).abs() < 1e-9);
        assert!(circ.sdf(Point::new(1.528 * 20.0, 0.0)).abs() < 1e-9);
        let first_inside = |r: &RoundedRect| {
            (0..2000)
                .map(|i| i as f64 * 0.01)
                .find(|&t| r.sdf(Point::new(t, t)) <= 0.0)
                .unwrap()
        };
        let (dc, ds) = (first_inside(&circ), first_inside(&cont));
        assert!(
            (dc - ds).abs() < 0.02,
            "diagonal points agree by construction: {dc} vs {ds}"
        );
        let near = Point::new(2.0, 24.0);
        assert!(
            cont.sdf(near) > circ.sdf(near),
            "the continuous corner starts bending earlier than the circular one"
        );
    }

    #[test]
    fn extents_shrink_to_fit() {
        let r = RoundedRect::continuous(Rect::new(0.0, 0.0, 40.0, 40.0), 30.0);
        let e = r.extents();
        for v in e {
            assert!((v - 20.0).abs() < 1e-9);
        }
        let r = RoundedRect::with_radii(
            Rect::new(0.0, 0.0, 100.0, 50.0),
            Radii::top(10.0),
            CornerStyle::Circular,
        );
        assert_eq!(r.extents(), [10.0, 10.0, 0.0, 0.0]);
        assert!(
            r.sdf(Point::new(0.0, 50.0)).abs() < 1e-12,
            "square bottom-left corner is on the boundary"
        );
        assert!(r.sdf(Point::new(0.0, 0.0)) > 0.0);
    }

    #[test]
    fn zero_radius_is_plain_rectangle() {
        let r = RoundedRect::circular(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0);
        assert!((r.sdf(Point::new(-3.0, -4.0)) - 5.0).abs() < 1e-12);
        assert_eq!(r.path().cubic_count(), 0);
        assert!(r.path().is_closed());
    }
}
