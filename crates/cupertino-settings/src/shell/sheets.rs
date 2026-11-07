//! A pane's sheet as a Kit dialog over the window: its title, its message, its field or list, Cancel and its action.

use cupertino_kit::controls::{Dialog, DialogAction, DialogButton, DialogKind};
use cupertino_kit::geometry::Insets;
use cupertino_kit::prelude::*;
use cupertino_kit::text::{ContentPurpose, SecureBuffer};

use super::state::State;
use super::Message;
use crate::sheet::{Kind, Outcome, Secret, Sheet};

/// What the user did in the sheet.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Cancel,
    Act,
    Text(String),
    /// The passphrase as typed so far, which the action button joins with.
    Typed(Secret),
    /// The passphrase as typed, then Join.
    Join(Secret),
    Row(usize),
    Parent,
}

/// Apply `action` to the open sheet: what the sheet decided, `None` when no sheet is open.
pub fn apply(state: &mut State, action: Action) -> Option<Outcome> {
    let sheet = state.sheet.as_mut()?;
    Some(match action {
        Action::Cancel => sheet.cancel(),
        Action::Act => sheet.action(),
        Action::Text(text) => {
            sheet.set_text(&text);
            Outcome::Stay
        }
        Action::Typed(secret) => {
            sheet.set_text(&secret);
            Outcome::Stay
        }
        Action::Join(secret) => {
            sheet.set_text(&secret);
            sheet.action()
        }
        Action::Row(row) => sheet.press_row(row),
        Action::Parent => sheet.parent(),
    })
}

fn typed(secret: &SecureBuffer) -> Secret {
    let mut typed = Secret::new();
    secret.with_secret(|s| typed.push_str(s));
    typed
}

fn body(sheet: &Sheet) -> Option<Element<Message>> {
    if let Some((rows, highlighted)) = sheet.rows() {
        let mut children = Vec::new();
        if matches!(sheet.kind, Kind::Picker { .. }) {
            children.push(
                Element::new(Button {
                    label: "Enclosing Folder".into(),
                    role: ButtonRole::Normal,
                    style: ButtonStyle::Bordered,
                    enabled: true,
                    busy: false,
                    on_press: Some(Message::Sheet(Action::Parent)),
                })
                .key("parent"),
            );
        }
        children.extend(rows.iter().enumerate().map(|(i, name)| {
            let label = if highlighted == Some(i) {
                format!("{name} ✓")
            } else {
                name.clone()
            };
            Element::new(Button {
                label,
                role: ButtonRole::Normal,
                style: ButtonStyle::Borderless,
                enabled: true,
                busy: false,
                on_press: Some(Message::Sheet(Action::Row(i))),
            })
            .key(i as u64)
        }));
        return Some(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 2.0,
            align: Align::Stretch,
            children,
        }));
    }
    match &sheet.kind {
        Kind::Passphrase { .. } => Some(
            Element::new(SecureField {
                label: "Password".into(),
                placeholder: "Password".into(),
                enabled: true,
                on_change: Some(Box::new(|secret| {
                    Message::Sheet(Action::Typed(typed(secret)))
                })),
                on_submit: Some(Box::new(|secret| {
                    Message::Sheet(Action::Join(typed(secret)))
                })),
                ..SecureField::default()
            })
            .key("passphrase"),
        ),
        Kind::Resolution { .. } | Kind::Pairing { .. } => {
            let value = sheet.field_text()?;
            Some(
                Element::new(TextField {
                    label: sheet.title(),
                    placeholder: String::new(),
                    value,
                    revision: 0,
                    purpose: ContentPurpose::Digits,
                    enabled: true,
                    on_change: Some(Box::new(|change| Message::Sheet(Action::Text(change.text)))),
                    on_submit: Some(Message::Sheet(Action::Act)),
                    selection: None,
                    initial: true,
                })
                .key("field"),
            )
        }
        Kind::Forget { .. } | Kind::Picker { .. } | Kind::Apps { .. } => None,
    }
}

pub fn dialog(sheet: &Sheet) -> Dialog<Message> {
    let destructive = matches!(sheet.kind, Kind::Forget { .. });
    Dialog {
        kind: if destructive {
            DialogKind::Alert
        } else {
            DialogKind::Dialog
        },
        title: sheet.title(),
        message: sheet.message(),
        body: body(sheet),
        buttons: vec![
            DialogButton {
                label: "Cancel".into(),
                action: DialogAction::Cancel,
                on_press: Message::Sheet(Action::Cancel),
            },
            DialogButton {
                label: sheet.action_title().into(),
                action: if destructive {
                    DialogAction::Destructive
                } else {
                    DialogAction::Default
                },
                on_press: Message::Sheet(Action::Act),
            },
        ],
    }
}
