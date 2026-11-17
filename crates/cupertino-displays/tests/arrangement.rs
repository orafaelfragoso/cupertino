//! A display arrangement is checked as a whole; displays that return, go away or leave the pointer behind are placed
//! by the same rules.

use cupertino_displays::arrangement::{
    check, free_place, reachable_point, relocate, Area, ArrangementError, COORDINATE_LIMIT,
};

const LEFT: Area = Area::new(0.0, 0.0, 1920.0, 1080.0);
const RIGHT: Area = Area::new(1920.0, 0.0, 2560.0, 1440.0);

#[test]
fn touching_displays_are_a_valid_arrangement() {
    assert_eq!(check(&[LEFT, RIGHT]), Ok(()));
    let stacked = Area::new(0.0, 1080.0, 1920.0, 1080.0);
    assert_eq!(check(&[LEFT, stacked]), Ok(()));
}

#[test]
fn an_arrangement_needs_a_display() {
    assert_eq!(check(&[]), Err(ArrangementError::Nothing));
}

#[test]
fn a_display_without_a_finite_area_is_empty() {
    for bad in [
        Area::new(0.0, 0.0, 0.0, 100.0),
        Area::new(0.0, 0.0, 100.0, -1.0),
        Area::new(f64::NAN, 0.0, 100.0, 100.0),
        Area::new(0.0, 0.0, f64::INFINITY, 100.0),
    ] {
        assert_eq!(check(&[LEFT, bad]), Err(ArrangementError::Empty(1)));
    }
}

#[test]
fn a_display_beyond_the_protocol_range_is_refused() {
    let far = Area::new(COORDINATE_LIMIT, 0.0, 100.0, 100.0);
    assert_eq!(check(&[far]), Err(ArrangementError::OutOfRange(0)));
    let negative = Area::new(-COORDINATE_LIMIT - 1.0, 0.0, 10.0, 10.0);
    assert_eq!(
        check(&[LEFT, negative]),
        Err(ArrangementError::OutOfRange(1))
    );
    let edge = Area::new(COORDINATE_LIMIT - 100.0, 0.0, 100.0, 100.0);
    assert_eq!(check(&[edge]), Ok(()));
}

#[test]
fn overlapping_displays_are_named_by_position() {
    let inside = Area::new(100.0, 100.0, 50.0, 50.0);
    assert_eq!(
        check(&[LEFT, RIGHT, inside]),
        Err(ArrangementError::Overlap(0, 2))
    );
    let partly = Area::new(1900.0, 0.0, 100.0, 100.0);
    assert_eq!(
        check(&[LEFT, partly, RIGHT]),
        Err(ArrangementError::Overlap(0, 1))
    );
}

#[test]
fn the_first_problem_found_is_the_one_reported() {
    let empty = Area::new(0.0, 0.0, 0.0, 0.0);
    assert_eq!(
        check(&[LEFT, LEFT, empty]),
        Err(ArrangementError::Empty(2)),
        "shape problems come before overlaps"
    );
}

#[test]
fn errors_describe_themselves() {
    assert_eq!(
        ArrangementError::Overlap(0, 2).to_string(),
        "displays 0 and 2 would overlap"
    );
    assert_eq!(
        ArrangementError::Nothing.to_string(),
        "at least one display must stay on"
    );
}

#[test]
fn a_free_place_is_kept() {
    assert_eq!(
        free_place(&[LEFT], Area::new(0.0, 1080.0, 800.0, 600.0)),
        (0.0, 1080.0)
    );
    assert_eq!(free_place(&[], LEFT), (0.0, 0.0));
}

#[test]
fn a_taken_place_goes_beside_the_others() {
    let taken = Area::new(100.0, 50.0, 800.0, 600.0);
    let others = [
        Area::new(0.0, 20.0, 1000.0, 700.0),
        Area::new(1000.0, -40.0, 500.0, 500.0),
    ];
    let (x, y) = free_place(&others, taken);
    assert_eq!((x, y), (1500.0, -40.0));
    let placed = Area::new(x, y, taken.w, taken.h);
    assert!(others.iter().all(|o| !o.overlaps(&placed)));
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
fn a_covered_point_stays() {
    assert_eq!(
        reachable_point(&[LEFT, RIGHT], (2000.0, 10.0)),
        Some((2000.0, 10.0))
    );
    assert_eq!(reachable_point(&[LEFT], (0.0, 0.0)), Some((0.0, 0.0)));
}

#[test]
fn an_uncovered_point_goes_to_the_nearest_display() {
    // Bottom and right edges are exclusive: the pointer stays one point inside them.
    assert_eq!(
        reachable_point(&[LEFT], (1920.0, 500.0)),
        Some((1919.0, 500.0))
    );
    assert_eq!(reachable_point(&[LEFT], (-50.0, -50.0)), Some((0.0, 0.0)));
    assert_eq!(
        reachable_point(&[LEFT], (100.0, 5000.0)),
        Some((100.0, 1079.0))
    );
    let apart = Area::new(5000.0, 0.0, 800.0, 600.0);
    assert_eq!(
        reachable_point(&[LEFT, apart], (4000.0, 300.0)),
        Some((5000.0, 300.0))
    );
    assert_eq!(
        reachable_point(&[LEFT, apart], (3000.0, 300.0)),
        Some((1919.0, 300.0))
    );
}

#[test]
fn without_displays_no_point_is_reachable() {
    assert_eq!(reachable_point(&[], (1.0, 1.0)), None);
}

#[test]
fn area_edges_and_containment_are_half_open() {
    assert_eq!(LEFT.right(), 1920.0);
    assert_eq!(LEFT.bottom(), 1080.0);
    assert!(LEFT.contains((0.0, 0.0)));
    assert!(!LEFT.contains((1920.0, 0.0)));
    assert!(!LEFT.contains((0.0, 1080.0)));
    assert!(!LEFT.overlaps(&RIGHT));
    assert!(LEFT.overlaps(&LEFT));
}
