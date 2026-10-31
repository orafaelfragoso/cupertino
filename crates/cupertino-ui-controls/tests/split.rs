mod support;

use cupertino_geometry::{Point, Rect};
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

fn shown(h: &Harness<Panes>, label: &str) -> bool {
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
            min: 60.0,
            max: 139.0,
            step: Some(10.0),
        })
    );
    assert!(node.states.focusable);
    assert_eq!(node.bounds, Rect::new(77.5, 0.0, 6.0, 100.0));
}
