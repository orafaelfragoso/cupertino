//! A number and two buttons that change it.

use cupertino_kit::controls::TextRole;
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::DecorationMode;
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, TextStyle};

pub struct Counter;

#[derive(Clone, Debug, PartialEq)]
pub struct Add(pub i64);

fn button(key: &'static str, label: &str, role: ButtonRole, add: i64) -> Element<Add> {
    Element::new(Button {
        label: label.into(),
        role,
        style: ButtonStyle::Bordered,
        enabled: true,
        busy: false,
        on_press: Some(Add(add)),
    })
    .key(key)
}

impl Application for Counter {
    type Model = i64;
    type Message = Add;

    fn init() -> (i64, Vec<Command<Add>>) {
        (0, Vec::new())
    }

    fn update(count: &mut i64, Add(n): Add) -> Vec<Command<Add>> {
        *count += n;
        Vec::new()
    }

    fn view(count: &i64) -> Vec<Window<Add>> {
        let count = Element::new(Text {
            content: AttributedText::plain(count.to_string(), TextStyle::new(13.0)),
            role: TextRole::Title,
            wrap: false,
        })
        .key("count");
        vec![Window {
            key: WindowKey("counter".into()),
            spec: WindowSpec {
                title: "Counter".into(),
                app_id: "org.cupertino.examples.Counter".into(),
                size: Size::new(200.0, 60.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            content: Element::new(Stack {
                axis: Axis::Horizontal,
                spacing: 8.0,
                align: Align::Center,
                children: vec![
                    button("less", "-", ButtonRole::Normal, -1),
                    count,
                    button("more", "+", ButtonRole::Default, 1),
                ],
            }),
        }]
    }
}
