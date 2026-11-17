//! Shadow kernels, blur approximations and blurred rectangles.

use cupertino_geometry::shadow::ShadowSpec;
use cupertino_geometry::shadow::{
    bilinear_taps, blurred_rect, box_blur_kernel, box_blur_sizes, box_blur_variance, erf,
    gaussian_cdf, gaussian_half_kernel, gaussian_weights, kernel_radius, svg_box_size,
};
use cupertino_geometry::{Point, Rect};

fn symmetric(k: &[f64]) -> bool {
    (0..k.len()).all(|i| (k[i] - k[k.len() - 1 - i]).abs() < 1e-12)
}

#[test]
fn kernel_radius_is_three_sigma_rounded_up() {
    for (sigma, radius) in [(0.0, 0), (0.29, 0), (1.0, 3), (2.1, 7)] {
        assert_eq!(kernel_radius(sigma), radius, "sigma {sigma}");
    }
}

#[test]
fn gaussian_kernels_are_normalised_symmetric_and_decreasing() {
    assert_eq!(
        gaussian_half_kernel(0.1),
        vec![1.0],
        "a tiny sigma is the identity"
    );
    assert_eq!(
        gaussian_weights(0.1),
        vec![1.0],
        "a tiny sigma is the identity"
    );

    let h = gaussian_half_kernel(2.0);
    assert_eq!(h.len(), kernel_radius(2.0) + 1);
    assert!((h[0] + 2.0 * h[1..].iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!(h.windows(2).all(|w| w[0] > w[1]));

    let w = gaussian_weights(1.5);
    assert_eq!(w.len(), 2 * kernel_radius(1.5) + 1);
    assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!(symmetric(&w));
    assert!(w[w.len() / 2] >= w[0]);
}

#[test]
fn erf_and_the_gaussian_cdf_match_known_values() {
    for (x, value) in [
        (0.0, 0.0),
        (1.0, 0.842_700_792_9),
        (-1.0, -0.842_700_792_9),
        (3.0, 0.999_977_909_5),
    ] {
        assert!((erf(x) - value).abs() < 1.5e-7, "erf({x})");
    }
    assert!((gaussian_cdf(0.0, 2.0) - 0.5).abs() < 1e-7);
    assert!((gaussian_cdf(1.3, 2.0) + gaussian_cdf(-1.3, 2.0) - 1.0).abs() < 1e-7);
    assert!((gaussian_cdf(2.0, 2.0) - 0.841_344_746).abs() < 1e-6);
    // without blur it is a step
    for (x, sigma, value) in [(-0.1, 0.0, 0.0), (0.0, 0.0, 1.0), (5.0, -1.0, 1.0)] {
        assert_eq!(gaussian_cdf(x, sigma), value, "cdf({x}, {sigma})");
    }
}

#[test]
fn a_blurred_rect_is_full_inside_half_on_an_edge_and_a_quarter_at_a_corner() {
    let r = Rect::new(0.0, 0.0, 100.0, 100.0);
    for (row, p, sigma, value) in [
        ("inside", Point::new(50.0, 50.0), 3.0, 1.0),
        ("far outside", Point::new(200.0, 50.0), 3.0, 0.0),
        ("edge", Point::new(0.0, 50.0), 4.0, 0.5),
        ("corner", Point::new(0.0, 0.0), 4.0, 0.25),
    ] {
        assert!((blurred_rect(p, r, sigma) - value).abs() < 1e-6, "{row}");
    }
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
    assert_eq!(box_blur_sizes(3.0, 0).len(), 1, "zero passes gives one");
    assert!((box_blur_variance(&[3]) - 8.0 / 12.0).abs() < 1e-12);
    assert_eq!(box_blur_variance(&[]), 0.0);
    // the SVG specification's box size
    assert_eq!((svg_box_size(0.0), svg_box_size(10.0)), (1, 19));
}

#[test]
fn a_box_blur_kernel_is_the_symmetric_unit_composite() {
    let k = box_blur_kernel(&[3, 5, 5]);
    assert_eq!(k.len(), 2 + 4 + 4 + 1);
    assert!((k.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!(symmetric(&k));
    assert_eq!(box_blur_kernel(&[]), vec![1.0]);
}

#[test]
fn bilinear_taps_merge_neighbouring_samples_and_keep_the_total_weight() {
    for sigma in [1.0, 2.0, 3.5] {
        let (centre, taps) = bilinear_taps(sigma);
        let total = centre + 2.0 * taps.iter().map(|(_, w)| w).sum::<f64>();
        assert!((total - 1.0).abs() < 1e-12, "sigma {sigma}");
        assert_eq!(
            taps.len(),
            kernel_radius(sigma).div_ceil(2),
            "sigma {sigma}"
        );
    }
    let (offset, _) = bilinear_taps(2.0).1[0];
    assert!(offset > 1.0 && offset < 2.0);
}

#[test]
fn shadow_bounds_cover_the_offset_caster_plus_spread_and_three_sigma() {
    let css = ShadowSpec::from_css_blur(Point::new(0.0, 10.0), 8.0, 2.0, 0.5);
    assert_eq!(css.sigma, 4.0, "a CSS blur is twice the sigma");
    assert_eq!(
        css.bounds(Rect::new(100.0, 100.0, 50.0, 50.0)),
        Rect::from_ltrb(86.0, 96.0, 164.0, 174.0)
    );
    let inward = ShadowSpec {
        offset: Point::ZERO,
        sigma: 1.0,
        spread: -5.0,
        alpha: 1.0,
    };
    assert_eq!(
        inward.bounds(Rect::new(0.0, 0.0, 10.0, 10.0)),
        Rect::new(-3.0, -3.0, 16.0, 16.0),
        "a negative spread does not shrink the bounds"
    );
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
