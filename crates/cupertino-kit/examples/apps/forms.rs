//! A sign-in form: a name, a password and a choice.

use cupertino_kit::controls::TextRole;
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::DecorationMode;
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};

pub struct Forms;

#[derive(Default)]
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
            spacing: 8.0,
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
        vec![Window {
            key: WindowKey("forms".into()),
            spec: WindowSpec {
                title: "Sign In".into(),
                app_id: "org.cupertino.examples.Forms".into(),
                size: Size::new(240.0, 140.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            content: Element::new(Stack {
                axis: Axis::Vertical,
                spacing: 6.0,
                align: Align::Start,
                children: vec![
                    text("Sign In", TextRole::Section),
                    Element::new(TextField {
                        label: "Name".into(),
                        placeholder: "Your name".into(),
                        value: account.name.clone(),
                        revision: account.revision,
                        purpose: ContentPurpose::Name,
                        enabled: true,
                        on_change: Some(Box::new(Message::Name)),
                        on_submit: None,
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
            }),
        }]
    }
}
