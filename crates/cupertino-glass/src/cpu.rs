//! CPU reference implementation of the material pipeline (validation), and the colour filter chain for measuring what a
//! material shows ([`apply_color_ops`]).
//!
//! `exact_blur = true` replaces the downsample pyramid by a single full Gaussian at capture
//! resolution, quantifying the pyramid approximation error. `exact_mask = true` uses the design
//! signed distance instead of the closed-form one the shaders implement.

use crate::blur::{gaussian_weights, plan_blur};
use crate::ops::{color_ops, fill_ops, Op};
use crate::renderer::MaterialRegion;
use cupertino_geometry::{rounded::coverage_from_distance, Point};
use std::collections::HashMap;

/// Rounds to the nearest IEEE half-precision value (the GPU stores intermediates as rgba16float).
pub fn round_f16(x: f32) -> f32 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let abs = x.abs();
    let exp = ((abs.to_bits() >> 23) & 0xff) as i32 - 127;
    let quantum = if exp < -14 {
        2f32.powi(-24)
    } else {
        2f32.powi(exp - 10)
    };
    let r = (abs / quantum).round_ties_even() * quantum;
    r.min(65504.0).copysign(x)
}

#[derive(Clone)]
struct Img {
    w: usize,
    h: usize,
    px: Vec<[f32; 3]>,
}

impl Img {
    fn at(&self, x: i64, y: i64) -> [f32; 3] {
        let x = x.clamp(0, self.w as i64 - 1) as usize;
        let y = y.clamp(0, self.h as i64 - 1) as usize;
        self.px[y * self.w + x]
    }

    fn down(&self, w: usize, h: usize) -> Img {
        let mut px = vec![[0.0; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut c = [0.0; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let s = self.at(2 * x as i64 + dx, 2 * y as i64 + dy);
                    for i in 0..3 {
                        c[i] += s[i] * 0.25;
                    }
                }
                px[y * w + x] = c.map(round_f16);
            }
        }
        Img { w, h, px }
    }

    fn blur(&self, wts: &[f32], dx: i64, dy: i64) -> Img {
        let mut px = vec![[0.0; 3]; self.w * self.h];
        for y in 0..self.h as i64 {
            for x in 0..self.w as i64 {
                let mut c = self.at(x, y).map(|v| v * wts[0]);
                for (i, &wt) in wts.iter().enumerate().skip(1) {
                    let a = self.at(x + dx * i as i64, y + dy * i as i64);
                    let b = self.at(x - dx * i as i64, y - dy * i as i64);
                    for k in 0..3 {
                        c[k] += (a[k] + b[k]) * wt;
                    }
                }
                px[y as usize * self.w + x as usize] = c.map(round_f16);
            }
        }
        Img {
            w: self.w,
            h: self.h,
            px,
        }
    }

    fn bilinear(&self, u: f32, v: f32) -> [f32; 3] {
        let fx = u * self.w as f32 - 0.5;
        let fy = v * self.h as f32 - 0.5;
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let l = |a: [f32; 3], b: [f32; 3], t: f32| {
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]
        };
        l(
            l(self.at(x0, y0), self.at(x0 + 1, y0), tx),
            l(self.at(x0, y0 + 1), self.at(x0 + 1, y0 + 1), tx),
            ty,
        )
    }
}

const LUMA: [f32; 3] = [0.2125, 0.7154, 0.0721];
const SAT: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn srgb_decode(v: f32) -> f32 {
    if v < 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_encode(v: f32) -> f32 {
    if v < 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// One colour operation; `next` is the operation after `o` (the tail of a linear matrix).
fn color_op(c: [f32; 3], o: &Op, next: Option<&Op>) -> [f32; 3] {
    match o.a[0] as i32 {
        1 => {
            let l = dot(c, SAT);
            c.map(|v| l * (1.0 - o.a[1]) + v * o.a[1])
        }
        2 => {
            let x0 = dot(c, LUMA);
            let p = o.b;
            let x = (((p[0] * x0 + p[1]) * x0 + p[2]) * x0 + p[3]).clamp(0.0, 1.0);
            c.map(|v| v + (x - v) * o.a[1])
        }
        3 => {
            let l = dot(c, LUMA);
            [0, 1, 2].map(|i| c[i] + (l * o.b[i] - c[i]) * o.a[1])
        }
        4 => {
            let tail = next.map_or([0.0; 4], |n| n.a);
            let m = [
                [o.a[1], o.a[2], o.a[3]],
                [o.b[0], o.b[1], o.b[2]],
                [o.b[3], tail[1], tail[2]],
            ];
            let lin = c.map(|v| srgb_decode(v.clamp(0.0, 1.0)));
            m.map(|row| srgb_encode(dot(row, lin).clamp(0.0, 1.0)))
        }
        5 => c.map(|v| 1.0 - v),
        6 => {
            let t = o.a[1];
            if t >= 1.0 {
                c.map(|v| if v >= 0.5 { 1.0 } else { 0.0 })
            } else {
                c.map(|v| ((v - 0.5 * t) / (1.0 - t)).clamp(0.0, 1.0))
            }
        }
        _ => c,
    }
}

/// The colour filter chain `ops` ([`color_ops`]) applied to one gamma-encoded colour, as the GPU's colour pass does.
pub fn apply_color_ops(c: [f32; 3], ops: &[Op]) -> [f32; 3] {
    ops.iter()
        .enumerate()
        .fold(c, |c, (i, o)| color_op(c, o, ops.get(i + 1)))
}

fn soft_light(s: f32, d: f32) -> f32 {
    if s <= 0.5 {
        return d - (1.0 - 2.0 * s) * d * (1.0 - d);
    }
    let g = if d <= 0.25 {
        ((16.0 * d - 12.0) * d + 4.0) * d
    } else {
        d.sqrt()
    };
    d + (2.0 * s - 1.0) * (g - d)
}

fn blend(mode: i32, s: [f32; 3], a: f32, d: [f32; 3]) -> [f32; 3] {
    let b: [f32; 3] = match mode {
        1 => [0, 1, 2].map(|i| (s[i] + d[i]).min(1.0)),
        2 => [0, 1, 2].map(|i| (s[i] + d[i] - 1.0).max(0.0)),
        3 => [0, 1, 2].map(|i| s[i].min(d[i])),
        4 => [0, 1, 2].map(|i| s[i].max(d[i])),
        5 => [0, 1, 2].map(|i| soft_light(s[i], d[i])),
        6 => return s.map(|v| v * a),
        _ => s,
    };
    [0, 1, 2].map(|i| d[i] * (1.0 - a) + b[i] * a)
}

fn processed_backdrop(
    base: &Img,
    region: &MaterialRegion,
    display_scale: f32,
    exact_blur: bool,
) -> Option<Img> {
    let b = region.recipe.backdrop.as_ref()?;
    let (w, h) = (base.w, base.h);
    let plan = plan_blur(w as u32, h as u32, b.scale, b.sigma_pt, display_scale);
    let mut cur = base.clone();
    let (levels, wts) = if exact_blur {
        let cap: Vec<(u32, u32)> = plan
            .levels
            .iter()
            .take_while({
                let mut s = 1.0f32;
                move |_| {
                    let keep = s > b.scale + 1e-3;
                    s *= 0.5;
                    keep
                }
            })
            .cloned()
            .collect();
        let n = cap.len() as f32;
        let sigma = ((b.sigma_pt * display_scale * b.scale).powi(2) - 0.0625 * n)
            .max(0.0)
            .sqrt();
        (cap, gaussian_weights(sigma))
    } else {
        (plan.levels.clone(), plan.weights.clone())
    };
    for (lw, lh) in levels {
        cur = cur.down(lw as usize, lh as usize);
    }
    if wts.len() > 1 {
        cur = cur.blur(&wts, 1, 0).blur(&wts, 0, 1);
    }
    let ops = color_ops(&b.ops);
    for p in cur.px.iter_mut() {
        *p = apply_color_ops(*p, &ops).map(round_f16);
    }
    Some(cur)
}

/// Render `regions` over `wall` the way the GPU renderer does: backdrops are computed from the
/// wallpaper, each region is alpha-blended (coverage × opacity) into an 8-bit target in order.
pub fn render_regions(
    wall: &image::RgbaImage,
    regions: &[MaterialRegion],
    display_scale: f32,
    tint: [f32; 3],
    exact_blur: bool,
    exact_mask: bool,
) -> image::RgbaImage {
    let (w, h) = (wall.width() as usize, wall.height() as usize);
    let base = Img {
        w,
        h,
        px: wall
            .pixels()
            .map(|p| {
                [
                    p[0] as f32 / 255.0,
                    p[1] as f32 / 255.0,
                    p[2] as f32 / 255.0,
                ]
            })
            .collect(),
    };
    let mut out = wall.clone();
    let mut cache: HashMap<(u32, u32, Vec<u32>), Option<Img>> = HashMap::new();
    for r in regions {
        let key = r
            .recipe
            .backdrop
            .as_ref()
            .map(|b| {
                let mut ops = vec![];
                for o in color_ops(&b.ops) {
                    ops.extend(o.a.iter().chain(o.b.iter()).map(|f| f.to_bits()));
                }
                (b.scale.to_bits(), b.sigma_pt.to_bits(), ops)
            })
            .unwrap_or((0, 0, vec![]));
        let back = cache
            .entry(key)
            .or_insert_with(|| processed_backdrop(&base, r, display_scale, exact_blur))
            .clone();
        let fops = fill_ops(r.recipe);
        let shape = r.rounded_rect();
        let (x0, y0) = (
            (r.rect[0] - 2.0).floor().max(0.0) as usize,
            (r.rect[1] - 2.0).floor().max(0.0) as usize,
        );
        let (x1, y1) = (
            ((r.rect[0] + r.rect[2] + 2.0).ceil() as usize).min(w),
            ((r.rect[1] + r.rect[3] + 2.0).ceil() as usize).min(h),
        );
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let p = Point::new(px as f64, py as f64);
                let d = if exact_mask {
                    shape.sdf(p)
                } else {
                    shape.sdf_approx(p)
                };
                let cov = coverage_from_distance(d, 1.0) as f32 * r.opacity.clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let mut c = match &back {
                    Some(bk) => bk.bilinear(px / w as f32, py / h as f32),
                    None => base.px[y * w + x],
                };
                for o in &fops {
                    let s = if o.a[1] > 0.5 {
                        tint
                    } else {
                        [o.b[0], o.b[1], o.b[2]]
                    };
                    c = blend(o.a[0] as i32, s, o.b[3], c);
                }
                let dst = out.get_pixel(x as u32, y as u32).0;
                let mut res = [0u8; 4];
                for i in 0..3 {
                    let v = c[i].clamp(0.0, 1.0) * cov + (dst[i] as f32 / 255.0) * (1.0 - cov);
                    res[i] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
                res[3] = dst[3];
                out.put_pixel(x as u32, y as u32, image::Rgba(res));
            }
        }
    }
    out
}

pub type V4 = [f32; 4];

fn mul4(a: V4, b: V4) -> V4 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]]
}

fn scale4(a: V4, s: f32) -> V4 {
    a.map(|v| v * s)
}

fn sover(s: V4, d: V4) -> V4 {
    [0, 1, 2, 3].map(|i| d[i] * (1.0 - s[3]) + s[i])
}

/// S scaled by (1−y)⁴, then source-over.
pub fn lighten_sover(s: V4, d: V4) -> V4 {
    let y = 1.0 - (d[0] * LUMA[0] + d[1] * LUMA[1] + d[2] * LUMA[2]);
    sover(scale4(s, (y * y) * (y * y)), d)
}

/// S scaled by y⁴, then source-over.
pub fn darken_sover(s: V4, d: V4) -> V4 {
    let y = d[0] * LUMA[0] + d[1] * LUMA[1] + d[2] * LUMA[2];
    sover(scale4(s, (y * y) * (y * y)), d)
}

fn step(edge: f32, x: f32) -> f32 {
    if x < edge {
        0.0
    } else {
        1.0
    }
}

pub fn color_dodge(s: V4, d: V4) -> V4 {
    let mut r = [0, 1, 2, 3].map(|i| d[i] * (1.0 - s[3]) + s[i] * (1.0 - d[3]));
    for i in 0..3 {
        let t = s[3] - s[i];
        let m = step(0.005, t);
        let hi = d[i] * s[3] * s[3] / t.max(0.005);
        r[i] += s[3] * (1.0 - m) + hi * m;
    }
    r[3] += d[3] * s[3];
    let a = r[3];
    for v in r.iter_mut().take(3) {
        *v = v.clamp(0.0, a);
    }
    r
}

pub fn color_burn(s: V4, d: V4) -> V4 {
    let mut r = [0, 1, 2, 3].map(|i| d[i] * (1.0 - s[3]) + s[i] * (1.0 - d[3]));
    let dasa = d[3] * s[3];
    for i in 0..3 {
        r[i] += step(0.005, s[i]) * (dasa - s[3] * s[3] * (d[3] - d[i]) / s[i].max(0.005));
    }
    r[3] += dasa;
    let a = r[3];
    for v in r.iter_mut().take(3) {
        *v = v.clamp(0.0, a);
    }
    r
}

/// `vibrantDark(s, d)` with the multipliers `c0` (dodge) and `c1` (lighten-sover).
pub fn vibrant_dark(s0: V4, d0: V4, reversed: bool, c0: V4, c1: V4) -> V4 {
    if reversed {
        color_dodge(mul4(s0, c0), lighten_sover(mul4(s0, c1), d0))
    } else {
        lighten_sover(mul4(s0, c1), color_dodge(mul4(s0, c0), d0))
    }
}

/// `vibrantLight(s, d)` with the multipliers `c0` (burn) and `c1` (darken-sover).
pub fn vibrant_light(s0: V4, d0: V4, reversed: bool, c0: V4, c1: V4) -> V4 {
    if reversed {
        color_burn(mul4(s0, c0), darken_sover(mul4(s0, c1), d0))
    } else {
        darken_sover(mul4(s0, c1), color_burn(mul4(s0, c0), d0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: V4 = [1.0; 4];

    fn chain(c: [f32; 3], ops: &[cupertino_theme::recipe::ColorOp]) -> [f32; 3] {
        let ops = color_ops(ops);
        ops.iter()
            .enumerate()
            .fold(c, |c, (i, o)| color_op(c, o, ops.get(i + 1)))
    }

    #[test]
    fn the_display_filter_matrix_mixes_linear_light_and_invert_and_contrast_work_on_encoded_values()
    {
        use cupertino_theme::recipe::ColorOp;
        let gray = ColorOp::LinearMatrix([[0.3, 0.59, 0.11]; 3]);
        // pure red is 0.3 of linear white: encoded 0.5838
        let g = chain([1.0, 0.0, 0.0], std::slice::from_ref(&gray));
        assert!(g.iter().all(|v| (v - 0.5838).abs() < 1e-3), "{g:?}");
        assert_eq!(
            chain([1.0, 1.0, 1.0], &[gray]),
            [1.0; 3].map(|v: f32| srgb_encode(srgb_decode(v)))
        );
        assert_eq!(
            chain([0.25, 0.5, 1.0], &[ColorOp::Invert]),
            [0.75, 0.5, 0.0]
        );
        assert_eq!(
            chain([0.1, 0.5, 0.9], &[ColorOp::Contrast(0.4)]),
            [0.0, 0.5, 1.0]
        );
        assert_eq!(
            chain([0.49, 0.5, 0.51], &[ColorOp::Contrast(1.0)]),
            [0.0, 1.0, 1.0],
            "full contrast is a threshold at the middle"
        );
    }

    #[test]
    fn half_rounding() {
        assert_eq!(round_f16(1.0), 1.0);
        assert_eq!(round_f16(0.0), 0.0);
        assert_eq!(
            round_f16(1.0 + 1.0 / 4096.0),
            1.0,
            "below half an ulp (ulp = 2^-10)"
        );
        assert_eq!(round_f16(1.0 + 3.0 / 4096.0), 1.0 + 1.0 / 1024.0);
        assert_eq!(round_f16(-0.5), -0.5);
        assert!((round_f16(0.3) - 0.3).abs() < 1.5e-4);
        assert_eq!(round_f16(1e-9), 0.0);
        assert_eq!(round_f16(1e6), 65504.0);
    }

    #[test]
    fn transparent_source_leaves_destination() {
        let d = [0.3, 0.5, 0.7, 1.0];
        let s = [0.0; 4];
        for rev in [false, true] {
            assert_eq!(vibrant_dark(s, d, rev, ONE, ONE), d);
            assert_eq!(vibrant_light(s, d, rev, ONE, ONE), d);
        }
    }

    #[test]
    fn vibrant_dark_lightens_and_light_darkens() {
        let d = [0.4, 0.4, 0.4, 1.0];
        let white = [0.6, 0.6, 0.6, 0.6];
        let black = [0.0, 0.0, 0.0, 0.6];
        let dark = vibrant_dark(white, d, false, ONE, ONE);
        let light = vibrant_light(black, d, false, ONE, ONE);
        assert!(dark[0] > d[0], "{dark:?}");
        assert!(light[0] < d[0], "{light:?}");
        assert!((dark[3] - 1.0).abs() < 1e-5 && (light[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn lighten_sover_weights_by_inverse_luma_to_the_fourth() {
        let s = [1.0, 1.0, 1.0, 1.0];
        let on_black = lighten_sover(s, [0.0, 0.0, 0.0, 1.0]);
        let on_white = lighten_sover(s, [1.0, 1.0, 1.0, 1.0]);
        assert!((on_black[0] - 1.0).abs() < 1e-6, "full strength on black");
        assert!(
            (on_white[0] - 1.0).abs() < 1e-6,
            "alpha ~0 on white keeps the (white) destination"
        );
        let y: f32 = 0.5;
        let mid = lighten_sover([0.5, 0.5, 0.5, 0.5], [y, y, y, 1.0]);
        let k = (1.0 - y).powi(4);
        let expect = y * (1.0 - 0.5 * k) + 0.5 * k;
        assert!((mid[0] - expect).abs() < 1e-5);
    }

    #[test]
    fn results_stay_premultiplied_valid() {
        for i in 0..5 {
            for j in 0..5 {
                let d = [i as f32 / 4.0, j as f32 / 4.0, 0.3, 1.0];
                let a = 0.2 + 0.2 * i as f32;
                let s = [a * 0.9, a * 0.5, a * j as f32 / 4.0, a];
                for r in [
                    vibrant_dark(s, d, false, ONE, ONE),
                    vibrant_dark(s, d, true, ONE, ONE),
                    vibrant_light(s, d, false, ONE, ONE),
                    vibrant_light(s, d, true, ONE, ONE),
                ] {
                    for c in 0..3 {
                        assert!(r[c] >= -1e-6 && r[c] <= r[3] + 1e-5, "{r:?}");
                    }
                }
            }
        }
    }

    /// A sidebar over a blue wallpaper, in a dark window: drawn over a dark window body and over the bare wallpaper.
    /// The active sidebar takes the colour of what lies behind it, so it differs between the two; an inactive window's
    /// sidebar is an opaque grey laid with the wallpaper's tint, the same over both and a navy whose blue stands well
    /// above its red and green (an inactive sidebar over a blue wallpaper reads as a dark navy, not a vivid blue).
    #[test]
    fn an_inactive_sidebar_is_the_same_navy_over_any_scene_and_an_active_one_follows_the_scene() {
        use cupertino_theme::{Appearance, MaterialOptions, Theme, WindowState};
        let theme = Theme::load().unwrap();
        let sidebar = |window| {
            theme
                .recipe(
                    Appearance::Dark,
                    "Sidebar",
                    &MaterialOptions {
                        window,
                        ..Default::default()
                    },
                )
                .unwrap()
        };
        let blue = [52.0 / 255.0, 53.0 / 255.0, 182.0 / 255.0];
        let wall = |rgb: [f32; 3]| {
            image::RgbaImage::from_pixel(
                32,
                32,
                image::Rgba([
                    (rgb[0] * 255.0) as u8,
                    (rgb[1] * 255.0) as u8,
                    (rgb[2] * 255.0) as u8,
                    255,
                ]),
            )
        };
        let draw = |window, scene: [f32; 3]| {
            let recipe = sidebar(window);
            let region = MaterialRegion::new([0.0, 0.0, 32.0, 32.0], &recipe);
            render_regions(&wall(scene), &[region], 1.0, blue, true, true)
                .get_pixel(16, 16)
                .0
        };
        let body = [30.0 / 255.0, 32.0 / 255.0, 38.0 / 255.0];
        let (over_body, over_wall) = (
            draw(WindowState::Inactive, body),
            draw(WindowState::Inactive, blue),
        );
        assert_eq!(
            over_body, over_wall,
            "the inactive sidebar ignores the scene"
        );
        let [r, g, b, _] = over_body.map(i32::from);
        assert!(
            r <= 45 && g <= 45 && b - r >= 10 && b <= 60,
            "a dark navy: {over_body:?}"
        );
        let (active_body, active_wall) = (
            draw(WindowState::Active, body),
            draw(WindowState::Active, blue),
        );
        assert!(
            i32::from(active_wall[2]) - i32::from(active_body[2]) > 30,
            "the active sidebar follows the scene: {active_body:?} {active_wall:?}"
        );
    }
}
