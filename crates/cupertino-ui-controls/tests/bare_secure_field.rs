//! The bare secure field: a password field without chrome whose text is set at its own size and weight, centred in it
//! when asked, with a strip at its end a press submits from.

use crate::support;

use std::cell::Cell;

use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_controls::{BareSecureField, SecureField, Stack};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{PaintOp, Shape};
use cupertino_ui_theme::Color;
use support::{window, Harness};

/// The placeholder's own colour.
const PLACEHOLDER: Color = Color::new(1.0, 1.0, 1.0, 0.5);

/// The strip at the end of the centred field that submits.
const SUBMIT: f64 = 30.0;

thread_local! {
    /// The next field of this thread centres its text and submits from its last [`SUBMIT`] points.
    static CENTRED: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Edited(usize),
    /// What was submitted, by its length: the model keeps it as 1000 more.
    Submitted(usize),
}

struct Lock;

impl Application for Lock {
    type Model = Vec<usize>;
    type Message = Message;

    fn init() -> (Vec<usize>, Vec<Command<Message>>) {
        (Vec::new(), Vec::new())
    }

    fn update(model: &mut Vec<usize>, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Edited(len) => model.push(len),
            Message::Submitted(len) => model.push(1000 + len),
        }
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
                on_submit: Some(Box::new(|buffer| {
                    Message::Submitted(buffer.with_secret(str::len))
                })),
                initial: true,
                ..SecureField::default()
            },
            size: 13.0,
            weight: Some(500),
            placeholder_color: Some(PLACEHOLDER),
            centered: CENTRED.get(),
            submit_width: if CENTRED.get() { SUBMIT } else { 0.0 },
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

#[test]
fn a_bare_secure_field_draws_its_placeholder_in_its_own_colour_until_something_is_typed() {
    let mut h = harness();
    let glyph_colors = |h: &Harness<Lock>| -> Vec<Color> {
        h.ops("password")
            .into_iter()
            .filter_map(|op| match op {
                PaintOp::Glyphs { color, .. } => Some(color),
                _ => None,
            })
            .collect()
    };
    assert_eq!(glyph_colors(&h), vec![PLACEHOLDER]);
    h.type_text("s");
    assert!(
        !glyph_colors(&h).contains(&PLACEHOLDER),
        "the dots take the text colour"
    );
}

/// Where the glyphs the field painted start, in its own points.
fn glyphs_x(h: &Harness<Lock>) -> Vec<f64> {
    h.ops("password")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Glyphs { origin, .. } => Some(origin.x),
            _ => None,
        })
        .collect()
}

/// The caret bars the field drew last, in its own points.
fn carets(h: &Harness<Lock>) -> Vec<Rect> {
    h.ops("password")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::Rect(r),
                ..
            } if r.width() <= 2.0 && r.height() > 4.0 => Some(r),
            _ => None,
        })
        .collect()
}

#[test]
fn a_centred_bare_secure_field_centres_its_placeholder_and_dots_clear_of_its_submit_strip() {
    // the field of every view on this thread centres
    CENTRED.set(true);
    let mut h = harness();
    let width = h.rect("password").width();
    let label = support::label_width("Password");
    let placeholder = glyphs_x(&h)[0];
    assert!(
        (placeholder - (width - SUBMIT - label) / 2.0).abs() < 2.0,
        "the placeholder {label} wide starts at {placeholder} in {width} less {SUBMIT}"
    );
    // while empty the caret stands just before the placeholder's first glyph, not through its letters
    let [caret] = carets(&h)[..] else {
        panic!("one caret: {:?}", carets(&h));
    };
    assert!(
        caret.max_x() <= placeholder && placeholder - caret.max_x() <= 2.0,
        "the caret {caret:?} stands just before the placeholder at {placeholder}"
    );
    h.type_text("ab");
    let dots = glyphs_x(&h)[0];
    assert!(
        dots > placeholder && dots < (width - SUBMIT) / 2.0,
        "two dots, narrower than the placeholder, start at {dots}"
    );
    // with text it follows the centred dots: after the second one
    let [caret] = carets(&h)[..] else {
        panic!("one caret: {:?}", carets(&h));
    };
    assert!(
        caret.min_x() > dots + 2.0 && caret.max_x() < (width - SUBMIT) / 2.0 + label,
        "the caret {caret:?} follows the dots from {dots}"
    );
}

#[test]
fn a_press_in_the_submit_strip_submits_and_a_press_before_it_does_not() {
    CENTRED.set(true);
    let mut h = harness();
    h.type_text("secret");
    let rect = h.rect("password");
    let y = rect.min_y() + rect.height() / 2.0;
    h.click(Point::new(rect.min_x() + 10.0, y));
    assert!(
        h.model().iter().all(|&n| n < 1000),
        "a press on the text only places the caret"
    );
    h.click(Point::new(rect.max_x() - SUBMIT / 2.0, y));
    assert_eq!(
        h.model().last(),
        Some(&1006),
        "the six characters were submitted"
    );
    assert_eq!(h.model().iter().filter(|&&n| n >= 1000).count(), 1);
}
