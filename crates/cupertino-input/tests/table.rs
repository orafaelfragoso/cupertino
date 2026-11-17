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

fn entries() -> Vec<TableEntry> {
    vec![
        TableEntry {
            acceleration: 0.0,
            points: vec![(1.0, 1.0), (4.0, 4.0)],
        },
        TableEntry {
            acceleration: 1.0,
            points: vec![(1.0, 2.0), (4.0, 8.0)],
        },
    ]
}

fn table() -> AccelTable {
    AccelTable {
        scale: 1.0,
        entries: entries(),
    }
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
fn a_table_is_parsed_from_its_fixed_point_form_and_a_malformed_one_refused() {
    let points = |e: &TableEntry| (e.acceleration, e.points.clone());
    let good: Vec<(f64, Vec<(f64, f64)>)> = entries().iter().map(points).collect();
    for (name, bytes, expected) in [
        ("@200", encode(b"@200", &good), Ok(table())),
        (
            "USB*",
            encode(b"USB*", &[]),
            Ok(AccelTable {
                scale: 1.0,
                entries: vec![],
            }),
        ),
        (
            "an unknown signature",
            encode(b"NOPE", &[]),
            Err(TableError::Signature(*b"NOPE")),
        ),
        (
            "an entry without points",
            encode(b"@200", &[(0.0, vec![])]),
            Err(TableError::Degenerate),
        ),
        (
            "a point at zero",
            encode(b"@200", &[(0.0, vec![(0.0, 1.0)])]),
            Err(TableError::Degenerate),
        ),
        (
            "a point below zero",
            encode(b"@200", &[(0.0, vec![(-1.0, 1.0)])]),
            Err(TableError::Degenerate),
        ),
    ] {
        assert_eq!(AccelTable::parse(&bytes), expected, "{name}");
    }
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
fn a_curve_is_the_entry_at_the_setting_scaled_past_the_last_one() {
    for (setting, points) in [
        (0.0, vec![(1.0, 1.0), (4.0, 4.0)]),
        (1.0, vec![(1.0, 2.0), (4.0, 8.0)]),
        // (3 + 1) / (1 + 1) = 2 times the last entry.
        (3.0, vec![(1.0, 4.0), (4.0, 16.0)]),
    ] {
        let curve = TableCurve::new(&table(), setting).unwrap();
        assert_eq!(curve.points(), &points[..], "{setting}");
    }
}

#[test]
fn a_setting_between_entries_blends_their_curves() {
    let curve = TableCurve::new(&table(), 0.5).unwrap();
    let lo = TableCurve::new(&table(), 0.0).unwrap();
    let hi = TableCurve::new(&table(), 1.0).unwrap();
    for (x, y) in curve.points() {
        let (lo, hi) = (lo.speed(*x), hi.speed(*x));
        assert!(
            *y >= lo - 1e-9 && *y <= hi + 1e-9,
            "({x}, {y}) outside [{lo}, {hi}]"
        );
    }
    assert!(curve.points().windows(2).all(|w| w[0].0 < w[1].0));
}

#[test]
fn an_unusable_setting_or_entry_gives_no_curve_instead_of_a_panic_or_a_nan() {
    let with = |change: fn(&mut AccelTable)| {
        let mut t = table();
        change(&mut t);
        t
    };
    let no_points_first = with(|t| t.entries[0].points.clear());
    let no_points_last = with(|t| t.entries[1].points.clear());
    let empty = AccelTable {
        scale: 1.0,
        entries: Vec::new(),
    };
    let mut rows = vec![
        ("a negative setting", table(), -0.5, false),
        ("an empty table", empty, 0.5, false),
        (
            "the entry has no points",
            no_points_first.clone(),
            0.0,
            false,
        ),
        (
            "a blend touches an empty entry",
            no_points_first,
            0.5,
            false,
        ),
        ("an unused malformed entry", no_points_last, 0.0, true),
    ];
    for x in [0.0, -1.0, f64::NAN] {
        let mut t = table();
        t.entries[1].points[0].0 = x;
        rows.push(("a point at or below zero", t.clone(), 1.0, false));
        rows.push(("a blend with a point at or below zero", t, 0.5, false));
    }
    for (name, table, setting, some) in rows {
        assert_eq!(TableCurve::new(&table, setting).is_some(), some, "{name}");
    }
}

#[test]
fn speed_interpolates_from_the_origin_between_points_and_past_the_last() {
    let base = TableCurve::new(&table(), 0.0).unwrap();
    for (x, speed) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (2.5, 2.5), (4.0, 4.0)] {
        assert_eq!(base.speed(x), speed, "{x}");
    }
    let steep = TableCurve::new(&table(), 1.0).unwrap();
    assert!(
        (steep.speed(6.0) - 12.0).abs() < 1e-9,
        "the last segment continues"
    );
}
