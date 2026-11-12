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

/// Two panes side by side in the 200 pt wide window, each at least 60 pt, the divider at 80; the model takes every
/// change.
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

#[test]
fn a_drag_on_the_strip_moves_the_divider_and_past_half_a_minimum_collapses_that_pane() {
    let mut h = Harness::<Panes>::new();
    assert_eq!(h.rect("leading").width(), 80.0);
    assert_eq!(h.rect("trailing").min_x(), 81.0);

    h.pointer(PointerEvent::Motion {
        position: Point::new(83.0, 50.0),
    });
    h.button(Point::new(83.0, 50.0), true);
    h.pointer(PointerEvent::Motion {
        position: Point::new(120.5, 50.0),
    });
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 120.0,
            collapsed: None
        }
    );
    assert_eq!(h.rect("leading").width(), 120.0);
    h.pointer(PointerEvent::Motion {
        position: Point::new(160.0, 50.0),
    });
    assert_eq!(
        h.model().position,
        139.0,
        "the trailing pane keeps its minimum"
    );
    h.pointer(PointerEvent::Motion {
        position: Point::new(20.0, 50.0),
    });
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 139.0,
            collapsed: Some(SplitPane::Leading)
        }
    );
    assert!(!shown(&h, "Leading"));
    assert_eq!(h.rect("trailing").width(), 200.0);
    h.button(Point::new(20.0, 50.0), false);
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
    assert_eq!(h.model().position, 139.0);
    h.tap(RETURN);
    assert_eq!(h.model().collapsed, Some(SplitPane::Leading));
    assert!(!shown(&h, "Leading"));
    h.tap(RETURN);
    assert_eq!(
        *h.model(),
        SplitLayout {
            position: 139.0,
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
            max: 199.0,
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
    assert_eq!(value(&h), (0.0, 0.0, 199.0));

    h.button(Point::new(1.5, 50.0), true);
    h.pointer(PointerEvent::Motion {
        position: Point::new(195.0, 50.0),
    });
    h.button(Point::new(195.0, 50.0), false);
    assert_eq!(h.model().collapsed, Some(SplitPane::Trailing));
    assert_eq!(value(&h), (199.0, 0.0, 199.0));
}

/// A 128 pt sidebar at 200 pt beside content of at least 128 pt, each pane holding a button the focus can take; the
/// model counts what it is told.
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
