//! The bare secure field: a password field without chrome whose text is set at its own size and weight.

use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{BareSecureField, SecureField, Stack};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{PaintOp, Shape};
use support::{window, Harness};

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Edited(usize),
}

struct Lock;

impl Application for Lock {
    type Model = Vec<usize>;
    type Message = Message;

    fn init() -> (Vec<usize>, Vec<Command<Message>>) {
        (Vec::new(), Vec::new())
    }

    fn update(model: &mut Vec<usize>, Message::Edited(len): Message) -> Vec<Command<Message>> {
        model.push(len);
        Vec::new()
    }

    fn view(_: &Vec<usize>) -> Vec<Window<Message>> {
        let field = Element::new(BareSecureField {
            field: SecureField {
                label: "Password".into(),
                placeholder: "Password".into(),
                on_change: Some(Box::new(|buffer| {
                    Message::Edited(buffer.with_secret(str::len))
                })),
                initial: true,
                ..SecureField::default()
            },
            size: 13.0,
            weight: Some(500),
        })
        .key("password");
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![field],
        }))
    }
}

fn harness() -> Harness<Lock> {
    let mut h = Harness::<Lock>::assistive();
    h.resize(Size::new(200.0, 60.0));
    h
}

#[test]
fn a_bare_secure_field_is_one_line_of_its_own_type_tall() {
    let h = harness();
    // 13 pt text and room for its ascenders and descenders
    assert_eq!(h.rect("password").height(), 17.0);
}

#[test]
fn a_bare_secure_field_paints_no_chrome_only_its_dots() {
    let mut h = harness();
    h.type_text("secret");
    assert_eq!(h.model().last(), Some(&6));
    let ops = h.ops("password");
    let chrome: Vec<&PaintOp> = ops
        .iter()
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
        chrome.is_empty(),
        "no background, border or ring: {chrome:?}"
    );
    assert!(ops.iter().any(|op| matches!(op, PaintOp::Glyphs { .. })));
}

#[test]
fn a_bare_secure_field_reads_as_a_password_field() {
    let mut h = harness();
    h.type_text("secret");
    let field = h.accessible("password");
    assert_eq!(
        (field.role, field.name.as_deref()),
        (Role::PasswordInput, Some("Password"))
    );
}
