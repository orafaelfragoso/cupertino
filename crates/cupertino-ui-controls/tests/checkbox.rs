use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{CheckBox, Stack};
use cupertino_ui_core::{AccessibleValue, Application, Checked, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use support::{color, label_size, window, Harness, SPACE, TAB};

struct Boxes<const ENABLED: bool>;

#[derive(Default)]
struct Model {
    toggled: Vec<(&'static str, bool)>,
}

impl<const ENABLED: bool> Application for Boxes<ENABLED> {
    type Model = Model;
    type Message = (&'static str, bool);

    fn init() -> (Model, Vec<Command<(&'static str, bool)>>) {
        (Model::default(), Vec::new())
    }

    fn update(
        model: &mut Model,
        message: (&'static str, bool),
    ) -> Vec<Command<(&'static str, bool)>> {
        model.toggled.push(message);
        Vec::new()
    }

    fn view(_: &Model) -> Vec<Window<(&'static str, bool)>> {
        let check = |key: &'static str, checked| {
            Element::new(CheckBox {
                label: key.into(),
                checked,
                enabled: ENABLED,
                on_toggle: Some(Box::new(move |on| (key, on))),
            })
            .key(key)
        };
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![
                check("off", Checked::False),
                check("on", Checked::True),
                check("mixed", Checked::Mixed),
            ],
        }))
    }
}

type App = Boxes<true>;

fn fill<A: Application>(h: &Harness<A>, key: &'static str) -> Option<Color> {
    h.ops(key).into_iter().find_map(|op| match op {
        PaintOp::Fill {
            shape: Shape::RoundedRect { .. },
            paint: Paint::Solid(c),
        } => Some(c),
        _ => None,
    })
}

#[test]
fn the_box_and_its_label_make_the_chrome() {
    let h = Harness::<App>::new();
    let label = label_size("off");
    assert_eq!(
        h.rect("off").size,
        Size::new(16.0 + 6.0 + label.width, label.height.max(16.0)),
        "the regular box is 16 pt"
    );
}

#[test]
fn a_click_or_space_asks_for_the_next_state_and_mixed_turns_on() {
    let mut h = Harness::<App>::new();
    h.click(h.center("off"));
    h.click(h.center("on"));
    h.click(h.center("mixed"));
    h.tap(TAB);
    h.tap(SPACE);
    assert_eq!(
        h.model().toggled,
        [("off", true), ("on", false), ("mixed", true), ("off", true)]
    );
}

#[test]
fn a_checked_box_fills_as_selected_and_is_a_check_box_to_accessibility() {
    let h = Harness::<App>::new();
    assert_eq!(
        fill(&h, "off"),
        Some(cupertino_ui_theme::Color::new(0.0, 0.0, 0.0, 0.095)),
        "unchecked, a quiet well"
    );
    assert_eq!(
        fill(&h, "on"),
        Some(color(
            ColorRole::ControlFill,
            InteractionVariant::Normal,
            true
        ))
    );
    let mark = h.ops("on").into_iter().any(|op| {
        matches!(
            op,
            PaintOp::Stroke {
                shape: Shape::Path { .. },
                ..
            }
        )
    });
    assert!(mark, "a checked box draws its tick");
    let node = h.accessible("mixed");
    assert_eq!(node.role, Role::CheckBox);
    assert_eq!(node.name.as_deref(), Some("mixed"));
    assert_eq!(node.value, Some(AccessibleValue::Toggle(Checked::Mixed)));
}

#[test]
fn a_disabled_box_ignores_activation() {
    let mut h = Harness::<Boxes<false>>::new();
    h.click(h.center("off"));
    assert!(h.model().toggled.is_empty());
    assert!(h.accessible("off").states.disabled);
}

/// A check box whose label is longer than the column it stands in.
struct Narrow;

const LONG: &str = "Reopen windows when logging back in";
const COLUMN: f64 = 150.0;

impl Application for Narrow {
    type Model = ();
    type Message = bool;

    fn init() -> ((), Vec<Command<bool>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: bool) -> Vec<Command<bool>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<bool>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(CheckBox {
                label: LONG.into(),
                checked: Checked::False,
                enabled: true,
                on_toggle: Some(Box::new(|on| on)),
            })
            .key("long")],
        }))
    }
}

#[test]
fn a_label_longer_than_its_column_wraps_beside_the_box_on_its_first_line() {
    let mut h = Harness::<Narrow>::new();
    h.resize(Size::new(COLUMN, 200.0));
    let line = label_size(LONG);
    assert!(
        16.0 + 6.0 + line.width > COLUMN,
        "the label is too long for one line"
    );
    let rect = h.rect("long");
    assert!(rect.width() <= COLUMN, "inside the column: {rect:?}");
    assert!(
        rect.height() >= 2.0 * line.height,
        "two lines or more: {rect:?}"
    );
    let boxed = h
        .ops("long")
        .into_iter()
        .find_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, .. },
                ..
            } => Some(rect),
            _ => None,
        })
        .expect("the box");
    assert_eq!(boxed.min_x(), 0.0);
    assert!(
        boxed.max_y() <= line.height + 1.0,
        "the box stands on the first line, not in the middle: {boxed:?}"
    );
    assert_eq!(h.accessible("long").name.as_deref(), Some(LONG));
}

#[test]
fn a_label_that_fits_stays_on_one_line_with_the_box_centred_beside_it() {
    let mut h = Harness::<Narrow>::new();
    h.resize(Size::new(400.0, 200.0));
    let line = label_size(LONG);
    assert_eq!(
        h.rect("long").size,
        Size::new(16.0 + 6.0 + line.width, line.height.max(16.0))
    );
}
