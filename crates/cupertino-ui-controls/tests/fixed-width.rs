use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{Spacer, Stack, TextField};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_text::ContentPurpose;
use support::{window, Harness};

struct Row;

impl Application for Row {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        window(Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Center,
            children: vec![
                Element::new(Spacer { min: 0.0 }),
                Element::new(TextField {
                    label: "Name".into(),
                    placeholder: String::new(),
                    value: String::new(),
                    revision: 0,
                    purpose: ContentPurpose::Normal,
                    enabled: true,
                    on_change: None,
                    on_submit: None,
                    selection: None,
                    initial: false,
                })
                .key("name")
                .width(260.0),
                Element::new(Spacer { min: 0.0 }),
            ],
        }))
    }
}

#[test]
fn a_fixed_width_element_keeps_its_width_in_a_wider_stack() {
    let mut h = Harness::<Row>::new();
    h.resize(Size::new(600.0, 100.0));
    let field = h.rect("name");
    assert_eq!((field.min_x(), field.width()), (170.0, 260.0));
    h.resize(Size::new(800.0, 100.0));
    let field = h.rect("name");
    assert_eq!((field.min_x(), field.width()), (270.0, 260.0));
}
