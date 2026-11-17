//! Rounded rectangles with circular and continuous corners.

use cupertino_geometry::rounded::{coverage_from_distance, cubic_at, CIRCLE_KAPPA};
use cupertino_geometry::{
    CornerStyle, Path, PathOptions, PathSeg, Point, Radii, Rect, RoundedRect,
};

fn rect() -> Rect {
    Rect::new(0.0, 0.0, 200.0, 100.0)
}

#[test]
fn circular_style_has_unit_expansion_and_exponent_two() {
    let s = CornerStyle::Circular;
    assert_eq!(s.expansion(), 1.0);
    assert!(s.is_circular());
    assert_eq!(s.exponent(), 2.0);
}

#[test]
fn the_default_continuous_expansion_is_1_528() {
    assert_eq!(CornerStyle::DEFAULT_EXPANSION, 1.528);
    assert_eq!(CornerStyle::continuous().expansion(), 1.528);
    assert!(!CornerStyle::continuous().is_circular());
}

#[test]
fn the_default_continuous_exponent_is_about_3_26() {
    let n = CornerStyle::continuous().exponent();
    assert!((n - 3.26).abs() < 0.02, "n = {n}");
}

#[test]
fn expansion_one_or_less_degenerates_to_a_circle() {
    let s = CornerStyle::Continuous { expansion: 1.0 };
    assert!(s.is_circular());
    assert_eq!(s.exponent(), 2.0);
    let below = CornerStyle::Continuous { expansion: 0.5 };
    assert_eq!(below.expansion(), 1.0);
    assert!(below.is_circular());
}

#[test]
fn a_larger_expansion_gives_a_larger_exponent() {
    let a = CornerStyle::Continuous { expansion: 1.2 }.exponent();
    let b = CornerStyle::Continuous { expansion: 1.8 }.exponent();
    assert!(2.0 < a && a < b);
}

#[test]
fn radii_constructors_round_the_named_corners() {
    assert_eq!(Radii::uniform(4.0), Radii::new(4.0, 4.0, 4.0, 4.0));
    assert_eq!(Radii::top(4.0), Radii::new(4.0, 4.0, 0.0, 0.0));
    assert_eq!(Radii::bottom(4.0), Radii::new(0.0, 0.0, 4.0, 4.0));
}

#[test]
fn circular_extents_equal_the_radii() {
    let rr = RoundedRect::circular(rect(), 12.0);
    assert_eq!(rr.extents(), [12.0; 4]);
}

#[test]
fn continuous_extents_are_the_radius_times_the_expansion() {
    let rr = RoundedRect::continuous(rect(), 10.0);
    for e in rr.extents() {
        assert!((e - 15.28).abs() < 1e-9);
    }
}

#[test]
fn extents_shrink_together_so_corners_fit_the_short_edge() {
    let rr = RoundedRect::circular(Rect::new(0.0, 0.0, 100.0, 20.0), 50.0);
    let e = rr.extents();
    assert!(e.iter().all(|x| (x - 10.0).abs() < 1e-9));
}

#[test]
fn circular_sdf_is_exact_on_edges_inside_and_at_corners() {
    let rr = RoundedRect::circular(rect(), 20.0);
    assert!((rr.sdf(Point::new(100.0, 50.0)) + 50.0).abs() < 1e-9);
    assert!((rr.sdf(Point::new(100.0, 0.0))).abs() < 1e-9);
    assert!((rr.sdf(Point::new(100.0, -7.0)) - 7.0).abs() < 1e-9);
    assert!((rr.sdf(Point::new(207.0, 50.0)) - 7.0).abs() < 1e-9);
    // the corner circle centre is at (20, 20): the boundary is 20 away along the diagonal
    let diag = 20.0 - 20.0 * std::f64::consts::FRAC_1_SQRT_2;
    assert!(rr.sdf(Point::new(diag, diag)).abs() < 1e-9);
    assert!((rr.sdf(Point::new(0.0, 0.0)) - (20.0 * 2f64.sqrt() - 20.0)).abs() < 1e-9);
}

#[test]
fn sdf_is_negative_inside_and_positive_outside() {
    for rr in [
        RoundedRect::circular(rect(), 20.0),
        RoundedRect::continuous(rect(), 20.0),
    ] {
        assert!(rr.sdf(Point::new(100.0, 50.0)) < 0.0);
        assert!(rr.sdf(Point::new(-5.0, -5.0)) > 0.0);
        assert!(rr.contains(Point::new(100.0, 50.0)));
        assert!(!rr.contains(Point::new(-1.0, -1.0)));
    }
}

#[test]
fn a_continuous_corner_cuts_more_than_a_circular_one_of_the_same_radius() {
    let circ = RoundedRect::circular(rect(), 20.0);
    let cont = RoundedRect::continuous(rect(), 20.0);
    let p = Point::new(2.0, 2.0);
    assert!(cont.sdf(p) > circ.sdf(p) - 1e-9);
    assert!(!cont.contains(p));
}

#[test]
fn continuous_corner_passes_through_the_circular_diagonal_point() {
    let rr = RoundedRect::continuous(rect(), 20.0);
    let d = 20.0 - 20.0 * std::f64::consts::FRAC_1_SQRT_2;
    let on_circle = Point::new(d, d);
    assert!(rr.sdf(on_circle).abs() < 0.05, "{}", rr.sdf(on_circle));
}

#[test]
fn the_approximate_sdf_stays_close_to_the_reference_near_the_boundary() {
    let rr = RoundedRect::continuous(rect(), 20.0);
    for i in 0..20 {
        let t = f64::from(i) * 0.5;
        for p in [
            Point::new(t, t),
            Point::new(t, 2.0 * t),
            Point::new(3.0 * t, t),
        ] {
            let (a, b) = (rr.sdf(p), rr.sdf_approx(p));
            if a.abs() < 3.0 {
                assert!((a - b).abs() < 0.1, "p = {p:?}: {a} vs {b}");
            }
        }
    }
}

#[test]
fn the_approximate_sdf_equals_the_reference_for_circular_corners() {
    let rr = RoundedRect::circular(rect(), 20.0);
    for p in [
        Point::new(1.0, 1.0),
        Point::new(-3.0, 4.0),
        Point::new(100.0, 50.0),
    ] {
        assert_eq!(rr.sdf(p), rr.sdf_approx(p));
    }
}

#[test]
fn square_corners_have_a_euclidean_distance_outside() {
    let rr = RoundedRect::with_radii(rect(), Radii::uniform(0.0), CornerStyle::continuous());
    assert!((rr.sdf(Point::new(-3.0, -4.0)) - 5.0).abs() < 1e-9);
}

#[test]
fn per_corner_radii_round_only_the_named_corners() {
    let rr = RoundedRect::with_radii(rect(), Radii::top(30.0), CornerStyle::Circular);
    assert!(!rr.contains(Point::new(1.0, 1.0)));
    assert!(rr.contains(Point::new(1.0, 99.0)));
    assert!(rr.contains(Point::new(199.0, 99.0)));
}

#[test]
fn coverage_ramps_across_one_pixel_centred_on_the_edge() {
    assert_eq!(coverage_from_distance(-10.0, 1.0), 1.0);
    assert_eq!(coverage_from_distance(10.0, 1.0), 0.0);
    assert_eq!(coverage_from_distance(0.0, 1.0), 0.5);
    assert_eq!(coverage_from_distance(-0.25, 1.0), 0.75);
    assert_eq!(coverage_from_distance(0.25, 1.0), 0.25);
}

#[test]
fn coverage_pixel_size_follows_the_scale() {
    // at scale 1.5 a pixel is 1/1.5 points wide
    let pixel = 1.0 / 1.5;
    assert_eq!(coverage_from_distance(0.0, pixel), 0.5);
    assert_eq!(coverage_from_distance(pixel / 2.0, pixel), 0.0);
    assert_eq!(coverage_from_distance(-pixel / 2.0, pixel), 1.0);
}

#[test]
fn rounded_rect_coverage_is_full_inside_half_on_the_edge_and_zero_outside() {
    let rr = RoundedRect::circular(rect(), 20.0);
    assert_eq!(rr.coverage(Point::new(100.0, 50.0), 1.0), 1.0);
    assert_eq!(rr.coverage(Point::new(100.0, 0.0), 1.0), 0.5);
    assert_eq!(rr.coverage(Point::new(100.0, -5.0), 1.0), 0.0);
    assert_eq!(rr.coverage_approx(Point::new(100.0, 0.0), 1.0), 0.5);
}

#[test]
fn a_circular_path_has_one_cubic_per_corner_and_is_closed() {
    let path = RoundedRect::circular(rect(), 20.0).path();
    assert!(path.is_closed());
    assert_eq!(path.cubic_count(), 4);
    assert_eq!(path.start(), Some(Point::new(20.0, 0.0)));
}

#[test]
fn a_continuous_path_has_the_configured_segments_per_corner() {
    let rr = RoundedRect::continuous(rect(), 20.0);
    assert_eq!(rr.path().cubic_count(), 16);
    let two = rr.path_with(&PathOptions {
        segments_per_corner: 2,
    });
    assert_eq!(two.cubic_count(), 8);
}

#[test]
fn a_square_cornered_path_has_no_curves() {
    let path = RoundedRect::circular(rect(), 0.0).path();
    assert_eq!(path.cubic_count(), 0);
    assert!(path.is_closed());
    assert_eq!(path.bounds(), rect());
}

#[test]
fn path_bounds_equal_the_rectangle() {
    for rr in [
        RoundedRect::circular(rect(), 20.0),
        RoundedRect::continuous(rect(), 20.0),
    ] {
        let b = rr.path().bounds();
        assert!((b.min_x()).abs() < 1e-3 && (b.min_y()).abs() < 1e-3);
        assert!((b.max_x() - 200.0).abs() < 1e-3 && (b.max_y() - 100.0).abs() < 1e-3);
    }
}

#[test]
fn path_end_is_the_last_drawn_point_before_close() {
    let path = RoundedRect::circular(rect(), 20.0).path();
    assert_eq!(path.end(), Some(Point::new(20.0, 0.0)));
    assert_eq!(Path::default().end(), None);
    assert_eq!(Path::default().start(), None);
    assert!(!Path::default().is_closed());
}

#[test]
fn flattened_continuous_path_lies_on_the_reference_boundary() {
    let rr = RoundedRect::continuous(rect(), 20.0);
    let pts = rr.path().flatten(0.01);
    assert!(pts.len() > 16);
    for p in pts {
        assert!(rr.sdf(p).abs() < 0.05, "{p:?} -> {}", rr.sdf(p));
    }
}

#[test]
fn flattened_circular_path_lies_on_the_circle() {
    let rr = RoundedRect::circular(rect(), 20.0);
    for p in rr.path().flatten(0.001) {
        assert!(rr.sdf(p).abs() < 0.01, "{p:?}");
    }
}

#[test]
fn a_quarter_circle_cubic_uses_the_standard_handle_length() {
    assert!((CIRCLE_KAPPA - 0.552_284_749_83).abs() < 1e-9);
    let path = RoundedRect::circular(Rect::new(0.0, 0.0, 100.0, 100.0), 10.0).path();
    let first_cubic = path
        .segments
        .iter()
        .find_map(|s| match s {
            PathSeg::CubicTo(c1, _, _) => Some(*c1),
            _ => None,
        })
        .unwrap();
    assert!((first_cubic.x - (90.0 + 10.0 * CIRCLE_KAPPA)).abs() < 1e-9);
    assert!((first_cubic.y).abs() < 1e-9);
}

#[test]
fn cubic_at_hits_the_end_points_and_the_midpoint() {
    let (p0, p1, p2, p3) = (
        Point::new(0.0, 0.0),
        Point::new(0.0, 10.0),
        Point::new(10.0, 10.0),
        Point::new(10.0, 0.0),
    );
    assert_eq!(cubic_at(p0, p1, p2, p3, 0.0), p0);
    assert_eq!(cubic_at(p0, p1, p2, p3, 1.0), p3);
    assert_eq!(cubic_at(p0, p1, p2, p3, 0.5), Point::new(5.0, 7.5));
}

#[test]
fn fractional_scale_coverage_of_a_scaled_shape_matches_the_unscaled_one() {
    let scale = 1.5;
    let small = RoundedRect::circular(Rect::new(0.0, 0.0, 40.0, 40.0), 8.0);
    let big = RoundedRect::circular(Rect::new(0.0, 0.0, 40.0 * scale, 40.0 * scale), 8.0 * scale);
    for p in [
        Point::new(1.0, 1.0),
        Point::new(3.0, 5.0),
        Point::new(20.0, 20.0),
    ] {
        let a = small.coverage(p, 1.0 / scale);
        let b = big.coverage(p * scale, 1.0);
        assert!((a - b).abs() < 1e-9, "{p:?}");
    }
}
