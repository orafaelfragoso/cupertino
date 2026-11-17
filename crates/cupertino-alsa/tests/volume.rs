//! The mapping between raw values and a level from 0 to 1.

use cupertino_alsa::{DbCurve, Error, VolumeRange, DB_GAIN_MUTE};

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
fn a_range_of_one_value_is_refused() {
    assert!(matches!(
        VolumeRange::new(5, 5, None),
        Err(Error::EmptyRange { min: 5, max: 5 })
    ));
    assert!(matches!(
        VolumeRange::new(9, 1, None),
        Err(Error::EmptyRange { min: 9, max: 1 })
    ));
}

#[test]
fn the_ends_are_reported() {
    let range = VolumeRange::new(-3, 40, None).unwrap();
    assert_eq!((range.raw_min(), range.raw_max()), (-3, 40));
}

#[test]
fn without_gain_information_the_level_is_linear_in_the_raw_value() {
    let range = VolumeRange::new(0, 40, None).unwrap();
    assert!(close(range.level(0), 0.0));
    assert!(close(range.level(10), 0.25));
    assert!(close(range.level(40), 1.0));
    assert_eq!(range.raw(0.25), 10);
    assert_eq!(range.raw(0.0), 0);
    assert_eq!(range.raw(1.0), 40);
}

#[test]
fn a_raw_value_outside_the_range_is_clamped() {
    let range = VolumeRange::new(10, 20, None).unwrap();
    assert!(close(range.level(-100), 0.0));
    assert!(close(range.level(100), 1.0));
}

#[test]
fn a_level_outside_zero_to_one_is_clamped() {
    let range = VolumeRange::new(10, 20, None).unwrap();
    assert_eq!(range.raw(-4.0), 10);
    assert_eq!(range.raw(4.0), 20);
}

#[test]
#[should_panic(expected = "a volume level is a number")]
fn a_level_that_is_not_a_number_is_a_programming_error() {
    let _ = VolumeRange::new(0, 10, None).unwrap().raw(f64::NAN);
}

#[test]
fn a_narrow_gain_range_is_linear_in_decibels() {
    let range = linear_db();
    assert!(close(range.level(0), 0.0));
    assert!(close(range.level(50), 0.5));
    assert!(close(range.level(100), 1.0));
    assert_eq!(range.raw(0.5), 50);
}

#[test]
fn a_wide_gain_range_follows_perceived_loudness() {
    let range = wide_db();
    // -30 dB is 10^(-0.5) of the loudest step, rescaled so that the lowest step is 0
    let min_norm = 10f64.powf(-1.0);
    let expected = (10f64.powf(-0.5) - min_norm) / (1.0 - min_norm);
    assert!(close(range.level(50), expected));
    assert!(range.level(50) < 0.5);
    assert!(close(range.level(0), 0.0));
    assert!(close(range.level(100), 1.0));
}

#[test]
fn a_wide_range_whose_lowest_step_is_mute_starts_at_zero_without_rescaling() {
    let curve = minmax(-6000, 0, true);
    assert_eq!(curve.db(0, 100, 0), DB_GAIN_MUTE);
    let range = VolumeRange::new(0, 100, Some(curve)).unwrap();
    assert!(close(range.level(0), 0.0));
    assert!(close(range.level(50), 10f64.powf(-0.5)));
}

#[test]
fn a_level_maps_back_to_the_nearest_raw_value() {
    for range in [linear_db(), wide_db()] {
        for raw in 0..=100 {
            assert_eq!(range.raw(range.level(raw)), raw, "raw {raw}");
        }
    }
}

#[test]
fn a_flat_gain_curve_falls_back_to_the_raw_mapping() {
    let range = VolumeRange::new(0, 10, Some(minmax(-500, -500, false))).unwrap();
    assert!(close(range.level(5), 0.5));
    assert_eq!(range.raw(0.5), 5);
}

#[test]
fn a_press_moves_by_the_step_and_at_least_one_raw_value() {
    let range = VolumeRange::new(0, 100, None).unwrap();
    assert_eq!(range.step(50, 1, 0.1), 60);
    assert_eq!(range.step(50, -1, 0.1), 40);
    assert_eq!(range.step(50, 1, 0.0), 51);
    assert_eq!(range.step(50, -1, 0.0), 49);
    assert_eq!(range.step(50, 0, 0.5), 50);
}

#[test]
fn only_the_sign_of_the_direction_counts() {
    let range = VolumeRange::new(0, 100, None).unwrap();
    assert_eq!(range.step(50, 7, 0.1), 60);
    assert_eq!(range.step(50, -7, 0.1), 40);
}

#[test]
fn a_press_stops_at_the_ends() {
    let range = VolumeRange::new(0, 100, None).unwrap();
    assert_eq!(range.step(98, 1, 0.5), 100);
    assert_eq!(range.step(100, 1, 0.1), 100);
    assert_eq!(range.step(2, -1, 0.5), 0);
    assert_eq!(range.step(0, -1, 0.1), 0);
}

#[test]
fn a_press_from_outside_the_range_starts_at_its_nearest_end() {
    let range = VolumeRange::new(0, 100, None).unwrap();
    assert_eq!(range.step(500, -1, 0.1), 90);
    assert_eq!(range.step(-500, 1, 0.1), 10);
}
