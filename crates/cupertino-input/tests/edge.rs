//! Dwelling at a screen edge reveals the dock or menu bar, and leaving it hides them again.

use cupertino_input::{Edge, EdgeConfig, EdgeEvent, EdgeReveal, Point, Rect};

const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

fn dock_bottom() -> EdgeReveal {
    EdgeReveal::new(EdgeConfig::dock(Edge::Bottom))
}

fn shown_bar() -> EdgeReveal {
    let mut bar = EdgeReveal::new(EdgeConfig::menu_bar());
    bar.update(Point::new(5.0, 0.2), &[SCREEN], 0.0);
    assert_eq!(bar.tick(0.15), Some(EdgeEvent::Reveal { output: 0 }));
    bar
}

#[test]
fn dwelling_at_the_edge_reveals_after_the_delay_and_leaving_earlier_cancels() {
    let edge = Point::new(500.0, 1079.5);
    let mut e = dock_bottom();
    assert_eq!(e.update(edge, &[SCREEN], 0.0), None);
    assert!(!e.is_shown());
    assert_eq!(e.next_deadline(), Some(0.15));
    assert_eq!(e.tick(0.1), None);
    assert_eq!(e.tick(0.15), Some(EdgeEvent::Reveal { output: 0 }));
    assert!(e.is_shown());
    assert_eq!(
        e.next_deadline(),
        None,
        "nothing to hide while the pointer is there"
    );

    let mut moving = dock_bottom();
    moving.update(edge, &[SCREEN], 0.0);
    assert_eq!(
        moving.update(Point::new(510.0, 1079.5), &[SCREEN], 0.2),
        Some(EdgeEvent::Reveal { output: 0 }),
        "a move along the edge after the delay reveals"
    );

    let mut left = dock_bottom();
    left.update(edge, &[SCREEN], 0.0);
    assert_eq!(left.update(Point::new(500.0, 900.0), &[SCREEN], 0.05), None);
    assert_eq!(left.next_deadline(), None);
    assert_eq!(left.tick(1.0), None);
    assert!(!left.is_shown());
}

#[test]
fn every_edge_is_found_on_its_own_side() {
    for (edge, hit, miss) in [
        (Edge::Top, Point::new(900.0, 0.5), Point::new(900.0, 1079.0)),
        (
            Edge::Bottom,
            Point::new(900.0, 1079.5),
            Point::new(900.0, 0.5),
        ),
        (
            Edge::Left,
            Point::new(0.5, 500.0),
            Point::new(1919.0, 500.0),
        ),
        (
            Edge::Right,
            Point::new(1919.5, 500.0),
            Point::new(0.5, 500.0),
        ),
    ] {
        let mut e = EdgeReveal::new(EdgeConfig::dock(edge));
        e.update(miss, &[SCREEN], 0.0);
        assert_eq!(e.next_deadline(), None, "{edge:?} miss");
        e.update(hit, &[SCREEN], 0.0);
        assert_eq!(e.next_deadline(), Some(0.15), "{edge:?} hit");
    }
}

#[test]
fn the_dock_hides_at_once_and_the_menu_bar_after_its_delay_unless_held() {
    let mut dock = dock_bottom();
    dock.update(Point::new(5.0, 1079.5), &[SCREEN], 0.0);
    dock.tick(0.2);
    assert_eq!(
        dock.update(Point::new(5.0, 500.0), &[SCREEN], 1.0),
        Some(EdgeEvent::Hide { output: 0 })
    );

    let mut bar = shown_bar();
    assert_eq!(bar.update(Point::new(5.0, 400.0), &[SCREEN], 1.0), None);
    assert_eq!(bar.next_deadline(), Some(1.3));
    assert_eq!(bar.tick(1.2), None);
    assert_eq!(bar.tick(1.3), Some(EdgeEvent::Hide { output: 0 }));
    assert!(!bar.is_shown());

    let mut back = shown_bar();
    back.update(Point::new(5.0, 400.0), &[SCREEN], 1.0);
    assert_eq!(back.update(Point::new(5.0, 0.2), &[SCREEN], 1.1), None);
    assert_eq!(back.next_deadline(), None, "returning cancels the hide");
    assert_eq!(back.tick(5.0), None);
    assert!(back.is_shown());

    let mut held = shown_bar();
    held.set_hold_region(Some(Rect::new(0.0, 0.0, 300.0, 40.0)));
    assert_eq!(held.update(Point::new(100.0, 30.0), &[SCREEN], 2.0), None);
    assert_eq!(held.next_deadline(), None, "the hold region keeps it shown");
    assert!(held.is_shown());
    held.set_hold_region(None);
    held.update(Point::new(100.0, 30.0), &[SCREEN], 3.0);
    assert_eq!(held.tick(3.3), Some(EdgeEvent::Hide { output: 0 }));
}

#[test]
fn only_an_outer_edge_of_an_output_under_the_pointer_dwells() {
    let upper = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    let lower = Rect::new(0.0, 1080.0, 1920.0, 1080.0);
    for (name, outputs, point, deadline) in [
        (
            "an edge shared with another output",
            vec![upper, lower],
            Point::new(100.0, 1079.5),
            None,
        ),
        (
            "the outer edge below",
            vec![upper, lower],
            Point::new(100.0, 2159.5),
            Some(0.15),
        ),
        (
            "outside every output",
            vec![SCREEN],
            Point::new(5000.0, 1079.5),
            None,
        ),
        ("no outputs", vec![], Point::new(5.0, 1079.5), None),
    ] {
        let mut e = dock_bottom();
        e.update(point, &outputs, 0.0);
        assert_eq!(e.next_deadline(), deadline, "{name}");
    }
}

#[test]
fn moving_to_the_same_edge_of_another_output_restarts_the_dwell() {
    let left = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    let right = Rect::new(1000.0, 0.0, 1000.0, 1000.0);
    let mut e = dock_bottom();
    e.update(Point::new(100.0, 999.5), &[left, right], 0.0);
    e.update(Point::new(1100.0, 999.5), &[left, right], 0.1);
    assert_eq!(
        e.tick(0.15),
        None,
        "the clock restarted on the second output"
    );
    assert_eq!(e.tick(0.25), Some(EdgeEvent::Reveal { output: 1 }));
}
