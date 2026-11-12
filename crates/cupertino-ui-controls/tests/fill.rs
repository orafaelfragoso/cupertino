use crate::support;

use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_controls::{Stack, TextField};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_text::ContentPurpose;
use support::{window, Harness};

fn field(label: &str) -> Element<()> {
    Element::new(TextField {
        label: label.into(),
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
    .key(label.to_owned())
}

fn column(children: Vec<Element<()>>) -> Stack<()> {
    Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children,
    }
}

/// A field over a page that fills the rest, inside a stack that hugs it.
struct Page;

impl Application for Page {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let page = Element::new(column(vec![field("body")])).key("page").fill();
        window(Element::new(column(vec![
            field("header"),
            Element::new(column(vec![page])).fill(),
        ])))
    }
}

#[test]
fn a_filling_element_takes_what_its_siblings_leave_in_every_stack_above_it() {
    let mut h = Harness::<Page>::new();
    h.resize(Size::new(600.0, 300.0));
    let header = h.rect("header").height();
    assert_eq!(
        h.rect("page"),
        Rect::new(0.0, header, 600.0, 300.0 - header)
    );
    h.resize(Size::new(600.0, 400.0));
    assert_eq!(
        h.rect("page"),
        Rect::new(0.0, header, 600.0, 400.0 - header)
    );
}
