//! A press away from a focused text field ends its editing, unless it lands in content that asked to keep it: a search
//! panel whose field is where the keys always go.

use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, Stack, TextChange, TextField};
use cupertino_ui_core::{
    Application, Change, Command, Element, Event, EventCx, MeasureCx, PaintCx, Propagation,
    SemanticsCx, Widget, Window,
};
use cupertino_ui_layout::{Align, Axis, Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_text::ContentPurpose;
use support::{window, Harness, TAB};

struct Page;

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Edit(TextChange),
    Pressed,
}

/// Lays its children out in a column and asks that a press among them leave the field editing.
struct KeepsEditing {
    children: Vec<Element<Message>>,
}

impl Widget<Message> for KeepsEditing {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Start,
            justify: cupertino_ui_layout::Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.keeps_editing();
    }
}

fn button(label: &'static str) -> Element<Message> {
    Element::new(Button {
        label: label.into(),
        role: ButtonRole::Normal,
        style: ButtonStyle::Bordered,
        enabled: true,
        busy: false,
        on_press: Some(Message::Pressed),
    })
    .key(label)
}

impl Application for Page {
    type Model = String;
    type Message = Message;

    fn init() -> (String, Vec<Command<Message>>) {
        (String::new(), Vec::new())
    }

    fn update(model: &mut String, message: Message) -> Vec<Command<Message>> {
        if let Message::Edit(change) = message {
            *model = change.text;
        }
        Vec::new()
    }

    fn view(model: &String) -> Vec<Window<Message>> {
        let mut windows = window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(4.0),
            spacing: 4.0,
            align: Align::Start,
            children: vec![
                Element::new(TextField {
                    label: "Name".into(),
                    placeholder: String::new(),
                    value: model.clone(),
                    revision: 0,
                    purpose: ContentPurpose::Normal,
                    enabled: true,
                    on_change: Some(Box::new(Message::Edit)),
                    on_submit: None,
                    selection: None,
                    initial: false,
                })
                .key("name"),
                Element::new(KeepsEditing {
                    children: vec![button("Inside")],
                }),
                button("Away"),
            ],
        }));
        windows[0].spec.size = Size::new(200.0, 140.0);
        windows
    }
}

#[test]
fn a_press_on_a_button_ends_the_editing_but_one_in_content_that_keeps_it_does_not() {
    let mut h = Harness::<Page>::new();
    h.resize(Size::new(200.0, 140.0));
    h.tap(TAB);
    assert!(h.focused("name"));
    h.click(h.center("Inside"));
    assert!(
        h.focused("name"),
        "the press inside the content that keeps the editing"
    );
    h.click(h.center("Away"));
    assert!(!h.focused("name"), "a press on a button elsewhere ends it");
}
