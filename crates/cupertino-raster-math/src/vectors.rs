//! Test vectors: inputs with the outputs the definitions give, committed as plain text so every renderer, in any
//! language, can check itself against the same numbers.
//!
//! Each file is lines of numbers separated by spaces; a line starting with `#` is a comment, and the first comment
//! names the columns. The crate's own tests compute every line again and fail on a difference; after a change to a
//! definition, which is a change to the look, `CUPERTINO_WRITE_VECTORS=1` writes them again.

/// `px py hx hy extent exponent pixels distance coverage`.
pub const COVERAGE: &str = include_str!("../vectors/coverage.txt");
/// `sigma radius w0 w1 ... wr`.
pub const KERNEL: &str = include_str!("../vectors/kernel.txt");
/// `w h backdrop_scale sigma_pt display_scale levels (lw lh)... sigma radius`.
pub const PLAN: &str = include_str!("../vectors/plan.txt");
/// `edge precision x y r g b a`: the 16 × 16 pattern of [`pattern`] blurred by the plan of
/// `plan_blur(16, 16, 0.5, 6, 1)`; edge 0 is clamp and 1 transparent, precision 0 half and 1 byte.
pub const BLUR: &str = include_str!("../vectors/blur.txt");
/// `px py hx hy radius extent exponent sigma ox oy density pixels drop drop_cut inner`.
pub const SHADOW: &str = include_str!("../vectors/shadow.txt");
/// `gradient t r g b a`: premultiplied colour at `t` of the stops of [`gradient_stops`] number `gradient`.
pub const GRADIENT: &str = include_str!("../vectors/gradient.txt");
/// `x y v offset stored`.
pub const DITHER: &str = include_str!("../vectors/dither.txt");

/// The numbers of each line of a vector file, comments left out.
pub fn rows(text: &str) -> Vec<Vec<f64>> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            l.split_whitespace()
                .map(|n| {
                    n.parse()
                        .unwrap_or_else(|_| panic!("not a number: {n} in {l}"))
                })
                .collect()
        })
        .collect()
}

/// The 16 × 16 premultiplied test pattern the blur vectors start from: a translucent disc on a ramp with a hard
/// opaque bar, so edges, alpha and the halving grid all show.
pub fn pattern() -> crate::blur::Image {
    let (w, h) = (16usize, 16usize);
    let pixels = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            if (5..7).contains(&x) {
                return [1.0, 1.0, 1.0, 1.0];
            }
            let r = ((fx - 10.0).powi(2) + (fy - 9.0).powi(2)).sqrt();
            let a = if r < 4.0 { 0.75 } else { fy / 16.0 };
            [a * fx / 16.0, a * 0.5, a * (1.0 - fx / 16.0), a]
        })
        .collect();
    crate::blur::Image::new(w, h, pixels)
}

/// The stops of the gradient vectors.
pub fn gradient_stops() -> Vec<crate::gradient::Stops> {
    use crate::gradient::{Stop, Stops};
    let stop = |offset, color| Stop { offset, color };
    [
        vec![
            stop(0.0, [1.0, 0.0, 0.0, 1.0]),
            stop(1.0, [0.0, 0.0, 1.0, 1.0]),
        ],
        vec![
            stop(0.1, [1.0, 1.0, 1.0, 0.0]),
            stop(0.6, [0.2, 0.4, 0.6, 0.8]),
            stop(0.9, [0.0, 0.0, 0.0, 1.0]),
        ],
        (0..8)
            .map(|i| {
                let k = i as f32 / 7.0;
                stop(k, [k, 1.0 - k, (k * 3.0).fract(), 0.5 + 0.5 * k])
            })
            .collect(),
    ]
    .iter()
    .map(|s| Stops::new(s).expect("valid stops"))
    .collect()
}
