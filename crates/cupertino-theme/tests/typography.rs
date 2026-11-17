//! Tracking, line height and the size classes of the type scale.

use cupertino_theme::typography::{
    FontMetrics, TrackingTable, DEFAULT_SUBSTITUTE_SIZE_SCALE, SUBSTITUTE_FAMILY,
};
use cupertino_theme::Typography;

#[test]
fn the_default_size_classes_are_thirteen_eleven_nine_and_ten() {
    let s = Typography::default().sizes;
    assert_eq!(
        (s.regular, s.small, s.mini, s.label),
        (13.0, 11.0, 9.0, 10.0)
    );
}

#[test]
fn tracking_at_a_table_size_is_its_value() {
    let t = TrackingTable::builtin();
    assert_eq!(t.units(12.0, 0.0), 0.0);
    assert_eq!(t.units(13.0, 0.0), -12.0);
    assert_eq!(t.units(17.0, 0.0), -52.0);
}

#[test]
fn tracking_between_table_sizes_is_linear() {
    let t = TrackingTable::builtin();
    assert!((t.units(12.5, 0.0) - -6.0).abs() < 1e-9);
    assert!((t.units(18.5, 0.0) - -49.0).abs() < 1e-9);
}

#[test]
fn tracking_outside_the_table_is_clamped() {
    let t = TrackingTable::builtin();
    assert_eq!(t.units(1.0, 0.0), t.units(6.0, 0.0));
    assert_eq!(t.units(500.0, 0.0), 0.0);
}

#[test]
fn the_builtin_table_has_only_track_zero_and_other_tracks_use_it() {
    let t = TrackingTable::builtin();
    assert_eq!(t.track_values(), vec![0.0]);
    assert_eq!(t.units(13.0, 1.0), t.units(13.0, 0.0));
    assert_eq!(t.units(13.0, -1.0), t.units(13.0, 0.0));
}

#[test]
fn a_loaded_table_interpolates_between_tracks() {
    let t = TrackingTable::from_tracks(
        1000.0,
        vec![
            (-1.0, vec![(10.0, 10.0), (20.0, 10.0)]),
            (1.0, vec![(10.0, -10.0), (20.0, -10.0)]),
        ],
    );
    assert_eq!(t.units(15.0, -1.0), 10.0);
    assert_eq!(t.units(15.0, 1.0), -10.0);
    assert!(t.units(15.0, 0.0).abs() < 1e-9);
    assert_eq!(t.units(15.0, 5.0), -10.0);
    assert_eq!(t.units(15.0, -5.0), 10.0);
}

#[test]
fn letter_spacing_is_units_over_the_em_times_the_size() {
    let t = TrackingTable::builtin();
    let expected = -12.0 / 2048.0 * 13.0;
    assert!((t.letter_spacing(13.0, 0.0) - expected).abs() < 1e-12);
    assert!((Typography::default().letter_spacing(13.0) - expected).abs() < 1e-12);
}

#[test]
fn the_line_height_is_ascender_minus_descender_plus_gap_in_ems() {
    let m = FontMetrics::BUILTIN;
    assert!((m.line_height_em() - (1980.0 + 432.0) / 2048.0).abs() < 1e-12);
    let t = Typography::default();
    assert!((t.line_height(13.0) - 13.0 * (1980.0 + 432.0) / 2048.0).abs() < 1e-9);
}

#[test]
fn the_substitute_family_is_drawn_a_little_smaller() {
    assert_eq!(DEFAULT_SUBSTITUTE_SIZE_SCALE, 0.96);
    let t = Typography::default();
    assert_eq!(t.family, SUBSTITUTE_FAMILY);
    assert!((t.substitute_size(13.0) - 13.0 * 0.96).abs() < 1e-12);
}
