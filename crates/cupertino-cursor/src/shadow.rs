//! Baked drop shadow: Gaussian blur of the cursor's alpha channel, tinted and composited behind
//! the cursor.
//!
//! The shadow is `alpha ⊗ G(σ)` shifted by the offset, multiplied by the shadow colour and its
//! alpha, drawn with source-over under the original. σ and the offset are in points and are scaled
//! by the raster scale. The canvas grows by `ceil(3σ) + |offset|` pixels on every side so nothing is
//! clipped; the returned origin tells how far the original moved (the hotspot moves by the same
//! amount).

use tiny_skia::{Pixmap, PremultipliedColorU8};

/// A drop shadow: colour, offset and blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// Straight RGBA, 0…1.
    pub color: [f32; 4],
    /// Offset in points, screen convention (x right, y down).
    pub dx: f64,
    pub dy: f64,
    /// Gaussian standard deviation in points.
    pub sigma: f64,
}

impl Shadow {
    /// The shadow of every cursor of the procedural set (colour `0,0,0,0.65`, blur 1.8, one point down); an approximate
    /// choice.
    pub const COMMON: Shadow = Shadow {
        color: [0.0, 0.0, 0.0, 0.65],
        dx: 0.0,
        dy: 1.0,
        sigma: 1.8,
    };
}

/// Normalised one-sided Gaussian kernel `w[0..=r]`, `w[0] + 2·Σ w[1..] = 1`, radius `ceil(3σ)`.
pub fn kernel(sigma_px: f64) -> Vec<f64> {
    if sigma_px < 0.05 {
        return vec![1.0];
    }
    let r = (3.0 * sigma_px).ceil() as usize;
    let mut w: Vec<f64> = (0..=r)
        .map(|i| (-((i * i) as f64) / (2.0 * sigma_px * sigma_px)).exp())
        .collect();
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
                    let (sx, sy) = if horizontal {
                        (x as isize + sign, y as isize)
                    } else {
                        (x as isize, y as isize + sign)
                    };
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
    ((3.0 * shadow.sigma * scale).ceil() + shadow.dx.abs().max(shadow.dy.abs()) * scale)
        .ceil()
        .max(0.0) as u32
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
    let (dx, dy) = (
        (shadow.dx * scale).round() as i64,
        (shadow.dy * scale).round() as i64,
    );
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
        *p = PremultipliedColorU8::from_rgba(
            q(cr).min(alpha8),
            q(cg).min(alpha8),
            q(cb).min(alpha8),
            alpha8,
        )
        .unwrap_or(PremultipliedColorU8::TRANSPARENT);
    }
    out.draw_pixmap(
        pad as i32,
        pad as i32,
        src.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
    Shadowed {
        image: out,
        origin: (pad, pad),
    }
}
