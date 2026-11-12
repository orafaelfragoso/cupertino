//! The keyring prompt's panel as geometry and keyboard focus. It is the authentication prompt's alert: as wide, with
//! its corners, insets, lock symbol and capsule buttons. Under the symbol come the caller's message in bold and its
//! description, then the password field (and its confirmation for a new password), the choice, the warning and the
//! buttons. A confirmation has no field. Sizes are in points.

use cupertino_geometry::{Point, Rect};

use crate::keyring::model::{Field, Kind, Prompt};
use crate::panel::{
    side_by_side, BODY_LINE, BUTTON_GAP, BUTTON_HEIGHT, CAPTION_LINE, EDGE, FIELD_HEIGHT, SYMBOL,
    TEXT_INSET, TEXT_WIDTH, TITLE_LINE, TOP, WIDTH,
};

/// The heading and the description each wrap to at most this many lines.
pub const MAX_HEADING_LINES: usize = 3;
pub const MAX_DESCRIPTION_LINES: usize = 6;
/// The choice's row: at least this tall, a check box this big beside its first line, and its label in at most
/// [`MAX_CHOICE_LINES`] lines.
pub const CHOICE_HEIGHT: f64 = 18.0;
pub const MAX_CHOICE_LINES: usize = 2;
pub const CHECK_BOX: f64 = 14.0;
/// Between the check box and its label.
pub const CHECK_GAP: f64 = 6.0;
/// Below the symbol, between the heading and the description, and after the text.
const SYMBOL_GAP: f64 = 14.0;
const HEADING_GAP: f64 = 6.0;
const TEXT_GAP: f64 = 16.0;
/// Between the two fields, and before the choice and the caption lines.
const FIELD_GAP: f64 = 8.0;
const CHOICE_GAP: f64 = 10.0;
const STATUS_GAP: f64 = 6.0;
const BUTTONS_GAP: f64 = 16.0;

pub const PASSWORD: &str = "Password";
pub const CONFIRM: &str = "Confirm";
pub const CAPS_LOCK: &str = "Caps Lock is on";

/// What the panel shows, as far as its size depends on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Content {
    pub heading_lines: usize,
    pub description_lines: usize,
    /// A password question: the field shows.
    pub password: bool,
    /// A new password: its confirmation shows.
    pub confirm: bool,
    pub choice: bool,
    /// The lines the choice's label wraps to (one or two).
    pub choice_lines: usize,
    /// Caption lines under the fields: the warning, the mismatch, Caps Lock.
    pub status_lines: usize,
    /// The widths of the buttons' titles.
    pub cancel_title: f64,
    pub continue_title: f64,
}

/// Where each part of the panel is, in points from its top left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    pub width: f64,
    pub height: f64,
    pub symbol: Rect,
    pub heading: Vec<Rect>,
    pub description: Vec<Rect>,
    pub password: Option<Rect>,
    pub confirm: Option<Rect>,
    pub choice: Option<Rect>,
    pub status: Vec<Rect>,
    pub cancel: Rect,
    pub continue_: Rect,
}

impl Frames {
    pub fn new(content: &Content) -> Frames {
        let control = |y: f64, height: f64| Rect::new(EDGE, y, WIDTH - 2.0 * EDGE, height);
        let text = |y: f64, height: f64| Rect::new(EDGE + TEXT_INSET, y, TEXT_WIDTH, height);
        let mut y = TOP;
        let symbol = Rect::new(EDGE + TEXT_INSET, y, SYMBOL, SYMBOL);
        y += SYMBOL + SYMBOL_GAP;
        let heading = (0..content.heading_lines.clamp(1, MAX_HEADING_LINES))
            .map(|i| text(y + i as f64 * TITLE_LINE, TITLE_LINE))
            .collect::<Vec<_>>();
        y += heading.len() as f64 * TITLE_LINE;
        let lines = content.description_lines.min(MAX_DESCRIPTION_LINES);
        if lines > 0 {
            y += HEADING_GAP;
        }
        let description = (0..lines)
            .map(|i| text(y + i as f64 * BODY_LINE, BODY_LINE))
            .collect::<Vec<_>>();
        y += description.len() as f64 * BODY_LINE + TEXT_GAP;
        let password = content.password.then(|| {
            let field = control(y, FIELD_HEIGHT);
            y += FIELD_HEIGHT;
            field
        });
        let confirm = (content.password && content.confirm).then(|| {
            y += FIELD_GAP;
            let field = control(y, FIELD_HEIGHT);
            y += FIELD_HEIGHT;
            field
        });
        let choice = content.choice.then(|| {
            if password.is_some() {
                y += CHOICE_GAP;
            }
            let lines = content.choice_lines.clamp(1, MAX_CHOICE_LINES) as f64;
            let height = CHOICE_HEIGHT.max(lines * BODY_LINE);
            let row = control(y, height);
            y += height;
            row
        });
        if content.status_lines > 0 && (password.is_some() || choice.is_some()) {
            y += STATUS_GAP;
        }
        let status = (0..content.status_lines)
            .map(|i| control(y + i as f64 * CAPTION_LINE, CAPTION_LINE))
            .collect::<Vec<_>>();
        y += status.len() as f64 * CAPTION_LINE;
        if password.is_some() || choice.is_some() || !status.is_empty() {
            y += BUTTONS_GAP;
        }
        let (cancel, continue_) = if side_by_side(&[content.cancel_title, content.continue_title]) {
            let half = (WIDTH - 2.0 * EDGE - BUTTON_GAP) / 2.0;
            (
                Rect::new(EDGE, y, half, BUTTON_HEIGHT),
                Rect::new(EDGE + half + BUTTON_GAP, y, half, BUTTON_HEIGHT),
            )
        } else {
            let continue_ = control(y, BUTTON_HEIGHT);
            y += BUTTON_HEIGHT + BUTTON_GAP;
            (control(y, BUTTON_HEIGHT), continue_)
        };
        y += BUTTON_HEIGHT + EDGE;
        Frames {
            width: WIDTH,
            height: y.ceil(),
            symbol,
            heading,
            description,
            password,
            confirm,
            choice,
            status,
            cancel,
            continue_,
        }
    }

    /// The control under `point`.
    pub fn control_at(&self, point: Point) -> Option<Control> {
        [
            (self.password, Control::Password),
            (self.confirm, Control::Confirm),
            (self.choice, Control::Choice),
            (Some(self.cancel), Control::Cancel),
            (Some(self.continue_), Control::Continue),
        ]
        .into_iter()
        .find_map(|(frame, control)| frame.filter(|f| f.contains(point)).map(|_| control))
    }
}

/// A part of the panel that takes the keyboard, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Password,
    Confirm,
    Choice,
    Cancel,
    Continue,
}

/// A key as the panel tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKey {
    Tab { back: bool },
    Space,
    Return,
    Escape,
    Backspace,
    Char(char),
}

/// What a key or a click on the panel asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// The key goes to this field.
    Type(Field, crate::keyring::model::Key),
    ToggleChoice,
    Continue,
    Cancel,
    /// Only the focus moved.
    Focused,
}

/// The panel's own state: which control has the keyboard, among those the prompt shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub focus: Control,
    order: Vec<Control>,
}

impl Panel {
    /// The panel for `prompt`'s question: the password field has the keyboard, else Continue.
    pub fn new(prompt: &Prompt) -> Panel {
        let mut order = Vec::new();
        if prompt.kind == Kind::Password {
            order.push(Control::Password);
            if prompt.confirms() {
                order.push(Control::Confirm);
            }
        }
        if prompt.has_choice() {
            order.push(Control::Choice);
        }
        order.extend([Control::Cancel, Control::Continue]);
        let focus = if order[0] == Control::Password {
            Control::Password
        } else {
            Control::Continue
        };
        Panel { focus, order }
    }

    pub fn order(&self) -> &[Control] {
        &self.order
    }

    pub fn key(&mut self, key: PanelKey) -> Option<Command> {
        use crate::keyring::model::Key;
        let field = match self.focus {
            Control::Password => Some(Field::Password),
            Control::Confirm => Some(Field::Confirm),
            _ => None,
        };
        let command = match (key, field) {
            (PanelKey::Escape, _) => Command::Cancel,
            // Return in a new password's first field goes on to its confirmation
            (PanelKey::Return, Some(Field::Password)) if self.order.contains(&Control::Confirm) => {
                self.focus = Control::Confirm;
                Command::Focused
            }
            (PanelKey::Return, _) => Command::Continue,
            (PanelKey::Tab { back }, _) => {
                let i = self
                    .order
                    .iter()
                    .position(|c| *c == self.focus)
                    .unwrap_or(0);
                let n = self.order.len();
                self.focus = self.order[if back { (i + n - 1) % n } else { (i + 1) % n }];
                Command::Focused
            }
            (PanelKey::Char(c), Some(field)) => Command::Type(field, Key::Char(c)),
            (PanelKey::Space, Some(field)) => Command::Type(field, Key::Char(' ')),
            (PanelKey::Backspace, Some(field)) => Command::Type(field, Key::Backspace),
            (PanelKey::Space, None) => match self.focus {
                Control::Choice => Command::ToggleChoice,
                Control::Cancel => Command::Cancel,
                _ => Command::Continue,
            },
            _ => return None,
        };
        Some(command)
    }

    /// A click on `control`, which takes the keyboard.
    pub fn click(&mut self, control: Control) -> Command {
        if self.order.contains(&control) {
            self.focus = control;
        }
        match control {
            Control::Password | Control::Confirm => Command::Focused,
            Control::Choice => Command::ToggleChoice,
            Control::Cancel => Command::Cancel,
            Control::Continue => Command::Continue,
        }
    }
}

/// A caption line under the fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The caller's warning, such as a wrong password, in red.
    Warning(String),
    /// The new password and its confirmation differ, in red.
    Mismatch,
    CapsLock,
}

impl Status {
    pub fn text(&self) -> &str {
        match self {
            Status::Warning(text) => text,
            Status::Mismatch => crate::keyring::model::MISMATCH,
            Status::CapsLock => CAPS_LOCK,
        }
    }

    /// The lines `prompt` shows, Caps Lock last and only while a password is asked.
    pub fn of(prompt: &Prompt, caps_lock: bool) -> Vec<Status> {
        let warning = prompt.properties.warning.trim();
        let warning = (!warning.is_empty()).then(|| Status::Warning(warning.to_owned()));
        let mismatch = prompt.mismatch.then_some(Status::Mismatch);
        let caps = (caps_lock && prompt.kind == Kind::Password).then_some(Status::CapsLock);
        warning.into_iter().chain(mismatch).chain(caps).collect()
    }
}

/// Everything one frame of the panel shows.
pub struct View<'a> {
    pub frames: &'a Frames,
    pub panel: &'a Panel,
    pub prompt: &'a Prompt,
    /// The heading and the description as they wrap.
    pub heading: &'a [String],
    pub description: &'a [String],
    pub status: &'a [Status],
    /// The choice's label as it wraps.
    pub choice: &'a [String],
    pub cancel_title: &'a str,
    pub continue_title: &'a str,
}

#[cfg(test)]
mod tests;
