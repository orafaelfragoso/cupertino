//! A display arrangement is checked as a whole; displays that return, go away or leave the pointer behind are placed
//! by the same rules.

use cupertino_displays::arrangement::{
    check, free_place, reachable_point, relocate, Area, ArrangementError, COORDINATE_LIMIT,
};

const LEFT: Area = Area::new(0.0, 0.0, 1920.0, 1080.0);
const RIGHT: Area = Area::new(1920.0, 0.0, 2560.0, 1440.0);

#[test]
fn an_arrangement_is_checked_as_a_whole_and_its_first_problem_is_named() {
    let empty = Area::new(0.0, 0.0, 0.0, 0.0);
    for (name, areas, expected) in [
        ("side by side", vec![LEFT, RIGHT], Ok(())),
        (
            "stacked",
            vec![LEFT, Area::new(0.0, 1080.0, 1920.0, 1080.0)],
            Ok(()),
        ),
        ("no display", vec![], Err(ArrangementError::Nothing)),
        (
            "zero width",
            vec![LEFT, Area::new(0.0, 0.0, 0.0, 100.0)],
            Err(ArrangementError::Empty(1)),
        ),
        (
            "negative height",
            vec![LEFT, Area::new(0.0, 0.0, 100.0, -1.0)],
            Err(ArrangementError::Empty(1)),
        ),
        (
            "not a number",
            vec![LEFT, Area::new(f64::NAN, 0.0, 100.0, 100.0)],
            Err(ArrangementError::Empty(1)),
        ),
        (
            "infinite",
            vec![LEFT, Area::new(0.0, 0.0, f64::INFINITY, 100.0)],
            Err(ArrangementError::Empty(1)),
        ),
        (
            "beyond the protocol range",
            vec![Area::new(COORDINATE_LIMIT, 0.0, 100.0, 100.0)],
            Err(ArrangementError::OutOfRange(0)),
        ),
        (
            "below the protocol range",
            vec![LEFT, Area::new(-COORDINATE_LIMIT - 1.0, 0.0, 10.0, 10.0)],
            Err(ArrangementError::OutOfRange(1)),
        ),
        (
            "at the range's edge",
            vec![Area::new(COORDINATE_LIMIT - 100.0, 0.0, 100.0, 100.0)],
            Ok(()),
        ),
        (
            "one inside another",
            vec![LEFT, RIGHT, Area::new(100.0, 100.0, 50.0, 50.0)],
            Err(ArrangementError::Overlap(0, 2)),
        ),
        (
            "partly over another",
            vec![LEFT, Area::new(1900.0, 0.0, 100.0, 100.0), RIGHT],
            Err(ArrangementError::Overlap(0, 1)),
        ),
        (
            "shape problems before overlaps",
            vec![LEFT, LEFT, empty],
            Err(ArrangementError::Empty(2)),
        ),
    ] {
        assert_eq!(check(&areas), expected, "{name}");
    }
}

#[test]
fn a_returning_display_keeps_a_free_place_or_goes_beside_the_others() {
    let others = [
        Area::new(0.0, 20.0, 1000.0, 700.0),
        Area::new(1000.0, -40.0, 500.0, 500.0),
    ];
    for (name, others, area, expected) in [
        (
            "free",
            &[LEFT][..],
            Area::new(0.0, 1080.0, 800.0, 600.0),
            (0.0, 1080.0),
        ),
        ("alone", &[][..], LEFT, (0.0, 0.0)),
        (
            "taken",
            &others[..],
            Area::new(100.0, 50.0, 800.0, 600.0),
            (1500.0, -40.0),
        ),
    ] {
        let (x, y) = free_place(others, area);
        assert_eq!((x, y), expected, "{name}");
        let placed = Area::new(x, y, area.w, area.h);
        assert!(others.iter().all(|o| !o.overlaps(&placed)), "{name}");
    }
}

#[test]
fn a_moved_window_keeps_its_offset_from_the_corner() {
    let from = Area::new(100.0, 200.0, 1000.0, 800.0);
    let to = Area::new(-1500.0, 0.0, 1500.0, 900.0);
    let frame = Area::new(150.0, 260.0, 300.0, 200.0);
    assert_eq!(
        relocate(frame, from, to),
        Area::new(-1450.0, 60.0, 300.0, 200.0)
    );
}

#[test]
fn a_pointer_left_outside_every_display_goes_to_the_nearest_one() {
    let apart = Area::new(5000.0, 0.0, 800.0, 600.0);
    for (name, areas, point, expected) in [
        (
            "covered",
            vec![LEFT, RIGHT],
            (2000.0, 10.0),
            Some((2000.0, 10.0)),
        ),
        ("on the origin", vec![LEFT], (0.0, 0.0), Some((0.0, 0.0))),
        // Bottom and right edges are exclusive: the pointer stays one point inside them.
        (
            "on the right edge",
            vec![LEFT],
            (1920.0, 500.0),
            Some((1919.0, 500.0)),
        ),
        (
            "above and left",
            vec![LEFT],
            (-50.0, -50.0),
            Some((0.0, 0.0)),
        ),
        ("below", vec![LEFT], (100.0, 5000.0), Some((100.0, 1079.0))),
        (
            "nearer the far display",
            vec![LEFT, apart],
            (4000.0, 300.0),
            Some((5000.0, 300.0)),
        ),
        (
            "nearer the first display",
            vec![LEFT, apart],
            (3000.0, 300.0),
            Some((1919.0, 300.0)),
        ),
        ("no display", vec![], (1.0, 1.0), None),
    ] {
        assert_eq!(reachable_point(&areas, point), expected, "{name}");
    }
}

#[test]
fn area_edges_and_containment_are_half_open() {
    assert_eq!((LEFT.right(), LEFT.bottom()), (1920.0, 1080.0));
    for (point, inside) in [
        ((0.0, 0.0), true),
        ((1920.0, 0.0), false),
        ((0.0, 1080.0), false),
    ] {
        assert_eq!(LEFT.contains(point), inside, "{point:?}");
    }
    assert!(!LEFT.overlaps(&RIGHT));
    assert!(LEFT.overlaps(&LEFT));
}
