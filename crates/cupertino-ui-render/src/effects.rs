use crate::Effect;
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
        // a shadow is one channel until it is tinted: its silhouette and blur hold coverage only, a byte a pixel
        Effect::DropShadow {
            offset,
            blur: b,
            color,
        } => {
            let mut shadow = silhouette(layer.data(), w, h, offset, scale, false);
            blur_coverage(&mut shadow, w, h, box_radius(b, scale));
            under(layer.data_mut(), &shadow, tint(color.premultiplied()));
        }
        Effect::InnerShadow {
            offset,
            blur: b,
            color,
        } => {
            let mut shadow = silhouette(layer.data(), w, h, offset, scale, true);
            blur_coverage(&mut shadow, w, h, box_radius(b, scale));
            for (s, c) in shadow.iter_mut().zip(layer.data().as_chunks::<4>().0) {
                *s = div255(u32::from(*s) * u32::from(c[3])) as u8;
            }
            over(layer.data_mut(), &shadow, tint(color.premultiplied()));
        }
    }
}

/// The layer's alpha, shifted by `offset` points and optionally inverted: one coverage byte a pixel.
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
    let outside = if invert { 255 } else { 0 };
    let mut out = vec![outside; w * h];
    for y in 0..h {
        let sy = y as i64 - dy;
        if sy < 0 || sy >= h as i64 {
            continue;
        }
        let row = &data[sy as usize * w * 4..(sy as usize + 1) * w * 4];
        // the columns whose source column lies inside the layer
        let x0 = dx.clamp(0, w as i64) as usize;
        let x1 = (w as i64 + dx).clamp(0, w as i64) as usize;
        for (x, o) in out[y * w..(y + 1) * w]
            .iter_mut()
            .enumerate()
            .take(x1)
            .skip(x0)
        {
            let alpha = row[(x as i64 - dx) as usize * 4 + 3];
            *o = if invert { 255 - alpha } else { alpha };
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

/// [`blur`] of a buffer of one coverage byte a pixel, with the same sums and rounding. Rows are blurred in place and
/// columns a row at a time with a running sum per column, so both passes read memory in order.
fn blur_coverage(data: &mut [u8], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let mut line = vec![0u8; w];
    let mut source = vec![0u8; w * h];
    let mut sums = vec![0u32; w];
    for _ in 0..3 {
        for row in data.chunks_exact_mut(w) {
            line.copy_from_slice(row);
            box_line(&line, row, r);
        }
        source.copy_from_slice(data);
        box_columns(&source, data, &mut sums, w, h, r);
    }
}

/// One box pass of radius `r` along `src` into `out`, values past either end counting as 0.
fn box_line(src: &[u8], out: &mut [u8], r: usize) {
    let len = src.len();
    let window = (2 * r + 1) as u32;
    let half = window / 2;
    let mut sum = 0u32;
    let mut i = 0;
    let first = r.min(len - 1);
    while i <= first {
        sum += u32::from(src[i]);
        i += 1;
    }
    let mut i = 0;
    while i < len {
        out[i] = ((sum + half) / window) as u8;
        if i + r + 1 < len {
            sum += u32::from(src[i + r + 1]);
        }
        if i >= r {
            sum -= u32::from(src[i - r]);
        }
        i += 1;
    }
}

/// [`box_line`] down every column of the `w`-wide `src` into `out`, a row at a time.
fn box_columns(src: &[u8], out: &mut [u8], sums: &mut [u32], w: usize, h: usize, r: usize) {
    let window = (2 * r + 1) as u32;
    let half = window / 2;
    sums.fill(0);
    let first = r.min(h - 1);
    let mut y = 0;
    while y <= first {
        add_row(sums, &src[y * w..(y + 1) * w]);
        y += 1;
    }
    let mut y = 0;
    while y < h {
        let row = &mut out[y * w..(y + 1) * w];
        let mut x = 0;
        while x < w {
            row[x] = ((sums[x] + half) / window) as u8;
            x += 1;
        }
        if y + r + 1 < h {
            add_row(sums, &src[(y + r + 1) * w..(y + r + 2) * w]);
        }
        if y >= r {
            let gone = &src[(y - r) * w..(y - r + 1) * w];
            let mut x = 0;
            while x < w {
                sums[x] -= u32::from(gone[x]);
                x += 1;
            }
        }
        y += 1;
    }
}

fn add_row(sums: &mut [u32], row: &[u8]) {
    let mut x = 0;
    while x < row.len() {
        sums[x] += u32::from(row[x]);
        x += 1;
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

    /// The shadows as four-channel images: the silhouette's alpha in RGBA, blurred on every channel, tinted, then laid
    /// under or over the content. The coverage path must give the same bytes.
    fn reference(layer: &mut Pixmap, effect: &Effect, scale: f64) {
        let (w, h) = (layer.width() as usize, layer.height() as usize);
        let (offset, b, color, inner) = match *effect {
            Effect::DropShadow {
                offset,
                blur,
                color,
            } => (offset, blur, color, false),
            Effect::InnerShadow {
                offset,
                blur,
                color,
            } => (offset, blur, color, true),
            Effect::Blur { .. } => unreachable!(),
        };
        let dx = (offset.x * scale).round() as i64;
        let dy = (offset.y * scale).round() as i64;
        let mut shadow = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x as i64 - dx, y as i64 - dy);
                let inside = sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h;
                let alpha = if inside {
                    layer.data()[(sy as usize * w + sx as usize) * 4 + 3]
                } else {
                    0
                };
                shadow[(y * w + x) * 4 + 3] = if inner { 255 - alpha } else { alpha };
            }
        }
        blur(&mut shadow, w, h, box_radius(b, scale));
        if inner {
            for (s, c) in shadow
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(layer.data().as_chunks::<4>().0)
            {
                s[3] = div255(u32::from(s[3]) * u32::from(c[3])) as u8;
            }
        }
        let c = tint(color.premultiplied());
        for px in shadow.as_chunks_mut::<4>().0.iter_mut() {
            let a = u32::from(px[3]);
            for i in 0..4 {
                px[i] = div255(c[i] * a) as u8;
            }
        }
        for (c, s) in layer
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(shadow.as_chunks::<4>().0)
        {
            if inner {
                let keep = 255 - u32::from(s[3]);
                for i in 0..4 {
                    c[i] = (u32::from(s[i]) + div255(u32::from(c[i]) * keep)).min(255) as u8;
                }
            } else {
                let keep = 255 - u32::from(c[3]);
                for i in 0..4 {
                    c[i] = (u32::from(c[i]) + div255(u32::from(s[i]) * keep)).min(255) as u8;
                }
            }
        }
    }

    /// A premultiplied layer of `w` × `h`: an opaque rounded panel with soft edges, translucent specks and holes.
    fn layer(w: u32, h: u32, seed: u32) -> Pixmap {
        let mut pm = Pixmap::new(w, h).unwrap();
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let (fw, fh) = (f64::from(w), f64::from(h));
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                // distance outside a rectangle inset by a quarter, rounded by an eighth of the short side
                let r = fw.min(fh) / 8.0;
                let qx = (px - fw / 2.0).abs() - (fw / 4.0 - r);
                let qy = (py - fh / 2.0).abs() - (fh / 4.0 - r);
                let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
                let mut a = ((0.5 - outside).clamp(0.0, 1.0) * 255.0).round() as u32;
                let noise = next();
                if noise % 11 == 0 {
                    a = noise >> 24;
                }
                let i = ((y * w + x) * 4) as usize;
                let a8 = a as u8;
                let px = &mut pm.data_mut()[i..i + 4];
                for (k, v) in px.iter_mut().take(3).enumerate() {
                    *v = div255(((noise >> (8 * k)) & 0xff) * a) as u8;
                }
                px[3] = a8;
            }
        }
        pm
    }

    #[test]
    fn the_shadows_match_four_channel_shadows_byte_for_byte_at_every_scale() {
        // translucent and coloured, opaque and saturated, and light enough that the sums reach the clamp
        let colors = [
            Color::new(0.1, 0.2, 0.3, 0.28),
            Color::new(1.0, 0.4, 0.0, 1.0),
            Color::new(1.0, 1.0, 1.0, 0.6),
        ];
        let mut seed = 0;
        for &scale in &[1.0, 1.5, 2.0] {
            for &(w, h) in &[(97, 61), (5, 140), (160, 3), (1, 1), (40, 40)] {
                for (&(offset, b), color) in [
                    (Point::new(0.0, 8.0), 24.0),
                    (Point::new(-3.0, 2.5), 6.0),
                    (Point::new(40.0, -50.0), 1.0),
                    (Point::new(0.0, 0.0), 0.0),
                    // a kernel wider than every layer here
                    (Point::new(2.0, 0.0), 200.0),
                ]
                .iter()
                .flat_map(|case| colors.iter().map(move |&c| (case, c)))
                {
                    for effect in [
                        Effect::DropShadow {
                            offset,
                            blur: b,
                            color,
                        },
                        Effect::InnerShadow {
                            offset,
                            blur: b,
                            color,
                        },
                    ] {
                        seed += 1;
                        let mut fast = layer(w, h, seed);
                        let mut slow = fast.clone();
                        apply(&mut fast, &effect, scale);
                        reference(&mut slow, &effect, scale);
                        assert!(
                            fast.data() == slow.data(),
                            "{effect:?} at {scale} on {w} × {h}"
                        );
                    }
                }
            }
        }
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
            apply(&mut pm, &effect, scale);
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
