use cupertino_geometry::{Point, Size};
use cupertino_ui_layout::{Axis, ItemMetrics, LayoutError, ScrollState, VirtualList};

fn state(offset: Point) -> ScrollState {
    ScrollState {
        offset,
        content: Size::new(500.0, 1000.0),
        viewport: Size::new(200.0, 300.0),
    }
}

#[test]
fn the_offset_stays_inside_the_content() {
    assert_eq!(
        state(Point::new(-5.0, 900.0)).clamped().offset,
        Point::new(0.0, 700.0)
    );
    assert_eq!(
        state(Point::new(f64::NAN, f64::INFINITY)).clamped().offset,
        Point::ZERO
    );
    let fits = ScrollState {
        content: Size::new(10.0, 10.0),
        ..state(Point::new(4.0, 4.0))
    };
    assert_eq!(fits.clamped().offset, Point::ZERO);
}

#[test]
fn scrolling_returns_what_did_not_fit_for_the_parent() {
    let (moved, rest) = state(Point::new(0.0, 650.0)).scrolled_by(Point::new(10.0, 100.0));
    assert_eq!(moved.offset, Point::new(10.0, 700.0));
    assert_eq!(rest, Point::new(0.0, 50.0));

    let (moved, rest) = state(Point::new(0.0, 20.0)).scrolled_by(Point::new(-30.0, -30.0));
    assert_eq!(moved.offset, Point::ZERO);
    assert_eq!(rest, Point::new(-30.0, -10.0));
}

#[test]
fn a_page_leaves_some_of_the_old_page_in_view() {
    let s = state(Point::ZERO);
    assert!(s.page_step(Axis::Vertical) < s.viewport.height && s.page_step(Axis::Vertical) > 0.0);
    assert!(s.page_step(Axis::Horizontal) < s.viewport.width);
}

#[test]
fn fixed_rows_are_found_by_arithmetic() {
    let mut list = VirtualList::new(50_000, ItemMetrics::Fixed(20.0)).unwrap();
    assert_eq!(list.total_height(), 1_000_000.0);
    assert_eq!(list.index_at(0.0), 0);
    assert_eq!(list.index_at(39.9), 1);
    assert_eq!(list.index_at(1e12), 49_999);
    let window = list.window(100_000.0, 400.0, 2.0);
    assert_eq!((window.first, window.last), (4960, 5061));
    assert_eq!(window.anchor_adjust, 0.0);
}

#[test]
fn a_fifty_thousand_row_list_creates_a_window_not_a_node_per_row() {
    for metrics in [ItemMetrics::Fixed(24.0), ItemMetrics::Estimated(24.0)] {
        let mut list = VirtualList::new(50_000, metrics).unwrap();
        let window = list.window(600_000.0, 800.0, 2.0);
        assert!(
            window.last - window.first <= 5 * 34 + 2,
            "{metrics:?}: {window:?}"
        );
        assert!(window.first <= 600_000 / 24 && window.last > 600_000 / 24);
    }
}

#[test]
fn measured_rows_change_where_later_rows_start() {
    let mut list = VirtualList::new(10, ItemMetrics::Estimated(10.0)).unwrap();
    assert_eq!(list.total_height(), 100.0);
    list.set_height(2, 50.0);
    assert_eq!(list.top(2), 20.0);
    assert_eq!(list.top(3), 70.0);
    assert_eq!(list.total_height(), 140.0);
    assert_eq!(list.index_at(69.0), 2);
    assert_eq!(list.index_at(70.0), 3);
    list.set_height(99, 5.0);
    list.set_height(0, f64::NAN);
    list.set_height(0, -1.0);
    assert_eq!(list.total_height(), 140.0);
}

#[test]
fn rows_found_by_offset_match_a_linear_scan() {
    let mut list = VirtualList::new(200, ItemMetrics::Estimated(17.0)).unwrap();
    for i in (0..200).step_by(3) {
        list.set_height(i, 5.0 + (i % 11) as f64 * 9.0);
    }
    let mut offset = 0.0;
    while offset < list.total_height() {
        let expected = (0..200).rev().find(|i| list.top(*i) <= offset).unwrap();
        assert_eq!(list.index_at(offset), expected, "offset {offset}");
        offset += 13.7;
    }
}

#[test]
fn measuring_rows_above_the_top_row_moves_the_offset_so_the_view_stays_put() {
    let mut list = VirtualList::new(100, ItemMetrics::Estimated(20.0)).unwrap();
    let before = list.window(1000.0, 200.0, 0.0);
    assert_eq!(before.first, 50);
    list.set_height(10, 60.0);
    let after = list.window(1000.0, 200.0, 0.0);
    assert_eq!(after.anchor_adjust, 40.0);
    assert_eq!(after.first, 50);
    list.set_height(80, 100.0);
    assert_eq!(list.window(1040.0, 200.0, 0.0).anchor_adjust, 0.0);
}

#[test]
fn an_empty_list_has_an_empty_window_and_bad_metrics_are_refused() {
    let mut list = VirtualList::new(0, ItemMetrics::Estimated(10.0)).unwrap();
    let window = list.window(50.0, 100.0, 2.0);
    assert_eq!((window.first, window.last), (0, 0));
    list.set_count(5);
    assert_eq!(list.total_height(), 50.0);
    list.set_count(2);
    assert_eq!(list.total_height(), 20.0);
    assert_eq!(
        VirtualList::new(1, ItemMetrics::Fixed(0.0)).unwrap_err(),
        LayoutError::InvalidConstraint
    );
    assert_eq!(
        VirtualList::new(1, ItemMetrics::Fixed(f64::NAN)).unwrap_err(),
        LayoutError::NonFiniteValue
    );
}

#[test]
fn rows_are_spaced_apart_but_the_list_ends_with_its_last_row() {
    for metrics in [ItemMetrics::Fixed(20.0), ItemMetrics::Estimated(20.0)] {
        let mut list = VirtualList::with_spacing(5, metrics, 4.0).unwrap();
        assert_eq!(list.top(0), 0.0);
        assert_eq!(list.top(3), 72.0);
        assert_eq!(list.total_height(), 5.0 * 20.0 + 4.0 * 4.0);
        assert_eq!(list.index_at(23.0), 0, "inside the gap below row 0");
        assert_eq!(list.index_at(24.0), 1);
        let window = list.window(30.0, 20.0, 0.0);
        assert_eq!((window.first, window.last), (1, 3));
    }
    let mut list = VirtualList::with_spacing(4, ItemMetrics::Estimated(10.0), 2.0).unwrap();
    list.set_height(1, 30.0);
    assert_eq!(list.top(2), 12.0 + 32.0);
    assert_eq!(list.total_height(), 10.0 + 30.0 + 10.0 + 10.0 + 6.0);
}

#[test]
fn spacing_must_be_finite_and_not_negative() {
    assert_eq!(
        VirtualList::with_spacing(1, ItemMetrics::Fixed(1.0), -1.0).unwrap_err(),
        LayoutError::InvalidConstraint
    );
    assert_eq!(
        VirtualList::with_spacing(1, ItemMetrics::Fixed(1.0), f64::NAN).unwrap_err(),
        LayoutError::NonFiniteValue
    );
}
