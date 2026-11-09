//! A search field's look follows what it lies on: a light capsule on a material (a sidebar), a text field's recessed
//! well on the window's content; and its height follows the size table.

mod support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, SearchField, Stack, TextChange, TextField,
};
use cupertino_ui_core::{
    Application, Change, Command, Element, Event, EventCx, MeasureCx, PaintCx, Propagation,
    SemanticsCx, Widget, Window,
};
use cupertino_ui_layout::{Align, Axis, Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::ContentPurpose;
use cupertino_ui_theme::{Appearance, Color, ColorRole, ControlSize, InteractionVariant, Surface};
use std::cell::Cell;
use support::{color, window, Harness};

thread_local! {
    /// The search field's control size.
    static SIZE: Cell<Option<ControlSize>> = const { Cell::new(None) };
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Query(TextChange),
    /// The panel around the field asks for a material, or stops asking.
    Toggle,
}

/// Whether the panel holding the search field asks for the sidebar's material.
struct Model {
    material: bool,
}

struct Searching;

impl Application for Searching {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model { material: false }, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        if message == Message::Toggle {
            model.material = !model.material;
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let search = Element::new(SearchField {
            field: TextField {
                label: "Search".into(),
                placeholder: "Search".into(),
                value: String::new(),
                revision: 0,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(Box::new(Message::Query)),
                on_submit: None,
                selection: None,
                initial: false,
            },
            size: SIZE.with(Cell::get),
        })
        .key("search");
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![
                Element::new(Panel {
                    material: model.material,
                    child: Some(search),
                }),
                Element::new(Button {
                    label: "Toggle".into(),
                    role: ButtonRole::Normal,
                    style: ButtonStyle::Bordered,
                    enabled: true,
                    busy: false,
                    on_press: Some(Message::Toggle),
                })
                .key("toggle"),
            ],
        }))
    }
}

/// A box around the field that asks for the sidebar's material under itself while `material`.
struct Panel {
    material: bool,
    child: Option<Element<Message>>,
}

impl Widget<Message> for Panel {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        self.child.take().into_iter().collect()
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.material != old.material {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.material {
            cx.material(Surface::Sidebar, 0.0, 1.0);
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

fn harness(appearance: Appearance) -> Harness<Searching> {
    let mut theme = support::theme();
    theme.preferences.appearance = appearance;
    Harness::with_theme(theme)
}

/// The search field's capsule: its fill and the colour of its edge.
fn chrome(h: &Harness<Searching>) -> (Color, Color) {
    let ops = h.ops("search");
    let fill = ops.iter().find_map(|op| match op {
        PaintOp::Fill {
            shape: Shape::RoundedRect { .. },
            paint: Paint::Solid(c),
        } => Some(*c),
        _ => None,
    });
    let edge = ops.iter().find_map(|op| match op {
        PaintOp::Stroke {
            paint: Paint::Solid(c),
            ..
        } => Some(*c),
        _ => None,
    });
    (fill.expect("a fill"), edge.expect("an edge"))
}

fn white(a: f32) -> Color {
    Color::new(1.0, 1.0, 1.0, a)
}

fn black(a: f32) -> Color {
    Color::new(0.0, 0.0, 0.0, a)
}

fn normal(role: ColorRole) -> Color {
    color(role, InteractionVariant::Normal, false)
}

/// The well and the capsule in each appearance: on the window's content, then on a material.
#[test]
fn a_search_field_is_a_recessed_well_on_content_and_a_light_capsule_on_a_material() {
    let border = normal(ColorRole::TextFieldBorder);
    let cases = [
        (
            Appearance::Light,
            (normal(ColorRole::TextBackground), black(0.08)),
            (white(0.45), black(0.09)),
        ),
        (
            Appearance::Dark,
            (black(0.25), white(0.05)),
            (white(0.23), white(0.12)),
        ),
        (
            Appearance::LightHighContrast,
            (normal(ColorRole::TextBackground), border),
            (white(0.45), border),
        ),
        (
            Appearance::DarkHighContrast,
            (black(0.25), border),
            (white(0.23), border),
        ),
    ];
    for (appearance, well, capsule) in cases {
        let mut h = harness(appearance);
        assert_eq!(chrome(&h), well, "{appearance:?} on the window's content");
        // the panel around the field asks for a material: the field, unchanged itself, paints again
        h.click(h.center("toggle"));
        assert!(h.model().material);
        assert_eq!(chrome(&h), capsule, "{appearance:?} on a material");
        h.click(h.center("toggle"));
        assert_eq!(chrome(&h), well, "{appearance:?} back on the content");
    }
}

#[test]
fn a_search_field_is_the_size_tables_height_and_a_large_one_is_a_sidebars() {
    for (size, height) in [(None, 24.0), (Some(ControlSize::Large), 28.0)] {
        SIZE.set(size);
        let h = harness(Appearance::Dark);
        assert_eq!(h.rect("search").height(), height, "{size:?}");
    }
    SIZE.set(None);
}
