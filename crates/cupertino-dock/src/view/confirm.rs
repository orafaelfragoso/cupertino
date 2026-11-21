//! The trash's alerts, as a window centred on the dock's output with the keyboard: the question the Empty Trash item
//! asks before it deletes, and what could not be deleted after. Both are the toolkit's alert (the picture of the full
//! trash, a title, a message and capsule buttons) with the words of `cupertino-trash`, so the file manager's
//! question and the dock's are one. Return presses the default button, Escape the one that cancels, Tab moves between
//! them.

use cupertino_kit::controls::{Dialog, DialogAction, DialogButton, DialogKind, Modal, Spacer};
use cupertino_kit::core::{CommandRegistry, Element, Key, Window, WindowBackground, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{DecorationMode, WindowSpec};

use super::{Input, ALERT_APP_ID};

/// The window: the alert hangs centred near its top, with room around it for its shadow and under it for a longer
/// message. Nothing else of it is drawn.
pub const WINDOW: Size = Size {
    width: 340.0,
    height: 400.0,
};

/// The alert's window on the output with key `output`: a new key, a new surface, when the dock moves.
pub fn key(output: u32) -> WindowKey {
    WindowKey(Key::from(format!("confirm-{output}")))
}

/// What the alert says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Alert {
    /// The question before the trash is emptied.
    Question,
    /// The emptying left something: why.
    Failed(String),
}

impl Alert {
    pub fn title(&self) -> &'static str {
        match self {
            Alert::Question => cupertino_trash::EMPTY_TITLE,
            Alert::Failed(_) => cupertino_trash::FAILED_TITLE,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Alert::Question => cupertino_trash::EMPTY_MESSAGE,
            Alert::Failed(why) => why,
        }
    }
}

/// The alert on the output with key `output`. The question sends `ConfirmDone` from its default button and
/// `ConfirmClosed` from the other; the failure's one button, the default, sends `ConfirmClosed`.
pub fn window<M: Clone + 'static>(alert: &Alert, output: u32, input: fn(Input) -> M) -> Window<M> {
    let buttons = match alert {
        Alert::Question => vec![
            DialogButton {
                label: cupertino_trash::CANCEL_BUTTON.into(),
                action: DialogAction::Cancel,
                on_press: input(Input::ConfirmClosed),
            },
            DialogButton {
                label: cupertino_trash::EMPTY_BUTTON.into(),
                action: DialogAction::Default,
                on_press: input(Input::ConfirmDone),
            },
        ],
        Alert::Failed(_) => vec![DialogButton {
            label: cupertino_trash::FAILED_BUTTON.into(),
            action: DialogAction::Default,
            on_press: input(Input::ConfirmClosed),
        }],
    };
    Window {
        key: key(output),
        spec: WindowSpec {
            title: alert.title().to_owned(),
            app_id: format!("{ALERT_APP_ID}@{output}"),
            size: WINDOW,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(key(output)))),
        background: WindowBackground::None,
        content: Element::from(Modal {
            content: Element::new(Spacer { min: 0.0 }),
            dialog: Some(Dialog {
                kind: DialogKind::Alert,
                icon: Some(cupertino_trash::EMPTY_ICON.into()),
                title: alert.title().to_owned(),
                message: alert.message().to_owned(),
                body: None,
                buttons,
            }),
        }),
        popup: None,
        menus: None,
    }
}
