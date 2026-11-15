use crate::support;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, SplitLayout, SplitPane, SplitView};
use cupertino_ui_core::{AccessibleValue, Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Axis, LayoutDirection};
use cupertino_ui_platform::PointerEvent;
use support::{window, Harness, TAB};

const RETURN: u32 = 0xff0d;
const HOME: u32 = 0xff50;
const LEFT: u32 = 0xff51;
const RIGHT: u32 = 0xff53;
const END: u32 = 0xff57;

fn pane(label: &str) -> Element<SplitLayout> {
    Element::new(Button {
        label: label.into(),
        role: ButtonRole::Normal,
        style: ButtonStyle::Bordered,
        enabled: false,
        busy: false,
        on_press: None,
    })
}

/// Two panes side by side in the 200 pt wide window, each at least 60 pt and the leading one at most 120 pt, the
/// divider at 80; the model takes every change.
struct Panes;

impl Application for Panes {
    type Model = SplitLayout;
    type Message = SplitLayout;

    fn init() -> (SplitLayout, Vec<Command<SplitLayout>>) {
        let layout = SplitLayout {
            position: 80.0,
            collapsed: None,
        };
        (layout, Vec::new())
    }

    fn update(layout: &mut SplitLayout, new: SplitLayout) -> Vec<Command<SplitLayout>> {
        *layout = new;
        Vec::new()
    }

    fn view(layout: &SplitLayout) -> Vec<Window<SplitLayout>> {
        window(Element::from(SplitView {
            label: "Sidebar".into(),
            axis: Axis::Horizontal,
            leading: pane("Leading").key("leading"),
            trailing: pane("Trailing").key("trailing"),
            layout: *layout,
            min_leading: 60.0,
            max_leading: 120.0,
            min_trailing: 60.0,
            on_change: Some(Box::new(|l| l)),
        }))
    }
}

fn divider(h: &Harness<Panes>) -> cupertino_ui_core::AccessibilityNode {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::Splitter)
        .expect("a splitter")
        .1
}

fn shown<A: Application>(h: &Harness<A>, label: &str) -> bool {
    h.snapshot()
        .accessibility
        .iter()
        .any(|(_, n)| n.name.as_deref() == Some(label))
}

/// A pointer pressed at `from` along the middle of a split view's height, then moved to each of `to`.
fn drag<A: Application>(h: &mut Harness<A>, from: f64, to: &[f64]) {
    h.pointer(PointerEvent::Motion {
        position: Point::new(from, 50.0),
    });
    h.button(Point::new(from, 50.0), true);
    for &x in to {
        move_to(h, x);
    }
}

fn move_to<A: Application>(h: &mut Harness<A>, x: f64) {
    h.pointer(PointerEvent::Motion {
        position: Point::new(x, 50.0),
    });
}

fn release<A: Application>(h: &mut Harness<A>, x: f64) {
    h.button(Point::new(x, 50.0), false);
}

#[test]
fn a_drag_on_the_strip_keeps_the_leading_pane_between_its_minimum_and_its_maximum() {
    let mut h = Harness::<Panes>::new();
    assert_eq!(h.rect("leading").width(), 80.0);
    assert_eq!(h.rect("trailing").min_x(), 81.0);

    drag(&mut h, 83.0, &[100.5]);
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 100.0,
            collapsed: None
        }
    );
    assert_eq!(h.rect("leading").width(), 100.0);

    move_to(&mut h, 160.0);
    assert_eq!(h.model().position, 120.0, "the leading pane's maximum");
    assert_eq!(h.rect("leading").width(), 120.0);
    assert_eq!(h.rect("trailing").width(), 79.0);

    move_to(&mut h, 45.0);
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 60.0,
            collapsed: None
        },
        "the leading pane's minimum, short of half of it"
    );
    assert_eq!(h.rect("leading").width(), 60.0);
    release(&mut h, 45.0);
}

#[test]
fn a_drag_past_half_the_leading_minimum_collapses_it_and_back_out_restores_it() {
    let mut h = Harness::<Panes>::new();
    drag(&mut h, 83.0, &[100.5, 20.0]);
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 80.0,
            collapsed: Some(SplitPane::Leading)
        },
        "it keeps the width it had before the drag"
    );
    assert!(!shown(&h, "Leading"));
    assert_eq!(h.rect("trailing").width(), 200.0);

    move_to(&mut h, 90.5);
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 90.0,
            collapsed: None
        },
        "the same drag brings it back"
    );
    assert!(shown(&h, "Leading"));
    release(&mut h, 90.5);

    drag(&mut h, 91.5, &[10.0]);
    release(&mut h, 10.0);
    assert_eq!(h.model().collapsed, Some(SplitPane::Leading));
    drag(&mut h, 1.5, &[70.5]);
    release(&mut h, 70.5);
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 70.0,
            collapsed: None
        },
        "a drag from the edge restores a collapsed pane"
    );
}

#[test]
fn a_drag_to_the_far_edge_never_collapses_the_trailing_pane() {
    let mut h = Harness::<Panes>::new();
    for to in [199.0, 400.0] {
        drag(&mut h, 83.0, &[to]);
        release(&mut h, to);
        assert_eq!(
            *h.model(),
            SplitLayout {
                position: 120.0,
                collapsed: None
            }
        );
        assert!(shown(&h, "Trailing"));
        drag(&mut h, 120.5, &[80.5]);
        release(&mut h, 80.5);
    }
}

#[test]
fn each_panes_content_fills_the_pane() {
    let mut h = Harness::<Panes>::new();
    h.resize(Size::new(200.0, 120.0));
    assert_eq!(h.rect("leading"), Rect::new(0.0, 0.0, 80.0, 120.0));
    assert_eq!(h.rect("trailing"), Rect::new(81.0, 0.0, 119.0, 120.0));
}

#[test]
fn a_press_beside_the_strip_reaches_the_pane_under_it() {
    let mut h = Harness::<Panes>::new();
    h.click(Point::new(77.0, 50.0));
    h.pointer(PointerEvent::Motion {
        position: Point::new(120.0, 50.0),
    });
    assert_eq!(h.model().position, 80.0);
}

#[test]
fn the_arrows_home_and_end_move_the_divider_and_return_collapses_and_restores() {
    let mut h = Harness::<Panes>::new();
    h.tap(TAB);
    assert!(divider(&h).states.focused);
    h.tap(RIGHT);
    assert_eq!(h.model().position, 90.0);
    h.tap(HOME);
    assert_eq!(h.model().position, 60.0);
    h.tap(END);
    assert_eq!(h.model().position, 120.0, "the leading pane's maximum");
    h.tap(RETURN);
    assert_eq!(h.model().collapsed, Some(SplitPane::Leading));
    assert!(!shown(&h, "Leading"));
    h.tap(RETURN);
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 120.0,
            collapsed: None
        }
    );
    assert!(shown(&h, "Leading"));

    let mut h = Harness::<Panes>::with_direction(LayoutDirection::Rtl);
    assert_eq!(
        h.rect("leading").min_x(),
        120.0,
        "the leading pane starts on the right"
    );
    h.tap(TAB);
    h.tap(LEFT);
    assert_eq!(
        h.model().position,
        90.0,
        "left widens the leading pane right to left"
    );
}

#[test]
fn it_is_a_splitter_whose_value_is_the_leading_panes_extent_shown_as_its_strip() {
    let h = Harness::<Panes>::new();
    let node = divider(&h);
    assert_eq!(node.name.as_deref(), Some("Sidebar"));
    assert_eq!(
        node.value,
        Some(AccessibleValue::Number {
            value: 80.0,
            min: 0.0,
            max: 120.0,
            step: Some(10.0),
        })
    );
    assert!(node.states.focusable);
    assert_eq!(node.bounds, Rect::new(77.5, 0.0, 6.0, 100.0));
}

#[test]
fn a_collapsed_pane_puts_the_value_at_an_end_of_its_range() {
    let mut h = Harness::<Panes>::new();
    h.tap(TAB);
    h.tap(RETURN);
    let value = |h: &Harness<Panes>| match divider(h).value {
        Some(AccessibleValue::Number {
            value, min, max, ..
        }) => (value, min, max),
        other => panic!("{other:?}"),
    };
    assert_eq!(value(&h), (0.0, 0.0, 120.0));

    drag(&mut h, 1.5, &[195.0]);
    release(&mut h, 195.0);
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 120.0,
            collapsed: None
        },
        "a drag to the far end restores the leading pane at its maximum"
    );
    assert_eq!(value(&h), (120.0, 0.0, 120.0));
}

/// A sidebar of 128 to 300 pt at 200 pt beside content of at least 128 pt, each pane holding a button the focus can
/// take; the model counts what it is told.
struct Sidebar;

struct Told {
    layout: SplitLayout,
    changes: usize,
}

impl Application for Sidebar {
    type Model = Told;
    type Message = SplitLayout;

    fn init() -> (Told, Vec<Command<SplitLayout>>) {
        let layout = SplitLayout {
            position: 200.0,
            collapsed: None,
        };
        (Told { layout, changes: 0 }, Vec::new())
    }

    fn update(told: &mut Told, new: SplitLayout) -> Vec<Command<SplitLayout>> {
        told.layout = new;
        told.changes += 1;
        Vec::new()
    }

    fn view(told: &Told) -> Vec<Window<SplitLayout>> {
        let button = |label: &str| {
            Element::new(Button {
                label: label.into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: None,
            })
            .key(label.to_lowercase())
        };
        window(Element::from(SplitView {
            label: "Sidebar".into(),
            axis: Axis::Horizontal,
            leading: button("Leading"),
            trailing: button("Trailing"),
            layout: told.layout,
            min_leading: 128.0,
            max_leading: 300.0,
            min_trailing: 128.0,
            on_change: Some(Box::new(|l| l)),
        }))
    }
}

fn focused(h: &Harness<Sidebar>) -> Option<String> {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.states.focused)
        .and_then(|(_, n)| n.name)
}

fn at(h: &mut Harness<Sidebar>, width: f64) {
    h.resize(Size::new(width, 100.0));
    h.settle();
}

#[test]
fn a_window_too_narrow_for_both_minimums_hides_the_sidebar_and_widening_brings_it_back() {
    let mut h = Harness::<Sidebar>::new();
    let value = |h: &Harness<Sidebar>| match h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::Splitter)
        .and_then(|(_, n)| n.value)
    {
        Some(AccessibleValue::Number { value, .. }) => value,
        other => panic!("{other:?}"),
    };
    at(&mut h, 400.0);
    assert_eq!(h.rect("leading").max_x(), 200.0);

    at(&mut h, 300.0);
    assert_eq!(
        h.rect("trailing").max_x() - h.rect("trailing").min_x(),
        128.0
    );
    assert_eq!(value(&h), 171.0, "the sidebar gives way first");

    at(&mut h, 250.0);
    assert!(!shown(&h, "Leading"));
    assert!(!shown(&h, "Sidebar"), "the divider goes with the pane");

    at(&mut h, 400.0);
    assert_eq!(h.rect("leading").max_x(), 200.0);
    assert_eq!(value(&h), 200.0);
    assert_eq!(h.model().changes, 0, "a resize tells the model nothing");
    assert_eq!(h.model().layout.position, 200.0);
}

#[test]
fn the_focus_in_a_sidebar_that_hides_moves_to_the_content() {
    let mut h = Harness::<Sidebar>::new();
    at(&mut h, 400.0);
    h.tap(TAB);
    assert_eq!(focused(&h).as_deref(), Some("Leading"));
    at(&mut h, 250.0);
    assert_eq!(focused(&h).as_deref(), Some("Trailing"));

    at(&mut h, 400.0);
    h.tap(TAB);
    assert_eq!(focused(&h).as_deref(), Some("Sidebar"));
    at(&mut h, 250.0);
    assert_eq!(
        focused(&h).as_deref(),
        Some("Trailing"),
        "the divider hides too"
    );
}

#[test]
fn a_collapsed_sidebar_stays_collapsed_whatever_the_width() {
    let mut h = Harness::<Sidebar>::new();
    at(&mut h, 400.0);
    for _ in 0..3 {
        h.tap(TAB);
    }
    assert_eq!(focused(&h).as_deref(), Some("Sidebar"));
    h.tap(RETURN);
    for width in [250.0, 600.0] {
        at(&mut h, width);
        assert_eq!(h.model().layout.collapsed, Some(SplitPane::Leading));
        assert!(!shown(&h, "Leading"));
    }
    assert_eq!(h.model().changes, 1);
}

#[test]
fn a_drag_to_the_far_edge_keeps_the_content_at_its_minimum_or_the_sidebar_at_its_maximum() {
    let mut h = Harness::<Sidebar>::new();
    at(&mut h, 700.0);
    drag(&mut h, 200.5, &[699.0]);
    release(&mut h, 699.0);
    assert_eq!(h.model().layout.position, 300.0, "the sidebar's maximum");
    assert_eq!(h.rect("leading").max_x(), 300.0);
    assert_eq!(h.rect("trailing").min_x(), 301.0);

    // in a window too narrow for the sidebar's maximum the content's minimum wins
    at(&mut h, 400.0);
    assert_eq!(h.rect("leading").max_x(), 271.0);
    drag(&mut h, 271.5, &[399.0, 1000.0]);
    release(&mut h, 1000.0);
    assert_eq!(
        h.model().layout,
        SplitLayout {
            position: 271.0,
            collapsed: None
        }
    );
    assert_eq!(h.rect("trailing").min_x(), 272.0);
    assert_eq!(
        h.rect("trailing").max_x(),
        400.0,
        "the content keeps 128 pt"
    );
    assert!(shown(&h, "Trailing"));

    // narrower still, the sidebar gives way down to its minimum, then hides rather than the content
    at(&mut h, 300.0);
    assert_eq!(h.rect("leading").max_x(), 171.0);
    assert_eq!(
        h.rect("trailing").max_x() - h.rect("trailing").min_x(),
        128.0
    );
    at(&mut h, 250.0);
    assert!(!shown(&h, "Leading"));
    assert!(shown(&h, "Trailing"));
    at(&mut h, 700.0);
    assert_eq!(h.rect("leading").max_x(), 271.0, "widening brings it back");
}
