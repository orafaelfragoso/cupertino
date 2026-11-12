use crate::support;

use cupertino_geometry::{Insets, Point};
use cupertino_ui_controls::{DisclosureButton, DisclosureStyle, Stack};
use cupertino_ui_core::{Action, Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_render::{PaintOp, PathVerb, Shape};
use support::{window, Harness, RETURN, SPACE, TAB};

/// A "Details" triangle; the model takes every toggle.
struct Details;

impl Application for Details {
    type Model = bool;
    type Message = bool;

    fn init() -> (bool, Vec<Command<bool>>) {
        (false, Vec::new())
    }

    fn update(expanded: &mut bool, new: bool) -> Vec<Command<bool>> {
        *expanded = new;
        Vec::new()
    }

    fn view(expanded: &bool) -> Vec<Window<bool>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(DisclosureButton {
                label: "Details".into(),
                expanded: *expanded,
                style: DisclosureStyle::Triangle,
                enabled: true,
                on_toggle: Some(Box::new(|e| e)),
            })
            .key("details")],
        }))
    }
}

/// The triangle's tip less the node's centre.
fn tip(h: &Harness<Details>) -> (f64, f64) {
    let centre = h.center("details");
    let tip = h
        .ops("details")
        .into_iter()
        .find_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::Path { verbs, .. },
                ..
            } => match verbs[1] {
                PathVerb::LineTo(p) => Some(p),
                _ => None,
            },
            _ => None,
        })
        .expect("a triangle");
    let origin = h.rect("details").origin;
    let tip = Point::new(origin.x + tip.x, origin.y + tip.y);
    (
        ((tip.x - centre.x) * 100.0).round() / 100.0,
        ((tip.y - centre.y) * 100.0).round() / 100.0,
    )
}

#[test]
fn space_return_and_a_click_toggle_and_it_says_whether_it_is_expanded() {
    let mut h = Harness::<Details>::new();
    let node = h.accessible("details");
    assert_eq!(
        (node.role, node.states.expanded),
        (Role::Button, Some(false))
    );
    assert!(node.actions.contains(&Action::Expand));
    h.tap(TAB);
    h.tap(SPACE);
    assert!(*h.model());
    assert_eq!(h.accessible("details").states.expanded, Some(true));
    assert!(h.accessible("details").actions.contains(&Action::Collapse));
    h.tap(RETURN);
    assert!(!*h.model());
    h.click(h.center("details"));
    assert!(*h.model());
}

#[test]
fn the_triangle_points_along_the_reading_direction_while_closed_and_down_while_open() {
    let mut h = Harness::<Details>::new();
    assert_eq!(tip(&h), (3.9, 0.0));
    h.click(h.center("details"));
    assert_eq!(tip(&h), (0.0, 3.9));
    let h = Harness::<Details>::with_direction(LayoutDirection::Rtl);
    assert_eq!(tip(&h), (-3.9, 0.0));
}
