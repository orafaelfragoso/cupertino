//! Acceleration tables are parsed from their binary form and blended between settings into a speed curve.

use cupertino_input::{AccelTable, TableCurve, TableEntry, TableError, DEFAULT_TABLE};

fn fixed(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

/// A table with the given entries: (acceleration, points).
fn encode(signature: &[u8; 4], entries: &[(f64, Vec<(f64, f64)>)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(fixed(1.0));
    bytes.extend(signature);
    bytes.extend((entries.len() as u16).to_be_bytes());
    for (acceleration, points) in entries {
        bytes.extend(fixed(*acceleration));
        bytes.extend((points.len() as u16).to_be_bytes());
        for (x, y) in points {
            bytes.extend(fixed(*x));
            bytes.extend(fixed(*y));
        }
    }
    bytes
}

fn table() -> AccelTable {
    AccelTable::parse(&encode(
        b"@200",
        &[
            (0.0, vec![(1.0, 1.0), (4.0, 4.0)]),
            (1.0, vec![(1.0, 2.0), (4.0, 8.0)]),
        ],
    ))
    .unwrap()
}

#[test]
fn the_embedded_default_table_parses_into_ordered_entries() {
    let t = AccelTable::default_table();
    assert_eq!(t, AccelTable::parse(&DEFAULT_TABLE).unwrap());
    assert!(!t.entries.is_empty());
    assert!(t
        .entries
        .windows(2)
        .all(|w| w[0].acceleration < w[1].acceleration));
    for e in &t.entries {
        assert!(!e.points.is_empty());
        assert!(
            e.points.windows(2).all(|w| w[0].0 < w[1].0),
            "points run left to right"
        );
    }
}

#[test]
fn a_table_round_trips_its_fixed_point_fields() {
    let t = table();
    assert_eq!(t.scale, 1.0);
    assert_eq!(
        t.entries,
        vec![
            TableEntry {
                acceleration: 0.0,
                points: vec![(1.0, 1.0), (4.0, 4.0)]
            },
            TableEntry {
                acceleration: 1.0,
                points: vec![(1.0, 2.0), (4.0, 8.0)]
            },
        ]
    );
}

#[test]
fn both_known_signatures_are_accepted_and_others_are_named_in_the_error() {
    assert!(AccelTable::parse(&encode(b"USB*", &[])).is_ok());
    assert_eq!(
        AccelTable::parse(&encode(b"NOPE", &[])),
        Err(TableError::Signature(*b"NOPE"))
    );
}

#[test]
fn a_truncated_table_is_refused_at_every_length() {
    let bytes = encode(b"@200", &[(0.5, vec![(1.0, 1.0), (2.0, 3.0)])]);
    assert!(AccelTable::parse(&bytes).is_ok());
    for cut in 0..bytes.len() {
        assert_eq!(
            AccelTable::parse(&bytes[..cut]),
            Err(TableError::Truncated),
            "cut at {cut}"
        );
    }
}

#[test]
fn an_entry_without_points_or_with_a_non_positive_x_is_degenerate() {
    assert_eq!(
        AccelTable::parse(&encode(b"@200", &[(0.0, vec![])])),
        Err(TableError::Degenerate)
    );
    assert_eq!(
        AccelTable::parse(&encode(b"@200", &[(0.0, vec![(0.0, 1.0)])])),
        Err(TableError::Degenerate)
    );
    assert_eq!(
        AccelTable::parse(&encode(b"@200", &[(0.0, vec![(-1.0, 1.0)])])),
        Err(TableError::Degenerate)
    );
}

#[test]
fn a_negative_setting_or_an_empty_table_has_no_curve() {
    assert!(TableCurve::new(&table(), -0.5).is_none());
    let empty = AccelTable {
        scale: 1.0,
        entries: Vec::new(),
    };
    assert!(TableCurve::new(&empty, 0.5).is_none());
}

#[test]
fn an_exact_setting_uses_that_entry() {
    let curve = TableCurve::new(&table(), 1.0).unwrap();
    assert_eq!(curve.points(), &[(1.0, 2.0), (4.0, 8.0)]);
    let base = TableCurve::new(&table(), 0.0).unwrap();
    assert_eq!(base.points(), &[(1.0, 1.0), (4.0, 4.0)]);
}

#[test]
fn a_setting_beyond_the_last_entry_scales_that_entry() {
    let curve = TableCurve::new(&table(), 3.0).unwrap();
    // (3 + 1) / (1 + 1) = 2 times the last entry.
    assert_eq!(curve.points(), &[(1.0, 4.0), (4.0, 16.0)]);
}

#[test]
fn a_setting_between_entries_blends_their_curves() {
    let curve = TableCurve::new(&table(), 0.5).unwrap();
    for (x, y) in curve.points() {
        let lo = TableCurve::new(&table(), 0.0).unwrap().speed(*x);
        let hi = TableCurve::new(&table(), 1.0).unwrap().speed(*x);
        assert!(
            *y >= lo - 1e-9 && *y <= hi + 1e-9,
            "({x}, {y}) outside [{lo}, {hi}]"
        );
    }
    assert!(curve.points().windows(2).all(|w| w[0].0 < w[1].0));
}

#[test]
fn speed_interpolates_linearly_from_the_origin_and_between_points() {
    let curve = TableCurve::new(&table(), 0.0).unwrap();
    assert_eq!(curve.speed(0.0), 0.0);
    assert_eq!(curve.speed(0.5), 0.5, "from the origin to the first point");
    assert_eq!(curve.speed(1.0), 1.0);
    assert_eq!(curve.speed(2.5), 2.5);
    assert_eq!(curve.speed(4.0), 4.0);
}

#[test]
fn past_the_last_point_the_last_segment_continues() {
    let curve = TableCurve::new(&table(), 1.0).unwrap();
    assert!((curve.speed(6.0) - 12.0).abs() < 1e-9);
}
