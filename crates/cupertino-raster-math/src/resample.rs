//! Shrinking a frame drawn at a whole scale to the pixels of a display at a smaller, fractional one.
//!
//! **The area filter.** Each pixel of the smaller image is the average of the larger image over the area the pixel
//! covers there, each texel weighted by how much of it the pixel covers. Along an axis that shrinks by `ratio` (the
//! smaller size over the larger, 0.75 for a frame drawn at 2 for a display at 1.5), pixel `i` covers the texels from
//! `i / ratio` to `(i + 1) / ratio`; the two axes multiply. Texels are read past the image's edge as the edge's own.
//!
//! Why this filter: its weights are never negative, so an edge never rings (overshoots); every texel gives the same
//! total to the pixels it reaches, so a line keeps its weight wherever it lies and an edge moves through the pixels at
//! an even pace; and it folds back less of the detail finer than the display can show. Plain bilinear lookups at the
//! pixels' centres weigh texels by where each centre falls, which repeats only every few pixels (every three at
//! 0.75): a texel gives two thirds of a pixel in one place and five sixths in the next, so a thin line is fainter in
//! some places than in others and an edge moves unevenly. The tests measure both, with their response to patterns of
//! each frequency.
//!
//! [`GLSL_TAPS`] is the shader implementation (four bilinear lookups when the footprint is at most two texels a side,
//! as it is for every ratio from 0.5 up); the compositor's final pass is tested against [`shrink`].

/// The texels of the larger image pixel `i` of the smaller one covers along an axis shrunk by `ratio`: from `lo` to
/// `hi`, in texels.
pub fn footprint(i: u32, ratio: f32) -> (f32, f32) {
    (i as f32 / ratio, (i + 1) as f32 / ratio)
}

/// The weights of the texels pixel `i` covers along an axis of `len` texels shrunk by `ratio`: `(texel, weight)`,
/// weights summing to 1, a texel past the edge counted as the edge's.
pub fn weights(i: u32, ratio: f32, len: u32) -> Vec<(u32, f32)> {
    let (lo, hi) = footprint(i, ratio);
    let mut out: Vec<(u32, f32)> = Vec::new();
    let mut t = lo.floor();
    while t < hi {
        let w = (t + 1.0).min(hi) - t.max(lo);
        if w > 0.0 {
            let texel = (t.max(0.0) as u32).min(len.saturating_sub(1));
            match out.iter_mut().find(|(at, _)| *at == texel) {
                Some((_, sum)) => *sum += w,
                None => out.push((texel, w)),
            }
        }
        t += 1.0;
    }
    let total: f32 = out.iter().map(|(_, w)| w).sum();
    for (_, w) in &mut out {
        *w /= total;
    }
    out
}

/// One row of `len` values shrunk by `ratio` to `out` values with the area filter.
pub fn shrink_row(row: &[f32], out: u32, ratio: f32) -> Vec<f32> {
    (0..out)
        .map(|i| {
            weights(i, ratio, row.len() as u32)
                .iter()
                .map(|(t, w)| row[*t as usize] * w)
                .sum()
        })
        .collect()
}

/// An image of `size` texels, four channels each, row by row, shrunk to `out` pixels with the area filter.
pub fn shrink(image: &[[f32; 4]], size: (u32, u32), out: (u32, u32)) -> Vec<[f32; 4]> {
    let ratio = (out.0 as f32 / size.0 as f32, out.1 as f32 / size.1 as f32);
    let mut result = Vec::with_capacity((out.0 * out.1) as usize);
    for y in 0..out.1 {
        let wy = weights(y, ratio.1, size.1);
        for x in 0..out.0 {
            let wx = weights(x, ratio.0, size.0);
            let mut px = [0.0f32; 4];
            for (ty, a) in &wy {
                for (tx, b) in &wx {
                    let texel = image[(ty * size.0 + tx) as usize];
                    for c in 0..4 {
                        px[c] += texel[c] * a * b;
                    }
                }
            }
            result.push(px);
        }
    }
    result
}

/// For comparison: one row shrunk by plain bilinear lookups, each pixel the larger row read between its two texels
/// nearest the pixel's centre.
pub fn bilinear_row(row: &[f32], out: u32, ratio: f32) -> Vec<f32> {
    let last = row.len() as f32 - 1.0;
    (0..out)
        .map(|i| {
            let at = ((i as f32 + 0.5) / ratio - 0.5).clamp(0.0, last);
            let first = at.floor();
            let f = at - first;
            let a = row[first as usize];
            let b = row[(first + 1.0).min(last) as usize];
            a + (b - a) * f
        })
        .collect()
}

/// For comparison: one row shrunk by the Catmull-Rom cubic widened to the pixel's footprint, a sharper filter with
/// negative lobes.
pub fn catmull_rom_row(row: &[f32], out: u32, ratio: f32) -> Vec<f32> {
    let cubic = |x: f32| {
        let x = x.abs();
        if x < 1.0 {
            1.5 * x * x * x - 2.5 * x * x + 1.0
        } else if x < 2.0 {
            -0.5 * x * x * x + 2.5 * x * x - 4.0 * x + 2.0
        } else {
            0.0
        }
    };
    let len = row.len() as i32;
    (0..out)
        .map(|i| {
            let centre = (i as f32 + 0.5) / ratio;
            let reach = 2.0 / ratio;
            let (lo, hi) = (
                (centre - reach).floor() as i32,
                (centre + reach).ceil() as i32,
            );
            let (mut sum, mut total) = (0.0, 0.0);
            for t in lo..=hi {
                let w = cubic((t as f32 + 0.5 - centre) * ratio);
                sum += row[t.clamp(0, len - 1) as usize] * w;
                total += w;
            }
            sum / total
        })
        .collect()
}

/// The shader implementation of [`shrink`] (GLSL ES 1.00): `area_shrink(tex, centre, footprint, texels)` reads the
/// texels a pixel covers about its `centre` (texels), `footprint` texels a side (at most two), from a texture of
/// `texels` filtered linearly.
pub const GLSL_TAPS: &str = r"
// One axis of a box from `lo` to `hi` (texels) over at most three texels, as two linear lookups: the first two texels
// blended where their weights put the lookup, and the third at its centre. Places in `at`, weights in `weight`.
void area_taps(float lo, float hi, out vec2 at, out vec2 weight) {
    float first = floor(lo);
    float w0 = min(first + 1.0, hi) - lo;
    float w1 = clamp(min(first + 2.0, hi) - (first + 1.0), 0.0, 1.0);
    float w2 = max(hi - (first + 2.0), 0.0);
    float pair = w0 + w1;
    at = vec2(first + 0.5 + w1 / max(pair, 1e-6), first + 2.5);
    weight = vec2(pair, w2);
}

vec4 area_shrink(sampler2D image, vec2 centre, vec2 footprint, vec2 texels) {
    vec2 lo = centre - footprint * 0.5;
    vec2 hi = centre + footprint * 0.5;
    vec2 ax, wx, ay, wy;
    area_taps(lo.x, hi.x, ax, wx);
    area_taps(lo.y, hi.y, ay, wy);
    vec4 c = texture2D(image, vec2(ax.x, ay.x) / texels) * (wx.x * wy.x)
        + texture2D(image, vec2(ax.y, ay.x) / texels) * (wx.y * wy.x)
        + texture2D(image, vec2(ax.x, ay.y) / texels) * (wx.x * wy.y)
        + texture2D(image, vec2(ax.y, ay.y) / texels) * (wx.y * wy.y);
    return c / max((wx.x + wx.y) * (wy.x + wy.y), 1e-6);
}
";

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame drawn at 2 for a display at 1.5.
    const RATIO: f32 = 0.75;

    /// The coverage of a texel `[t, t + 1)` by everything right of `edge`.
    fn covered(t: f32, edge: f32) -> f32 {
        (t + 1.0 - t.max(edge)).clamp(0.0, 1.0)
    }

    #[test]
    fn the_weights_are_never_negative_and_sum_to_one_at_every_ratio() {
        for ratio in [0.5, 0.625, 0.75, 0.8, 0.875, 0.9375] {
            for i in 0..64 {
                let w = weights(i, ratio, 128);
                assert!(w.iter().all(|(_, w)| *w >= 0.0), "{ratio} {i}: {w:?}");
                let sum: f32 = w.iter().map(|(_, w)| w).sum();
                assert!((sum - 1.0).abs() < 1e-5, "{ratio} {i}: {sum}");
                // the shader's four lookups reach every texel: at most three a side, from a footprint of at most two
                assert!(w.len() <= 3, "{ratio} {i}: {w:?}");
            }
        }
    }

    #[test]
    fn an_edge_moves_through_the_pixels_at_an_even_pace_where_bilinear_speeds_up_and_slows_down() {
        // an edge right of which all is covered, drawn as coverage at 2x and moved over four texels (three pixels) in
        // 64ths of a texel: what each step uncovers of the row of pixels
        let (texels, pixels) = (32, 24);
        let uncovered = |filter: fn(&[f32], u32, f32) -> Vec<f32>, edge: f32| {
            let row: Vec<f32> = (0..texels).map(|t| covered(t as f32, edge)).collect();
            filter(&row, pixels, RATIO)
                .iter()
                .map(|v| 1.0 - v)
                .sum::<f32>()
        };
        let paces = |filter: fn(&[f32], u32, f32) -> Vec<f32>| {
            (0..256)
                .map(|step| {
                    let edge = 12.0 + step as f32 / 64.0;
                    (uncovered(filter, edge + 1.0 / 64.0) - uncovered(filter, edge)) * 64.0
                })
                .collect::<Vec<f32>>()
        };
        let range = |s: &[f32]| {
            (
                s.iter().cloned().fold(f32::MAX, f32::min),
                s.iter().cloned().fold(f32::MIN, f32::max),
            )
        };
        // a texel's worth of travel uncovers three quarters of a pixel, at every step
        let (lo, hi) = range(&paces(shrink_row));
        assert!(
            (lo - RATIO).abs() < 1e-3 && (hi - RATIO).abs() < 1e-3,
            "{lo} {hi}"
        );
        // bilinear: two thirds of a pixel over some texels, five sixths over others
        let (lo, hi) = range(&paces(bilinear_row));
        assert!(lo < 0.7 && hi > 0.8, "{lo} {hi}");
    }

    #[test]
    fn a_thin_line_keeps_its_weight_wherever_it_lies_where_bilinear_does_not() {
        // a line one texel wide (half a point at 2x) at every texel of the row
        let (texels, pixels) = (64, 48);
        let mut area_sums = Vec::new();
        let mut bilinear_sums = Vec::new();
        for at in 8..56 {
            let row: Vec<f32> = (0..texels).map(|t| f32::from(u8::from(t == at))).collect();
            area_sums.push(shrink_row(&row, pixels, RATIO).iter().sum::<f32>());
            bilinear_sums.push(bilinear_row(&row, pixels, RATIO).iter().sum::<f32>());
        }
        let spread = |s: &[f32]| {
            s.iter().cloned().fold(f32::MIN, f32::max) - s.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!(spread(&area_sums) < 1e-5, "{area_sums:?}");
        assert!(
            (area_sums[0] - RATIO).abs() < 1e-5,
            "a line of one texel covers three quarters of a pixel"
        );
        assert!(spread(&bilinear_sums) > 0.15, "{bilinear_sums:?}");
    }

    #[test]
    fn a_step_never_rings_where_a_sharper_cubic_does() {
        for step in 0..64 {
            let edge = 16.0 + step as f32 / 64.0;
            let row: Vec<f32> = (0..48).map(|t| covered(t as f32, edge)).collect();
            for v in shrink_row(&row, 36, RATIO) {
                assert!((-1e-6..=1.0 + 1e-6).contains(&v), "{v}");
            }
        }
        let row: Vec<f32> = (0..48).map(|t| f32::from(u8::from(t >= 24))).collect();
        let cubic = catmull_rom_row(&row, 36, RATIO);
        let over = cubic.iter().cloned().fold(0.0f32, f32::max) - 1.0;
        let under = -cubic.iter().cloned().fold(1.0f32, f32::min);
        assert!(
            over > 1e-3 && under > 1e-3,
            "the cubic rings: {over} over, {under} under"
        );
    }

    /// The amplitude of the frequency `f` (cycles a pixel) in `values`, sampled one a pixel.
    fn amplitude(values: &[f32], f: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in values.iter().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * f64::from(f) * (i as f64 + 0.5);
            re += f64::from(*v) * phase.cos();
            im += f64::from(*v) * phase.sin();
        }
        (2.0 * (re * re + im * im).sqrt() / values.len() as f64) as f32
    }

    /// A pattern of `f` cycles a pixel (of the smaller image) drawn into the larger one at each texel's centre, and
    /// shrunk by `filter`: the amplitude kept at `f`, and that which shows at `alias` instead.
    fn response(filter: fn(&[f32], u32, f32) -> Vec<f32>, f: f32, alias: f32) -> (f32, f32) {
        // whole periods of the pattern and of the 4 : 3 grid, long enough for the leakage between bins to vanish
        let pixels = 2400u32;
        let texels = (pixels as f32 / RATIO) as u32;
        let row: Vec<f32> = (0..texels)
            .map(|t| {
                let x = (t as f32 + 0.5) * RATIO;
                0.5 + 0.5 * (2.0 * std::f32::consts::PI * f * x).cos()
            })
            .collect();
        let out = filter(&row, pixels, RATIO);
        (amplitude(&out, f) / 0.5, amplitude(&out, alias) / 0.5)
    }

    #[test]
    fn the_area_filter_keeps_the_detail_a_display_can_show_and_folds_back_less_of_what_it_cannot_than_bilinear(
    ) {
        // a pattern of a period of four pixels: kept, a little softer than bilinear keeps it
        let (area, _) = response(shrink_row, 0.25, 0.0);
        let (bilinear, _) = response(bilinear_row, 0.25, 0.0);
        assert!(area > 0.84, "{area}");
        assert!(bilinear > area, "{bilinear} {area}");
        // finer than the display can show (0.6 cycles a pixel, which the 2x frame holds): what folds back as a coarser
        // pattern (0.4 cycles a pixel) that was never drawn, less of it through the area filter
        let (_, area_alias) = response(shrink_row, 0.6, 0.4);
        let (_, bilinear_alias) = response(bilinear_row, 0.6, 0.4);
        assert!(area_alias < 0.4, "{area_alias}");
        assert!(
            bilinear_alias > area_alias + 0.05,
            "{bilinear_alias} {area_alias}"
        );
    }

    #[test]
    fn an_image_shrinks_axis_by_axis_as_its_rows_do() {
        let size = (8u32, 4u32);
        let image: Vec<[f32; 4]> = (0..size.0 * size.1)
            .map(|i| {
                let v = (i % 7) as f32 / 6.0;
                [v, 1.0 - v, 0.5, 1.0]
            })
            .collect();
        let out = shrink(&image, size, (6, 3));
        assert_eq!(out.len(), 18);
        // a flat image stays flat; every pixel stays inside what it reads
        let flat = shrink(&vec![[0.25, 0.5, 0.75, 1.0]; 32], size, (6, 3));
        for px in &flat {
            for (c, want) in px.iter().zip([0.25, 0.5, 0.75, 1.0]) {
                assert!((c - want).abs() < 1e-6);
            }
        }
        for px in &out {
            assert!(px.iter().all(|c| (-1e-6..=1.0 + 1e-6).contains(c)));
        }
        // a row's first pixel is the first texel and a third of the second
        let first = (image[0][0] + image[1][0] / 3.0) / (4.0 / 3.0);
        let first_rows = (first + {
            // the second row
            let row = size.0 as usize;
            (image[row][0] + image[row + 1][0] / 3.0) / (4.0 / 3.0)
        } / 3.0)
            / (4.0 / 3.0);
        assert!(
            (out[0][0] - first_rows).abs() < 1e-5,
            "{} {first_rows}",
            out[0][0]
        );
    }
}
