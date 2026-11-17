//! Tracking, line height and substitute sizing of the type scale.

use cupertino_theme::typography::{FontMetrics, TrackingTable};
use cupertino_theme::Typography;

#[test]
fn the_built_in_tracking_interpolates_between_sizes_and_clamps_outside_them() {
    let t = TrackingTable::builtin();
    // (size, track, units)
    for (size, track, units) in [
        (12.0, 0.0, 0.0),
        (13.0, 0.0, -12.0),
        (17.0, 0.0, -52.0),
        (12.5, 0.0, -6.0),
        (18.5, 0.0, -49.0),
        (500.0, 0.0, 0.0),
        (1.0, 0.0, t.units(6.0, 0.0)),
        // the built-in table has only track 0; other tracks read it
        (13.0, 1.0, -12.0),
        (13.0, -1.0, -12.0),
    ] {
        assert!(
            (t.units(size, track) - units).abs() < 1e-9,
            "size {size} track {track}"
        );
    }
    assert_eq!(t.track_values(), vec![0.0]);
}

#[test]
fn a_loaded_table_interpolates_between_tracks_and_clamps_beyond_them() {
    let t = TrackingTable::from_tracks(
        1000.0,
        vec![
            (-1.0, vec![(10.0, 10.0), (20.0, 10.0)]),
            (1.0, vec![(10.0, -10.0), (20.0, -10.0)]),
        ],
    );
    for (track, units) in [
        (-1.0, 10.0),
        (1.0, -10.0),
        (0.0, 0.0),
        (5.0, -10.0),
        (-5.0, 10.0),
    ] {
        assert!((t.units(15.0, track) - units).abs() < 1e-9, "track {track}");
    }
}

#[test]
fn spacing_line_height_and_substitute_size_follow_the_metrics_and_the_point_size() {
    let t = Typography {
        tracking: TrackingTable::from_tracks(
            1000.0,
            vec![(0.0, vec![(10.0, 20.0), (20.0, -20.0)])],
        ),
        metrics: FontMetrics {
            units_per_em: 1000.0,
            ascender: 900.0,
            descender: -200.0,
            line_gap: 100.0,
            cap_height: 700.0,
            x_height: 500.0,
        },
        substitute_size_scale: 0.5,
        ..Typography::default()
    };
    for (label, got, expected) in [
        ("letter spacing at 10", t.letter_spacing(10.0), 0.2),
        ("letter spacing at 15", t.letter_spacing(15.0), 0.0),
        ("line height in ems", t.metrics.line_height_em(), 1.2),
        ("line height at 10", t.line_height(10.0), 12.0),
        ("substitute size at 10", t.substitute_size(10.0), 5.0),
    ] {
        assert!((got - expected).abs() < 1e-9, "{label}: {got}");
    }
}
