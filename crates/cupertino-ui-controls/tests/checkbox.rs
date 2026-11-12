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
