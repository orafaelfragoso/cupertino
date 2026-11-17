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
