//! Rounded rectangles with circular and continuous corners.

use std::f64::consts::FRAC_1_SQRT_2;

use cupertino_geometry::rounded::{coverage_from_distance, cubic_at, CIRCLE_KAPPA};
use cupertino_geometry::{
    CornerStyle, Path, PathOptions, PathSeg, Point, Radii, Rect, RoundedRect,
};

fn rect() -> Rect {
    Rect::new(0.0, 0.0, 200.0, 100.0)
}

#[test]
fn the_corner_exponent_grows_with_the_expansion_from_a_circle() {
    // (row, style, expansion, circular, exponent range)
    for (row, style, expansion, circular, (lo, hi)) in [
        ("circular", CornerStyle::Circular, 1.0, true, (2.0, 2.0)),
        (
            "continuous",
            CornerStyle::continuous(),
            1.528,
            false,
            (3.24, 3.28),
        ),
        (
            "expansion 1",
            CornerStyle::Continuous { expansion: 1.0 },
            1.0,
            true,
            (2.0, 2.0),
        ),
        (
            "expansion below 1",
            CornerStyle::Continuous { expansion: 0.5 },
            1.0,
            true,
            (2.0, 2.0),
        ),
    ] {
        assert_eq!(style.expansion(), expansion, "{row}");
        assert_eq!(style.is_circular(), circular, "{row}");
        let n = style.exponent();
        assert!(lo <= n && n <= hi, "{row}: n = {n}");
    }
    let a = CornerStyle::Continuous { expansion: 1.2 }.exponent();
    let b = CornerStyle::Continuous { expansion: 1.8 }.exponent();
    assert!(2.0 < a && a < b);
}

#[test]
fn extents_are_the_radius_times_the_expansion_fitted_to_the_short_edge() {
    for (row, rr, extent) in [
        ("circular", RoundedRect::circular(rect(), 12.0), 12.0),
        ("continuous", RoundedRect::continuous(rect(), 10.0), 15.28),
        (
            "too large",
            RoundedRect::circular(Rect::new(0.0, 0.0, 100.0, 20.0), 50.0),
            10.0,
        ),
    ] {
        for e in rr.extents() {
            assert!((e - extent).abs() < 1e-9, "{row}: {e}");
        }
    }
}

#[test]
fn the_circular_sdf_is_exact() {
    let rr = RoundedRect::circular(rect(), 20.0);
    // the corner circle centre is at (20, 20): the boundary is 20 away along the diagonal
    let diag = 20.0 - 20.0 * FRAC_1_SQRT_2;
    for (p, distance) in [
        (Point::new(100.0, 50.0), -50.0),
        (Point::new(100.0, 0.0), 0.0),
        (Point::new(100.0, -7.0), 7.0),
        (Point::new(207.0, 50.0), 7.0),
        (Point::new(diag, diag), 0.0),
        (Point::new(0.0, 0.0), 20.0 * 2f64.sqrt() - 20.0),
    ] {
        assert!((rr.sdf(p) - distance).abs() < 1e-9, "{p:?}");
    }
    let square = RoundedRect::with_radii(rect(), Radii::uniform(0.0), CornerStyle::continuous());
    assert!(
        (square.sdf(Point::new(-3.0, -4.0)) - 5.0).abs() < 1e-9,
        "square corner"
    );
}

#[test]
fn a_continuous_corner_cuts_more_than_a_circular_one_through_the_same_diagonal_point() {
    let circ = RoundedRect::circular(rect(), 20.0);
    let cont = RoundedRect::continuous(rect(), 20.0);
    for rr in [circ, cont] {
        assert!(rr.sdf(Point::new(100.0, 50.0)) < 0.0);
        assert!(rr.sdf(Point::new(-5.0, -5.0)) > 0.0);
        assert!(rr.contains(Point::new(100.0, 50.0)));
        assert!(!rr.contains(Point::new(-1.0, -1.0)));
    }
    let p = Point::new(2.0, 2.0);
    assert!(cont.sdf(p) > circ.sdf(p) - 1e-9);
    assert!(!cont.contains(p));
    let d = 20.0 - 20.0 * FRAC_1_SQRT_2;
    assert!(cont.sdf(Point::new(d, d)).abs() < 0.05);
}

#[test]
fn the_approximate_sdf_follows_the_reference() {
    let circ = RoundedRect::circular(rect(), 20.0);
    for p in [
        Point::new(1.0, 1.0),
        Point::new(-3.0, 4.0),
        Point::new(100.0, 50.0),
    ] {
        assert_eq!(circ.sdf(p), circ.sdf_approx(p), "circular at {p:?}");
    }
    let cont = RoundedRect::continuous(rect(), 20.0);
    for i in 0..20 {
        let t = f64::from(i) * 0.5;
        for p in [
            Point::new(t, t),
            Point::new(t, 2.0 * t),
            Point::new(3.0 * t, t),
        ] {
            let (a, b) = (cont.sdf(p), cont.sdf_approx(p));
            if a.abs() < 3.0 {
                assert!((a - b).abs() < 0.1, "p = {p:?}: {a} vs {b}");
            }
        }
    }
}

#[test]
fn per_corner_radii_round_only_the_named_corners() {
    for (row, radii, rounded) in [
        ("top", Radii::top(30.0), [true, true, false, false]),
        ("bottom", Radii::bottom(30.0), [false, false, true, true]),
        ("all", Radii::uniform(30.0), [true; 4]),
    ] {
        let rr = RoundedRect::with_radii(rect(), radii, CornerStyle::Circular);
        let corners = [
            Point::new(1.0, 1.0),
            Point::new(199.0, 1.0),
            Point::new(199.0, 99.0),
            Point::new(1.0, 99.0),
        ];
        for (p, cut) in corners.into_iter().zip(rounded) {
            assert_eq!(rr.contains(p), !cut, "{row} at {p:?}");
        }
    }
}

#[test]
fn coverage_ramps_across_one_pixel_centred_on_the_edge() {
    // at scale 1.5 a pixel is 1/1.5 points wide
    let pixel = 1.0 / 1.5;
    for (distance, size, coverage) in [
        (-10.0, 1.0, 1.0),
        (10.0, 1.0, 0.0),
        (0.0, 1.0, 0.5),
        (-0.25, 1.0, 0.75),
        (0.25, 1.0, 0.25),
        (0.0, pixel, 0.5),
        (pixel / 2.0, pixel, 0.0),
        (-pixel / 2.0, pixel, 1.0),
    ] {
        assert_eq!(
            coverage_from_distance(distance, size),
            coverage,
            "({distance}, {size})"
        );
    }
}

#[test]
fn rounded_rect_coverage_is_scale_independent() {
    let rr = RoundedRect::circular(rect(), 20.0);
    assert_eq!(rr.coverage(Point::new(100.0, 50.0), 1.0), 1.0);
    assert_eq!(rr.coverage(Point::new(100.0, 0.0), 1.0), 0.5);
    assert_eq!(rr.coverage(Point::new(100.0, -5.0), 1.0), 0.0);
    assert_eq!(rr.coverage_approx(Point::new(100.0, 0.0), 1.0), 0.5);
    let scale = 1.5;
    let small = RoundedRect::circular(Rect::new(0.0, 0.0, 40.0, 40.0), 8.0);
    let big = RoundedRect::circular(Rect::new(0.0, 0.0, 40.0 * scale, 40.0 * scale), 8.0 * scale);
    for p in [
        Point::new(1.0, 1.0),
        Point::new(3.0, 5.0),
        Point::new(20.0, 20.0),
    ] {
        let (a, b) = (small.coverage(p, 1.0 / scale), big.coverage(p * scale, 1.0));
        assert!((a - b).abs() < 1e-9, "{p:?}");
    }
}

#[test]
fn a_path_is_closed_with_its_cubics_per_corner_and_bounded_by_the_rect() {
    let cont = RoundedRect::continuous(rect(), 20.0);
    for (row, path, cubics) in [
        ("circular", RoundedRect::circular(rect(), 20.0).path(), 4),
        ("continuous", cont.path(), 16),
        (
            "two per corner",
            cont.path_with(&PathOptions {
                segments_per_corner: 2,
            }),
            8,
        ),
        ("square", RoundedRect::circular(rect(), 0.0).path(), 0),
    ] {
        assert!(path.is_closed(), "{row}");
        assert_eq!(path.cubic_count(), cubics, "{row}");
        let b = path.bounds();
        assert!(
            b.min_x().abs() < 1e-3 && b.min_y().abs() < 1e-3,
            "{row}: {b:?}"
        );
        assert!(
            (b.max_x() - 200.0).abs() < 1e-3 && (b.max_y() - 100.0).abs() < 1e-3,
            "{row}: {b:?}"
        );
    }
    let circ = RoundedRect::circular(rect(), 20.0).path();
    assert_eq!(circ.start(), Some(Point::new(20.0, 0.0)));
    assert_eq!(circ.end(), Some(Point::new(20.0, 0.0)));
    let empty = Path::default();
    assert_eq!(
        (empty.start(), empty.end(), empty.is_closed()),
        (None, None, false)
    );
}

#[test]
fn a_flattened_path_lies_on_the_boundary() {
    for (row, rr, tolerance, error) in [
        (
            "continuous",
            RoundedRect::continuous(rect(), 20.0),
            0.01,
            0.05,
        ),
        ("circular", RoundedRect::circular(rect(), 20.0), 0.001, 0.01),
    ] {
        let pts = rr.path().flatten(tolerance);
        assert!(pts.len() > 16, "{row}");
        for p in pts {
            assert!(rr.sdf(p).abs() < error, "{row}: {p:?} -> {}", rr.sdf(p));
        }
    }
}

#[test]
fn a_quarter_circle_cubic_uses_the_standard_handle_length() {
    let path = RoundedRect::circular(Rect::new(0.0, 0.0, 100.0, 100.0), 10.0).path();
    let first_handle = path
        .segments
        .iter()
        .find_map(|s| match s {
            PathSeg::CubicTo(c1, _, _) => Some(*c1),
            _ => None,
        })
        .unwrap();
    assert!((CIRCLE_KAPPA - 0.552_284_749_83).abs() < 1e-9);
    assert!((first_handle.x - (90.0 + 10.0 * CIRCLE_KAPPA)).abs() < 1e-9);
    assert!(first_handle.y.abs() < 1e-9);
}

#[test]
fn cubic_at_hits_the_end_points_and_the_midpoint() {
    let (p0, p1, p2, p3) = (
        Point::new(0.0, 0.0),
        Point::new(0.0, 10.0),
        Point::new(10.0, 10.0),
        Point::new(10.0, 0.0),
    );
    for (t, expected) in [(0.0, p0), (0.5, Point::new(5.0, 7.5)), (1.0, p3)] {
        assert_eq!(cubic_at(p0, p1, p2, p3, t), expected, "t = {t}");
    }
}
