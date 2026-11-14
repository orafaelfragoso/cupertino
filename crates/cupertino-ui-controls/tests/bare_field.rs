//! The bare field: a text field without chrome whose text is set at its own size.

use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{BareField, Stack, TextChange, TextField};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::ContentPurpose;
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use support::{color, window, Harness};

thread_local! {
    /// The field holds its caret.
    static HOLD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

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
            hold_caret: HOLD.get(),
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

/// Standing alone in a larger surface, a bare field draws its caret in its text's colour, not the accent colour a
/// field with chrome uses.
#[test]
fn a_bare_field_s_caret_is_its_text_s_colour() {
    let mut h = harness();
    h.type_text("dock");
    let bars: Vec<_> = h
        .ops("query")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::Rect(r),
                paint: Paint::Solid(c),
            } if r.width() <= 2.0 && r.height() > 4.0 => Some(c),
            _ => None,
        })
        .collect();
    let text = color(ColorRole::Text, InteractionVariant::Normal, false);
    assert_eq!(bars, vec![text], "one caret, in the text's colour");
}

/// A held caret's blink draws nothing: through a fade out, the hidden stretch and a fade in the field draws no frame,
/// so it shows what it showed when the hold began; let go, the caret blinks again.
#[test]
fn a_held_caret_draws_no_frame_until_it_is_let_go() {
    use cupertino_ui_platform::Timestamp;
    const MS: u64 = 1_000_000;
    let blink = |h: &mut Harness<Query>, from: u64, to: u64| {
        for ms in (from..=to).step_by(16) {
            h.clock().advance_to(Timestamp(ms * MS)).unwrap();
            h.settle();
        }
    };
    let mut h = harness();
    h.type_text("doc");
    // the edit that takes the hold is the last to restart the blink
    HOLD.set(true);
    h.type_text("k");
    let frames = h.rt.window_system().frames(h.window).unwrap().len();
    blink(&mut h, 16, 1100);
    assert_eq!(
        h.rt.window_system().frames(h.window).unwrap().len(),
        frames,
        "the held caret drew no frame"
    );
    HOLD.set(false);
    h.type_text("s");
    let frames = h.rt.window_system().frames(h.window).unwrap().len();
    blink(&mut h, 1100, 2200);
    assert!(
        h.rt.window_system().frames(h.window).unwrap().len() > frames,
        "let go, the caret blinks"
    );
}
