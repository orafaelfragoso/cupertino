//! Points, sizes, insets and rectangles.

use cupertino_geometry::{Insets, Point, Rect, Size};

const EPS: f64 = 1e-12;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < EPS
}

#[test]
fn point_arithmetic_is_componentwise() {
    let a = Point::new(1.0, 2.0);
    let b = Point::new(3.0, -4.0);
    assert_eq!(a + b, Point::new(4.0, -2.0));
    assert_eq!(a - b, Point::new(-2.0, 6.0));
    assert_eq!(a * 2.0, Point::new(2.0, 4.0));
    assert_eq!(-a, Point::new(-1.0, -2.0));
    assert_eq!(Point::ZERO, Point::default());
}

#[test]
fn point_length_distance_and_dot() {
    assert!(close(Point::new(3.0, 4.0).length(), 5.0));
    assert!(close(
        Point::new(1.0, 1.0).distance(Point::new(4.0, 5.0)),
        5.0
    ));
    assert!(close(Point::new(1.0, 2.0).dot(Point::new(3.0, 4.0)), 11.0));
}

#[test]
fn point_lerp_hits_both_ends_and_the_middle() {
    let a = Point::new(0.0, 10.0);
    let b = Point::new(10.0, 20.0);
    assert_eq!(a.lerp(b, 0.0), a);
    assert_eq!(a.lerp(b, 1.0), b);
    assert_eq!(a.lerp(b, 0.5), Point::new(5.0, 15.0));
}

#[test]
fn point_converts_to_f32() {
    assert_eq!(Point::new(1.5, -2.25).to_f32(), [1.5f32, -2.25f32]);
}

#[test]
fn size_is_empty_when_a_side_is_not_positive() {
    assert!(Size::ZERO.is_empty());
    assert!(Size::new(0.0, 5.0).is_empty());
    assert!(Size::new(5.0, -1.0).is_empty());
    assert!(!Size::new(0.5, 0.5).is_empty());
}

#[test]
fn size_scales_both_sides() {
    assert_eq!(Size::new(10.0, 20.0).scale(1.5), Size::new(15.0, 30.0));
}

#[test]
fn insets_constructors_and_sums() {
    let i = Insets::new(1.0, 2.0, 3.0, 4.0);
    assert_eq!(i.horizontal(), 6.0);
    assert_eq!(i.vertical(), 4.0);
    assert_eq!(Insets::uniform(5.0), Insets::new(5.0, 5.0, 5.0, 5.0));
    assert_eq!(Insets::symmetric(2.0, 3.0), Insets::new(3.0, 2.0, 3.0, 2.0));
    assert_eq!(Insets::ZERO, Insets::default());
}

#[test]
fn insets_scale_every_side() {
    assert_eq!(
        Insets::new(1.0, 2.0, 3.0, 4.0).scale(2.0),
        Insets::new(2.0, 4.0, 6.0, 8.0)
    );
}

#[test]
fn rect_constructors_agree() {
    let a = Rect::new(10.0, 20.0, 30.0, 40.0);
    assert_eq!(a, Rect::from_ltrb(10.0, 20.0, 40.0, 60.0));
    assert_eq!(
        a,
        Rect::from_origin_size(Point::new(10.0, 20.0), Size::new(30.0, 40.0))
    );
}

#[test]
fn rect_accessors_report_edges_and_centre() {
    let r = Rect::new(10.0, 20.0, 30.0, 40.0);
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
}

#[test]
fn degenerate_rects_are_empty_and_have_no_area() {
    assert!(Rect::ZERO.is_empty());
    assert!(Rect::new(0.0, 0.0, -5.0, 5.0).is_empty());
    assert_eq!(Rect::new(0.0, 0.0, -5.0, 5.0).area(), 0.0);
}

#[test]
fn containment_is_half_open() {
    let r = Rect::new(0.0, 0.0, 10.0, 10.0);
    assert!(r.contains(Point::new(0.0, 0.0)));
    assert!(r.contains(Point::new(9.999, 9.999)));
    assert!(!r.contains(Point::new(10.0, 5.0)));
    assert!(!r.contains(Point::new(5.0, 10.0)));
    assert!(!r.contains(Point::new(-0.001, 5.0)));
}

#[test]
fn rects_that_only_touch_do_not_intersect() {
    let a = Rect::new(0.0, 0.0, 10.0, 10.0);
    let right = Rect::new(10.0, 0.0, 10.0, 10.0);
    let below = Rect::new(0.0, 10.0, 10.0, 10.0);
    assert!(!a.intersects(&right));
    assert!(!a.intersects(&below));
    assert_eq!(a.intersection(&right), None);
}

#[test]
fn overlapping_rects_intersect_symmetrically() {
    let a = Rect::new(0.0, 0.0, 10.0, 10.0);
    let b = Rect::new(5.0, 5.0, 10.0, 10.0);
    assert!(a.intersects(&b));
    assert!(b.intersects(&a));
    assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
    assert_eq!(b.intersection(&a), a.intersection(&b));
}

#[test]
fn a_rect_inside_another_intersects_as_itself() {
    let outer = Rect::new(0.0, 0.0, 100.0, 100.0);
    let inner = Rect::new(10.0, 10.0, 5.0, 5.0);
    assert_eq!(outer.intersection(&inner), Some(inner));
}

#[test]
fn union_covers_both_rects() {
    let a = Rect::new(0.0, 0.0, 10.0, 10.0);
    let b = Rect::new(20.0, 5.0, 10.0, 10.0);
    assert_eq!(a.union(&b), Rect::from_ltrb(0.0, 0.0, 30.0, 15.0));
}

#[test]
fn union_ignores_an_empty_rect() {
    let a = Rect::new(5.0, 5.0, 10.0, 10.0);
    let empty = Rect::new(-100.0, -100.0, 0.0, 0.0);
    assert_eq!(a.union(&empty), a);
    assert_eq!(empty.union(&a), a);
}

#[test]
fn translate_moves_the_origin_only() {
    assert_eq!(
        Rect::new(1.0, 2.0, 3.0, 4.0).translate(10.0, -2.0),
        Rect::new(11.0, 0.0, 3.0, 4.0)
    );
}

#[test]
fn inset_shrinks_from_each_edge() {
    let r = Rect::new(0.0, 0.0, 100.0, 50.0).inset(Insets::new(5.0, 10.0, 15.0, 20.0));
    assert_eq!(r, Rect::new(10.0, 5.0, 70.0, 30.0));
}

#[test]
fn negative_insets_grow_the_rect() {
    let r = Rect::new(10.0, 10.0, 20.0, 20.0).inset(Insets::uniform(-5.0));
    assert_eq!(r, Rect::new(5.0, 5.0, 30.0, 30.0));
}

#[test]
fn inset_clamps_the_size_at_zero() {
    let r = Rect::new(0.0, 0.0, 10.0, 10.0).inset(Insets::uniform(8.0));
    assert_eq!(r.size, Size::ZERO);
    assert!(r.is_empty());
}

#[test]
fn outset_grows_every_side() {
    assert_eq!(
        Rect::new(10.0, 10.0, 20.0, 20.0).outset(2.0),
        Rect::new(8.0, 8.0, 24.0, 24.0)
    );
}

#[test]
fn scale_multiplies_origin_and_size() {
    assert_eq!(
        Rect::new(10.0, 20.0, 30.0, 40.0).scale(1.5),
        Rect::new(15.0, 30.0, 45.0, 60.0)
    );
}

#[test]
fn round_out_at_a_fractional_scale_covers_the_scaled_rect() {
    let logical = Rect::new(10.3, 20.7, 100.1, 50.5);
    let device = logical.scale(1.5).round_out();
    assert_eq!(device.min_x(), (10.3f64 * 1.5).floor());
    assert_eq!(device.min_y(), (20.7f64 * 1.5).floor());
    assert_eq!(device.max_x(), ((10.3f64 + 100.1) * 1.5).ceil());
    assert_eq!(device.max_y(), ((20.7f64 + 50.5) * 1.5).ceil());
    assert_eq!(device.x().fract(), 0.0);
    assert_eq!(device.width().fract(), 0.0);
}

#[test]
fn round_out_of_an_aligned_rect_is_unchanged() {
    let r = Rect::new(3.0, 4.0, 5.0, 6.0);
    assert_eq!(r.round_out(), r);
}

#[test]
fn round_out_handles_negative_coordinates() {
    assert_eq!(
        Rect::new(-1.2, -2.5, 1.0, 1.0).round_out(),
        Rect::from_ltrb(-2.0, -3.0, 0.0, -1.0)
    );
}

#[test]
fn rect_converts_to_f32() {
    assert_eq!(
        Rect::new(1.0, 2.0, 3.5, 4.5).to_f32(),
        [1.0f32, 2.0, 3.5, 4.5]
    );
}
