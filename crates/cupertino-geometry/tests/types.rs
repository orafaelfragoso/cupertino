//! Points, sizes, insets and rectangles.

use cupertino_geometry::{Insets, Point, Rect, Size};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

#[test]
fn points_are_vectors() {
    let a = Point::new(1.0, 2.0);
    let b = Point::new(3.0, -4.0);
    assert_eq!(a + b, Point::new(4.0, -2.0));
    assert_eq!(a - b, Point::new(-2.0, 6.0));
    assert_eq!(a * 2.0, Point::new(2.0, 4.0));
    assert_eq!(-a, Point::new(-1.0, -2.0));
    assert!(close(Point::new(3.0, 4.0).length(), 5.0));
    assert!(close(
        Point::new(1.0, 1.0).distance(Point::new(4.0, 5.0)),
        5.0
    ));
    assert!(close(a.dot(Point::new(3.0, 4.0)), 11.0));
    let (from, to) = (Point::new(0.0, 10.0), Point::new(10.0, 20.0));
    for (t, expected) in [(0.0, from), (0.5, Point::new(5.0, 15.0)), (1.0, to)] {
        assert_eq!(from.lerp(to, t), expected, "lerp {t}");
    }
}

#[test]
fn sizes_and_insets_scale_and_sum() {
    for (size, empty) in [
        (Size::ZERO, true),
        (Size::new(0.0, 5.0), true),
        (Size::new(5.0, -1.0), true),
        (Size::new(0.5, 0.5), false),
    ] {
        assert_eq!(size.is_empty(), empty, "{size:?}");
    }
    assert_eq!(Size::new(10.0, 20.0).scale(1.5), Size::new(15.0, 30.0));
    let i = Insets::new(1.0, 2.0, 3.0, 4.0);
    assert_eq!((i.horizontal(), i.vertical()), (6.0, 4.0));
    assert_eq!(i.scale(2.0), Insets::new(2.0, 4.0, 6.0, 8.0));
    assert_eq!(Insets::uniform(5.0), Insets::new(5.0, 5.0, 5.0, 5.0));
    assert_eq!(Insets::symmetric(2.0, 3.0), Insets::new(3.0, 2.0, 3.0, 2.0));
}

#[test]
fn rect_constructors_and_accessors_agree() {
    let r = Rect::new(10.0, 20.0, 30.0, 40.0);
    assert_eq!(r, Rect::from_ltrb(10.0, 20.0, 40.0, 60.0));
    assert_eq!(
        r,
        Rect::from_origin_size(Point::new(10.0, 20.0), Size::new(30.0, 40.0))
    );
    assert_eq!(
        (r.x(), r.y(), r.width(), r.height()),
        (10.0, 20.0, 30.0, 40.0)
    );
    assert_eq!(
        (r.min_x(), r.min_y(), r.max_x(), r.max_y()),
        (10.0, 20.0, 40.0, 60.0)
    );
    assert_eq!(r.center(), Point::new(25.0, 40.0));
    assert_eq!(r.area(), 1200.0);
    let degenerate = Rect::new(0.0, 0.0, -5.0, 5.0);
    assert!(Rect::ZERO.is_empty() && degenerate.is_empty());
    assert_eq!(degenerate.area(), 0.0);
}

#[test]
fn containment_is_half_open() {
    let r = Rect::new(0.0, 0.0, 10.0, 10.0);
    for (p, inside) in [
        (Point::new(0.0, 0.0), true),
        (Point::new(9.999, 9.999), true),
        (Point::new(10.0, 5.0), false),
        (Point::new(5.0, 10.0), false),
        (Point::new(-0.001, 5.0), false),
    ] {
        assert_eq!(r.contains(p), inside, "{p:?}");
    }
}

#[test]
fn intersection_is_symmetric_and_touching_rects_do_not_intersect() {
    let a = Rect::new(0.0, 0.0, 10.0, 10.0);
    // (row, other, intersection)
    for (row, other, expected) in [
        ("touching right", Rect::new(10.0, 0.0, 10.0, 10.0), None),
        ("touching below", Rect::new(0.0, 10.0, 10.0, 10.0), None),
        (
            "overlapping",
            Rect::new(5.0, 5.0, 10.0, 10.0),
            Some(Rect::new(5.0, 5.0, 5.0, 5.0)),
        ),
        (
            "inside",
            Rect::new(2.0, 3.0, 5.0, 5.0),
            Some(Rect::new(2.0, 3.0, 5.0, 5.0)),
        ),
    ] {
        assert_eq!(a.intersection(&other), expected, "{row}");
        assert_eq!(other.intersection(&a), expected, "{row} reversed");
        assert_eq!(a.intersects(&other), expected.is_some(), "{row}");
        assert_eq!(other.intersects(&a), expected.is_some(), "{row} reversed");
    }
}

#[test]
fn union_covers_both_rects_and_ignores_an_empty_one() {
    let a = Rect::new(0.0, 0.0, 10.0, 10.0);
    let b = Rect::new(20.0, 5.0, 10.0, 10.0);
    assert_eq!(a.union(&b), Rect::from_ltrb(0.0, 0.0, 30.0, 15.0));
    let empty = Rect::new(-100.0, -100.0, 0.0, 0.0);
    assert_eq!(b.union(&empty), b);
    assert_eq!(empty.union(&b), b);
}

#[test]
fn moving_and_resizing_a_rect() {
    for (row, got, expected) in [
        (
            "translate",
            Rect::new(1.0, 2.0, 3.0, 4.0).translate(10.0, -2.0),
            Rect::new(11.0, 0.0, 3.0, 4.0),
        ),
        (
            "inset",
            Rect::new(0.0, 0.0, 100.0, 50.0).inset(Insets::new(5.0, 10.0, 15.0, 20.0)),
            Rect::new(10.0, 5.0, 70.0, 30.0),
        ),
        (
            "negative inset",
            Rect::new(10.0, 10.0, 20.0, 20.0).inset(Insets::uniform(-5.0)),
            Rect::new(5.0, 5.0, 30.0, 30.0),
        ),
        (
            "inset beyond the size",
            Rect::new(0.0, 0.0, 10.0, 10.0).inset(Insets::uniform(8.0)),
            Rect::new(8.0, 8.0, 0.0, 0.0),
        ),
        (
            "outset",
            Rect::new(10.0, 10.0, 20.0, 20.0).outset(2.0),
            Rect::new(8.0, 8.0, 24.0, 24.0),
        ),
        (
            "scale",
            Rect::new(10.0, 20.0, 30.0, 40.0).scale(1.5),
            Rect::new(15.0, 30.0, 45.0, 60.0),
        ),
    ] {
        assert_eq!(got, expected, "{row}");
    }
}

#[test]
fn round_out_covers_the_rect_with_whole_pixels() {
    for (row, r, expected) in [
        (
            "aligned",
            Rect::new(3.0, 4.0, 5.0, 6.0),
            Rect::new(3.0, 4.0, 5.0, 6.0),
        ),
        (
            "negative",
            Rect::new(-1.2, -2.5, 1.0, 1.0),
            Rect::from_ltrb(-2.0, -3.0, 0.0, -1.0),
        ),
        (
            "fractional scale",
            Rect::new(10.3, 20.7, 100.1, 50.5).scale(1.5),
            Rect::from_ltrb(
                (10.3f64 * 1.5).floor(),
                (20.7f64 * 1.5).floor(),
                ((10.3f64 + 100.1) * 1.5).ceil(),
                ((20.7f64 + 50.5) * 1.5).ceil(),
            ),
        ),
    ] {
        assert_eq!(r.round_out(), expected, "{row}");
    }
}
