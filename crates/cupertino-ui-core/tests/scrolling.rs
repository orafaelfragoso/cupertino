//! Scroll arithmetic, the window onto a long list, a window's shortcuts, and the motion of dragged and flung content.

use cupertino_geometry::{Point, Size};
use cupertino_ui_core::kinetic::{
    AxisPolicy, Phase, ScrollMotion, EDGE_RESISTANCE, MIN_FLING_SPEED, MOMENTUM_TIME_CONSTANT,
    SETTLED,
};
use cupertino_ui_core::layout::{
    Axis, ItemMetrics, LayoutError, ScrollState, VirtualList, VirtualWindow,
};
use cupertino_ui_core::platform::{KeyEvent, Keysym, Mods};
use cupertino_ui_core::{CommandRegistry, Shortcut};

fn near(a: f64, b: f64, tolerance: f64, what: &str) {
    assert!(
        (a - b).abs() <= tolerance,
        "{what}: {a} is not within {tolerance} of {b}"
    );
}

#[test]
fn a_scroll_state_clamps_its_offset_and_hands_on_the_part_that_did_not_fit() {
    let state = |x, y| ScrollState {
        offset: Point::new(x, y),
        content: Size::new(300.0, 1000.0),
        viewport: Size::new(200.0, 400.0),
    };
    assert_eq!(state(0.0, 0.0).max_offset(), Point::new(100.0, 600.0));
    let fits = ScrollState {
        content: Size::new(100.0, 100.0),
        ..state(0.0, 0.0)
    };
    assert_eq!(
        fits.max_offset(),
        Point::ZERO,
        "content that fits does not scroll"
    );

    let rows = [
        (
            "past the end",
            state(500.0, 900.0).clamped().offset,
            Point::new(100.0, 600.0),
        ),
        (
            "before the start",
            state(-5.0, -1.0).clamped().offset,
            Point::ZERO,
        ),
        (
            "not a number",
            state(f64::NAN, f64::INFINITY).clamped().offset,
            Point::ZERO,
        ),
        (
            "within",
            state(40.0, 250.0).clamped().offset,
            Point::new(40.0, 250.0),
        ),
    ];
    for (what, got, expected) in rows {
        assert_eq!(got, expected, "{what}");
    }

    let (moved, left) = state(0.0, 550.0).scrolled_by(Point::new(30.0, 100.0));
    assert_eq!(
        (moved.offset, left),
        (Point::new(30.0, 600.0), Point::new(0.0, 50.0)),
        "the rest goes to the parent"
    );
    let (moved, left) = state(0.0, 100.0).scrolled_by(Point::new(-20.0, f64::NAN));
    assert_eq!(
        (moved.offset, left),
        (Point::new(0.0, 100.0), Point::new(-20.0, 0.0))
    );
    near(
        state(0.0, 0.0).page_step(Axis::Vertical),
        360.0,
        1e-9,
        "a page leaves the last tenth in view",
    );
}

#[test]
fn a_virtual_list_finds_the_rows_in_a_viewport_whether_their_heights_are_fixed_or_learned() {
    let mut fixed = VirtualList::with_spacing(100, ItemMetrics::Fixed(20.0), 4.0).unwrap();
    assert_eq!(
        fixed.total_height(),
        100.0 * 24.0 - 4.0,
        "no spacing after the last row"
    );
    assert_eq!(
        (fixed.top(0), fixed.top(3), fixed.top(500)),
        (0.0, 72.0, 2400.0)
    );
    let rows = [
        (0.0, 0),
        (23.9, 0),
        (24.0, 1),
        (100.0, 4),
        (1e9, 99),
        (-5.0, 0),
        (f64::NAN, 0),
    ];
    for (offset, row) in rows {
        assert_eq!(fixed.index_at(offset), row, "the row at {offset}");
    }
    let window = fixed.window(240.0, 96.0, 0.25);
    assert_eq!(
        (window.first, window.last),
        (9, 16),
        "the viewport's rows and a quarter of a viewport of overscan on each side"
    );
    assert!(
        fixed.window(1e9, 96.0, 0.0).last == 100,
        "the window ends at the list"
    );
    assert_eq!(
        VirtualList::new(0, ItemMetrics::Fixed(10.0))
            .unwrap()
            .window(0.0, 100.0, 1.0)
            .last,
        0
    );

    // rows learned to be taller than estimated move what is under them down; the window says by how much
    let mut learned = VirtualList::new(50, ItemMetrics::Estimated(10.0)).unwrap();
    assert_eq!(learned.total_height(), 500.0);
    let before = learned.window(200.0, 45.0, 0.0);
    assert_eq!(
        (before.first, before.last, before.anchor_adjust),
        (20, 25, 0.0)
    );
    learned.set_height(3, 30.0);
    learned.set_height(10, 15.0);
    assert_eq!(learned.total_height(), 500.0 + 20.0 + 5.0);
    assert_eq!(learned.top(11), 11.0 * 10.0 + 20.0 + 5.0);
    let after = learned.window(200.0, 45.0, 0.0);
    assert_eq!(
        after.anchor_adjust, 25.0,
        "the row at the top stays where it was seen"
    );
    assert_eq!(learned.index_at(learned.top(30) + 1.0), 30);
    learned.set_count(5);
    assert_eq!(learned.count(), 5);
    assert_eq!(
        learned.window(0.0, 1000.0, 0.0),
        VirtualWindow {
            first: 0,
            last: 5,
            anchor_adjust: 0.0
        }
    );

    assert_eq!(
        VirtualList::new(3, ItemMetrics::Fixed(0.0)).err(),
        Some(LayoutError::InvalidConstraint)
    );
    assert_eq!(
        VirtualList::new(3, ItemMetrics::Fixed(f64::NAN)).err(),
        Some(LayoutError::NonFiniteValue)
    );
    assert_eq!(
        VirtualList::with_spacing(3, ItemMetrics::Fixed(5.0), -1.0).err(),
        Some(LayoutError::InvalidConstraint)
    );
}

#[test]
fn a_shortcut_names_its_letter_either_case_and_a_held_key_repeats_only_where_it_may() {
    let command = Mods::PRIMARY;
    let shortcut = |mods, keysym| Shortcut {
        mods,
        keysym: Keysym(keysym),
    };
    let key = |mods, keysym, pressed, repeat| KeyEvent {
        code: 0,
        keysym: Keysym(keysym),
        text: None,
        pressed,
        repeat,
        mods,
    };
    let (small_s, capital_s, small_q) = (0x73, 0x53, 0x71);
    let registry = CommandRegistry::new()
        .with(shortcut(command, small_s), "save")
        .with_fresh(shortcut(command, small_q), "quit");
    let rows = [
        (
            "the shortcut",
            key(command, small_s, true, false),
            Some("save"),
        ),
        (
            "the capital letter shift types",
            key(command, capital_s, true, false),
            Some("save"),
        ),
        (
            "a repeat of a shortcut that takes them",
            key(command, small_s, true, true),
            Some("save"),
        ),
        ("a release", key(command, small_s, false, false), None),
        (
            "another modifier",
            key(Mods::CTRL, small_s, true, false),
            None,
        ),
        ("no modifier", key(Mods::NONE, small_s, true, false), None),
        (
            "a fresh press",
            key(command, small_q, true, false),
            Some("quit"),
        ),
        (
            "a repeat of a fresh-only shortcut",
            key(command, small_q, true, true),
            None,
        ),
    ];
    for (what, event, expected) in rows {
        assert_eq!(registry.lookup(&event), expected, "{what}");
    }
    let mut registry = registry;
    assert_eq!(
        registry.insert(shortcut(command, capital_s), "save as"),
        Some("save"),
        "the same shortcut replaces"
    );
    assert_eq!(
        registry.lookup(&key(command, small_s, true, false)),
        Some("save as")
    );
}

/// Content 1000 high in a viewport 200 high, at `offset`, which scrolls vertically only.
fn column(offset: f64) -> ScrollMotion {
    ScrollMotion::new(
        Point::new(0.0, offset),
        Point::new(0.0, 800.0),
        Size::new(100.0, 200.0),
        (false, true),
    )
}

#[test]
fn a_drag_follows_the_finger_inside_the_range_and_gives_a_little_past_an_edge() {
    let mut motion = column(100.0);
    assert_eq!(motion.phases(0.0), [Phase::Rest; 2]);
    motion.drag(Point::new(0.0, 30.0), 0.0);
    assert!(motion.is_dragging());
    assert_eq!(motion.offset(0.0).y, 130.0, "one for one inside the range");
    assert_eq!(motion.stretch(0.0), Point::ZERO);
    assert_eq!(
        motion.shown(0.0).x,
        0.0,
        "an axis that does not scroll does not move"
    );

    // 670 to the end, then 50 more: the stretch is d(1 - 1/(1 + rx/d)) for the viewport's length d
    motion.drag(Point::new(0.0, 670.0), 0.01);
    assert_eq!(motion.offset(0.01).y, 800.0);
    motion.drag(Point::new(0.0, 50.0), 0.02);
    let expected = 200.0 * (1.0 - 1.0 / (1.0 + EDGE_RESISTANCE * 50.0 / 200.0));
    near(
        motion.stretch(0.02).y,
        expected,
        1e-6,
        "the stretch past the end",
    );
    assert_eq!(motion.offset(0.02).y, 800.0, "the offset stays at the edge");
    motion.drag(Point::new(0.0, 1e7), 0.03);
    assert!(
        motion.stretch(0.03).y < 200.0,
        "never more than the viewport"
    );

    // moving back unwinds the stretch along the same curve before the content moves
    let mut back = column(800.0);
    back.drag(Point::new(0.0, 50.0), 0.0);
    back.drag(Point::new(0.0, -50.0), 0.01);
    near(back.stretch(0.01).y, 0.0, 1e-6, "unwound");
    back.drag(Point::new(0.0, -20.0), 0.02);
    near(back.offset(0.02).y, 780.0, 1e-6, "then the content follows");

    // the start edge stretches the other way
    let mut start = column(0.0);
    start.drag(Point::new(0.0, -50.0), 0.0);
    assert!(start.stretch(0.0).y < 0.0);
}

#[test]
fn a_release_returns_a_stretch_to_its_edge_and_a_fast_one_leaves_motion_that_slows_to_rest() {
    let drag_to_the_end = |speed: f64, rest: f64| {
        // a finger moving `speed` points a second for a tenth of a second, then lifted `rest` seconds later
        let mut motion = column(0.0);
        let mut at = 0.0;
        for _ in 0..10 {
            at += 0.01;
            motion.drag(Point::new(0.0, speed * 0.01), at);
        }
        (motion, at + rest)
    };

    // a stretch returns to its edge whether or not there is inertia
    let mut pulled = column(800.0);
    pulled.drag(Point::new(0.0, 100.0), 0.0);
    pulled.release(0.5, false);
    assert_eq!(pulled.phases(0.5)[1], Phase::Returning);
    assert!(pulled.is_animating(0.5));
    assert!(pulled.stretch(0.5).y > 0.0);
    near(
        pulled.stretch(0.5 + 1.0).y,
        0.0,
        SETTLED,
        "back at its edge",
    );
    assert!(!pulled.is_animating(0.5 + 1.0));
    assert_eq!(pulled.phases(1.5)[1], Phase::Rest);

    // a fast release coasts on in its direction, its speed falling by the same share in equal times, and rests about
    // the speed times the time constant further on
    let (mut fast, lifted) = drag_to_the_end(800.0, 0.0);
    let at_release = fast.offset(lifted).y;
    fast.release(lifted, true);
    assert_eq!(fast.phases(lifted)[1], Phase::Coasting);
    let (early, later) = (fast.velocity(lifted + 0.2).y, fast.velocity(lifted + 0.6).y);
    near(
        early / later,
        (0.4 / MOMENTUM_TIME_CONSTANT).exp(),
        0.02 * (0.4 / MOMENTUM_TIME_CONSTANT).exp(),
        "the speed slows exponentially",
    );
    assert!(!fast.is_animating(lifted + 20.0), "and comes to rest");
    let carried = fast.offset(lifted + 20.0).y - at_release;
    near(
        carried,
        800.0 * MOMENTUM_TIME_CONSTANT,
        0.1 * 800.0 * MOMENTUM_TIME_CONSTANT,
        "how far it carries on",
    );

    // none of these leaves motion: no inertia asked for, a slow finger, a finger that rested before it lifted
    let slow = MIN_FLING_SPEED * 0.5;
    for (what, speed, rest_before_lifting, inertia) in [
        ("no inertia", 800.0, 0.0, false),
        ("a slow finger", slow, 0.0, true),
        ("a finger that rested", 800.0, 0.2, true),
    ] {
        let (mut motion, lifted) = drag_to_the_end(speed, rest_before_lifting);
        let at = motion.offset(lifted).y;
        motion.release(lifted, inertia);
        assert_eq!(motion.phases(lifted)[1], Phase::Rest, "{what}");
        assert_eq!(
            motion.offset(lifted + 5.0).y,
            at,
            "{what}: nothing moves on"
        );
    }

    // a fling into the end carries a little past it and comes back
    let mut flung = column(700.0);
    let mut at = 0.0;
    for _ in 0..10 {
        at += 0.01;
        flung.drag(Point::new(0.0, 20.0), at);
    }
    flung.release(at, true);
    let mut most = 0.0f64;
    for step in 1..200 {
        most = most.max(flung.stretch(at + f64::from(step) * 0.01).y);
    }
    assert!(most > 0.0, "it bounced past the end");
    near(flung.stretch(at + 5.0).y, 0.0, 1e-9, "settled");
    assert_eq!(flung.offset(at + 5.0).y, 800.0);
}

#[test]
fn a_drag_moves_the_axis_it_began_on_unless_the_view_is_free_and_a_step_moves_at_once() {
    let canvas = || {
        ScrollMotion::new(
            Point::ZERO,
            Point::new(500.0, 500.0),
            Size::new(100.0, 100.0),
            (true, true),
        )
    };
    let mut document = canvas();
    assert_eq!(document.policy(), AxisPolicy::Predominant);
    document.drag(Point::new(2.0, 10.0), 0.0);
    document.drag(Point::new(8.0, 1.0), 0.01);
    assert_eq!(
        document.offset(0.01),
        Point::new(0.0, 11.0),
        "a mostly vertical drag does not drift sideways"
    );
    document.release(0.02, false);
    document.drag(Point::new(10.0, 1.0), 0.5);
    assert_eq!(document.offset(0.5).x, 10.0, "the next drag chooses again");

    let mut free = canvas().with_policy(AxisPolicy::Free);
    free.drag(Point::new(2.0, 10.0), 0.0);
    assert_eq!(free.offset(0.0), Point::new(2.0, 10.0));

    let mut fits = ScrollMotion::new(
        Point::ZERO,
        Point::new(0.0, 500.0),
        Size::new(100.0, 100.0),
        (true, true),
    );
    assert!(
        !fits.can_move(Point::new(5.0, 0.0), 0.0),
        "an axis whose content fits has nowhere to go"
    );
    assert!(fits.can_move(Point::new(5.0, 5.0), 0.0));
    assert!(
        !fits.can_move(Point::new(0.0, -5.0), 0.0),
        "nor before the start"
    );
    fits.drag(Point::new(10.0, 10.0), 0.0);
    assert_eq!(fits.offset(0.0), Point::new(0.0, 10.0));

    let mut wheel = canvas();
    wheel.step(Point::new(0.0, 40.0), 0.0);
    assert_eq!(wheel.offset(0.0), Point::new(0.0, 40.0));
    assert_eq!(
        wheel.phases(0.0),
        [Phase::Rest; 2],
        "a wheel has no inertia"
    );
    wheel.step(Point::new(0.0, 1e6), 0.0);
    assert_eq!(wheel.offset(0.0).y, 500.0, "inside the range");
    wheel.jump_to(Point::new(7.0, 9.0));
    assert_eq!(wheel.offset(1.0), Point::new(7.0, 9.0));
    wheel.set_bounds(Point::new(5.0, 5.0), Size::new(100.0, 100.0), 1.0);
    assert_eq!(
        wheel.offset(1.0),
        Point::new(5.0, 5.0),
        "a range that shrank takes the content in"
    );
}
