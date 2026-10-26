//! Baked drop shadow: Gaussian blur of the cursor's alpha channel, tinted and composited behind
//! the cursor.
//!
//! The shadow is `alpha ⊗ G(σ)` shifted by the offset, multiplied by the shadow colour and its
//! alpha, drawn with source-over under the original. σ and the offset are in points and are scaled
//! by the raster scale. The canvas grows by `ceil(3σ) + |offset|` pixels on every side so nothing is
//! clipped; the returned origin tells how far the original moved (the hotspot moves by the same
//! amount).

use crate::meta::Shadow;
use tiny_skia::{Pixmap, PremultipliedColorU8};

/// Normalised one-sided Gaussian kernel `w[0..=r]`, `w[0] + 2·Σ w[1..] = 1`, radius `ceil(3σ)`.
pub fn kernel(sigma_px: f64) -> Vec<f64> {
    if sigma_px < 0.05 {
        return vec![1.0];
    }
    let r = (3.0 * sigma_px).ceil() as usize;
    let mut w: Vec<f64> = (0..=r).map(|i| (-((i * i) as f64) / (2.0 * sigma_px * sigma_px)).exp()).collect();
    let sum = w[0] + 2.0 * w[1..].iter().sum::<f64>();
    w.iter_mut().for_each(|x| *x /= sum);
    w
}

fn blur_axis(src: &[f32], dst: &mut [f32], w: usize, h: usize, k: &[f64], horizontal: bool) {
    for y in 0..h {
        for x in 0..w {
            let mut acc = k[0] * src[y * w + x] as f64;
            for (i, kv) in k.iter().enumerate().skip(1) {
                for sign in [-(i as isize), i as isize] {
                    let (sx, sy) = if horizontal { (x as isize + sign, y as isize) } else { (x as isize, y as isize + sign) };
                    if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                        acc += kv * src[sy as usize * w + sx as usize] as f64;
                    }
                }
            }
            dst[y * w + x] = acc as f32;
        }
    }
}

/// Gaussian blur of a single-channel image (zero outside).
pub fn gaussian_blur(data: &mut [f32], w: usize, h: usize, sigma_px: f64) {
    let k = kernel(sigma_px);
    if k.len() == 1 {
        return;
    }
    let mut tmp = vec![0.0f32; data.len()];
    blur_axis(data, &mut tmp, w, h, &k, true);
    blur_axis(&tmp, data, w, h, &k, false);
}

/// Padding in pixels needed on every side for `shadow` at `scale`.
pub fn padding(shadow: &Shadow, scale: f64) -> u32 {
    ((3.0 * shadow.sigma * scale).ceil() + shadow.dx.abs().max(shadow.dy.abs()) * scale).ceil().max(0.0) as u32
}

pub struct Shadowed {
    pub image: Pixmap,
    /// Position of the original's top-left corner in `image`.
    pub origin: (u32, u32),
}

/// Add the shadow behind `src`. `scale`: pixels per point.
pub fn apply(src: &Pixmap, shadow: &Shadow, scale: f64) -> Shadowed {
    let pad = padding(shadow, scale);
    let (w, h) = (src.width() + 2 * pad, src.height() + 2 * pad);
    let (wu, hu) = (w as usize, h as usize);
    let (dx, dy) = ((shadow.dx * scale).round() as i64, (shadow.dy * scale).round() as i64);
    let mut alpha = vec![0.0f32; wu * hu];
    for y in 0..src.height() as i64 {
        for x in 0..src.width() as i64 {
            let a = src.pixels()[(y as usize) * src.width() as usize + x as usize].alpha();
            let (tx, ty) = (x + pad as i64 + dx, y + pad as i64 + dy);
            if a > 0 && tx >= 0 && ty >= 0 && (tx as usize) < wu && (ty as usize) < hu {
                alpha[ty as usize * wu + tx as usize] = a as f32 / 255.0;
            }
        }
    }
    gaussian_blur(&mut alpha, wu, hu, shadow.sigma * scale);
    let mut out = Pixmap::new(w, h).expect("non-empty canvas");
    let [cr, cg, cb, ca] = shadow.color;
    for (i, p) in out.pixels_mut().iter_mut().enumerate() {
        let a = (alpha[i] * ca).clamp(0.0, 1.0);
        let q = |c: f32| ((c * a) * 255.0).round().clamp(0.0, 255.0) as u8;
        let alpha8 = (a * 255.0).round() as u8;
        *p = PremultipliedColorU8::from_rgba(q(cr).min(alpha8), q(cg).min(alpha8), q(cb).min(alpha8), alpha8).unwrap_or(PremultipliedColorU8::TRANSPARENT);
    }
    out.draw_pixmap(pad as i32, pad as i32, src.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    Shadowed { image: out, origin: (pad, pad) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::ColorU8;

    fn dot(size: u32, at: (u32, u32), rgba: [u8; 4]) -> Pixmap {
        let mut p = Pixmap::new(size, size).unwrap();
        p.pixels_mut()[(at.1 * size + at.0) as usize] = ColorU8::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).premultiply();
        p
    }

    fn alpha_sum(p: &Pixmap) -> f64 {
        p.pixels().iter().map(|q| q.alpha() as f64).sum()
    }

    fn centroid(p: &Pixmap, exclude: Option<(u32, u32)>) -> (f64, f64) {
        let (mut sx, mut sy, mut s) = (0.0, 0.0, 0.0);
        for y in 0..p.height() {
            for x in 0..p.width() {
                if Some((x, y)) == exclude {
                    continue;
                }
                let a = p.pixels()[(y * p.width() + x) as usize].alpha() as f64;
                sx += a * x as f64;
                sy += a * y as f64;
                s += a;
            }
        }
        (sx / s, sy / s)
    }

    #[test]
    fn kernel_is_normalised_and_wide_enough() {
        for sigma in [0.5, 1.0, 1.8, 3.6, 7.2] {
            let k = kernel(sigma);
            let sum = k[0] + 2.0 * k[1..].iter().sum::<f64>();
            assert!((sum - 1.0).abs() < 1e-12);
            assert_eq!(k.len(), (3.0 * sigma).ceil() as usize + 1);
            assert!(k.windows(2).all(|w| w[0] >= w[1]), "monotonic");
        }
        assert_eq!(kernel(0.0), vec![1.0]);
    }

    #[test]
    fn blur_conserves_energy_away_from_the_border() {
        let (w, h) = (41usize, 41usize);
        let mut d = vec![0.0f32; w * h];
        d[20 * w + 20] = 1.0;
        gaussian_blur(&mut d, w, h, 2.0);
        let sum: f32 = d.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
        let peak = d[20 * w + 20];
        let expected = 1.0 / (2.0 * std::f32::consts::PI * 4.0);
        assert!((peak - expected).abs() < 0.1 * expected, "peak {peak} vs {expected}");
        assert!((d[20 * w + 22] / peak - (-0.5f32).exp()).abs() < 0.02, "one sigma away");
    }

    #[test]
    fn shadow_extends_the_canvas_and_keeps_the_original_on_top() {
        let src = dot(9, (4, 4), [255, 0, 0, 255]);
        let s = Shadow { color: [0.0, 0.0, 0.0, 0.65], dx: 0.0, dy: 1.0, sigma: 1.8 };
        let r = apply(&src, &s, 1.0);
        let pad = padding(&s, 1.0);
        assert_eq!(pad, 7);
        assert_eq!(r.image.width(), 9 + 2 * pad);
        assert_eq!(r.origin, (pad, pad));
        let c = r.image.pixels()[((pad + 4) * r.image.width() + pad + 4) as usize];
        assert_eq!((c.red(), c.green(), c.blue(), c.alpha()), (255, 0, 0, 255), "source pixel untouched");
    }

    #[test]
    fn shadow_colour_alpha_and_offset_are_applied() {
        let src = dot(15, (7, 7), [255, 255, 255, 255]);
        let s = Shadow { color: [0.0, 0.0, 1.0, 0.5], dx: 0.0, dy: 2.0, sigma: 1.0 };
        let r = apply(&src, &s, 1.0);
        let origin = (r.origin.0 + 7, r.origin.1 + 7);
        let (cx, cy) = centroid(&r.image, Some(origin));
        assert!((cx - origin.0 as f64).abs() < 0.3, "no horizontal shift: {cx} vs {}", origin.0);
        assert!((cy - (origin.1 as f64 + 2.0)).abs() < 0.3, "two pixels down: {cy} vs {}", origin.1 + 2);
        let max_alpha = r.image.pixels().iter().enumerate().filter(|(i, _)| (*i as u32 % r.image.width(), *i as u32 / r.image.width()) != origin).map(|(_, p)| p.alpha()).max().unwrap();
        assert!(max_alpha as f32 <= 0.5 * 255.0 * 0.4 + 1.0, "shadow alpha is below colour alpha times the blurred peak: {max_alpha}");
        let tinted = r.image.pixels().iter().filter(|p| p.alpha() > 4 && p.blue() as u16 * 10 > p.alpha() as u16 * 9 && p.red() == 0).count();
        assert!(tinted > 5, "blue shadow pixels: {tinted}");
    }

    #[test]
    fn scale_multiplies_sigma_offset_and_padding() {
        let s = Shadow { color: [0.0, 0.0, 0.0, 1.0], dx: 1.0, dy: 1.0, sigma: 1.5 };
        assert_eq!(padding(&s, 1.0), 6);
        assert_eq!(padding(&s, 2.0), 11);
        let big = apply(&dot(8, (4, 4), [0, 0, 0, 255]), &s, 2.0);
        assert_eq!(big.image.width(), 8 + 22);
    }

    #[test]
    fn transparent_source_gives_no_shadow() {
        let src = Pixmap::new(8, 8).unwrap();
        let r = apply(&src, &Shadow::COMMON, 1.0);
        assert_eq!(alpha_sum(&r.image), 0.0);
    }

    #[test]
    fn shadow_alpha_total_follows_the_colour_alpha() {
        let src = dot(31, (15, 15), [0, 0, 0, 255]);
        let full = apply(&src, &Shadow { color: [0.0, 0.0, 0.0, 1.0], dx: 0.0, dy: 0.0, sigma: 2.0 }, 1.0);
        let half = apply(&src, &Shadow { color: [0.0, 0.0, 0.0, 0.5], dx: 0.0, dy: 0.0, sigma: 2.0 }, 1.0);
        let ratio = (alpha_sum(&half.image) - 255.0) / (alpha_sum(&full.image) - 255.0);
        assert!((ratio - 0.5).abs() < 0.03, "{ratio}");
    }
}
