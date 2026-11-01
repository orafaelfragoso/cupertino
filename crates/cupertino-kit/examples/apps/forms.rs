//! A sign-in form: a name, a password and a choice.

use cupertino_kit::controls::{TextRole, WINDOW_CONTENT_INSET};
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::platform::DecorationMode;
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};

/// Between a form's rows and between controls in a row, an approximate design value.
const ROW_GAP: f64 = 8.0;
/// Between a section label and its first row, an approximate design value.
const SECTION_GAP: f64 = 12.0;

pub struct Forms;

#[derive(Debug, Default)]
pub struct Account {
    pub name: String,
    pub revision: u64,
    pub remember: bool,
    /// The length of the password submitted last; the password itself never leaves its field.
    pub signed_in: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Name(TextChange),
    Remember(bool),
    SignIn(usize),
}

fn text(content: &str, role: TextRole) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: false,
        secondary: false,
        selected: false,
    })
}

impl Application for Forms {
    type Model = Account;
    type Message = Message;

    fn init() -> (Account, Vec<Command<Message>>) {
        (Account::default(), Vec::new())
    }

    fn update(account: &mut Account, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Name(change) => {
                account.name = change.text;
                account.revision = change.revision;
            }
            Message::Remember(on) => account.remember = on,
            Message::SignIn(len) => account.signed_in = Some(len),
        }
        Vec::new()
    }

    fn view(account: &Account) -> Vec<Window<Message>> {
        let remember = Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: ROW_GAP,
            align: Align::Center,
            children: vec![
                text("Remember me", TextRole::Body),
                Element::new(Toggle {
                    label: "Remember me".into(),
                    on: account.remember,
                    enabled: true,
                    on_toggle: Some(Box::new(Message::Remember)),
                })
                .key("remember"),
            ],
        });
        let rows = Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: ROW_GAP,
            align: Align::Stretch,
            children: vec![
                Element::new(TextField {
                    label: "Name".into(),
                    placeholder: "Your name".into(),
                    value: account.name.clone(),
                    revision: account.revision,
                    purpose: ContentPurpose::Name,
                    enabled: true,
                    on_change: Some(Box::new(Message::Name)),
                    on_submit: None,
                    selection: None,
                    initial: false,
                })
                .key("name"),
                Element::new(SecureField {
                    label: "Password".into(),
                    placeholder: "Required".into(),
                    enabled: true,
                    on_submit: Some(Box::new(|secret| {
                        Message::SignIn(secret.with_secret(|s| s.chars().count()))
                    })),
                })
                .key("password"),
                remember,
            ],
        });
        vec![Window {
            key: WindowKey("forms".into()),
            spec: WindowSpec {
                title: "Forms".into(),
                app_id: "org.cupertino.examples.Forms".into(),
                size: Size::new(280.0, 180.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: WINDOW_CONTENT_INSET,
                spacing: SECTION_GAP,
                align: Align::Stretch,
                children: vec![text("Sign In", TextRole::Section), rows],
            }),
            menus: None,
        }]
    }
}
