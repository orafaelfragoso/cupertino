//! The prompts' windows as CupertinoKit draws them. Each prompt is an alert on glass, 260 points wide, centred on the
//! active output over a scrim on every output: the lock symbol, the question in bold, what it is about, the fields and
//! two capsule buttons side by side. Nothing a person types reaches a window's title or any text the alert shows.

pub mod consent;
pub mod widgets;

use std::time::Instant;

use cupertino_kit::controls::{
    Button, ButtonRole, ButtonStyle, CheckBox, DisclosureButton, DisclosureStyle, Icon,
    IconDirection, PopUpButton, SecureField, Stack, Text, TextChange, TextField, TextRole,
};
use cupertino_kit::core::{
    Checked, CommandRegistry, Element, Key, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::text::{AttributedText, ContentPurpose, FontWeight, TextStyle};
use cupertino_kit::theme::ColorRole;

use crate::keyring::model::{Field, MAX_PASSWORD};
use crate::polkit::model::MAX_ANSWER;
use crate::program::{AuthInput, KeyringInput, Message, Secret};
use widgets::{AlertBody, AlertFrame, Scrim, Sized, ESCAPE};

/// The `app_id` of the prompts' windows.
pub const APP_ID: &str = "org.cupertino.prompts";
/// The alert: this wide, with these corners and this padding; its text sits further in than its controls.
pub const WIDTH: f64 = 260.0;
pub const RADIUS: f64 = 26.0;
pub const PADDING: Insets = Insets {
    top: 22.0,
    left: 16.0,
    bottom: 16.0,
    right: 16.0,
};
pub const TEXT_INSET: f64 = 6.0;
/// The lock symbol at the text's leading edge, and the room under it, under the title and above the buttons.
pub const SYMBOL: f64 = 64.0;
pub const SYMBOL_NAME: &str = "symbol.lock";
const SYMBOL_GAP: f64 = 14.0;
const TITLE_GAP: f64 = 10.0;
const SECTION_GAP: f64 = 16.0;
/// Between the controls of the form, and between a detail's name and its value.
const CONTROL_GAP: f64 = 8.0;
const DETAIL_GAP: f64 = 2.0;
/// The two capsule buttons side by side, this far apart, filling the alert's width between its insets.
pub const BUTTON_GAP: f64 = 8.0;
pub const BUTTON_HEIGHT: f64 = 28.0;
pub const BUTTON_WIDTH: f64 = (WIDTH - PADDING.left - PADDING.right - BUTTON_GAP) / 2.0;
/// The alert's text: the body size, and the caption size of the lines under the fields and the details.
const TEXT_SIZE: f32 = 13.0;
const CAPTION_SIZE: f32 = 11.0;
/// The surfaces the alerts are centred in: as wide as the alert and tall enough for the longest message, the most
/// lines under the fields and, for the authentication, its open details; the rest is clear.
const AUTHENTICATION_HEIGHT: f64 = 470.0;
const AUTHENTICATION_DETAILS_HEIGHT: f64 = 590.0;
pub const KEYRING_SIZE: Size = Size::new(WIDTH, 520.0);

pub const TITLE: &str = "Authentication Required";
pub const KEYRING_TITLE: &str = "Keyring";
pub const CANCEL: &str = "Cancel";
pub const AUTHENTICATE: &str = "Authenticate";
pub const DETAILS: &str = "Details";
pub const USER: &str = "User";
pub const CAPS_LOCK: &str = "Caps Lock is on";
pub const PASSWORD: &str = "Password";
pub const CONFIRM: &str = "Confirm";

/// The size of the authentication alert's surface.
pub fn authentication_size(details: bool) -> Size {
    let height = if details {
        AUTHENTICATION_DETAILS_HEIGHT
    } else {
        AUTHENTICATION_HEIGHT
    };
    Size::new(WIDTH, height)
}

/// What an authentication prompt shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Authentication {
    pub message: String,
    /// The users polkitd accepts, by the names the prompt gives them; the menu shows with more than one.
    pub users: Vec<String>,
    pub user: usize,
    /// The users' menu is open.
    pub menu: bool,
    /// The field's label: PAM's question.
    pub question: String,
    /// PAM asks for text to show as typed: what the field holds and its revision; `None` for a secret.
    pub echo: Option<(String, u64)>,
    /// The helper's questions so far: each is a new, empty field.
    pub asked: u32,
    /// The helper asked and waits for the answer.
    pub asking: bool,
    /// The field holds an answer Authenticate can submit.
    pub can_submit: bool,
    /// The answer is being checked.
    pub checking: bool,
    /// The last answer was refused.
    pub failed: bool,
    /// The lines under the field: the error, PAM's information, Caps Lock.
    pub status: Vec<String>,
    pub details: Vec<(&'static str, String)>,
    pub details_shown: bool,
}

/// What a keyring prompt shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyring {
    pub heading: String,
    pub description: String,
    /// A password question: the field shows.
    pub password: bool,
    /// A new password: its confirmation shows.
    pub confirm: bool,
    /// The choice's label and whether it is chosen.
    pub choice: Option<(String, bool)>,
    /// The lines under the fields: the warning, the mismatch, Caps Lock.
    pub status: Vec<String>,
    pub continue_label: String,
    pub cancel_label: String,
    /// The caller's questions so far: each is a new, empty field.
    pub asked: u32,
    pub checking: bool,
    pub mismatch: bool,
}

/// The field's label and placeholder: "Password" for PAM's password question, else the question PAM asks.
pub fn field_label(question: &str) -> String {
    let question = question.trim().trim_end_matches(':').trim_end();
    if question.is_empty() || question.eq_ignore_ascii_case(PASSWORD) {
        PASSWORD.to_owned()
    } else {
        question.to_owned()
    }
}

/// The key of the authentication's field for the helper's question number `asked`.
pub fn field_key(asked: u32) -> Key {
    Key::from(format!("answer-{asked}"))
}

/// The key of a keyring field for the caller's question number `asked`.
pub fn keyring_field_key(asked: u32, field: Field) -> Key {
    let name = match field {
        Field::Password => "password",
        Field::Confirm => "confirm",
    };
    Key::from(format!("{name}-{asked}"))
}

fn style(size: f32, weight: FontWeight) -> TextStyle {
    TextStyle {
        weight,
        ..TextStyle::new(size)
    }
}

fn text(content: &str, size: f32, weight: FontWeight, secondary: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, style(size, weight)),
        role: if size < TEXT_SIZE {
            TextRole::Caption
        } else {
            TextRole::Body
        },
        wrap: true,
        secondary,
        selected: false,
    })
}

fn stack(
    axis: Axis,
    spacing: f64,
    align: Align,
    padding: Insets,
    children: Vec<Element<Message>>,
) -> Element<Message> {
    Element::new(Stack {
        axis,
        padding,
        spacing,
        align,
        children,
    })
}

fn column(spacing: f64, children: Vec<Element<Message>>) -> Element<Message> {
    stack(
        Axis::Vertical,
        spacing,
        Align::Stretch,
        Insets::ZERO,
        children,
    )
}

/// The lock symbol, the bold heading and the text under it, inset from the controls.
fn head(heading: &str, body: &str) -> Element<Message> {
    let symbol = Element::new(Icon {
        name: SYMBOL_NAME.to_owned(),
        size: SYMBOL,
        direction: IconDirection::Fixed,
        label: None,
        tint: Some(ColorRole::SecondaryText),
    });
    head_with(symbol, heading, body)
}

/// `symbol` at the text's leading edge, the bold heading and the text under it, inset from the controls.
fn head_with(symbol: Element<Message>, heading: &str, body: &str) -> Element<Message> {
    let mut lines = vec![text(heading, TEXT_SIZE, FontWeight::BOLD, false)];
    if !body.trim().is_empty() {
        lines.push(text(body, TEXT_SIZE, FontWeight::REGULAR, false));
    }
    stack(
        Axis::Vertical,
        SYMBOL_GAP,
        Align::Stretch,
        Insets {
            top: 0.0,
            left: TEXT_INSET,
            bottom: 0.0,
            right: TEXT_INSET,
        },
        vec![
            stack(
                Axis::Horizontal,
                0.0,
                Align::Start,
                Insets::ZERO,
                vec![symbol],
            ),
            column(TITLE_GAP, lines),
        ],
    )
}

/// The lines under the fields, in the caption size.
fn status(lines: &[String]) -> Vec<Element<Message>> {
    lines
        .iter()
        .map(|line| text(line, CAPTION_SIZE, FontWeight::REGULAR, true))
        .collect()
}

/// A capsule button of the pair: `role` keys it, the default one or the other.
fn button(
    label: &str,
    role: ButtonRole,
    enabled: bool,
    busy: bool,
    initial: bool,
    press: Message,
) -> Element<Message> {
    let key = if role == ButtonRole::Default {
        "default"
    } else {
        "other"
    };
    Element::new(Sized {
        size: Size::new(BUTTON_WIDTH, BUTTON_HEIGHT),
        initial,
        children: vec![Element::new(Button {
            label: label.to_owned(),
            role,
            style: ButtonStyle::Pill,
            enabled,
            busy,
            on_press: Some(press),
        })],
    })
    .key(key)
}

/// The alert's window: `children` in its body, Escape dismissing it.
fn alert(
    key: WindowKey,
    title: &str,
    description: &str,
    size: Size,
    children: Vec<Element<Message>>,
) -> Window<Message> {
    let content = Element::new(AlertFrame {
        keys: Box::new(|sym| (sym == ESCAPE).then_some(Message::Escape)),
        label: title.to_owned(),
        description: (!description.is_empty()).then(|| description.to_owned()),
        children: vec![Element::new(AlertBody {
            width: WIDTH,
            radius: RADIUS,
            padding: PADDING,
            children: vec![column(SECTION_GAP, children)],
        })],
    });
    Window {
        key,
        spec: WindowSpec {
            title: title.to_owned(),
            app_id: APP_ID.to_owned(),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        // the server closes the alert only when it can't show it, which the program hears as such
        on_close: None,
        background: WindowBackground::None,
        content,
        popup: None,
        menus: None,
    }
}

/// The scrim over one output, fading in from `since` (shown at once without it).
pub fn scrim(key: WindowKey, since: Option<Instant>) -> Window<Message> {
    let content = Element::new(Scrim {
        since,
        frame: Message::Frame,
    });
    Window {
        key,
        spec: WindowSpec {
            title: String::new(),
            app_id: APP_ID.to_owned(),
            size: Size::new(0.0, 0.0),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::None,
        content,
        popup: None,
        menus: None,
    }
}

fn auth(input: AuthInput) -> Message {
    Message::Authentication(input)
}

/// The authentication field: secret, or showing what is typed when PAM asks so.
fn answer_field(prompt: &Authentication) -> Element<Message> {
    let key = field_key(prompt.asked);
    match &prompt.echo {
        Some((value, revision)) => Element::new(TextField {
            label: prompt.question.clone(),
            placeholder: prompt.question.clone(),
            value: value.clone(),
            revision: *revision,
            purpose: ContentPurpose::Normal,
            enabled: !prompt.checking,
            on_change: Some(Box::new(|change: TextChange| {
                auth(AuthInput::Text(change.text))
            })),
            on_submit: Some(auth(AuthInput::Submit)),
            selection: None,
            initial: true,
        })
        .key(key),
        None => Element::new(SecureField {
            label: prompt.question.clone(),
            placeholder: prompt.question.clone(),
            enabled: true,
            on_change: Some(Box::new(|buffer| {
                auth(AuthInput::Secret(Secret::of(buffer)))
            })),
            on_submit: Some(Box::new(|buffer| {
                auth(AuthInput::SubmitSecret(Secret::of(buffer)))
            })),
            busy: prompt.checking,
            invalid: prompt.failed,
            max_len: Some(MAX_ANSWER),
            initial: true,
        })
        .key(key),
    }
}

/// The users' pop-up button, and its menu while it is open.
fn users(prompt: &Authentication) -> PopUpButton<Message> {
    PopUpButton {
        label: USER.to_owned(),
        options: prompt.users.clone(),
        selected: prompt.user,
        open: prompt.menu,
        enabled: !prompt.checking,
        on_open: Some(auth(AuthInput::Users(true))),
    }
}

const USERS_KEY: &str = "users";

/// The authentication alert, and the users' menu while it is open.
pub fn authentication(key: WindowKey, prompt: &Authentication) -> Vec<Window<Message>> {
    let mut form = Vec::new();
    if prompt.users.len() > 1 {
        form.push(Element::new(users(prompt)).key(USERS_KEY));
    }
    form.push(answer_field(prompt));
    form.extend(status(&prompt.status));
    if !prompt.details.is_empty() {
        let disclosure = Element::new(DisclosureButton {
            label: DETAILS.to_owned(),
            expanded: prompt.details_shown,
            style: DisclosureStyle::Triangle,
            enabled: true,
            on_toggle: Some(Box::new(|open| auth(AuthInput::Details(open)))),
        })
        .key("details");
        form.push(stack(
            Axis::Horizontal,
            CONTROL_GAP,
            Align::Center,
            Insets::ZERO,
            vec![
                disclosure,
                text(DETAILS, CAPTION_SIZE, FontWeight::REGULAR, false),
            ],
        ));
        if prompt.details_shown {
            form.extend(prompt.details.iter().map(|(name, value)| {
                column(
                    DETAIL_GAP,
                    vec![
                        text(name, CAPTION_SIZE, FontWeight::REGULAR, true),
                        text(value, CAPTION_SIZE, FontWeight::REGULAR, false),
                    ],
                )
            }));
        }
    }
    let buttons = stack(
        Axis::Horizontal,
        BUTTON_GAP,
        Align::Center,
        Insets::ZERO,
        vec![
            button(
                CANCEL,
                ButtonRole::Normal,
                true,
                false,
                false,
                auth(AuthInput::Cancel),
            ),
            button(
                AUTHENTICATE,
                ButtonRole::Default,
                prompt.asking && prompt.can_submit,
                prompt.checking,
                false,
                auth(AuthInput::Submit),
            ),
        ],
    );
    let children = vec![
        head(TITLE, &prompt.message),
        column(CONTROL_GAP, form),
        buttons,
    ];
    let size = authentication_size(prompt.details_shown);
    let mut windows = Vec::new();
    if prompt.menu && prompt.users.len() > 1 {
        let menu_key = WindowKey(Key::from(format!("{:?}-users", key.0)));
        windows.extend(
            users(prompt)
                .menu(
                    key.clone(),
                    Key::from(USERS_KEY),
                    Box::new(|index| auth(AuthInput::User(index))),
                    auth(AuthInput::Users(false)),
                )
                .windows(menu_key),
        );
    }
    windows.insert(0, alert(key, TITLE, &prompt.message, size, children));
    windows
}

fn keyring_input(input: KeyringInput) -> Message {
    Message::Keyring(input)
}

fn keyring_field(prompt: &Keyring, field: Field) -> Element<Message> {
    let label = match field {
        Field::Password => PASSWORD,
        Field::Confirm => CONFIRM,
    };
    Element::new(SecureField {
        label: label.to_owned(),
        placeholder: label.to_owned(),
        enabled: true,
        on_change: Some(Box::new(move |buffer| {
            keyring_input(KeyringInput::Field(field, Secret::of(buffer)))
        })),
        on_submit: Some(Box::new(|_| keyring_input(KeyringInput::Continue))),
        busy: prompt.checking,
        invalid: field == Field::Confirm && prompt.mismatch,
        max_len: Some(MAX_PASSWORD),
        initial: field == Field::Password,
    })
    .key(keyring_field_key(prompt.asked, field))
}

/// The keyring's alert.
pub fn keyring(key: WindowKey, prompt: &Keyring) -> Window<Message> {
    let mut form = Vec::new();
    if prompt.password {
        form.push(keyring_field(prompt, Field::Password));
        if prompt.confirm {
            form.push(keyring_field(prompt, Field::Confirm));
        }
    }
    if let Some((label, chosen)) = &prompt.choice {
        form.push(
            Element::new(CheckBox {
                label: label.clone(),
                checked: if *chosen {
                    Checked::True
                } else {
                    Checked::False
                },
                enabled: !prompt.checking,
                on_toggle: Some(Box::new(|on| keyring_input(KeyringInput::Choice(on)))),
            })
            .key("choice"),
        );
    }
    form.extend(status(&prompt.status));
    let buttons = stack(
        Axis::Horizontal,
        BUTTON_GAP,
        Align::Center,
        Insets::ZERO,
        vec![
            button(
                &prompt.cancel_label,
                ButtonRole::Normal,
                true,
                false,
                false,
                keyring_input(KeyringInput::Cancel),
            ),
            button(
                &prompt.continue_label,
                ButtonRole::Default,
                !prompt.checking,
                prompt.checking,
                !prompt.password,
                keyring_input(KeyringInput::Continue),
            ),
        ],
    );
    let mut children = vec![head(&prompt.heading, &prompt.description)];
    if !form.is_empty() {
        children.push(column(CONTROL_GAP, form));
    }
    children.push(buttons);
    alert(key, KEYRING_TITLE, &prompt.heading, KEYRING_SIZE, children)
}
