//! A hand-built table with an unusable entry gives no curve instead of a panic or a NaN.

use cupertino_input::{AccelTable, TableCurve, TableEntry};

fn table() -> AccelTable {
    AccelTable {
        scale: 1.0,
        entries: vec![
            TableEntry {
                acceleration: 0.0,
                points: vec![(1.0, 1.0), (4.0, 4.0)],
            },
            TableEntry {
                acceleration: 1.0,
                points: vec![(1.0, 2.0), (4.0, 8.0)],
            },
        ],
    }
}

#[test]
fn an_entry_without_points_gives_no_curve() {
    let mut t = table();
    t.entries[0].points.clear();
    assert!(TableCurve::new(&t, 0.0).is_none());
    // A blend that touches the empty entry fails the same way.
    assert!(TableCurve::new(&t, 0.5).is_none());
}

#[test]
fn a_point_at_or_below_zero_gives_no_curve() {
    for x in [0.0, -1.0, f64::NAN] {
        let mut t = table();
        t.entries[1].points[0].0 = x;
        assert!(TableCurve::new(&t, 1.0).is_none(), "x = {x}");
        assert!(TableCurve::new(&t, 0.5).is_none(), "x = {x}");
    }
}

#[test]
fn an_unused_malformed_entry_does_not_block_a_curve() {
    let mut t = table();
    t.entries[1].points.clear();
    assert!(TableCurve::new(&t, 0.0).is_some());
}
