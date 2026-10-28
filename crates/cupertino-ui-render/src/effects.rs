use crate::Effect;
use tiny_skia::Pixmap;

pub(crate) fn div255(v: u32) -> u32 {
    let v = v + 128;
    (v + (v >> 8)) >> 8
}

/// Largest box radius, in pixels, a blur may have.
const MAX_BOX_RADIUS: usize = 1 << 12;

/// Box radius of the three passes that approximate a Gaussian whose standard deviation is half of `radius` points.
fn box_radius(radius: f64, scale: f64) -> usize {
    ((radius * scale / 2.0).round() as usize).min(MAX_BOX_RADIUS)
}

/// How far, in pixels, an effect reaches beyond the content it is computed from.
pub(crate) fn support(effects: &[Effect], scale: f64) -> i32 {
    effects
        .iter()
        .map(|effect| match *effect {
            Effect::Blur { radius } => 3 * box_radius(radius, scale) as i32,
            Effect::DropShadow { offset, blur, .. } | Effect::InnerShadow { offset, blur, .. } => {
                let shift = (offset.x.abs().max(offset.y.abs()) * scale)
                    .ceil()
                    .min(f64::from(1 << 20)) as i32;
                shift + 3 * box_radius(blur, scale) as i32
            }
        })
        .fold(0, i32::saturating_add)
}

pub(crate) fn apply(layer: &mut Pixmap, effect: &Effect, scale: f64) {
    let (w, h) = (layer.width() as usize, layer.height() as usize);
    match *effect {
        Effect::Blur { radius } => blur(layer.data_mut(), w, h, box_radius(radius, scale)),
        Effect::DropShadow {
            offset,
            blur: b,
            color,
        } => {
            let mut shadow = silhouette(layer.data(), w, h, offset, scale, false);
            blur(&mut shadow, w, h, box_radius(b, scale));
            tint(&mut shadow, color.premultiplied());
            under(layer.data_mut(), &shadow);
        }
        Effect::InnerShadow {
            offset,
            blur: b,
            color,
        } => {
            let mut shadow = silhouette(layer.data(), w, h, offset, scale, true);
            blur(&mut shadow, w, h, box_radius(b, scale));
            for (s, c) in shadow
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(layer.data().as_chunks::<4>().0)
            {
                s[3] = div255(u32::from(s[3]) * u32::from(c[3])) as u8;
            }
            tint(&mut shadow, color.premultiplied());
            over(layer.data_mut(), &shadow);
        }
    }
}

/// The layer's alpha, shifted by `offset` points and optionally inverted, as RGBA with only alpha set.
fn silhouette(
    data: &[u8],
    w: usize,
    h: usize,
    offset: cupertino_geometry::Point,
    scale: f64,
    invert: bool,
) -> Vec<u8> {
    let dx = (offset.x * scale).round().clamp(-1e9, 1e9) as i64;
    let dy = (offset.y * scale).round().clamp(-1e9, 1e9) as i64;
    let mut out = vec![0u8; data.len()];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x as i64 - dx, y as i64 - dy);
            let inside = sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h;
            let alpha = if inside {
                data[(sy as usize * w + sx as usize) * 4 + 3]
            } else {
                0
            };
            out[(y * w + x) * 4 + 3] = if invert { 255 - alpha } else { alpha };
        }
    }
    out
}

/// Replaces the colour of an alpha-only buffer with `color` (premultiplied, 0..=1) scaled by that alpha.
fn tint(data: &mut [u8], color: [f32; 4]) {
    let c = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u32);
    for px in data.as_chunks_mut::<4>().0.iter_mut() {
        let a = u32::from(px[3]);
        for i in 0..3 {
            px[i] = div255(c[i] * a) as u8;
        }
        px[3] = div255(c[3] * a) as u8;
    }
}

fn under(content: &mut [u8], shadow: &[u8]) {
    for (c, s) in content
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(shadow.as_chunks::<4>().0)
    {
        let keep = 255 - u32::from(c[3]);
        for i in 0..4 {
            c[i] = (u32::from(c[i]) + div255(u32::from(s[i]) * keep)).min(255) as u8;
        }
    }
}

fn over(content: &mut [u8], shadow: &[u8]) {
    for (c, s) in content
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(shadow.as_chunks::<4>().0)
    {
        let keep = 255 - u32::from(s[3]);
        for i in 0..4 {
            c[i] = (u32::from(s[i]) + div255(u32::from(c[i]) * keep)).min(255) as u8;
        }
    }
}

/// Three box passes on each axis, premultiplied channels, pixels outside the buffer counting as transparent.
fn blur(data: &mut [u8], w: usize, h: usize, r: usize) {
    if r == 0 {
        return;
    }
    for _ in 0..3 {
        blur_axis(data, w, h, r, true);
        blur_axis(data, w, h, r, false);
    }
}

fn blur_axis(data: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines, step, line_step) = if horizontal {
        (w, h, 4, w * 4)
    } else {
        (h, w, w * 4, 4)
    };
    let window = (2 * r + 1) as u32;
    let mut line = vec![0u8; len * 4];
    for l in 0..lines {
        let base = l * line_step;
        for i in 0..len {
            line[i * 4..i * 4 + 4].copy_from_slice(&data[base + i * step..base + i * step + 4]);
        }
        let mut sum = [0u32; 4];
        for i in 0..=r.min(len - 1) {
            for c in 0..4 {
                sum[c] += u32::from(line[i * 4 + c]);
            }
        }
        for i in 0..len {
            for c in 0..4 {
                data[base + i * step + c] = ((sum[c] + window / 2) / window) as u8;
            }
            if i + r + 1 < len {
                for c in 0..4 {
                    sum[c] += u32::from(line[(i + r + 1) * 4 + c]);
                }
            }
            if i >= r {
                for c in 0..4 {
                    sum[c] -= u32::from(line[(i - r) * 4 + c]);
                }
            }
        }
    }
}
