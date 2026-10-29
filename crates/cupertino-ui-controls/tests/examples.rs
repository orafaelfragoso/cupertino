//! Two small apps driven by input, checked against Inspector dumps in `tests/goldens/`.
//! `CUPERTINO_UPDATE_GOLDENS=1` rewrites the dumps.

mod support;

use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, SecureField, Stack, Text, TextChange, TextField, TextRole,
    Toggle,
};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_test::Inspector;
use cupertino_ui_text::{AttributedText, ContentPurpose, TextStyle};
use support::{window, Harness, RETURN, SPACE, TAB};

fn text(content: impl Into<String>, role: TextRole) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: false,
    })
}

fn button(key: &'static str, label: &str, role: ButtonRole, message: Message) -> Element<Message> {
    Element::new(Button {
        label: label.into(),
        role,
        style: ButtonStyle::Bordered,
        enabled: true,
        busy: false,
        on_press: Some(message),
    })
    .key(key)
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Add(i64),
    Name(TextChange),
    Remember(bool),
    SignIn(usize),
}

struct Counter;

impl Application for Counter {
    type Model = i64;
    type Message = Message;

    fn init() -> (i64, Vec<Command<Message>>) {
        (0, Vec::new())
    }

    fn update(count: &mut i64, message: Message) -> Vec<Command<Message>> {
        if let Message::Add(n) = message {
            *count += n;
        }
        Vec::new()
    }

    fn view(count: &i64) -> Vec<Window<Message>> {
        window(Element::new(Stack {
            axis: Axis::Horizontal,
            spacing: 8.0,
            align: Align::Center,
            children: vec![
                button("less", "-", ButtonRole::Normal, Message::Add(-1)),
                text(count.to_string(), TextRole::Title).key("count"),
                button("more", "+", ButtonRole::Default, Message::Add(1)),
            ],
        }))
    }
}

#[derive(Default)]
struct Account {
    name: String,
    revision: u64,
    remember: bool,
    signed_in: Option<usize>,
}

struct Forms;

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
            Message::Add(_) => {}
        }
        Vec::new()
    }

    fn view(account: &Account) -> Vec<Window<Message>> {
        window(Element::new(Stack {
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
                Element::new(Stack {
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
                }),
            ],
        }))
    }
}

fn check_golden<A: Application>(h: &Harness<A>, name: &str) -> String {
    let dump = Inspector::default().dump(&h.snapshot());
    let path = format!("{}/tests/goldens/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(&path, &dump).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    if let Some(difference) = Inspector::first_difference(&expected, &dump) {
        panic!("{name} differs from its golden: {difference}");
    }
    dump
}

#[test]
fn the_counter_counts_clicks_and_keys() {
    let mut h = Harness::<Counter>::new();
    let more = h.center("more");
    h.click(more);
    h.click(more);
    h.tap(TAB);
    h.tap(SPACE);
    assert_eq!(*h.model(), 1);
    check_golden(&h, "counter");
}

#[test]
fn the_form_takes_a_name_a_password_and_a_choice() {
    let mut h = Harness::<Forms>::new();
    h.tap(TAB);
    h.type_text("Ada");
    h.tap(TAB);
    h.type_text("hunter2");
    h.tap(RETURN);
    h.tap(TAB);
    h.tap(SPACE);
    let account = h.model();
    assert_eq!(
        (account.name.as_str(), account.signed_in, account.remember),
        ("Ada", Some(7), true)
    );
    let dump = check_golden(&h, "forms");
    assert!(!dump.contains("hunter2"));
}
