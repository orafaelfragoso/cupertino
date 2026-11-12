use crate::support;

use cupertino_geometry::{Insets, Point, Rect};
use cupertino_ui_controls::{Stack, Stepper};
use cupertino_ui_core::{AccessibleValue, Action, Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{Paint, PaintOp};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use support::{color, window, Harness, TAB};

const UP: u32 = 0xff52;
const DOWN: u32 = 0xff54;
const PAGE_UP: u32 = 0xff55;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;

/// A count from 0 to 10 in steps of 1, starting at 5; the model takes every change.
struct Count;

impl Application for Count {
    type Model = f64;
    type Message = f64;

    fn init() -> (f64, Vec<Command<f64>>) {
        (5.0, Vec::new())
    }

    fn update(value: &mut f64, new: f64) -> Vec<Command<f64>> {
        *value = new;
        Vec::new()
    }

    fn view(value: &f64) -> Vec<Window<f64>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Stepper {
                label: "Copies".into(),
                value: *value,
                min: 0.0,
                max: 10.0,
                step: 1.0,
                enabled: true,
                on_change: Some(Box::new(|v| v)),
            })
            .key("stepper")],
        }))
    }
}

/// The up and the down chevron's colours, in that order.
fn chevrons(h: &Harness<Count>) -> Vec<Color> {
    h.ops("stepper")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Stroke {
                paint: Paint::Solid(color),
                width: 1.5,
                ..
            } => Some(color),
            _ => None,
        })
        .collect()
}

#[test]
fn the_keys_step_page_and_jump_to_the_limits_and_never_pass_them() {
    let mut h = Harness::<Count>::new();
    h.tap(TAB);
    h.tap(UP);
    assert_eq!(*h.model(), 6.0);
    h.tap(DOWN);
    h.tap(DOWN);
    assert_eq!(*h.model(), 4.0);
    h.tap(PAGE_UP);
    assert_eq!(*h.model(), 10.0, "ten steps, clamped at the maximum");
    h.tap(UP);
    assert_eq!(*h.model(), 10.0);
    h.tap(HOME);
    assert_eq!(*h.model(), 0.0);
    h.tap(END);
    assert_eq!(*h.model(), 10.0);
}

#[test]
fn the_upper_half_steps_up_the_lower_down_and_the_arrow_at_a_limit_is_disabled() {
    let mut h = Harness::<Count>::new();
    let r = h.rect("stepper");
    assert_eq!(r, Rect::new(0.0, 0.0, 20.0, 24.0), "the regular stepper");
    let (x, middle) = (r.min_x() + r.width() / 2.0, r.min_y() + r.height() / 2.0);
    h.click(Point::new(x, middle - 7.0));
    assert_eq!(*h.model(), 6.0);
    h.click(Point::new(x, middle + 7.0));
    h.click(Point::new(x, middle + 7.0));
    assert_eq!(*h.model(), 4.0);
    let enabled = chevrons(&h);
    assert_eq!(enabled.len(), 2);
    assert_eq!(enabled[0], enabled[1]);
    h.tap(END);
    let disabled = color(ColorRole::ControlText, InteractionVariant::Disabled, false);
    assert_eq!(chevrons(&h), [disabled, enabled[1]]);
    h.click(Point::new(x, middle - 7.0));
    assert_eq!(*h.model(), 10.0);
}

#[test]
fn it_is_a_spin_button_with_its_range_and_step() {
    let h = Harness::<Count>::new();
    let node = h.accessible("stepper");
    assert_eq!(node.role, Role::SpinButton);
    assert_eq!(node.name.as_deref(), Some("Copies"));
    assert_eq!(
        node.value,
        Some(AccessibleValue::Number {
            value: 5.0,
            min: 0.0,
            max: 10.0,
            step: Some(1.0),
        })
    );
    assert!(node.states.focusable);
    assert!(node.actions.contains(&Action::Increment) && node.actions.contains(&Action::Decrement));
}
