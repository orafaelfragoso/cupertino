//! Dithering keyed to the pixel position, and gradients.

use cupertino_raster_math::dither::{self, dither, matrix, offset, quantize, store, with_dither};
use cupertino_raster_math::gradient::{
    linear_t, pixel_centre, radial_t, Stop, Stops, StopsError, MAX_STOPS,
};

fn stop(offset: f32, color: [f32; 4]) -> Stop {
    Stop { offset, color }
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

#[test]
fn the_matrix_is_the_ordered_pattern_holding_every_entry_once_per_eight_pixels() {
    let mut seen = [false; 64];
    for y in 0..dither::SIZE {
        for x in 0..dither::SIZE {
            let m = matrix(x, y) as usize;
            assert!(m < 64 && !seen[m], "entry {m} at {x},{y}");
            seen[m] = true;
        }
    }
    for (x, y) in [(0, 0), (3, 5), (7, 7)] {
        assert_eq!(matrix(x + 8, y), matrix(x, y));
        assert_eq!(matrix(x, y + 16), matrix(x, y));
        assert_eq!(matrix(x + 800, y + 8), matrix(x, y));
    }
    // [[0, 2], [3, 1]] at the lowest bits, each higher bit level scaled by 4
    for ((x, y), m) in [((0, 0), 0), ((1, 0), 32), ((0, 1), 48), ((1, 1), 16)] {
        assert_eq!(matrix(x, y), m, "{x},{y}");
    }
}

#[test]
fn offsets_stay_inside_half_a_step_and_average_to_zero() {
    let mut sum = 0.0;
    for y in 0..8 {
        for x in 0..8 {
            let o = offset(x, y);
            assert!(o > -0.5 && o < 0.5, "{o}");
            sum += o;
        }
    }
    assert!(sum.abs() < 1e-4, "{sum}");
    assert_eq!(dither(0.5, 3, 4), 0.5 + offset(3, 4) / 255.0);
}

#[test]
fn storing_keeps_exact_8_bit_values_and_spreads_others_over_both_steps_in_proportion() {
    let pixels = || (0..8).flat_map(|y| (0..8).map(move |x| (x, y)));
    for code in [0u8, 1, 100, 128, 254, 255] {
        let v = f32::from(code) / 255.0;
        for (x, y) in pixels() {
            assert_eq!(store(v, x, y), code, "{code} at {x},{y}");
        }
    }
    // a quarter of the way from 100 to 101
    let v = 100.25 / 255.0;
    let ones = pixels().filter(|&(x, y)| store(v, x, y) == 101).count();
    assert_eq!(ones, 16, "a quarter of 64 pixels");
}

#[test]
fn quantize_rounds_to_nearest_and_holds_the_range() {
    for (v, expected) in [
        (0.0, 0),
        (1.0, 255),
        (-0.5, 0),
        (7.0, 255),
        (0.5 / 255.0 - 1e-5, 0),
        (0.5 / 255.0 + 1e-5, 1),
        (0.5, 128),
        (f32::NAN, 0),
    ] {
        assert_eq!(quantize(v), expected, "{v}");
    }
}

#[test]
fn with_dither_puts_the_implementation_at_the_marker_only_when_on() {
    let source = format!(
        "precision mediump float;\n{}\nvoid main() {{}}\n",
        dither::MARKER
    );
    assert_eq!(with_dither(&source, false), source);
    let out = with_dither(&source, true);
    assert!(!out.contains(dither::MARKER));
    assert!(out.contains("#define CUPERTINO_DITHER 1"));
    assert!(out.contains("float dither_offset(vec2 frag)"));
    assert!(out.starts_with("precision mediump float;\n"));
    assert!(out.ends_with("void main() {}\n"));
}

#[test]
fn stops_are_checked_once() {
    let a = stop(0.0, [0.0; 4]);
    let b = stop(1.0, [1.0; 4]);
    let many = vec![a; MAX_STOPS + 1];
    for (name, stops, expected) in [
        ("none", vec![], Err(StopsError::TooFew)),
        ("one", vec![a], Err(StopsError::TooFew)),
        ("two", vec![a, b], Ok(())),
        ("the most", many[..MAX_STOPS].to_vec(), Ok(())),
        ("too many", many.clone(), Err(StopsError::TooMany)),
        (
            "descending",
            vec![stop(0.6, [0.0; 4]), stop(0.4, [0.0; 4])],
            Err(StopsError::Unordered),
        ),
        (
            "below zero",
            vec![stop(-0.1, [0.0; 4]), b],
            Err(StopsError::Unordered),
        ),
        (
            "above one",
            vec![a, stop(1.5, [0.0; 4])],
            Err(StopsError::Unordered),
        ),
        (
            "a hard step",
            vec![a, stop(0.5, [0.0; 4]), stop(0.5, [1.0; 4]), b],
            Ok(()),
        ),
    ] {
        let made = Stops::new(&stops);
        assert_eq!(
            made.as_ref().map(|_| ()).map_err(|e| *e),
            expected,
            "{name}"
        );
        if let Ok(made) = made {
            assert_eq!(made.as_slice(), &stops[..], "{name}");
        }
    }
}

#[test]
fn colours_hold_at_the_ends_interpolate_straight_then_premultiply_and_step_hard() {
    let s = Stops::new(&[stop(0.2, RED), stop(0.8, BLUE)]).unwrap();
    for (t, expected) in [
        (-5.0, RED),
        (0.2, RED),
        (0.8, BLUE),
        (9.0, BLUE),
        (f32::NAN, RED),
    ] {
        assert_eq!(s.at(t), expected, "{t}");
    }

    // transparent white to opaque black: halfway is straight (0.5, 0.5, 0.5) at alpha 0.5, premultiplied by 0.5
    let s = Stops::new(&[
        stop(0.0, [1.0, 1.0, 1.0, 0.0]),
        stop(1.0, [0.0, 0.0, 0.0, 1.0]),
    ])
    .unwrap();
    let c = s.at(0.5);
    assert!(c[..3].iter().all(|v| (v - 0.25).abs() < 1e-6), "{c:?}");
    assert_eq!(c[3], 0.5);

    let s = Stops::new(&[
        stop(0.0, RED),
        stop(0.5, RED),
        stop(0.5, GREEN),
        stop(1.0, GREEN),
    ])
    .unwrap();
    assert_eq!(s.at(0.25), RED);
    assert_eq!(s.at(0.75), GREEN);
    assert!([RED, GREEN].contains(&s.at(0.5)));
}

#[test]
fn the_gradient_parameter_is_the_projection_or_the_distance_over_the_radius_at_the_pixel_centre() {
    let (start, end) = ([10.0, 0.0], [30.0, 0.0]);
    for (p, from, to, expected) in [
        ([10.0, 99.0], start, end, 0.0),
        ([20.0, -4.0], start, end, 0.5),
        ([30.0, 0.0], start, end, 1.0),
        ([50.0, 0.0], start, end, 2.0),
        ([0.0, 0.0], start, end, -0.5),
        ([1.0, 1.0], [0.0, 0.0], [2.0, 2.0], 0.5),
        // a degenerate line is at its start
        ([5.0, 5.0], [1.0, 1.0], [1.0, 1.0], 0.0),
    ] {
        assert!(
            (linear_t(p, from, to) - expected).abs() < 1e-6,
            "linear {p:?} on {from:?} to {to:?}"
        );
    }
    for (p, centre, radius, expected) in [
        ([3.0, 4.0], [0.0, 0.0], 10.0, 0.5),
        ([0.0, 0.0], [0.0, 0.0], 10.0, 0.0),
        ([13.0, 4.0], [10.0, 0.0], 5.0, 1.0),
        ([1.0, 1.0], [0.0, 0.0], 0.0, 1.0),
        ([1.0, 1.0], [0.0, 0.0], -3.0, 1.0),
    ] {
        assert_eq!(
            radial_t(p, centre, radius),
            expected,
            "radial {p:?} around {centre:?} by {radius}"
        );
    }
    assert_eq!(pixel_centre(0, 0), [0.5, 0.5]);
    assert_eq!(pixel_centre(9, 3), [9.5, 3.5]);
}
