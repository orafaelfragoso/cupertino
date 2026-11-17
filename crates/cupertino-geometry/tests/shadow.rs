//! Shadow kernels, blur approximations and blurred rectangles.

use cupertino_geometry::shadow::ShadowSpec;
use cupertino_geometry::shadow::{
    bilinear_taps, blurred_rect, box_blur_kernel, box_blur_sizes, box_blur_variance, erf,
    gaussian_cdf, gaussian_half_kernel, gaussian_weights, kernel_radius, svg_box_size,
};
use cupertino_geometry::{Point, Rect};

#[test]
fn kernel_radius_is_three_sigma_rounded_up() {
    assert_eq!(kernel_radius(0.0), 0);
    assert_eq!(kernel_radius(0.29), 0);
    assert_eq!(kernel_radius(1.0), 3);
    assert_eq!(kernel_radius(2.1), 7);
}

#[test]
fn a_tiny_sigma_gives_the_identity_kernel() {
    assert_eq!(gaussian_half_kernel(0.1), vec![1.0]);
    assert_eq!(gaussian_weights(0.1), vec![1.0]);
}

#[test]
fn half_kernel_is_normalised_and_decreasing() {
    let h = gaussian_half_kernel(2.0);
    assert_eq!(h.len(), kernel_radius(2.0) + 1);
    let sum = h[0] + 2.0 * h[1..].iter().sum::<f64>();
    assert!((sum - 1.0).abs() < 1e-12);
    assert!(h.windows(2).all(|w| w[0] > w[1]));
}

#[test]
fn full_kernel_is_symmetric_and_sums_to_one() {
    let w = gaussian_weights(1.5);
    assert_eq!(w.len(), 2 * kernel_radius(1.5) + 1);
    assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    for i in 0..w.len() {
        assert_eq!(w[i], w[w.len() - 1 - i]);
    }
    assert!(w[w.len() / 2] >= w[0]);
}

#[test]
fn erf_matches_known_values_within_its_stated_error() {
    assert!(erf(0.0).abs() < 1e-7);
    assert!((erf(1.0) - 0.842_700_792_9).abs() < 1.5e-7);
    assert!((erf(-1.0) + 0.842_700_792_9).abs() < 1.5e-7);
    assert!((erf(3.0) - 0.999_977_909_5).abs() < 1.5e-7);
}

#[test]
fn gaussian_cdf_is_half_at_zero_and_symmetric() {
    assert!((gaussian_cdf(0.0, 2.0) - 0.5).abs() < 1e-7);
    assert!((gaussian_cdf(1.3, 2.0) + gaussian_cdf(-1.3, 2.0) - 1.0).abs() < 1e-7);
    assert!((gaussian_cdf(2.0, 2.0) - 0.841_344_746).abs() < 1e-6);
}

#[test]
fn gaussian_cdf_without_blur_is_a_step() {
    assert_eq!(gaussian_cdf(-0.1, 0.0), 0.0);
    assert_eq!(gaussian_cdf(0.0, 0.0), 1.0);
    assert_eq!(gaussian_cdf(5.0, -1.0), 1.0);
}

#[test]
fn blurred_rect_is_full_deep_inside_and_empty_far_outside() {
    let r = Rect::new(0.0, 0.0, 100.0, 100.0);
    assert!((blurred_rect(Point::new(50.0, 50.0), r, 3.0) - 1.0).abs() < 1e-6);
    assert!(blurred_rect(Point::new(200.0, 50.0), r, 3.0) < 1e-6);
}

#[test]
fn blurred_rect_is_half_on_a_straight_edge() {
    let r = Rect::new(0.0, 0.0, 100.0, 100.0);
    assert!((blurred_rect(Point::new(0.0, 50.0), r, 4.0) - 0.5).abs() < 1e-6);
}

#[test]
fn blurred_rect_is_a_quarter_at_a_corner() {
    let r = Rect::new(0.0, 0.0, 100.0, 100.0);
    assert!((blurred_rect(Point::new(0.0, 0.0), r, 4.0) - 0.25).abs() < 1e-6);
}

#[test]
fn box_blur_sizes_are_odd_and_nearly_match_the_target_variance() {
    for sigma in [2.0, 5.0, 12.0] {
        let sizes = box_blur_sizes(sigma, 3);
        assert_eq!(sizes.len(), 3);
        assert!(sizes.iter().all(|w| w % 2 == 1));
        let v = box_blur_variance(&sizes);
        assert!(
            (v - sigma * sigma).abs() / (sigma * sigma) < 0.25,
            "sigma {sigma}: {v}"
        );
    }
}

#[test]
fn box_blur_sizes_with_zero_passes_gives_one_pass() {
    assert_eq!(box_blur_sizes(3.0, 0).len(), 1);
}

#[test]
fn box_blur_variance_of_one_width_is_the_uniform_variance() {
    assert!((box_blur_variance(&[3]) - 8.0 / 12.0).abs() < 1e-12);
    assert_eq!(box_blur_variance(&[]), 0.0);
}

#[test]
fn box_blur_kernel_has_the_composite_length_and_unit_sum() {
    let k = box_blur_kernel(&[3, 5, 5]);
    assert_eq!(k.len(), 2 + 4 + 4 + 1);
    assert!((k.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    for i in 0..k.len() {
        assert!((k[i] - k[k.len() - 1 - i]).abs() < 1e-12);
    }
}

#[test]
fn box_blur_kernel_of_nothing_is_the_identity() {
    assert_eq!(box_blur_kernel(&[]), vec![1.0]);
}

#[test]
fn svg_box_size_follows_the_specification_formula() {
    assert_eq!(svg_box_size(0.0), 1);
    assert_eq!(svg_box_size(10.0), 19);
}

#[test]
fn bilinear_taps_preserve_the_total_weight() {
    for sigma in [1.0, 2.0, 3.5] {
        let (centre, taps) = bilinear_taps(sigma);
        let total = centre + 2.0 * taps.iter().map(|(_, w)| w).sum::<f64>();
        assert!((total - 1.0).abs() < 1e-12, "sigma {sigma}");
    }
}

#[test]
fn bilinear_taps_merge_neighbouring_samples_between_their_offsets() {
    let (_, taps) = bilinear_taps(2.0);
    assert_eq!(taps.len(), kernel_radius(2.0).div_ceil(2));
    let (offset, _) = taps[0];
    assert!(offset > 1.0 && offset < 2.0);
}

#[test]
fn css_blur_is_twice_the_sigma() {
    let s = ShadowSpec::from_css_blur(Point::new(0.0, 4.0), 20.0, 2.0, 0.4);
    assert_eq!(s.sigma, 10.0);
    assert_eq!(s.offset, Point::new(0.0, 4.0));
    assert_eq!(s.spread, 2.0);
    assert_eq!(s.alpha, 0.4);
}

#[test]
fn shadow_bounds_cover_the_offset_caster_plus_spread_and_three_sigma() {
    let s = ShadowSpec::from_css_blur(Point::new(0.0, 10.0), 8.0, 2.0, 0.5);
    let b = s.bounds(Rect::new(100.0, 100.0, 50.0, 50.0));
    assert_eq!(
        b,
        Rect::from_ltrb(100.0 - 14.0, 110.0 - 14.0, 150.0 + 14.0, 160.0 + 14.0)
    );
}

#[test]
fn negative_spread_does_not_shrink_the_shadow_bounds() {
    let s = ShadowSpec {
        offset: Point::ZERO,
        sigma: 1.0,
        spread: -5.0,
        alpha: 1.0,
    };
    let b = s.bounds(Rect::new(0.0, 0.0, 10.0, 10.0));
    assert_eq!(b, Rect::new(-3.0, -3.0, 16.0, 16.0));
}

#[test]
fn rect_alpha_scales_with_the_shadow_alpha_and_follows_the_offset() {
    let s = ShadowSpec {
        offset: Point::new(0.0, 20.0),
        sigma: 2.0,
        spread: 0.0,
        alpha: 0.4,
    };
    let caster = Rect::new(0.0, 0.0, 100.0, 100.0);
    assert!((s.rect_alpha(Point::new(50.0, 70.0), caster) - 0.4).abs() < 1e-6);
    assert!((s.rect_alpha(Point::new(50.0, 20.0), caster) - 0.2).abs() < 1e-6);
    assert!(s.rect_alpha(Point::new(50.0, 10.0), caster) < 0.01);
}
