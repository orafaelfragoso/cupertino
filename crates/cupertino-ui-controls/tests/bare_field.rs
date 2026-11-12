//! The bare field: a text field without chrome whose text is set at its own size.

use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{BareField, Stack, TextChange, TextField};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{PaintOp, Shape};
use cupertino_ui_text::ContentPurpose;
use support::{window, Harness};

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Edit(TextChange),
}

struct Model {
    query: String,
    revision: u64,
}

struct Query;

impl Application for Query {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (
            Model {
                query: String::new(),
                revision: 0,
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, Message::Edit(change): Message) -> Vec<Command<Message>> {
        model.query = change.text;
        model.revision = change.revision;
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let field = Element::new(BareField {
            field: TextField {
                label: "Search".into(),
                placeholder: "Search".into(),
                value: model.query.clone(),
                revision: model.revision,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(Box::new(Message::Edit)),
                on_submit: None,
                selection: None,
                initial: true,
            },
            size: 26.0,
        })
        .key("query");
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![field],
        }))
    }
}

fn harness() -> Harness<Query> {
    let mut h = Harness::<Query>::assistive();
    h.resize(Size::new(400.0, 100.0));
    h
}

#[test]
fn a_bare_field_is_one_line_of_its_own_type_tall() {
    let h = harness();
    // 26 pt text and room for its ascenders and descenders
    assert_eq!(h.rect("query").height(), 34.0);
}

#[test]
fn a_bare_field_paints_no_chrome_only_its_text() {
    let mut h = harness();
    h.type_text("dock");
    let shapes: Vec<PaintOp> = h
        .ops("query")
        .into_iter()
        .filter(|op| {
            matches!(
                op,
                PaintOp::Fill {
                    shape: Shape::RoundedRect { .. },
                    ..
                } | PaintOp::Stroke { .. }
            )
        })
        .collect();
    assert!(
        shapes.is_empty(),
        "no background, border or ring: {shapes:?}"
    );
    assert!(h
        .ops("query")
        .iter()
        .any(|op| matches!(op, PaintOp::Glyphs { .. })));
}

#[test]
fn a_bare_field_edits_and_reads_as_its_text_field() {
    let mut h = harness();
    h.type_text("calc");
    assert_eq!(h.model().query, "calc");
    let field = h.accessible("query");
    assert_eq!(
        (field.role, field.name.as_deref()),
        (Role::TextInput, Some("Search"))
    );
}
