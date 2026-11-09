//! {{name}}: a CupertinoKit application. A window with a number and two buttons that change it.

use std::process::ExitCode;

use cupertino_kit::controls::{TextRole, WINDOW_CONTENT_INSET};
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::DecorationMode;
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, TextStyle};

/// The desktop entry's name, `{{name}}.desktop`, which the desktop matches the window to.
const APP_ID: &str = "{{app_id}}";
const TITLE: &str = "{{name}}";

struct App;

/// What a button press asks for: add this to the count.
#[derive(Clone, Debug, PartialEq)]
struct Add(i64);

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

impl Application for App {
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
            secondary: false,
            selected: false,
        })
        .key("count");
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: TITLE.into(),
                app_id: APP_ID.into(),
                size: Size::new(200.0, 64.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Stack {
                axis: Axis::Horizontal,
                padding: WINDOW_CONTENT_INSET,
                spacing: 8.0,
                align: Align::Center,
                children: vec![
                    button("less", "-", ButtonRole::Normal, -1),
                    count,
                    button("more", "+", ButtonRole::Default, 1),
                ],
            }),
            menus: None,
        }]
    }
}

fn main() -> ExitCode {
    match cupertino_kit::app::run::<App>() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{TITLE}: {e}");
            ExitCode::FAILURE
        }
    }
}
