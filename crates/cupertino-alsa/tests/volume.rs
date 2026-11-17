//! The mapping between raw values and a level from 0 to 1.

use cupertino_alsa::{DbCurve, Error, VolumeRange};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn minmax(min: i32, max: i32, mute: bool) -> DbCurve {
    DbCurve::MinMax { min, max, mute }
}

/// 0 to 100 raw over -20 dB to 0 dB: a range of at most 24 dB is linear in dB.
fn linear_db() -> VolumeRange {
    VolumeRange::new(0, 100, Some(minmax(-2000, 0, false))).unwrap()
}

/// 0 to 100 raw over -60 dB to 0 dB: wider than 24 dB, so logarithmic.
fn wide_db() -> VolumeRange {
    VolumeRange::new(0, 100, Some(minmax(-6000, 0, false))).unwrap()
}

#[test]
fn a_range_of_fewer_than_two_values_is_refused() {
    for (min, max) in [(5, 5), (9, 1)] {
        let result = VolumeRange::new(min, max, None);
        assert!(
            matches!(result, Err(Error::EmptyRange { min: a, max: b }) if (a, b) == (min, max)),
            "{min}..{max}"
        );
    }
    let range = VolumeRange::new(-3, 40, None).unwrap();
    assert_eq!((range.raw_min(), range.raw_max()), (-3, 40));
}

#[test]
fn the_level_of_a_raw_value_follows_the_gain_curve_and_is_clamped_to_zero_to_one() {
    let min_norm = 10f64.powf(-1.0);
    let wide_half = (10f64.powf(-0.5) - min_norm) / (1.0 - min_norm);
    for (name, range, raw, expected) in [
        (
            "no curve, low",
            VolumeRange::new(0, 40, None).unwrap(),
            0,
            0.0,
        ),
        (
            "no curve, linear",
            VolumeRange::new(0, 40, None).unwrap(),
            10,
            0.25,
        ),
        (
            "no curve, high",
            VolumeRange::new(0, 40, None).unwrap(),
            40,
            1.0,
        ),
        (
            "below the range",
            VolumeRange::new(10, 20, None).unwrap(),
            -100,
            0.0,
        ),
        (
            "above the range",
            VolumeRange::new(10, 20, None).unwrap(),
            100,
            1.0,
        ),
        ("narrow gain, low", linear_db(), 0, 0.0),
        ("narrow gain is linear in dB", linear_db(), 50, 0.5),
        ("narrow gain, high", linear_db(), 100, 1.0),
        // -30 dB is 10^(-0.5) of the loudest step, rescaled so that the lowest step is 0
        ("wide gain follows loudness", wide_db(), 50, wide_half),
        ("wide gain, low", wide_db(), 0, 0.0),
        ("wide gain, high", wide_db(), 100, 1.0),
        (
            "a muted lowest step is not rescaled",
            VolumeRange::new(0, 100, Some(minmax(-6000, 0, true))).unwrap(),
            50,
            10f64.powf(-0.5),
        ),
        (
            "a muted lowest step is zero",
            VolumeRange::new(0, 100, Some(minmax(-6000, 0, true))).unwrap(),
            0,
            0.0,
        ),
        (
            "a flat curve falls back to raw",
            VolumeRange::new(0, 10, Some(minmax(-500, -500, false))).unwrap(),
            5,
            0.5,
        ),
    ] {
        let level = range.level(raw);
        assert!(close(level, expected), "{name}: {level}");
    }
}

#[test]
fn a_level_maps_back_to_the_nearest_raw_value_within_the_range() {
    for range in [linear_db(), wide_db()] {
        for raw in 0..=100 {
            assert_eq!(range.raw(range.level(raw)), raw, "raw {raw}");
        }
    }
    for (name, range, level, expected) in [
        ("no curve", VolumeRange::new(0, 40, None).unwrap(), 0.25, 10),
        (
            "no curve, low",
            VolumeRange::new(0, 40, None).unwrap(),
            0.0,
            0,
        ),
        (
            "no curve, high",
            VolumeRange::new(0, 40, None).unwrap(),
            1.0,
            40,
        ),
        (
            "below zero",
            VolumeRange::new(10, 20, None).unwrap(),
            -4.0,
            10,
        ),
        (
            "above one",
            VolumeRange::new(10, 20, None).unwrap(),
            4.0,
            20,
        ),
        ("narrow gain", linear_db(), 0.5, 50),
        (
            "a flat curve",
            VolumeRange::new(0, 10, Some(minmax(-500, -500, false))).unwrap(),
            0.5,
            5,
        ),
    ] {
        assert_eq!(range.raw(level), expected, "{name}");
    }
}

#[test]
#[should_panic(expected = "a volume level is a number")]
fn a_level_that_is_not_a_number_is_a_programming_error() {
    let _ = VolumeRange::new(0, 10, None).unwrap().raw(f64::NAN);
}

#[test]
fn a_press_moves_by_the_step_and_at_least_one_raw_value_and_stops_at_the_ends() {
    let range = VolumeRange::new(0, 100, None).unwrap();
    for (name, raw, direction, step, expected) in [
        ("up", 50, 1, 0.1, 60),
        ("down", 50, -1, 0.1, 40),
        ("up at least one", 50, 1, 0.0, 51),
        ("down at least one", 50, -1, 0.0, 49),
        ("no direction", 50, 0, 0.5, 50),
        ("only the sign counts, up", 50, 7, 0.1, 60),
        ("only the sign counts, down", 50, -7, 0.1, 40),
        ("up to the top", 98, 1, 0.5, 100),
        ("at the top", 100, 1, 0.1, 100),
        ("down to the bottom", 2, -1, 0.5, 0),
        ("at the bottom", 0, -1, 0.1, 0),
        ("from above the range", 500, -1, 0.1, 90),
        ("from below the range", -500, 1, 0.1, 10),
    ] {
        assert_eq!(range.step(raw, direction, step), expected, "{name}");
    }
}
