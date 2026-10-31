//! The authentication panel as geometry and keyboard focus: where each part of the prompt sits for what it shows, and
//! what a key or a click on it does. Nothing here draws; sizes are in points.

use cupertino_geometry::{Point, Rect};

use crate::model::{Key, Prompt};

pub const WIDTH: f64 = 360.0;
pub const PADDING: f64 = 24.0;
pub const RADIUS: f64 = 12.0;
pub const SYMBOL: f64 = 40.0;
pub const TITLE_LINE: f64 = 16.0;
pub const BODY_LINE: f64 = 16.0;
pub const CAPTION_LINE: f64 = 14.0;
/// The message wraps to at most this many lines; the last one ends in an ellipsis when it is cut.
pub const MAX_MESSAGE_LINES: usize = 6;
pub const CONTROL_HEIGHT: f64 = 20.0;
pub const FIELD_HEIGHT: f64 = 22.0;
/// Below the symbol, and between the title and the message (APPROXIMATED).
const SYMBOL_GAP: f64 = 8.0;
const TITLE_GAP: f64 = 4.0;
const MESSAGE_GAP: f64 = 16.0;
const USER_GAP: f64 = 8.0;
const STATUS_GAP: f64 = 6.0;
const DETAILS_GAP: f64 = 8.0;
const BUTTONS_GAP: f64 = 20.0;
const BUTTON_SPACING: f64 = 8.0;
/// A push button is its title plus this on each side, and never narrower than [`BUTTON_MIN_WIDTH`] (APPROXIMATED).
pub const BUTTON_INSET: f64 = 12.0;
const BUTTON_MIN_WIDTH: f64 = 80.0;
/// The width of the label column of the user row and of the details (APPROXIMATED).
pub const LABEL_COLUMN: f64 = 64.0;

pub const TITLE: &str = "Authentication Required";
pub const CANCEL: &str = "Cancel";
pub const AUTHENTICATE: &str = "Authenticate";
pub const DETAILS: &str = "Details";
pub const USER: &str = "User";
pub const CAPS_LOCK: &str = "Caps Lock is on";
pub const PLACEHOLDER: &str = "Password";

/// What the panel shows, as far as its size depends on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Content {
    pub message_lines: usize,
    /// polkitd offers more than one user.
    pub users: bool,
    /// Caption lines under the field: the error, the helper's information, Caps Lock.
    pub status_lines: usize,
    /// The rows of the expanded details; `None` while they are collapsed.
    pub details: Option<usize>,
    /// The widths of the buttons' titles.
    pub cancel_title: f64,
    pub authenticate_title: f64,
}

/// Where each part of the panel is, in points from its top left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    pub width: f64,
    pub height: f64,
    pub symbol: Rect,
    pub title: Rect,
    /// The message's lines, one line box each.
    pub message: Vec<Rect>,
    pub user: Option<Rect>,
    pub field: Rect,
    pub status: Vec<Rect>,
    /// The disclosure's row.
    pub details: Rect,
    pub detail_rows: Vec<Rect>,
    pub cancel: Rect,
    pub authenticate: Rect,
}

fn button_width(title: f64) -> f64 {
    (title + 2.0 * BUTTON_INSET).max(BUTTON_MIN_WIDTH).ceil()
}

impl Frames {
    pub fn new(content: &Content) -> Frames {
        let inner = WIDTH - 2.0 * PADDING;
        let row = |y: f64, height: f64| Rect::new(PADDING, y, inner, height);
        let mut y = PADDING;
        let symbol = Rect::new((WIDTH - SYMBOL) / 2.0, y, SYMBOL, SYMBOL);
        y += SYMBOL + SYMBOL_GAP;
        let title = row(y, TITLE_LINE);
        y += TITLE_LINE + TITLE_GAP;
        let message = (0..content.message_lines.min(MAX_MESSAGE_LINES))
            .map(|i| row(y + i as f64 * BODY_LINE, BODY_LINE))
            .collect::<Vec<_>>();
        y += message.len() as f64 * BODY_LINE + MESSAGE_GAP;
        let user = content.users.then(|| {
            let user = row(y, CONTROL_HEIGHT);
            y += CONTROL_HEIGHT + USER_GAP;
            user
        });
        let field = row(y, FIELD_HEIGHT);
        y += FIELD_HEIGHT;
        if content.status_lines > 0 {
            y += STATUS_GAP;
        }
        let status = (0..content.status_lines)
            .map(|i| row(y + i as f64 * CAPTION_LINE, CAPTION_LINE))
            .collect::<Vec<_>>();
        y += status.len() as f64 * CAPTION_LINE + DETAILS_GAP;
        let details = row(y, CAPTION_LINE);
        y += CAPTION_LINE;
        let detail_rows = (0..content.details.unwrap_or(0))
            .map(|i| row(y + i as f64 * CAPTION_LINE, CAPTION_LINE))
            .collect::<Vec<_>>();
        y += detail_rows.len() as f64 * CAPTION_LINE + BUTTONS_GAP;
        let authenticate_width = button_width(content.authenticate_title);
        let authenticate = Rect::new(
            WIDTH - PADDING - authenticate_width,
            y,
            authenticate_width,
            CONTROL_HEIGHT,
        );
        let cancel_width = button_width(content.cancel_title);
        let cancel = Rect::new(
            authenticate.x() - BUTTON_SPACING - cancel_width,
            y,
            cancel_width,
            CONTROL_HEIGHT,
        );
        y += CONTROL_HEIGHT + PADDING;
        Frames {
            width: WIDTH,
            height: y.ceil(),
            symbol,
            title,
            message,
            user,
            field,
            status,
            details,
            detail_rows,
            cancel,
            authenticate,
        }
    }

    /// The control under `point`.
    pub fn control_at(&self, point: Point) -> Option<Control> {
        [
            (Some(self.field), Control::Field),
            (self.user, Control::User),
            (Some(self.details), Control::Details),
            (Some(self.cancel), Control::Cancel),
            (Some(self.authenticate), Control::Authenticate),
        ]
        .into_iter()
        .find_map(|(frame, control)| frame.filter(|f| f.contains(point)).map(|_| control))
    }
}

/// A part of the panel that takes the keyboard, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    User,
    Field,
    Details,
    Cancel,
    Authenticate,
}

/// A key as the panel tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKey {
    Tab { back: bool },
    Up,
    Down,
    Space,
    Typed(Key),
}

/// What a key or a click on the panel asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// The key goes to the prompt's field.
    Field(Key),
    /// Answer with what is typed, when the prompt can.
    Authenticate,
    Cancel,
    /// Another of polkitd's users: the next one, or the one before.
    User {
        back: bool,
    },
    ToggleDetails,
    /// Only the focus moved.
    Focused,
}

/// The panel's own state: which control has the keyboard and whether the details are shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub focus: Control,
    pub details: bool,
    users: bool,
}

impl Panel {
    /// A panel just shown; the field has the keyboard.
    pub fn new(users: bool) -> Panel {
        Panel {
            focus: Control::Field,
            details: false,
            users,
        }
    }

    /// polkitd offers more than one user, so the panel has the user row.
    pub fn users(&self) -> bool {
        self.users
    }

    fn order(&self) -> &'static [Control] {
        use Control::*;
        if self.users {
            &[User, Field, Details, Cancel, Authenticate]
        } else {
            &[Field, Details, Cancel, Authenticate]
        }
    }

    pub fn key(&mut self, key: PanelKey) -> Option<Command> {
        let command = match (key, self.focus) {
            (PanelKey::Typed(Key::Escape), _) => Command::Cancel,
            (PanelKey::Typed(Key::Return), _) => Command::Authenticate,
            (PanelKey::Tab { back }, _) => {
                let order = self.order();
                let i = order.iter().position(|c| *c == self.focus).unwrap_or(0);
                let n = order.len();
                self.focus = order[if back { (i + n - 1) % n } else { (i + 1) % n }];
                Command::Focused
            }
            (PanelKey::Typed(key), Control::Field) => Command::Field(key),
            (PanelKey::Space, Control::Field) => Command::Field(Key::Char(' ')),
            (PanelKey::Up, Control::User) => Command::User { back: true },
            (PanelKey::Down | PanelKey::Space, Control::User) => Command::User { back: false },
            (PanelKey::Space, Control::Details) => self.toggle_details(),
            (PanelKey::Space, Control::Cancel) => Command::Cancel,
            (PanelKey::Space, Control::Authenticate) => Command::Authenticate,
            _ => return None,
        };
        Some(command)
    }

    /// A click on `control`, which takes the keyboard.
    pub fn click(&mut self, control: Control) -> Command {
        self.focus = control;
        match control {
            Control::Field => Command::Focused,
            Control::User => Command::User { back: false },
            Control::Details => self.toggle_details(),
            Control::Cancel => Command::Cancel,
            Control::Authenticate => Command::Authenticate,
        }
    }

    fn toggle_details(&mut self) -> Command {
        self.details = !self.details;
        Command::ToggleDetails
    }
}

/// A caption line under the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The last attempt failed, or PAM reported an error.
    Error(String),
    /// What PAM tells the person, such as to touch a fingerprint reader.
    Info(String),
    CapsLock,
}

impl Status {
    pub fn text(&self) -> &str {
        match self {
            Status::Error(text) | Status::Info(text) => text,
            Status::CapsLock => CAPS_LOCK,
        }
    }

    /// The lines `prompt` shows under its field, Caps Lock last.
    pub fn of(prompt: &Prompt, caps_lock: bool) -> Vec<Status> {
        let error = prompt.error.clone().map(Status::Error);
        let info = prompt.info.clone().map(Status::Info);
        let caps = (caps_lock && !prompt.echo).then_some(Status::CapsLock);
        error.into_iter().chain(info).chain(caps).collect()
    }
}

/// The field's label and placeholder: "Password" for PAM's password question, else the question PAM asks.
pub fn field_label(question: &str) -> String {
    let question = question.trim().trim_end_matches(':').trim_end();
    if question.is_empty() || question.eq_ignore_ascii_case(PLACEHOLDER) {
        PLACEHOLDER.to_owned()
    } else {
        question.to_owned()
    }
}

/// Everything one frame of the panel shows.
pub struct View<'a> {
    pub frames: &'a Frames,
    pub panel: &'a Panel,
    pub prompt: &'a Prompt,
    /// The message as it wraps, cut to [`MAX_MESSAGE_LINES`].
    pub message: &'a [String],
    /// The chosen user's name, when polkitd offers several.
    pub user: Option<&'a str>,
    pub status: &'a [Status],
    /// The expanded details' rows: a label and its value.
    pub details: &'a [(&'static str, String)],
}

#[cfg(test)]
mod tests;
