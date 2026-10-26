//! Shadow kernel helpers: Gaussian weights, box-blur approximations, bilinear-tap merging and
//! closed-form blurred rectangles.
//!
//! the design window server draws shadows as a separable blur of the window shape
//! mask; shadow radius, opacity and offset are runtime values and
//! unknown. Everything here is parametric in `sigma` (standard deviation in layout units).

use crate::types::{Point, Rect};

/// Support radius in samples beyond which a Gaussian is truncated (3σ keeps 99.73 %).
pub fn kernel_radius(sigma: f64) -> usize {
    if sigma < 0.3 {
        0
    } else {
        (3.0 * sigma).ceil() as usize
    }
}

/// One-sided Gaussian weights `w[0..=r]` (`r = kernel_radius(sigma)`), normalised so that
/// `w[0] + 2·Σ w[1..] = 1`.
pub fn gaussian_half_kernel(sigma: f64) -> Vec<f64> {
    let r = kernel_radius(sigma);
    if r == 0 {
        return vec![1.0];
    }
    let mut w: Vec<f64> = (0..=r)
        .map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum = w[0] + 2.0 * w[1..].iter().sum::<f64>();
    w.iter_mut().for_each(|x| *x /= sum);
    w
}

/// Full symmetric kernel of `2r + 1` taps, normalised to sum 1.
pub fn gaussian_weights(sigma: f64) -> Vec<f64> {
    let h = gaussian_half_kernel(sigma);
    let mut full: Vec<f64> = h.iter().skip(1).rev().copied().collect();
    full.push(h[0]);
    full.extend(h.iter().skip(1));
    full
}

/// Error function (Abramowitz–Stegun 7.1.26, |error| < 1.5e-7).
pub fn erf(x: f64) -> f64 {
    let s = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    s * y
}

/// Cumulative distribution of a zero-mean Gaussian.
pub fn gaussian_cdf(x: f64, sigma: f64) -> f64 {
    if sigma <= 0.0 {
        return if x >= 0.0 { 1.0 } else { 0.0 };
    }
    0.5 * (1.0 + erf(x / (sigma * std::f64::consts::SQRT_2)))
}

/// Coverage at `p` of `rect` convolved with a 2D Gaussian: exact (separable) for rectangles.
pub fn blurred_rect(p: Point, rect: Rect, sigma: f64) -> f64 {
    let ax = gaussian_cdf(rect.max_x() - p.x, sigma) - gaussian_cdf(rect.min_x() - p.x, sigma);
    let ay = gaussian_cdf(rect.max_y() - p.y, sigma) - gaussian_cdf(rect.min_y() - p.y, sigma);
    ax * ay
}

/// Sizes (odd, in samples) of `passes` successive box blurs approximating a Gaussian of `sigma`
/// (the "ideal averaging filter" construction: widths `wl` and `wl + 2`; the number of narrow
/// passes is chosen to minimise the variance error, which is quantised for odd widths).
pub fn box_blur_sizes(sigma: f64, passes: usize) -> Vec<usize> {
    let n = passes.max(1) as f64;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let total = passes.max(1) as i64;
    let variance = |m: i64| -> f64 {
        let (a, b) = ((wl * wl - 1) as f64, (wu * wu - 1) as f64);
        (m as f64 * a + (total - m) as f64 * b) / 12.0
    };
    let target = sigma * sigma;
    let m = (0..=total)
        .min_by(|&a, &b| {
            (variance(a) - target)
                .abs()
                .total_cmp(&(variance(b) - target).abs())
        })
        .unwrap_or(0);
    (0..total)
        .map(|i| if i < m { wl as usize } else { wu as usize })
        .collect()
}

/// Variance of the composite of box blurs with the given widths: Σ (w² − 1) / 12.
pub fn box_blur_variance(sizes: &[usize]) -> f64 {
    sizes.iter().map(|&w| ((w * w) as f64 - 1.0) / 12.0).sum()
}

/// Effective kernel (length `Σ(w − 1) + 1`) of successive box blurs, by explicit convolution.
pub fn box_blur_kernel(sizes: &[usize]) -> Vec<f64> {
    let mut k = vec![1.0];
    for &w in sizes {
        let b = 1.0 / w as f64;
        let mut out = vec![0.0; k.len() + w - 1];
        for (i, &v) in k.iter().enumerate() {
            for j in 0..w {
                out[i + j] += v * b;
            }
        }
        k = out;
    }
    k
}

/// Box width `d` of the three-pass box blur recommended for Gaussian blurs in the SVG filter
/// specification: `d = floor(σ · 3·√(2π)/4 + 0.5)`.
pub fn svg_box_size(sigma: f64) -> usize {
    (sigma * 3.0 * (2.0 * std::f64::consts::PI).sqrt() / 4.0 + 0.5)
        .floor()
        .max(1.0) as usize
}

/// One-sided taps for a Gaussian blur using hardware bilinear filtering: adjacent sample pairs are
/// merged into one fetch at a fractional offset. Returns `(centre_weight, taps)` where each tap
/// `(offset, weight)` must be applied at `+offset` and `-offset`.
pub fn bilinear_taps(sigma: f64) -> (f64, Vec<(f64, f64)>) {
    let h = gaussian_half_kernel(sigma);
    let mut taps = Vec::new();
    let mut i = 1;
    while i < h.len() {
        if i + 1 < h.len() {
            let w = h[i] + h[i + 1];
            taps.push(((i as f64 * h[i] + (i + 1) as f64 * h[i + 1]) / w, w));
            i += 2;
        } else {
            taps.push((i as f64, h[i]));
            i += 1;
        }
    }
    (h[0], taps)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSpec {
    pub offset: Point,
    /// Gaussian standard deviation.
    pub sigma: f64,
    /// Growth (positive) or shrinkage of the caster shape before blurring.
    pub spread: f64,
    pub alpha: f64,
}

impl ShadowSpec {
    /// Conversion from the CSS-style blur radius, where `sigma = blur / 2`.
    pub fn from_css_blur(offset: Point, blur: f64, spread: f64, alpha: f64) -> Self {
        Self {
            offset,
            sigma: blur * 0.5,
            spread,
            alpha,
        }
    }

    /// Region touched by the shadow of a caster occupying `caster`.
    pub fn bounds(&self, caster: Rect) -> Rect {
        caster
            .translate(self.offset.x, self.offset.y)
            .outset(self.spread.max(0.0) + 3.0 * self.sigma)
    }

    /// Shadow opacity at `p` for a rectangular caster (closed form).
    pub fn rect_alpha(&self, p: Point, caster: Rect) -> f64 {
        let r = caster
            .translate(self.offset.x, self.offset.y)
            .outset(self.spread);
        self.alpha * blurred_rect(p, r, self.sigma)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_normalised_and_symmetric() {
        for s in [0.5, 1.0, 3.7, 20.0] {
            let w = gaussian_weights(s);
            assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12);
            assert_eq!(w.len(), 2 * kernel_radius(s) + 1);
            for i in 0..w.len() / 2 {
                assert!((w[i] - w[w.len() - 1 - i]).abs() < 1e-15);
            }
            let m = w.len() / 2;
            assert!(w[m] >= w[m + 1]);
        }
        assert_eq!(gaussian_weights(0.1), vec![1.0]);
    }

    #[test]
    fn variance_of_kernel_is_sigma_squared() {
        let s = 6.0;
        let w = gaussian_weights(s);
        let m = (w.len() / 2) as f64;
        let var: f64 = w
            .iter()
            .enumerate()
            .map(|(i, v)| (i as f64 - m).powi(2) * v)
            .sum();
        assert!(
            (var - s * s).abs() < 0.03 * s * s,
            "var {var} (3σ truncation loses about 2 %)"
        );
    }

    #[test]
    fn erf_reference_values() {
        assert!((erf(0.0)).abs() < 1e-7);
        assert!((erf(1.0) - 0.842_700_792_9).abs() < 2e-7);
        assert!((erf(-0.5) + 0.520_499_877_8).abs() < 2e-7);
        assert!((gaussian_cdf(0.0, 3.0) - 0.5).abs() < 1e-7);
    }

    #[test]
    fn blurred_rect_limits() {
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!((blurred_rect(Point::new(50.0, 50.0), r, 4.0) - 1.0).abs() < 1e-6);
        assert!(blurred_rect(Point::new(-40.0, 50.0), r, 4.0) < 1e-6);
        assert!((blurred_rect(Point::new(0.0, 50.0), r, 4.0) - 0.5).abs() < 1e-6);
        assert!((blurred_rect(Point::new(0.0, 0.0), r, 4.0) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn blurred_rect_matches_discrete_convolution() {
        let (sigma, n) = (5.0, 200usize);
        let w = gaussian_weights(sigma);
        let r = kernel_radius(sigma) as i64;
        let img: Vec<f64> = (0..n)
            .map(|i| if (60..140).contains(&i) { 1.0 } else { 0.0 })
            .collect();
        for x in [40usize, 58, 60, 62, 100, 139] {
            let mut acc = 0.0;
            for (k, wk) in w.iter().enumerate() {
                let i = x as i64 + k as i64 - r;
                if (0..n as i64).contains(&i) {
                    acc += wk * img[i as usize];
                }
            }
            let closed = gaussian_cdf(140.0 - (x as f64 + 0.5), sigma)
                - gaussian_cdf(60.0 - (x as f64 + 0.5), sigma);
            assert!((acc - closed).abs() < 5e-3, "x={x}: {acc} vs {closed}");
        }
    }

    #[test]
    fn box_sizes_are_odd_and_match_variance() {
        for s in [4.0, 5.0, 10.0, 25.0] {
            let sizes = box_blur_sizes(s, 3);
            assert_eq!(sizes.len(), 3);
            assert!(sizes.iter().all(|w| w % 2 == 1));
            let var = box_blur_variance(&sizes);
            assert!(
                (var - s * s).abs() / (s * s) < 0.10,
                "σ={s}: variance {var} vs {}",
                s * s
            );
        }
    }

    #[test]
    fn box_kernel_is_close_to_gaussian() {
        let s = 8.0;
        let k = box_blur_kernel(&box_blur_sizes(s, 3));
        assert!((k.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        let g = gaussian_weights(s);
        let (mk, mg) = (k.len() / 2, g.len() / 2);
        let mut worst: f64 = 0.0;
        for d in 0..=mg.min(mk) {
            worst = worst.max((k[mk + d] - g[mg + d]).abs());
        }
        assert!(worst < 0.1 * g[mg], "peak {}, worst {worst}", g[mg]);
    }

    #[test]
    fn svg_box_matches_formula() {
        assert_eq!(svg_box_size(2.0), 4);
        assert_eq!(svg_box_size(10.0), 19);
    }

    #[test]
    fn bilinear_taps_reproduce_kernel() {
        let sigma = 4.0;
        let h = gaussian_half_kernel(sigma);
        let (c, taps) = bilinear_taps(sigma);
        assert!(taps.len() < h.len() - 1);
        let total: f64 = c + 2.0 * taps.iter().map(|t| t.1).sum::<f64>();
        assert!((total - 1.0).abs() < 1e-12);
        let signal = |x: f64| (x * 0.31).sin() + 0.3 * (x * 0.07).cos();
        let linear = |x: f64| {
            let (i, f) = (x.floor(), x - x.floor());
            signal(i) * (1.0 - f) + signal(i + 1.0) * f
        };
        let x0 = 50.0;
        let direct: f64 = h[0] * signal(x0)
            + (1..h.len())
                .map(|i| h[i] * (signal(x0 + i as f64) + signal(x0 - i as f64)))
                .sum::<f64>();
        let merged: f64 = c * signal(x0)
            + taps
                .iter()
                .map(|&(o, w)| w * (linear(x0 + o) + linear(x0 - o)))
                .sum::<f64>();
        assert!((direct - merged).abs() < 1e-3, "{direct} vs {merged}");
    }

    #[test]
    fn shadow_spec_bounds_and_alpha() {
        let s = ShadowSpec::from_css_blur(Point::new(0.0, 10.0), 40.0, 0.0, 0.4);
        assert_eq!(s.sigma, 20.0);
        let caster = Rect::new(100.0, 100.0, 200.0, 200.0);
        let b = s.bounds(caster);
        assert_eq!(b, Rect::new(40.0, 50.0, 320.0, 320.0));
        let inside = s.rect_alpha(Point::new(200.0, 210.0), caster);
        assert!(inside > 0.39 && inside <= 0.4);
        assert!(s.rect_alpha(Point::new(b.min_x(), b.min_y()), caster) < 0.01);
    }
}
