//! A plain-text document that scrolls, with selection, the clipboard and undo.

use cupertino_kit::controls::{ScrollView, Scroller};
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::DecorationMode;
use cupertino_kit::prelude::*;

pub struct Editor;

#[derive(Debug)]
pub struct Document {
    pub text: String,
    pub revision: u64,
    pub scroll: ScrollState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Edited(TextChange),
    Scrolled(ScrollState),
}

impl Application for Editor {
    type Model = Document;
    type Message = Message;

    fn init() -> (Document, Vec<Command<Message>>) {
        let document = Document {
            text: String::new(),
            revision: 0,
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
        };
        (document, Vec::new())
    }

    fn update(document: &mut Document, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Edited(change) => {
                document.text = change.text;
                document.revision = change.revision;
            }
            Message::Scrolled(scroll) => document.scroll = scroll,
        }
        Vec::new()
    }

    fn view(document: &Document) -> Vec<Window<Message>> {
        let editor = Element::new(TextEditor {
            label: "Document".into(),
            placeholder: String::new(),
            value: document.text.clone(),
            revision: document.revision,
            enabled: true,
            bordered: false,
            on_change: Some(Box::new(Message::Edited)),
        })
        .key("text");
        vec![Window {
            key: WindowKey("editor".into()),
            spec: WindowSpec {
                title: "Untitled".into(),
                app_id: "org.cupertino.examples.Editor".into(),
                size: Size::new(480.0, 320.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::from(ScrollView {
                content: editor,
                state: document.scroll,
                horizontal: false,
                vertical: true,
                scroller: Scroller::Overlay,
                on_scroll: Box::new(Message::Scrolled),
            }),
            menus: None,
        }]
    }
}
