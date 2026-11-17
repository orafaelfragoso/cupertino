//! Dithering keyed to the pixel position, and gradients.

use cupertino_raster_math::dither::{self, dither, matrix, offset, quantize, store, with_dither};
use cupertino_raster_math::gradient::{
    linear_t, pixel_centre, radial_t, Stop, Stops, StopsError, MAX_STOPS,
};

fn stop(offset: f32, color: [f32; 4]) -> Stop {
    Stop { offset, color }
}

#[test]
fn the_matrix_holds_every_entry_once_per_period() {
    let mut seen = [false; 64];
    for y in 0..dither::SIZE {
        for x in 0..dither::SIZE {
            let m = matrix(x, y) as usize;
            assert!(m < 64);
            assert!(!seen[m], "entry {m} twice");
            seen[m] = true;
        }
    }
}

#[test]
fn the_matrix_repeats_every_eight_pixels() {
    for (x, y) in [(0, 0), (3, 5), (7, 7)] {
        assert_eq!(matrix(x + 8, y), matrix(x, y));
        assert_eq!(matrix(x, y + 16), matrix(x, y));
        assert_eq!(matrix(x + 800, y + 8), matrix(x, y));
    }
}

#[test]
fn the_matrix_starts_with_the_ordered_pair() {
    // [[0, 2], [3, 1]] at the lowest bits, each higher bit level scaled by 4
    assert_eq!(matrix(0, 0), 0);
    assert_eq!(matrix(1, 0), 32);
    assert_eq!(matrix(0, 1), 48);
    assert_eq!(matrix(1, 1), 16);
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
}

#[test]
fn values_8_bits_hold_exactly_are_stored_unchanged_everywhere() {
    for code in [0u8, 1, 100, 128, 254, 255] {
        let v = f32::from(code) / 255.0;
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(store(v, x, y), code, "{code} at {x},{y}");
            }
        }
    }
}

#[test]
fn a_value_between_two_steps_is_spread_over_both_in_proportion() {
    // a quarter of the way from 100 to 101
    let v = 100.25 / 255.0;
    let ones = (0..8)
        .flat_map(|y| (0..8).map(move |x| (x, y)))
        .filter(|&(x, y)| store(v, x, y) == 101)
        .count();
    assert_eq!(ones, 16, "a quarter of 64 pixels");
}

#[test]
fn the_same_pixel_always_gets_the_same_value() {
    for _ in 0..3 {
        assert_eq!(store(0.3333, 17, 42), store(0.3333, 17, 42));
    }
    assert_eq!(dither(0.5, 3, 4), 0.5 + offset(3, 4) / 255.0);
}

#[test]
fn quantize_rounds_to_nearest_and_holds_the_range() {
    assert_eq!(quantize(0.0), 0);
    assert_eq!(quantize(1.0), 255);
    assert_eq!(quantize(-0.5), 0);
    assert_eq!(quantize(7.0), 255);
    assert_eq!(quantize(0.5 / 255.0 - 1e-5), 0);
    assert_eq!(quantize(0.5 / 255.0 + 1e-5), 1);
    assert_eq!(quantize(0.5), 128);
    assert_eq!(quantize(f32::NAN), 0);
}

#[test]
fn with_dither_leaves_the_source_alone_when_off() {
    let source = "void main() {}\n//_DITHER_\n";
    assert_eq!(with_dither(source, false), source);
}

#[test]
fn with_dither_puts_the_implementation_at_the_marker() {
    let source = format!(
        "precision mediump float;\n{}\nvoid main() {{}}\n",
        dither::MARKER
    );
    let out = with_dither(&source, true);
    assert!(!out.contains(dither::MARKER));
    assert!(out.contains("#define CUPERTINO_DITHER 1"));
    assert!(out.contains("float dither_offset(vec2 frag)"));
    assert!(out.starts_with("precision mediump float;\n"));
    assert!(out.ends_with("void main() {}\n"));
}

#[test]
fn the_shader_implementation_is_the_same_matrix() {
    // the shader's pair is 2|a - b| + b, the Rust one 2 (a xor b) + b
    assert!(dither::GLSL.contains("2.0 * abs(a - b) + b"));
    assert!(dither::GLSL
        .contains("16.0 * dither_pair(p, 1.0) + 4.0 * dither_pair(p, 2.0) + dither_pair(p, 4.0)"));
}

#[test]
fn stops_are_checked_once() {
    let a = stop(0.0, [0.0; 4]);
    let b = stop(1.0, [1.0; 4]);
    assert_eq!(Stops::new(&[]), Err(StopsError::TooFew));
    assert_eq!(Stops::new(&[a]), Err(StopsError::TooFew));
    assert!(Stops::new(&[a, b]).is_ok());
    let nine = vec![a; MAX_STOPS + 1];
    assert_eq!(Stops::new(&nine), Err(StopsError::TooMany));
    assert!(Stops::new(&nine[..MAX_STOPS]).is_ok());
    assert_eq!(
        Stops::new(&[stop(0.6, [0.0; 4]), stop(0.4, [0.0; 4])]),
        Err(StopsError::Unordered)
    );
    assert_eq!(
        Stops::new(&[stop(-0.1, [0.0; 4]), b]),
        Err(StopsError::Unordered)
    );
    assert_eq!(
        Stops::new(&[a, stop(1.5, [0.0; 4])]),
        Err(StopsError::Unordered)
    );
    // equal offsets are a hard step, allowed
    assert!(Stops::new(&[a, stop(0.5, [0.0; 4]), stop(0.5, [1.0; 4]), b]).is_ok());
    assert_eq!(Stops::new(&[a, b]).unwrap().as_slice(), &[a, b]);
}

#[test]
fn the_end_colours_hold_outside_the_stops() {
    let s = Stops::new(&[
        stop(0.2, [1.0, 0.0, 0.0, 1.0]),
        stop(0.8, [0.0, 0.0, 1.0, 1.0]),
    ])
    .unwrap();
    assert_eq!(s.at(-5.0), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(s.at(0.2), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(s.at(0.8), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(s.at(9.0), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(s.at(f32::NAN), [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn colours_interpolate_straight_then_premultiply() {
    // transparent white to opaque black: halfway is straight (0.5, 0.5, 0.5) at alpha 0.5, premultiplied by 0.5
    let s = Stops::new(&[
        stop(0.0, [1.0, 1.0, 1.0, 0.0]),
        stop(1.0, [0.0, 0.0, 0.0, 1.0]),
    ])
    .unwrap();
    let c = s.at(0.5);
    for v in &c[..3] {
        assert!((v - 0.25).abs() < 1e-6, "{c:?}");
    }
    assert_eq!(c[3], 0.5);
}

#[test]
fn a_hard_step_picks_the_later_colour_at_the_step() {
    let s = Stops::new(&[
        stop(0.0, [1.0, 0.0, 0.0, 1.0]),
        stop(0.5, [1.0, 0.0, 0.0, 1.0]),
        stop(0.5, [0.0, 1.0, 0.0, 1.0]),
        stop(1.0, [0.0, 1.0, 0.0, 1.0]),
    ])
    .unwrap();
    assert_eq!(s.at(0.25), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(s.at(0.75), [0.0, 1.0, 0.0, 1.0]);
    let at_step = s.at(0.5);
    assert!(at_step == [1.0, 0.0, 0.0, 1.0] || at_step == [0.0, 1.0, 0.0, 1.0]);
}

#[test]
fn linear_t_projects_onto_the_line() {
    let (start, end) = ([10.0, 0.0], [30.0, 0.0]);
    assert_eq!(linear_t([10.0, 99.0], start, end), 0.0);
    assert_eq!(linear_t([20.0, -4.0], start, end), 0.5);
    assert_eq!(linear_t([30.0, 0.0], start, end), 1.0);
    assert_eq!(linear_t([50.0, 0.0], start, end), 2.0);
    assert_eq!(linear_t([0.0, 0.0], start, end), -0.5);
    // a diagonal line
    assert!((linear_t([1.0, 1.0], [0.0, 0.0], [2.0, 2.0]) - 0.5).abs() < 1e-6);
}

#[test]
fn a_degenerate_linear_gradient_is_at_its_start() {
    assert_eq!(linear_t([5.0, 5.0], [1.0, 1.0], [1.0, 1.0]), 0.0);
}

#[test]
fn radial_t_is_distance_over_radius() {
    assert_eq!(radial_t([3.0, 4.0], [0.0, 0.0], 10.0), 0.5);
    assert_eq!(radial_t([0.0, 0.0], [0.0, 0.0], 10.0), 0.0);
    assert_eq!(radial_t([13.0, 4.0], [10.0, 0.0], 5.0), 1.0);
    assert_eq!(radial_t([1.0, 1.0], [0.0, 0.0], 0.0), 1.0);
    assert_eq!(radial_t([1.0, 1.0], [0.0, 0.0], -3.0), 1.0);
}

#[test]
fn a_pixel_is_evaluated_at_its_centre() {
    assert_eq!(pixel_centre(0, 0), [0.5, 0.5]);
    assert_eq!(pixel_centre(9, 3), [9.5, 3.5]);
}
