//! Drop and inner shadows.
//!
//! A shadow is the Gaussian blur (deviation σ) of its caster's silhouette, moved by its offset, tinted by its colour.
//! A drop shadow lies under the caster (source-over of the caster on the shadow); an inner shadow is the blur of the
//! inverted silhouette, kept inside the caster (multiplied by the caster's coverage) and laid over it.
//!
//! For a rounded rectangle the blur has a closed form, which every renderer uses: across x the blurred box is a
//! difference of two error functions at the corner-curved half width of that row; across y it is integrated
//! numerically over ±3σ in [`SHADOW_STEPS`] midpoint steps. The error function is the four-term rational
//! approximation [`erf`]. The shadow's alpha is that value times the shadow's density, held at one.

use crate::coverage::{coverage, rounded_distance};

/// Midpoint steps of the integral across y.
pub const SHADOW_STEPS: usize = 8;

/// The error function, by `1 − 1 / (1 + a₁x + a₂x² + a₃x³ + a₄x⁴)⁴` (odd, absolute error under 5·10⁻⁴).
pub fn erf(x: f32) -> f32 {
    let a = x.abs();
    let mut t = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    t *= t;
    x.signum() * (1.0 - 1.0 / (t * t))
}

/// The normal density of deviation `sigma` at `x`.
fn gauss(x: f32, sigma: f32) -> f32 {
    (-(x * x) / (2.0 * sigma * sigma)).exp() / (2.506_628_3 * sigma)
}

/// The blurred box across x at height `y`, its half width curved by a circular `corner`.
fn box_x(x: f32, y: f32, sigma: f32, corner: f32, half: [f32; 2]) -> f32 {
    let delta = (half[1] - corner - y.abs()).min(0.0);
    let curved = half[0] - corner + (corner * corner - delta * delta).max(0.0).sqrt();
    let k = std::f32::consts::FRAC_1_SQRT_2 / sigma;
    let low = 0.5 + 0.5 * erf((x - curved) * k);
    let high = 0.5 + 0.5 * erf((x + curved) * k);
    high - low
}

/// The Gaussian blur, deviation `sigma`, of a rounded box of half size `half` and circular `corner`, at `p` relative
/// to its centre: the blurred silhouette's coverage, 0 to 1.
pub fn rounded_box_blur(p: [f32; 2], half: [f32; 2], sigma: f32, corner: f32) -> f32 {
    let low = p[1] - half[1];
    let high = p[1] + half[1];
    let start = (-3.0 * sigma).clamp(low, high);
    let end = (3.0 * sigma).clamp(low, high);
    let step = (end - start) / SHADOW_STEPS as f32;
    let mut y = start + step * 0.5;
    let mut value = 0.0;
    for _ in 0..SHADOW_STEPS {
        value += box_x(p[0], p[1] - y, sigma, corner, half) * gauss(y, sigma) * step;
        y += step;
    }
    value
}

/// A rounded-rectangle caster.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caster {
    /// Half size.
    pub half: [f32; 2],
    /// Corner radius of the blurred box (circular).
    pub radius: f32,
    /// Corner extent and exponent of the caster's own outline (for the cut-out and the inner shadow).
    pub extent: f32,
    pub exponent: f32,
}

/// A shadow: deviation, offset (both in the units of the points) and density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub sigma: f32,
    pub offset: [f32; 2],
    pub density: f32,
}

/// Alpha of a drop shadow at `p` (relative to the caster's centre). With `cut_out` the caster's own area is left
/// clear (coverage of its outline at `px` pixels per unit), so a translucent caster does not show its shadow.
pub fn drop_shadow(p: [f32; 2], caster: &Caster, shadow: &Shadow, cut_out: bool, px: f32) -> f32 {
    let radius = caster.radius.min(caster.half[0].min(caster.half[1]));
    let q = [p[0] - shadow.offset[0], p[1] - shadow.offset[1]];
    let mut a =
        (rounded_box_blur(q, caster.half, shadow.sigma, radius) * shadow.density).clamp(0.0, 1.0);
    if cut_out {
        let extent = caster.extent.min(caster.half[0].min(caster.half[1]));
        let d = rounded_distance(p, caster.half, extent, caster.exponent);
        a *= (0.5 + d * px).clamp(0.0, 1.0);
    }
    a
}

/// The most [`rounded_box_blur`] reaches, deep inside a box: the integral's weights over the whole ±3σ (just under
/// one, since the Gaussian past 3σ is left out).
pub fn blur_mass(sigma: f32) -> f32 {
    let step = 6.0 * sigma / SHADOW_STEPS as f32;
    (0..SHADOW_STEPS)
        .map(|i| gauss(-3.0 * sigma + step * (i as f32 + 0.5), sigma) * step)
        .sum()
}

/// Alpha of an inner shadow at `p`: the blur of the inverted silhouette moved by the offset, inside the caster. The
/// blur is divided by [`blur_mass`] so the middle of a large caster is clear, not tinted by the integral's missing
/// tails.
pub fn inner_shadow(p: [f32; 2], caster: &Caster, shadow: &Shadow, px: f32) -> f32 {
    let radius = caster.radius.min(caster.half[0].min(caster.half[1]));
    let q = [p[0] - shadow.offset[0], p[1] - shadow.offset[1]];
    let blurred = rounded_box_blur(q, caster.half, shadow.sigma, radius) / blur_mass(shadow.sigma);
    let outside = (1.0 - blurred).max(0.0);
    let extent = caster.extent.min(caster.half[0].min(caster.half[1]));
    let inside = coverage(
        rounded_distance(p, caster.half, extent, caster.exponent),
        px,
    );
    (outside * shadow.density).clamp(0.0, 1.0) * inside
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASTER: Caster = Caster {
        half: [50.0, 30.0],
        radius: 10.0,
        extent: 10.0,
        exponent: 2.0,
    };
    const SOFT: Shadow = Shadow {
        sigma: 6.0,
        offset: [0.0, 4.0],
        density: 1.0,
    };

    #[test]
    fn the_error_function_is_close_and_odd() {
        for (x, want) in [
            (0.0f32, 0.0f32),
            (0.5, 0.520_499_9),
            (1.0, 0.842_700_8),
            (2.0, 0.995_322_3),
        ] {
            assert!((erf(x) - want).abs() < 5e-4, "erf({x}) = {}", erf(x));
            assert_eq!(erf(-x), -erf(x));
        }
    }

    #[test]
    fn a_drop_shadow_is_dense_under_the_middle_and_gone_far_away() {
        assert!(drop_shadow([0.0, 0.0], &CASTER, &SOFT, false, 1.0) > 0.99);
        assert!(drop_shadow([0.0, 30.0 + 4.0], &CASTER, &SOFT, false, 1.0) - 0.5 < 0.02);
        assert!(drop_shadow([0.0, 80.0], &CASTER, &SOFT, false, 1.0) < 1e-4);
        assert_eq!(drop_shadow([0.0, 0.0], &CASTER, &SOFT, true, 1.0), 0.0);
        // the integral leaves the tails past 3σ out: the middle of a drop shadow is that much under one
        let mass = blur_mass(SOFT.sigma);
        assert!(mass > 0.99 && mass < 1.0, "{mass}");
        assert!((rounded_box_blur([0.0, 0.0], [500.0, 500.0], 6.0, 0.0) - mass).abs() < 1e-5);
    }

    #[test]
    fn an_inner_shadow_lies_inside_along_the_edge_it_is_moved_from() {
        // moved down: it shows under the top edge, not over the bottom one, and never outside
        let top = inner_shadow([0.0, -29.0], &CASTER, &SOFT, 1.0);
        let bottom = inner_shadow([0.0, 29.0], &CASTER, &SOFT, 1.0);
        assert!(top > 0.6 && bottom < top / 3.0, "{top} {bottom}");
        assert_eq!(inner_shadow([0.0, -40.0], &CASTER, &SOFT, 1.0), 0.0);
        assert!(inner_shadow([0.0, 0.0], &CASTER, &SOFT, 1.0) < 1e-3);
    }
}
