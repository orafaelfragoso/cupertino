use crate::Effect;
use cupertino_raster_math::blur::{plan_blur, sigma_for_radius};
use cupertino_raster_math::dither;
use tiny_skia::Pixmap;

pub(crate) fn div255(v: u32) -> u32 {
    let v = v + 128;
    (v + (v >> 8)) >> 8
}

/// [`div255`] of every channel of the RGBA pixel `px` (bytes in memory order, read little-endian) times `k` (0..=255):
/// two channels at a time in 16-bit lanes of one word, with the same rounding, so the result is the same bytes.
#[inline]
pub(crate) fn mul_px(px: u32, k: u32) -> u32 {
    const LANES: u32 = 0x00ff_00ff;
    let rb = (px & LANES) * k + 0x0080_0080;
    let rb = ((rb + ((rb >> 8) & LANES)) >> 8) & LANES;
    let ga = ((px >> 8) & LANES) * k + 0x0080_0080;
    let ga = (ga + ((ga >> 8) & LANES)) & !LANES;
    rb | ga
}

/// Source-over of the premultiplied pixel `s` on `d` (both as [`mul_px`] reads them): `s + d · (255 − s.alpha) / 255`
/// per channel, held at 255.
#[inline]
pub(crate) fn over_px(s: u32, d: u32) -> u32 {
    const LANES: u32 = 0x00ff_00ff;
    // a lane over 255 has its bit 8 set: fill the lane's low byte instead
    let held = |x: u32| {
        let over = x & 0x0100_0100;
        (x | (over - (over >> 8))) & LANES
    };
    let kept = mul_px(d, 255 - (s >> 24));
    let rb = (s & LANES) + (kept & LANES);
    let ga = ((s >> 8) & LANES) + ((kept >> 8) & LANES);
    held(rb) | (held(ga) << 8)
}

/// The deviation, in pixels, of the blur a toolkit `radius` (points) names at `scale`; zero below the smallest
/// deviation the kernel blurs at all, and held so that a pyramid of it stays small.
fn sigma_px(radius: f64, scale: f64) -> f32 {
    let sigma = (sigma_for_radius(radius as f32) as f64 * scale) as f32;
    if sigma.is_finite() && sigma >= 0.3 {
        sigma.min(MAX_SIGMA_PX)
    } else {
        0.0
    }
}

/// Largest deviation, in pixels, a layer's blur or shadow may have.
const MAX_SIGMA_PX: f32 = 4096.0;

/// How far, in pixels, a blur of deviation `sigma` reaches: three deviations, and a texel of the last pyramid level
/// for the halvings and the resampling back to the layer.
fn reach(sigma: f32) -> i32 {
    if sigma <= 0.0 {
        return 0;
    }
    let levels = pyramid_levels(sigma);
    (3.0 * sigma).ceil() as i32 + (1 << levels)
}

/// The number of 2x halvings the shared plan takes for a layer blur of deviation `sigma` pixels.
fn pyramid_levels(sigma: f32) -> u32 {
    let mut var = sigma * sigma;
    let mut levels = 0;
    while var > 64.0 {
        var = (var - 0.25) / 4.0;
        levels += 1;
    }
    levels
}

/// How far, in pixels, an effect reaches beyond the content it is computed from.
pub(crate) fn support(effects: &[Effect], scale: f64) -> i32 {
    effects
        .iter()
        .map(|effect| match *effect {
            Effect::Blur { radius } => reach(sigma_px(radius, scale)),
            Effect::DropShadow { offset, blur, .. } | Effect::InnerShadow { offset, blur, .. } => {
                let shift = (offset.x.abs().max(offset.y.abs()) * scale)
                    .ceil()
                    .min(f64::from(1 << 20)) as i32;
                shift + reach(sigma_px(blur, scale))
            }
        })
        .fold(0, i32::saturating_add)
}

/// Applies `effect` to `layer`, whose top-left pixel is window pixel `origin` (where dithering is keyed).
pub(crate) fn apply(layer: &mut Pixmap, effect: &Effect, scale: f64, origin: (i32, i32)) {
    let (w, h) = (layer.width() as usize, layer.height() as usize);
    let dithered = dither::BY_DEFAULT;
    match *effect {
        Effect::Blur { radius } => {
            let sigma = sigma_px(radius, scale);
            if sigma <= 0.0 {
                return;
            }
            let data = layer.data_mut();
            let planes: Vec<Vec<f32>> = (0..4)
                .map(|c| {
                    let plane: Vec<f32> = data
                        .iter()
                        .skip(c)
                        .step_by(4)
                        .map(|&v| f32::from(v) / 255.0)
                        .collect();
                    blur_plane(&plane, w, h, sigma, (0.0, 0.0))
                })
                .collect();
            for (i, px) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (x, y) = position(i, w, origin);
                let alpha = dither::quantize(planes[3][i]);
                for c in 0..3 {
                    let v = planes[c][i];
                    let stored = if dithered {
                        dither::store(v, x, y)
                    } else {
                        dither::quantize(v)
                    };
                    px[c] = stored.min(alpha);
                }
                px[3] = alpha;
            }
        }
        // a shadow is one channel until it is tinted: its silhouette and blur hold coverage only
        Effect::DropShadow {
            offset,
            blur: b,
            color,
        } => {
            let shadow =
                shadow_coverage(layer.data(), w, h, offset, sigma_px(b, scale), scale, false);
            let shadow = store_coverage(&shadow, w, origin, dithered);
            under(layer.data_mut(), &shadow, tint(color.premultiplied()));
        }
        Effect::InnerShadow {
            offset,
            blur: b,
            color,
        } => {
            let shadow =
                shadow_coverage(layer.data(), w, h, offset, sigma_px(b, scale), scale, true);
            let mut shadow = store_coverage(&shadow, w, origin, dithered);
            for (s, c) in shadow.iter_mut().zip(layer.data().as_chunks::<4>().0) {
                *s = div255(u32::from(*s) * u32::from(c[3])) as u8;
            }
            over(layer.data_mut(), &shadow, tint(color.premultiplied()));
        }
    }
}

/// The window pixel of the `i`th pixel of a layer `w` wide at `origin`, as the dither matrix counts it.
fn position(i: usize, w: usize, origin: (i32, i32)) -> (u32, u32) {
    let x = (i % w) as i64 + i64::from(origin.0);
    let y = (i / w) as i64 + i64::from(origin.1);
    (x.rem_euclid(1 << 30) as u32, y.rem_euclid(1 << 30) as u32)
}

/// Coverage (0 to 1) stored in bytes, dithered by position when `dithered`.
fn store_coverage(plane: &[f32], w: usize, origin: (i32, i32), dithered: bool) -> Vec<u8> {
    plane
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            if dithered {
                let (x, y) = position(i, w, origin);
                dither::store(v, x, y)
            } else {
                dither::quantize(v)
            }
        })
        .collect()
}

/// The shadow's coverage: the layer's alpha blurred by `sigma` pixels and moved by `offset` points, not rounded; for
/// an inner shadow, one minus that (the blur of the inverted silhouette, the outside counted as covered).
fn shadow_coverage(
    data: &[u8],
    w: usize,
    h: usize,
    offset: cupertino_geometry::Point,
    sigma: f32,
    scale: f64,
    inner: bool,
) -> Vec<f32> {
    let alpha: Vec<f32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| f32::from(p[3]) / 255.0)
        .collect();
    let shift = (
        (offset.x * scale).clamp(-1e9, 1e9) as f32,
        (offset.y * scale).clamp(-1e9, 1e9) as f32,
    );
    let mut out = blur_plane(&alpha, w, h, sigma, shift);
    if inner {
        out.iter_mut().for_each(|v| *v = (1.0 - *v).clamp(0.0, 1.0));
    }
    out
}

/// The shared blur (`cupertino_raster_math::blur`) of one plane of `w` x `h` values, a layer's own content, so read
/// past its edges as transparent: the plan's 2x halvings, then its Gaussian across and down the last level, then the
/// last level read back at every pixel of the plane moved by `shift` pixels, linearly between its texels. Levels are
/// kept in `f32`.
pub(crate) fn blur_plane(
    plane: &[f32],
    w: usize,
    h: usize,
    sigma: f32,
    shift: (f32, f32),
) -> Vec<f32> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let (level, lw, lh, k) = pyramid(plane, w, h, sigma);
    let factor = (1u64 << k) as f32;
    if k == 0 && shift.0.fract() == 0.0 && shift.1.fract() == 0.0 {
        // whole pixels: a move
        let (dx, dy) = (shift.0 as i64, shift.1 as i64);
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            let sy = y as i64 - dy;
            if sy < 0 || sy >= h as i64 {
                continue;
            }
            for x in 0..w {
                let sx = x as i64 - dx;
                if sx >= 0 && sx < w as i64 {
                    out[y * w + x] = level[sy as usize * lw + sx as usize];
                }
            }
        }
        return out;
    }
    let read = |x: i64, y: i64| {
        if x < 0 || y < 0 || x >= lw as i64 || y >= lh as i64 {
            0.0
        } else {
            level[y as usize * lw + x as usize]
        }
    };
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let v = (y as f32 + 0.5 - shift.1) / factor - 0.5;
        let (y0, fy) = (v.floor(), v - v.floor());
        for x in 0..w {
            let u = (x as f32 + 0.5 - shift.0) / factor - 0.5;
            let (x0, fx) = (u.floor(), u - u.floor());
            let (x0, y0) = (x0 as i64, y0 as i64);
            let top = read(x0, y0) * (1.0 - fx) + read(x0 + 1, y0) * fx;
            let bottom = read(x0, y0 + 1) * (1.0 - fx) + read(x0 + 1, y0 + 1) * fx;
            out.push(top * (1.0 - fy) + bottom * fy);
        }
    }
    out
}

/// The plan's halvings and Gaussian of a plane: the last level, its size and the number of halvings.
pub(crate) fn pyramid(
    plane: &[f32],
    w: usize,
    h: usize,
    sigma: f32,
) -> (Vec<f32>, usize, usize, u32) {
    let plan = plan_blur(w as u32, h as u32, 1.0, sigma, 1.0);
    pyramid_of(plane, w, h, &plan)
}

/// [`pyramid`] by a given plan.
pub(crate) fn pyramid_of(
    plane: &[f32],
    w: usize,
    h: usize,
    plan: &cupertino_raster_math::blur::BlurPlan,
) -> (Vec<f32>, usize, usize, u32) {
    let mut cur = plane.to_vec();
    let (mut cw, mut ch) = (w, h);
    for &(nw, nh) in &plan.levels {
        let (nw, nh) = (nw as usize, nh as usize);
        let mut next = vec![0.0; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = (2 * x + dx, 2 * y + dy);
                    if sx < cw && sy < ch {
                        sum += cur[sy * cw + sx];
                    }
                }
                next[y * nw + x] = sum * 0.25;
            }
        }
        cur = next;
        cw = nw;
        ch = nh;
    }
    if plan.weights.len() > 1 {
        cur = convolve_rows(&cur, cw, ch, &plan.weights);
        cur = convolve_columns(&cur, cw, ch, &plan.weights);
    }
    (cur, cw, ch, plan.levels.len() as u32)
}

/// One pass of the one-sided `weights` along every row, transparent past the ends.
fn convolve_rows(src: &[f32], w: usize, h: usize, weights: &[f32]) -> Vec<f32> {
    let r = weights.len() - 1;
    let mut padded = vec![0.0f32; w + 2 * r];
    let mut out = vec![0.0f32; w * h];
    for (row, dst) in src.chunks_exact(w).zip(out.chunks_exact_mut(w)) {
        padded[r..r + w].copy_from_slice(row);
        for (d, s) in dst.iter_mut().zip(&padded[r..r + w]) {
            *d = s * weights[0];
        }
        for (i, &wt) in weights.iter().enumerate().skip(1) {
            let (ahead, behind) = (&padded[r + i..r + i + w], &padded[r - i..r - i + w]);
            for ((d, a), b) in dst.iter_mut().zip(ahead).zip(behind) {
                *d += (a + b) * wt;
            }
        }
    }
    out
}

/// One pass of the one-sided `weights` down every column, transparent past the ends.
fn convolve_columns(src: &[f32], w: usize, h: usize, weights: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let dst = &mut out[y * w..(y + 1) * w];
        for (d, s) in dst.iter_mut().zip(&src[y * w..(y + 1) * w]) {
            *d = s * weights[0];
        }
        for (i, &wt) in weights.iter().enumerate().skip(1) {
            for yy in [y.checked_sub(i), Some(y + i).filter(|&v| v < h)]
                .into_iter()
                .flatten()
            {
                for (d, s) in dst.iter_mut().zip(&src[yy * w..(yy + 1) * w]) {
                    *d += s * wt;
                }
            }
        }
    }
    out
}

/// `color` (premultiplied, 0..=1) in bytes.
fn tint(color: [f32; 4]) -> [u32; 4] {
    color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u32)
}

/// `color` at `coverage`, premultiplied.
fn tinted(color: [u32; 4], coverage: u8) -> [u32; 4] {
    let a = u32::from(coverage);
    [
        div255(color[0] * a),
        div255(color[1] * a),
        div255(color[2] * a),
        div255(color[3] * a),
    ]
}

/// The shadow of `coverage` in `color` under the content. Where the content is opaque or the shadow empty, nothing
/// changes.
fn under(content: &mut [u8], coverage: &[u8], color: [u32; 4]) {
    for (c, &a) in content.as_chunks_mut::<4>().0.iter_mut().zip(coverage) {
        if a == 0 || c[3] == 255 {
            continue;
        }
        let s = tinted(color, a);
        let keep = 255 - u32::from(c[3]);
        for i in 0..4 {
            c[i] = (u32::from(c[i]) + div255(s[i] * keep)).min(255) as u8;
        }
    }
}

/// The shadow of `coverage` in `color` over the content. Where the shadow is empty, nothing changes.
fn over(content: &mut [u8], coverage: &[u8], color: [u32; 4]) {
    for (c, &a) in content.as_chunks_mut::<4>().0.iter_mut().zip(coverage) {
        if a == 0 {
            continue;
        }
        let s = tinted(color, a);
        let keep = 255 - s[3];
        for i in 0..4 {
            c[i] = (s[i] + div255(u32::from(c[i]) * keep)).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::Point;
    use cupertino_theme::Color;

    #[test]
    fn the_word_wide_pixel_arithmetic_gives_the_bytes_of_the_channel_by_channel_one() {
        // every channel value against every factor, through one channel of a pixel each time
        for c in 0..=255u32 {
            for k in 0..=255u32 {
                for lane in 0..4 {
                    let px = c << (8 * lane) | 0x5a << (8 * ((lane + 1) % 4));
                    let got = (mul_px(px, k) >> (8 * lane)) & 0xff;
                    assert_eq!(got, div255(c * k), "channel {lane}: {c} · {k}");
                }
            }
        }
        // source-over against the per-channel form, on a spread of pixels (premultiplied and not)
        let mut seed = 0x2545_f491_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..200_000 {
            let (s, d) = (next(), next());
            let (sb, db) = (s.to_le_bytes(), d.to_le_bytes());
            let keep = 255 - u32::from(sb[3]);
            let want: [u8; 4] = std::array::from_fn(|i| {
                (u32::from(sb[i]) + div255(u32::from(db[i]) * keep)).min(255) as u8
            });
            assert_eq!(over_px(s, d).to_le_bytes(), want, "{sb:?} over {db:?}");
        }
    }

    /// The plane blur against the shared blur vectors: the 16 x 16 pattern, each channel blurred by the vectors' plan
    /// with transparent edges, within what half-float levels round away.
    #[test]
    fn the_blur_matches_the_shared_vectors() {
        use cupertino_raster_math::vectors;
        let pattern = vectors::pattern();
        let plan = plan_blur(16, 16, 0.5, 6.0, 1.0);
        let channels: Vec<(Vec<f32>, usize)> = (0..4)
            .map(|c| {
                let plane: Vec<f32> = pattern.pixels.iter().map(|p| p[c]).collect();
                let (level, lw, _, _) = pyramid_of(&plane, 16, 16, &plan);
                (level, lw)
            })
            .collect();
        let mut checked = 0;
        for row in vectors::rows(vectors::BLUR) {
            let [edge, precision, x, y, r, g, b, a] = row[..].try_into().expect("eight columns");
            if edge != 1.0 || precision != 0.0 {
                continue;
            }
            for (c, want) in [r, g, b, a].into_iter().enumerate() {
                let (level, lw) = &channels[c];
                let got = level[y as usize * lw + x as usize];
                assert!(
                    (f64::from(got) - want).abs() < 2e-3,
                    "{row:?}: channel {c} is {got}"
                );
            }
            checked += 1;
        }
        assert!(checked >= 16, "{checked}");
    }

    /// The drop and inner shadows of a rounded rectangle against the closed form's vectors, at their pixel scale.
    #[test]
    fn the_shadows_of_a_rounded_rectangle_match_the_shared_closed_form() {
        use cupertino_raster_math::shadow::{drop_shadow, inner_shadow, Caster, Shadow};
        let (w, h) = (240usize, 200usize);
        let caster = Caster {
            half: [60.0, 40.0],
            radius: 10.0,
            extent: 10.0,
            exponent: 2.0,
        };
        let shadow = Shadow {
            sigma: 6.0,
            offset: [3.0, 5.0],
            density: 1.0,
        };
        let centre = [120.0f32, 100.0];
        let mut layer = Pixmap::new(w as u32, h as u32).unwrap();
        for y in 0..h {
            for x in 0..w {
                let p = [x as f32 + 0.5 - centre[0], y as f32 + 0.5 - centre[1]];
                let d =
                    cupertino_raster_math::coverage::rounded_distance(p, caster.half, 10.0, 2.0);
                let a = cupertino_raster_math::dither::quantize(
                    cupertino_raster_math::coverage::coverage(d, 1.0),
                );
                layer.data_mut()[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[a; 4]);
            }
        }
        let offset = Point::new(3.0, 5.0);
        // the toolkit names a deviation by twice it
        let drop = shadow_coverage(layer.data(), w, h, offset, sigma_px(12.0, 1.0), 1.0, false);
        let inner = shadow_coverage(layer.data(), w, h, offset, sigma_px(12.0, 1.0), 1.0, true);
        let mut worst = (0.0f32, 0.0f32);
        for y in (0..h).step_by(3) {
            for x in (0..w).step_by(3) {
                let p = [x as f32 + 0.5 - centre[0], y as f32 + 0.5 - centre[1]];
                let want = drop_shadow(p, &caster, &shadow, false, 1.0);
                worst.0 = worst.0.max((drop[y * w + x] - want).abs());
                let cover = f32::from(layer.data()[(y * w + x) * 4 + 3]) / 255.0;
                let want = inner_shadow(p, &caster, &shadow, 1.0);
                worst.1 = worst.1.max((inner[y * w + x] * cover - want).abs());
            }
        }
        // the closed form leaves the tails past three deviations out and approximates the error function
        assert!(worst.0 < 0.012 && worst.1 < 0.012, "{worst:?}");
    }

    /// A shadow's alpha is dithered by window position: its mean follows the coverage and the same pixel always gets
    /// the same value.
    #[test]
    fn a_shadow_is_dithered_by_its_window_position() {
        let plane = vec![0.3013f32; 64];
        let a = store_coverage(&plane, 8, (16, 24), true);
        let b = store_coverage(&plane, 8, (16, 24), true);
        assert_eq!(a, b);
        let mean = a.iter().map(|&v| f32::from(v)).sum::<f32>() / 64.0;
        assert!((mean - 0.3013 * 255.0).abs() < 0.05, "{mean}");
        assert!(a.iter().any(|&v| v != a[0]));
        // keyed to the window: the same layer one matrix period over gives the same bytes
        assert_eq!(store_coverage(&plane, 8, (24, 32), true), a);
    }

    /// The panel's shadow at scale 2 and 1.5 reaches past it as a falloff: dark beside the panel, nothing far away.
    #[test]
    fn a_drop_shadow_falls_off_past_the_shape() {
        for &scale in &[1.5, 2.0] {
            let side = (200.0 * scale) as u32;
            let mut pm = Pixmap::new(side, side).unwrap();
            let inset = (60.0 * scale) as usize;
            let s = side as usize;
            for y in inset..s - inset {
                for x in inset..s - inset {
                    pm.data_mut()[(y * s + x) * 4..(y * s + x) * 4 + 4].copy_from_slice(&[255; 4]);
                }
            }
            let effect = Effect::DropShadow {
                offset: Point::new(0.0, 8.0),
                blur: 24.0,
                color: Color::new(0.0, 0.0, 0.0, 0.28),
            };
            apply(&mut pm, &effect, scale, (0, 0));
            let alpha = |x: usize, y: usize| pm.data()[(y * s + x) * 4 + 3];
            let below = inset + (s - 2 * inset) / 2;
            assert_eq!(alpha(below, below), 255, "the shape stays opaque");
            let near = alpha(below, s - inset + 2);
            assert!(
                near > 20,
                "the shadow shows beside the shape at {scale}: {near}"
            );
            assert_eq!(alpha(0, 0), 0, "and fades out far from it at {scale}");
        }
    }
}
