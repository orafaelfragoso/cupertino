//! Distance and coverage of rectangles and rounded rectangles, strokes and clips.

use cupertino_raster_math::coverage::{
    clipped, coverage, rect_distance, rounded_distance, stroke_coverage, CIRCULAR_EXPONENT,
};
use cupertino_raster_math::placement::whole_pixel;

const HALF: [f32; 2] = [40.0, 20.0];

#[test]
fn rect_distance_is_signed_and_euclidean_outside_a_corner() {
    for (p, expected) in [
        ([0.0, 0.0], -20.0),
        ([40.0, 0.0], 0.0),
        ([45.0, 0.0], 5.0),
        ([0.0, -23.0], 3.0),
        ([43.0, 24.0], 5.0),
        ([-43.0, -24.0], 5.0),
    ] {
        assert_eq!(rect_distance(p, HALF), expected, "{p:?}");
    }
}

#[test]
fn rounded_distance_is_symmetric_finite_and_grows_away_from_the_shape() {
    for (e, n) in [(0.0, 2.0), (8.0, 2.0), (8.0, 3.26)] {
        let d = rounded_distance([31.0, 12.0], HALF, e, n);
        for p in [[-31.0, 12.0], [31.0, -12.0], [-31.0, -12.0]] {
            assert_eq!(
                rounded_distance(p, HALF, e, n),
                d,
                "extent {e} exponent {n}"
            );
        }
    }
    let mut last = f32::MIN;
    for i in 0..60 {
        // out along the diagonal of the top right corner, from inside to far outside
        let t = -7.0 + i as f32;
        let d = rounded_distance([28.0 + t, 8.0 + t], HALF, 12.0, 3.26);
        assert!(d.is_finite(), "at {t}");
        assert!(d >= last, "not monotonic at {t}: {d} after {last}");
        last = d;
    }
}

#[test]
fn a_circular_corner_cuts_the_corner_by_the_radius_up_to_the_threshold_exponent() {
    let (e, n) = (10.0, 2.0);
    // the corner circle's centre is (30, 10); the outline passes 10 from it
    let on_arc = [30.0 + 10.0 * 0.6, 10.0 + 10.0 * 0.8];
    assert!(rounded_distance(on_arc, HALF, e, n).abs() < 1e-5);
    // the sharp corner is outside the rounded outline by r (sqrt 2 - 1)
    let corner = rounded_distance([40.0, 20.0], HALF, e, n);
    assert!(
        (corner - 10.0 * (2.0f32.sqrt() - 1.0)).abs() < 1e-5,
        "{corner}"
    );
    // along the straight edges the radius changes nothing
    assert_eq!(rounded_distance([40.0, 0.0], HALF, e, n), 0.0);
    assert_eq!(rounded_distance([0.0, 20.0], HALF, e, n), 0.0);

    let p = [38.0, 18.0];
    let circular = rounded_distance(p, HALF, 10.0, 2.0);
    assert_eq!(rounded_distance(p, HALF, 10.0, CIRCULAR_EXPONENT), circular);
    assert_ne!(
        rounded_distance(p, HALF, 10.0, CIRCULAR_EXPONENT + 0.5),
        circular
    );
}

#[test]
fn a_continuous_corner_keeps_more_of_the_corner_and_meets_the_edges_without_a_jump() {
    let p = [38.0, 18.0];
    let circular = rounded_distance(p, HALF, 10.0, 2.0);
    let continuous = rounded_distance(p, HALF, 10.0, 3.26);
    assert!(continuous < circular, "{continuous} against {circular}");

    let (e, n) = (12.0, 3.26);
    for dy in [0.0, -1.0, -5.0] {
        let on_edge = rounded_distance([40.0, 20.0 - e + dy], HALF, e, n);
        assert!(on_edge.abs() < 1e-4, "{on_edge}");
    }
    let before = rounded_distance([40.0, 20.0 - e - 0.01], HALF, e, n);
    let after = rounded_distance([40.0, 20.0 - e + 0.01], HALF, e, n);
    assert!((before - after).abs() < 0.05, "{before} {after}");

    // a corner extent as large as the half height does not break the distance
    assert_eq!(rounded_distance([0.0, 0.0], HALF, 20.0, 2.0), -20.0);
    assert!(rounded_distance([40.0, 0.0], HALF, 20.0, 3.26).abs() < 1e-4);
}

#[test]
fn coverage_is_a_one_pixel_ramp_centred_on_the_outline() {
    for (d, px, expected) in [
        (0.0, 1.0, 0.5),
        (-0.5, 1.0, 1.0),
        (0.5, 1.0, 0.0),
        (-0.25, 1.0, 0.75),
        // at two pixels per unit the ramp is half a unit wide
        (-0.125, 2.0, 0.75),
        (-100.0, 1.0, 1.0),
        (100.0, 1.0, 0.0),
        // pixel centres on, inside and outside the edge of an aligned rect
        (rect_distance([40.0, 0.0], HALF), 1.0, 0.5),
        (rect_distance([39.5, 0.0], HALF), 1.0, 1.0),
        (rect_distance([40.5, 0.0], HALF), 1.0, 0.0),
    ] {
        assert_eq!(coverage(d, px), expected, "distance {d} at {px}");
    }
}

#[test]
fn a_stroke_covers_its_width_inside_the_outline_and_never_more_than_it() {
    for (d, width, expected) in [
        (-1.0, 2.0, 1.0),
        (0.0, 2.0, 0.5),
        (-2.0, 2.0, 0.5),
        (-3.0, 2.0, 0.0),
        (1.0, 2.0, 0.0),
        (-0.2, 0.0, 0.0),
        (-0.2, -1.0, 0.0),
    ] {
        assert_eq!(
            stroke_coverage(d, width, 1.0),
            expected,
            "{d} width {width}"
        );
    }
    // a stroke a quarter pixel wide never exceeds a quarter coverage
    let best = (-100..100)
        .map(|i| stroke_coverage(i as f32 * 0.01, 0.25, 1.0))
        .fold(0.0f32, f32::max);
    assert!((best - 0.25).abs() < 1e-5, "{best}");
}

#[test]
fn nested_clips_multiply() {
    for (clips, expected) in [
        (&[][..], 1.0),
        (&[0.5], 0.5),
        (&[0.5, 0.5, 1.0], 0.25),
        (&[0.9, 0.0], 0.0),
    ] {
        assert_eq!(clipped(clips), expected, "{clips:?}");
    }
}

#[test]
fn whole_pixel_rounds_a_half_up_and_commutes_with_whole_pixel_shifts() {
    for (v, expected) in [
        (2.5, 3.0),
        (-2.5, -2.0),
        (2.49, 2.0),
        (-2.51, -3.0),
        (0.0, 0.0),
    ] {
        assert_eq!(whole_pixel(v), expected, "{v}");
    }
    for v in [-3.5, -0.5, 0.5, 1.25, 7.5, 100.5] {
        for shift in [-8.0, -1.0, 3.0, 64.0] {
            assert_eq!(
                whole_pixel(v + shift),
                whole_pixel(v) + shift,
                "{v} {shift}"
            );
        }
    }
}
