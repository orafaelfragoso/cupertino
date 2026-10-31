//! The window's sheets (§6.4, §7.2): the Wi-Fi passphrase, the Bluetooth Forget confirmation and the pairing agent's
//! prompts. A sheet covers the
//! content column; it takes the keyboard while it is up. Its look and metrics are approximate (the platform sheet,
//! slides; here it appears at once).

use zeroize::{Zeroize, Zeroizing};

use cupertino_defaults::compositor::MOUSE_DPI_RANGE;

use cupertino_filepicker::{Key as PickerKey, Mode, Outcome as PickerOutcome, Picker};
use cupertino_typeahead::TypeAhead;

use crate::geom::{Point, Rect};
use crate::panes::Call;

/// Sheet metrics. approximate.
pub mod metrics {
    pub const WIDTH: f64 = 340.0;
    pub const PADDING: f64 = 20.0;
    pub const TITLE_HEIGHT: f64 = 18.0;
    pub const MESSAGE_HEIGHT: f64 = 32.0;
    pub const FIELD_HEIGHT: f64 = 22.0;
    pub const BUTTON: (f64, f64) = (96.0, 20.0);
    pub const GAP: f64 = 12.0;
    pub const RADIUS: f64 = 10.0;
    /// The chooser sheet: its width, a row of its list, the rows shown and the Up button. approximate.
    pub const PICKER_WIDTH: f64 = 460.0;
    pub const PICKER_ROW: f64 = 22.0;
    pub const PICKER_ROWS: usize = 12;
    pub const PICKER_UP: (f64, f64) = (48.0, 20.0);
}

use metrics as m;

/// Typed secret text (a passphrase, a passkey): overwritten with zeros when it is dropped, and shown as `[secret]` by
/// `Debug`, so no log of a sheet or a call prints it. It is made with room for [`SECRET_BYTES`] and never grows past
/// it, so no reallocation leaves a copy behind.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// An empty secret with its room.
    pub fn new() -> Self {
        Secret(Zeroizing::new(String::with_capacity(SECRET_BYTES)))
    }
}

impl Default for Secret {
    fn default() -> Self {
        Secret::new()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[secret]")
    }
}

impl std::ops::Deref for Secret {
    type Target = String;

    fn deref(&self) -> &String {
        &self.0
    }
}

impl std::ops::DerefMut for Secret {
    fn deref_mut(&mut self) -> &mut String {
        &mut self.0
    }
}

impl Zeroize for Secret {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

/// The room of a [`Secret`] (ours, approximate: a WPA passphrase is at most 63 characters, IEEE 802.11i).
pub const SECRET_BYTES: usize = 256;

/// An open file chooser; two of them are equal when they show the same folder with the same highlight.
#[derive(Clone, Debug)]
pub struct Chooser(pub Picker);

impl PartialEq for Chooser {
    fn eq(&self, other: &Self) -> bool {
        self.0.mode() == other.0.mode()
            && self.0.dir() == other.0.dir()
            && self.0.highlighted() == other.0.highlighted()
    }
}

impl Eq for Chooser {}

/// The type-to-select text of a list; it never makes two sheets differ.
#[derive(Clone, Debug, Default)]
pub struct Typed(pub TypeAhead);

impl PartialEq for Typed {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Typed {}

/// What a sheet asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The passphrase of a secured network that is not saved. The passphrase is never stored by the application.
    Passphrase {
        ssid: String,
        text: Secret,
        failure: Option<String>,
    },
    /// Forget a Bluetooth device, or take an application off Clipboard Access: Cancel is the default, the action is
    /// destructive (wording approximate).
    Forget { what: Forget, name: String },
    /// A typed resolution of the mouse `key` called `name`, in dots per inch.
    Resolution {
        key: String,
        name: String,
        text: String,
    },
    /// A chooser of a folder or a picture; `top` is the first row of its list that shows.
    Picker {
        chooser: Chooser,
        top: usize,
        /// The row under the pointer.
        hovered: Option<usize>,
        /// Why the chosen file was refused; the chooser stays up.
        failure: Option<String>,
    },
    /// The installed applications to allow to use the clipboard; `selected` is an enabled row, `top` the first row that
    /// shows. Return never allows: only the Allow button or a double click does.
    Apps {
        apps: Vec<crate::clipboard::App>,
        selected: Option<usize>,
        top: usize,
        hovered: Option<usize>,
        typed: Typed,
        failure: Option<String>,
    },
    /// A question of the Bluetooth daemon to the pairing agent (`org.cupertino.Bluetooth1.Agent`); `id` is the pairing.
    /// The wording is approximate (the design's pairing dialog is not known).
    Pairing {
        id: u64,
        device: String,
        prompt: Prompt,
        text: Secret,
    },
}

/// What the daemon asks during a pairing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Accept the pairing; `passkey` is the number both devices show, `None` for a pairing with no number (just works).
    Confirm { passkey: Option<u32> },
    /// Type the passkey the device shows.
    Request,
    /// Type `passkey` on the device; `entered` digits so far.
    Display { passkey: u32, entered: u8 },
}

/// The answer to a pairing prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Confirm,
    Passkey(u32),
    Reject,
}

/// The longest passkey: six decimal digits (Bluetooth Core, Vol 3 Part H, Passkey Entry: 000000 to 999999).
const PASSKEY_DIGITS: usize = 6;

/// The longest typed resolution: five digits, 32000 at most.
const RESOLUTION_DIGITS: usize = 5;

/// An open sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sheet {
    pub kind: Kind,
}

/// What a [`Kind::Forget`] sheet takes away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Forget {
    /// The Bluetooth device with this id.
    Device(String),
    /// The clipboard access of this program.
    Clipboard(std::path::PathBuf),
}

/// What a key or a press does to a sheet.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Stays up, changed or not.
    Stay,
    Cancel,
    /// The sheet's action: the call to make. The passphrase sheet stays up until it succeeds (§7.2).
    Do(Call),
    /// The pairing agent's answer; the sheet closes.
    Answer(Answer),
    /// The typed resolution of the mouse `key`; the sheet closes.
    Resolution {
        key: String,
        dpi: u32,
    },
    /// Take `program` off Clipboard Access; the sheet closes.
    Unallow(std::path::PathBuf),
    /// The program file the chooser picked to allow to use the clipboard; the sheet stays until it is allowed.
    Program(std::path::PathBuf),
    /// The chooser's result, a folder or a picture; the sheet closes.
    Picked {
        path: std::path::PathBuf,
        folder: bool,
    },
}

/// Where a sheet's parts lie in a content column `content`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetLayout {
    pub frame: Rect,
    pub title: Rect,
    pub message: Rect,
    pub field: Option<Rect>,
    pub cancel: Rect,
    pub action: Rect,
    /// The chooser's list of rows and its Up button.
    pub list: Option<Rect>,
    pub up: Option<Rect>,
}

impl Sheet {
    pub fn passphrase(ssid: &str) -> Self {
        Sheet {
            kind: Kind::Passphrase {
                ssid: ssid.to_string(),
                text: Secret::new(),
                failure: None,
            },
        }
    }

    pub fn forget(id: &str, name: &str) -> Self {
        Sheet {
            kind: Kind::Forget {
                what: Forget::Device(id.to_string()),
                name: name.to_string(),
            },
        }
    }

    /// The chooser of the installed applications `apps` to allow to use the clipboard, nothing selected.
    pub fn apps(apps: Vec<crate::clipboard::App>) -> Self {
        Sheet {
            kind: Kind::Apps {
                apps,
                selected: None,
                top: 0,
                hovered: None,
                typed: Typed::default(),
                failure: None,
            },
        }
    }

    /// Ask before `program`, shown as `name`, can no longer use the clipboard.
    pub fn unallow(program: &std::path::Path, name: &str) -> Self {
        Sheet {
            kind: Kind::Forget {
                what: Forget::Clipboard(program.to_path_buf()),
                name: name.to_string(),
            },
        }
    }

    pub fn resolution(key: &str, name: &str) -> Self {
        Sheet {
            kind: Kind::Resolution {
                key: key.to_string(),
                name: name.to_string(),
                text: String::new(),
            },
        }
    }

    /// A chooser of a folder (`Mode::Folder`) or a picture, opened at `start`.
    pub fn picker(start: &std::path::Path, mode: Mode) -> Self {
        Sheet {
            kind: Kind::Picker {
                chooser: Chooser(Picker::open(start, mode)),
                top: 0,
                hovered: None,
                failure: None,
            },
        }
    }

    /// [`Sheet::picker`] that lists nothing yet: the window hands [`Sheet::picker_request`] to a thread and the answer
    /// to [`Sheet::apply_listing`], so a folder that is slow to read never holds the window.
    pub fn picker_pending(start: &std::path::Path, mode: Mode) -> Self {
        Sheet {
            kind: Kind::Picker {
                chooser: Chooser(Picker::new(start, mode)),
                top: 0,
                hovered: None,
                failure: None,
            },
        }
    }

    /// The folder the chooser wants listed next, once.
    pub fn picker_request(&mut self) -> Option<cupertino_filepicker::Request> {
        match &mut self.kind {
            Kind::Picker { chooser, .. } => chooser.0.take_request(),
            _ => None,
        }
    }

    /// The listing a thread read for the chooser.
    pub fn apply_listing(&mut self, listing: cupertino_filepicker::Listing) {
        if let Kind::Picker { chooser, top, .. } = &mut self.kind {
            if chooser.0.apply(listing) == PickerOutcome::Changed {
                picked(chooser, top, PickerOutcome::Changed);
            }
        }
    }

    pub fn pairing(id: u64, device: &str, prompt: Prompt) -> Self {
        Sheet {
            kind: Kind::Pairing {
                id,
                device: device.to_string(),
                prompt,
                text: Secret::new(),
            },
        }
    }

    /// The pairing this sheet answers, if it is a pairing prompt.
    pub fn pairing_id(&self) -> Option<u64> {
        match &self.kind {
            Kind::Pairing { id, .. } => Some(*id),
            Kind::Passphrase { .. }
            | Kind::Forget { .. }
            | Kind::Resolution { .. }
            | Kind::Picker { .. }
            | Kind::Apps { .. } => None,
        }
    }

    pub fn title(&self) -> String {
        match &self.kind {
            Kind::Passphrase { ssid, .. } => {
                format!("Enter the password for \u{201c}{ssid}\u{201d}")
            }
            Kind::Forget {
                what: Forget::Device(_),
                name,
            } => format!("Forget \u{201c}{name}\u{201d}?"),
            Kind::Forget {
                what: Forget::Clipboard(_),
                name,
            } => format!("Remove \u{201c}{name}\u{201d} from Clipboard Access?"),
            Kind::Picker { chooser, .. } => match chooser.0.mode() {
                Mode::Folder => "Add a Folder".to_string(),
                Mode::Photo => "Add a Photo".to_string(),
                Mode::Program => "Choose a Program".to_string(),
            },
            Kind::Apps { .. } => "Allow an Application to Use the Clipboard".to_string(),
            Kind::Resolution { name, .. } => format!("Resolution of \u{201c}{name}\u{201d}"),
            Kind::Pairing {
                device,
                prompt: Prompt::Confirm { .. },
                ..
            } => format!("Pair with \u{201c}{device}\u{201d}?"),
            Kind::Pairing {
                device,
                prompt: Prompt::Request,
                ..
            } => format!("Enter the code shown on \u{201c}{device}\u{201d}"),
            Kind::Pairing {
                device,
                prompt: Prompt::Display { .. },
                ..
            } => format!("Pairing with \u{201c}{device}\u{201d}"),
        }
    }

    pub fn message(&self) -> String {
        match &self.kind {
            Kind::Passphrase { failure: Some(f), .. } => f.clone(),
            Kind::Passphrase { .. } => "The network needs a password to join.".to_string(),
            Kind::Picker {
                failure: Some(f), ..
            }
            | Kind::Apps {
                failure: Some(f), ..
            } => f.clone(),
            Kind::Apps { .. } => "It can read and change what you copy at any time.".to_string(),
            Kind::Picker { chooser, .. } if chooser.0.unreadable() => {
                "That folder cannot be opened.".to_string()
            }
            Kind::Picker { chooser, .. } => crate::panes::home_relative(chooser.0.dir()),
            Kind::Resolution { .. } => format!(
                "Type the resolution in dots per inch, from {} to {}.",
                MOUSE_DPI_RANGE.start(),
                MOUSE_DPI_RANGE.end()
            ),
            Kind::Forget { what: Forget::Device(_), .. } => "The device will need to be paired again to be used.".to_string(),
            Kind::Forget { what: Forget::Clipboard(_), .. } => "If it's open, it's disconnected and has to be opened again.".to_string(),
            Kind::Pairing { prompt: Prompt::Confirm { passkey: Some(p) }, .. } => format!("Check that the device shows {p:06}."),
            Kind::Pairing { prompt: Prompt::Confirm { passkey: None }, .. } => "The device asks to pair with this computer.".to_string(),
            Kind::Pairing { prompt: Prompt::Request, .. } => "Type the six-digit code.".to_string(),
            Kind::Pairing { prompt: Prompt::Display { passkey, entered }, .. } => format!("Type {passkey:06} on the device, then press Return on it. {entered} of 6 digits typed."),
        }
    }

    pub fn action_title(&self) -> &'static str {
        match &self.kind {
            Kind::Passphrase { .. } => "Join",
            Kind::Forget {
                what: Forget::Device(_),
                ..
            } => "Forget Device",
            Kind::Forget {
                what: Forget::Clipboard(_),
                ..
            } => "Remove",
            Kind::Resolution { .. } => "Set",
            Kind::Apps { .. } => "Allow",
            Kind::Picker { chooser, .. } => match chooser.0.mode() {
                Mode::Folder => "Choose",
                Mode::Photo => "Add",
                Mode::Program => "Allow",
            },
            Kind::Pairing {
                prompt: Prompt::Confirm { .. } | Prompt::Request,
                ..
            } => "Pair",
            // the daemon asks nothing back: the sheet only goes away
            Kind::Pairing {
                prompt: Prompt::Display { .. },
                ..
            } => "OK",
        }
    }

    /// The text of the sheet's field: bullets for the typed passphrase, the digits of a passkey; `None` without a field.
    pub fn field_text(&self) -> Option<String> {
        match &self.kind {
            Kind::Passphrase { text, .. } => Some("\u{2022}".repeat(text.chars().count())),
            Kind::Pairing {
                prompt: Prompt::Request,
                text,
                ..
            } => Some(text.to_string()),
            Kind::Resolution { text, .. } => Some(text.clone()),
            Kind::Forget { .. }
            | Kind::Pairing { .. }
            | Kind::Picker { .. }
            | Kind::Apps { .. } => None,
        }
    }

    /// The sheet's call: Join with the passphrase (moved out of the field, not copied), the confirmed removal, or the
    /// pairing answer.
    fn action(&mut self) -> Outcome {
        match &mut self.kind {
            Kind::Passphrase { text, .. } if text.is_empty() => Outcome::Stay,
            Kind::Passphrase { ssid, text, .. } => Outcome::Do(Call::Join {
                ssid: ssid.clone(),
                passphrase: Some(std::mem::take(text)),
            }),
            Kind::Picker { chooser, .. } => match chooser.0.choice() {
                Some(path) => chosen(chooser, path),
                None => Outcome::Stay,
            },
            Kind::Apps { apps, selected, .. } => {
                match selected
                    .and_then(|i| apps.get(i))
                    .and_then(|a| a.program.clone())
                {
                    Some(program) => Outcome::Program(program),
                    None => Outcome::Stay,
                }
            }
            Kind::Resolution { key, text, .. } => match text.parse::<u32>() {
                Ok(dpi) if MOUSE_DPI_RANGE.contains(&dpi) => Outcome::Resolution {
                    key: key.clone(),
                    dpi,
                },
                _ => Outcome::Stay,
            },
            Kind::Forget {
                what: Forget::Device(id),
                ..
            } => Outcome::Do(Call::Remove {
                id: id.clone(),
                confirmed: true,
            }),
            Kind::Forget {
                what: Forget::Clipboard(program),
                ..
            } => Outcome::Unallow(program.clone()),
            Kind::Pairing {
                prompt: Prompt::Confirm { .. },
                ..
            } => Outcome::Answer(Answer::Confirm),
            Kind::Pairing {
                prompt: Prompt::Request,
                text,
                ..
            } if text.len() < PASSKEY_DIGITS => Outcome::Stay,
            Kind::Pairing {
                prompt: Prompt::Request,
                text,
                ..
            } => Outcome::Answer(Answer::Passkey(text.parse().expect("six decimal digits"))),
            Kind::Pairing {
                prompt: Prompt::Display { .. },
                ..
            } => Outcome::Cancel,
        }
    }

    /// Cancel: a pairing question is refused; every other sheet just closes.
    fn cancel(&self) -> Outcome {
        match &self.kind {
            Kind::Pairing {
                prompt: Prompt::Confirm { .. } | Prompt::Request,
                ..
            } => Outcome::Answer(Answer::Reject),
            Kind::Passphrase { .. }
            | Kind::Forget { .. }
            | Kind::Resolution { .. }
            | Kind::Picker { .. }
            | Kind::Apps { .. }
            | Kind::Pairing {
                prompt: Prompt::Display { .. },
                ..
            } => Outcome::Cancel,
        }
    }

    /// A typed character, Backspace, Return or Escape. Return is the default button: Join or Pair, or Cancel when
    /// forgetting.
    pub fn key(&mut self, key: SheetKey) -> Outcome {
        self.key_at(key, 0.0)
    }

    /// [`Sheet::key`] at `now` seconds: the chooser's type-to-select forgets the typed text after a pause.
    pub fn key_at(&mut self, key: SheetKey, now: f64) -> Outcome {
        if let Kind::Picker { chooser, top, .. } = &mut self.kind {
            let picker_key = match key {
                SheetKey::Escape => return Outcome::Cancel,
                SheetKey::Return => PickerKey::Return,
                SheetKey::Up => PickerKey::Up,
                SheetKey::Down => PickerKey::Down,
                SheetKey::Home => PickerKey::Home,
                SheetKey::End => PickerKey::End,
                SheetKey::Parent => PickerKey::Parent,
                SheetKey::Choose => PickerKey::Choose,
                SheetKey::Type(c) => PickerKey::Type(c),
                SheetKey::Backspace => return Outcome::Stay,
            };
            let outcome = chooser.0.key(picker_key, now);
            return picked(chooser, top, outcome);
        }
        if let Kind::Apps {
            apps,
            selected,
            top,
            typed,
            ..
        } = &mut self.kind
        {
            let enabled: Vec<usize> = (0..apps.len())
                .filter(|i| apps[*i].reason.is_none())
                .collect();
            let at = selected.and_then(|s| enabled.iter().position(|i| *i == s));
            let to = match key {
                SheetKey::Escape => return Outcome::Cancel,
                SheetKey::Up => at.map_or(enabled.last(), |p| enabled.get(p.saturating_sub(1))),
                SheetKey::Down => {
                    at.map_or(enabled.first(), |p| enabled.get(p + 1).or(enabled.get(p)))
                }
                SheetKey::Home => enabled.first(),
                SheetKey::End => enabled.last(),
                SheetKey::Type(c) => {
                    let titles: Vec<Option<&str>> = apps
                        .iter()
                        .map(|a| a.reason.is_none().then_some(a.name.as_str()))
                        .collect();
                    let from = selected.unwrap_or(apps.len().saturating_sub(1));
                    return match typed.0.find(c, now, from, &titles) {
                        Some(i) => {
                            *selected = Some(i);
                            *top = scrolled_to(i, *top, apps.len());
                            Outcome::Stay
                        }
                        None => Outcome::Stay,
                    };
                }
                // Return in the list never allows: a key press alone grants no access
                SheetKey::Return | SheetKey::Choose | SheetKey::Parent | SheetKey::Backspace => {
                    None
                }
            };
            if let Some(&i) = to {
                *selected = Some(i);
                *top = scrolled_to(i, *top, apps.len());
            }
            return Outcome::Stay;
        }
        match (key, &mut self.kind) {
            (SheetKey::Escape, _) | (SheetKey::Return, Kind::Forget { .. }) => self.cancel(),
            (
                SheetKey::Return,
                Kind::Passphrase { .. } | Kind::Resolution { .. } | Kind::Pairing { .. },
            ) => self.action(),
            (SheetKey::Type(c), Kind::Resolution { text, .. }) => {
                if c.is_ascii_digit() && text.len() < RESOLUTION_DIGITS {
                    text.push(c);
                }
                Outcome::Stay
            }
            (SheetKey::Type(c), Kind::Passphrase { text, .. }) => {
                if text.len() + c.len_utf8() <= SECRET_BYTES {
                    text.push(c);
                }
                Outcome::Stay
            }
            (
                SheetKey::Type(c),
                Kind::Pairing {
                    prompt: Prompt::Request,
                    text,
                    ..
                },
            ) => {
                if c.is_ascii_digit() && text.len() < PASSKEY_DIGITS {
                    text.push(c);
                }
                Outcome::Stay
            }
            (
                SheetKey::Backspace,
                Kind::Passphrase { text, .. }
                | Kind::Pairing {
                    prompt: Prompt::Request,
                    text,
                    ..
                },
            ) => {
                text.pop();
                Outcome::Stay
            }
            (SheetKey::Backspace, Kind::Resolution { text, .. }) => {
                text.pop();
                Outcome::Stay
            }
            (
                SheetKey::Return | SheetKey::Type(_) | SheetKey::Backspace,
                Kind::Picker { .. } | Kind::Apps { .. },
            ) => Outcome::Stay,
            (
                SheetKey::Type(_) | SheetKey::Backspace,
                Kind::Forget { .. } | Kind::Pairing { .. },
            ) => Outcome::Stay,
            (
                SheetKey::Up
                | SheetKey::Down
                | SheetKey::Home
                | SheetKey::End
                | SheetKey::Parent
                | SheetKey::Choose,
                _,
            ) => Outcome::Stay,
        }
    }

    /// The daemon refused the passphrase: the sheet stays with the error and an empty field.
    pub fn failed(&mut self, error: &str) {
        match &mut self.kind {
            Kind::Passphrase { text, failure, .. } => {
                // zeroes the typed bytes, not only the length
                text.zeroize();
                *failure = Some(error.to_string());
            }
            Kind::Picker { failure, .. } | Kind::Apps { failure, .. } => {
                *failure = Some(error.to_string())
            }
            _ => {}
        }
    }

    /// The layout of the sheet centred in the content column `content`, near its top.
    pub fn layout(&self, content: &Rect) -> SheetLayout {
        if let Kind::Picker { .. } | Kind::Apps { .. } = self.kind {
            return self.picker_layout(content);
        }
        let field = self.field_text().is_some();
        let h = m::PADDING * 2.0
            + m::TITLE_HEIGHT
            + m::GAP / 2.0
            + m::MESSAGE_HEIGHT
            + if field { m::FIELD_HEIGHT + m::GAP } else { 0.0 }
            + m::GAP
            + m::BUTTON.1;
        let frame = Rect::new(
            content.x + (content.w - m::WIDTH) / 2.0,
            content.y + m::PADDING,
            m::WIDTH,
            h,
        );
        let inner = frame.inset(m::PADDING, m::PADDING);
        let title = Rect::new(inner.x, inner.y, inner.w, m::TITLE_HEIGHT);
        let message = Rect::new(
            inner.x,
            title.bottom() + m::GAP / 2.0,
            inner.w,
            m::MESSAGE_HEIGHT,
        );
        let field_rect =
            field.then(|| Rect::new(inner.x, message.bottom(), inner.w, m::FIELD_HEIGHT));
        let by = inner.bottom() - m::BUTTON.1;
        let action = Rect::new(inner.right() - m::BUTTON.0, by, m::BUTTON.0, m::BUTTON.1);
        let cancel = Rect::new(
            action.x - m::GAP - m::BUTTON.0,
            by,
            m::BUTTON.0,
            m::BUTTON.1,
        );
        SheetLayout {
            frame,
            title,
            message,
            field: field_rect,
            cancel,
            action,
            list: None,
            up: None,
        }
    }

    /// The chooser: the title with the Up button at its end, the folder under it, the list, then the buttons.
    fn picker_layout(&self, content: &Rect) -> SheetLayout {
        let list_h = m::PICKER_ROW * m::PICKER_ROWS as f64;
        let h = m::PADDING * 2.0
            + m::TITLE_HEIGHT
            + m::GAP / 2.0
            + m::FIELD_HEIGHT
            + m::GAP
            + list_h
            + m::GAP
            + m::BUTTON.1;
        let frame = Rect::new(
            content.x + (content.w - m::PICKER_WIDTH).max(0.0) / 2.0,
            content.y + m::PADDING,
            m::PICKER_WIDTH.min(content.w),
            h,
        );
        let inner = frame.inset(m::PADDING, m::PADDING);
        // the application chooser has no folders to go up from
        let up = matches!(self.kind, Kind::Picker { .. }).then(|| {
            Rect::new(
                inner.right() - m::PICKER_UP.0,
                inner.y - 1.0,
                m::PICKER_UP.0,
                m::PICKER_UP.1,
            )
        });
        let title = Rect::new(
            inner.x,
            inner.y,
            inner.w - up.map_or(0.0, |u| u.w + m::GAP),
            m::TITLE_HEIGHT,
        );
        let message = Rect::new(
            inner.x,
            title.bottom() + m::GAP / 2.0,
            inner.w,
            m::FIELD_HEIGHT,
        );
        let list = Rect::new(inner.x, message.bottom() + m::GAP, inner.w, list_h);
        let by = inner.bottom() - m::BUTTON.1;
        let action = Rect::new(inner.right() - m::BUTTON.0, by, m::BUTTON.0, m::BUTTON.1);
        let cancel = Rect::new(
            action.x - m::GAP - m::BUTTON.0,
            by,
            m::BUTTON.0,
            m::BUTTON.1,
        );
        SheetLayout {
            frame,
            title,
            message,
            field: None,
            cancel,
            action,
            list: Some(list),
            up,
        }
    }

    /// The pointer is at `p` (`None`: it left): the chooser's row under it shows its hover. `true` when that changed.
    pub fn hover(&mut self, layout: &SheetLayout, p: Option<Point>) -> bool {
        let (rows, top, hovered) = match &mut self.kind {
            Kind::Picker {
                chooser,
                top,
                hovered,
                ..
            } => (chooser.0.entries().len(), top, hovered),
            Kind::Apps {
                apps, top, hovered, ..
            } => (apps.len(), top, hovered),
            _ => return false,
        };
        let row = p.zip(layout.list).and_then(|(p, list)| {
            list.contains(p)
                .then(|| *top + ((p.y - list.y) / m::PICKER_ROW) as usize)
        });
        let row = row.filter(|r| *r < rows);
        std::mem::replace(hovered, row) != row
    }

    /// The wheel moved by `dy` points with the pointer at `p`: the chooser's list scrolls when `p` is over it. `true`
    /// when it moved.
    pub fn scroll(&mut self, layout: &SheetLayout, p: Point, dy: f64) -> bool {
        let (count, top) = match &mut self.kind {
            Kind::Picker { chooser, top, .. } => (chooser.0.entries().len(), top),
            Kind::Apps { apps, top, .. } => (apps.len(), top),
            _ => return false,
        };
        if !layout.list.is_some_and(|list| list.contains(p)) || dy == 0.0 {
            return false;
        }
        let rows = (dy / m::PICKER_ROW).round().abs().max(1.0) as usize;
        let last = count.saturating_sub(m::PICKER_ROWS);
        let to = if dy > 0.0 {
            (*top + rows).min(last)
        } else {
            top.saturating_sub(rows)
        };
        std::mem::replace(top, to) != to
    }

    /// A press at `p` in a sheet laid out as `layout`.
    pub fn press(&mut self, layout: &SheetLayout, p: Point) -> Outcome {
        if let Kind::Picker { chooser, top, .. } = &mut self.kind {
            let outcome = if layout.up.is_some_and(|up| up.contains(p)) {
                chooser.0.parent()
            } else if let Some(list) = layout.list.filter(|l| l.contains(p)) {
                let row = *top + ((p.y - list.y) / m::PICKER_ROW) as usize;
                // a press on the highlighted row opens it (the second press of a double click)
                if chooser.0.highlighted() == Some(row) {
                    chooser.0.activate(row)
                } else {
                    chooser.0.highlight(row)
                }
            } else if layout.cancel.contains(p) {
                return Outcome::Cancel;
            } else if layout.action.contains(p) {
                return self.action();
            } else {
                return Outcome::Stay;
            };
            return picked(chooser, top, outcome);
        }
        if let Kind::Apps {
            apps,
            selected,
            top,
            ..
        } = &mut self.kind
        {
            if let Some(list) = layout.list.filter(|l| l.contains(p)) {
                let row = *top + ((p.y - list.y) / m::PICKER_ROW) as usize;
                if apps.get(row).is_some_and(|a| a.reason.is_none()) {
                    // a press on the selected row allows it (the second press of a double click)
                    if *selected == Some(row) {
                        return self.action();
                    }
                    *selected = Some(row);
                }
                return Outcome::Stay;
            }
        }
        if layout.cancel.contains(p) {
            self.cancel()
        } else if layout.action.contains(p) {
            self.action()
        } else {
            Outcome::Stay
        }
    }
}

/// The keys a sheet reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetKey {
    Type(char),
    Backspace,
    Return,
    Escape,
    /// The chooser's keys: the arrows, Home and End, Super+Up (the parent folder) and Super+Return (choose).
    Up,
    Down,
    Home,
    End,
    Parent,
    Choose,
}

/// The sheet's outcome of what the chooser did, with its list scrolled to show the highlighted row.
fn picked(chooser: &Chooser, top: &mut usize, outcome: PickerOutcome) -> Outcome {
    let rows = chooser.0.entries().len();
    *top = match chooser.0.highlighted() {
        Some(i) if i < *top => i,
        Some(i) if i >= *top + m::PICKER_ROWS => i + 1 - m::PICKER_ROWS,
        Some(_) => *top,
        None => 0,
    }
    .min(rows.saturating_sub(m::PICKER_ROWS));
    match outcome {
        PickerOutcome::Choose(path) => chosen(chooser, path),
        PickerOutcome::Cancel => Outcome::Cancel,
        PickerOutcome::Changed | PickerOutcome::Ignore => Outcome::Stay,
    }
}

/// The first row of a list of `rows` that shows, moved from `top` just enough to show row `i`.
fn scrolled_to(i: usize, top: usize, rows: usize) -> usize {
    let top = if i < top {
        i
    } else if i >= top + m::PICKER_ROWS {
        i + 1 - m::PICKER_ROWS
    } else {
        top
    };
    top.min(rows.saturating_sub(m::PICKER_ROWS))
}

fn chosen(chooser: &Chooser, path: std::path::PathBuf) -> Outcome {
    match chooser.0.mode() {
        Mode::Program => Outcome::Program(path),
        mode => Outcome::Picked {
            path,
            folder: mode == Mode::Folder,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Secret {
        let mut s = Secret::new();
        s.push_str(text);
        s
    }

    #[test]
    fn no_debug_output_shows_a_typed_secret() {
        let mut s = Sheet::passphrase("office");
        for c in "hunter22".chars() {
            s.key(SheetKey::Type(c));
        }
        let mut pairing = Sheet::pairing(3, "Keyboard", Prompt::Request);
        for c in "482916".chars() {
            pairing.key(SheetKey::Type(c));
        }
        for shown in [format!("{s:?}"), format!("{pairing:?}")] {
            assert!(
                !shown.contains("hunter22")
                    && !shown.contains("482916")
                    && shown.contains("[secret]"),
                "{shown}"
            );
        }
        let call = s.key(SheetKey::Return);
        let shown = format!("{call:?}");
        assert!(
            !shown.contains("hunter22") && shown.contains("[secret]"),
            "{shown}"
        );
        assert_eq!(
            s.field_text().as_deref(),
            Some(""),
            "the passphrase moved into the call"
        );
    }

    #[test]
    fn the_passphrase_sheet_joins_with_what_was_typed_and_stays_up_on_a_refusal() {
        let mut s = Sheet::passphrase("office");
        assert_eq!(s.key(SheetKey::Return), Outcome::Stay, "nothing typed yet");
        for c in "secret".chars() {
            s.key(SheetKey::Type(c));
        }
        s.key(SheetKey::Backspace);
        assert_eq!(s.field_text().as_deref(), Some("•••••"));
        assert_eq!(
            s.key(SheetKey::Return),
            Outcome::Do(Call::Join {
                ssid: "office".into(),
                passphrase: Some(typed("secre"))
            })
        );
        s.failed("The password is not correct.");
        assert_eq!(
            (s.message().as_str(), s.field_text().as_deref()),
            ("The password is not correct.", Some(""))
        );
        assert_eq!(s.key(SheetKey::Escape), Outcome::Cancel);
    }

    #[test]
    fn forget_needs_its_destructive_button_and_return_cancels() {
        let mut s = Sheet::forget("dev-1", "Headphones");
        let l = s.layout(&Rect::new(216.0, 52.0, 499.0, 573.0));
        assert_eq!(l.frame.w, 340.0);
        assert!(l.field.is_none());
        assert_eq!(s.press(&l, l.cancel.center()), Outcome::Cancel);
        assert_eq!(
            s.press(&l, l.action.center()),
            Outcome::Do(Call::Remove {
                id: "dev-1".into(),
                confirmed: true
            })
        );
        assert_eq!(
            s.clone().key(SheetKey::Return),
            Outcome::Cancel,
            "Cancel is the default"
        );
        assert_eq!(s.action_title(), "Forget Device");
    }

    #[test]
    fn pairing_prompts_answer_the_agent_and_cancel_refuses() {
        let mut confirm = Sheet::pairing(7, "Keyboard", Prompt::Confirm { passkey: Some(42) });
        assert_eq!(confirm.message(), "Check that the device shows 000042.");
        assert_eq!(confirm.pairing_id(), Some(7));
        assert_eq!(
            confirm.key(SheetKey::Return),
            Outcome::Answer(Answer::Confirm)
        );
        assert_eq!(
            confirm.key(SheetKey::Escape),
            Outcome::Answer(Answer::Reject)
        );
        let mut request = Sheet::pairing(8, "Keyboard", Prompt::Request);
        for c in "12a34567".chars() {
            request.key(SheetKey::Type(c));
        }
        assert_eq!(
            request.field_text().as_deref(),
            Some("123456"),
            "digits only, six at most"
        );
        request.key(SheetKey::Backspace);
        assert_eq!(
            request.key(SheetKey::Return),
            Outcome::Stay,
            "five digits are not a passkey"
        );
        request.key(SheetKey::Type('0'));
        assert_eq!(
            request.key(SheetKey::Return),
            Outcome::Answer(Answer::Passkey(123450))
        );
        let mut display = Sheet::pairing(
            9,
            "Keyboard",
            Prompt::Display {
                passkey: 987654,
                entered: 2,
            },
        );
        let l = display.layout(&Rect::new(216.0, 52.0, 499.0, 573.0));
        assert!(l.field.is_none());
        assert_eq!(
            (
                display.press(&l, l.cancel.center()),
                display.press(&l, l.action.center())
            ),
            (Outcome::Cancel, Outcome::Cancel),
            "nothing to answer"
        );
    }

    #[test]
    fn the_resolution_sheet_takes_digits_and_sets_only_a_value_in_range() {
        let typed = |text: &str| {
            let mut s = Sheet::resolution("1:1:A", "A");
            for c in text.chars() {
                s.key(SheetKey::Type(c));
            }
            (s.field_text(), s.key(SheetKey::Return))
        };
        let set = |dpi| Outcome::Resolution {
            key: "1:1:A".into(),
            dpi,
        };
        assert_eq!(typed("12x00"), (Some("1200".into()), set(1200)));
        assert_eq!(typed("").1, Outcome::Stay);
        assert_eq!(typed("49").1, Outcome::Stay);
        assert_eq!(typed("32001").1, Outcome::Stay);
        assert_eq!(typed("32000").1, set(32_000));
        assert_eq!(typed("320000").0, Some("32000".into()), "no sixth digit");
        let mut s = Sheet::resolution("1:1:A", "A");
        s.key(SheetKey::Type('7'));
        s.key(SheetKey::Backspace);
        assert_eq!(s.field_text(), Some(String::new()));
        assert_eq!(s.key(SheetKey::Escape), Outcome::Cancel);
    }

    fn pictures_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Trips")).unwrap();
        for f in ["a.jpg", "b.png", "notes.txt"] {
            std::fs::write(d.path().join(f), b"x").unwrap();
        }
        d
    }

    /// Answer the chooser's requests here, as the window's threads do.
    fn settle(sheet: &mut Sheet) {
        while let Some(request) = sheet.picker_request() {
            sheet.apply_listing(request.run());
        }
    }

    fn picked(path: std::path::PathBuf, folder: bool) -> Outcome {
        Outcome::Picked { path, folder }
    }

    #[test]
    fn the_chooser_sheet_moves_with_the_keys_opens_folders_and_returns_the_picture() {
        let d = pictures_dir();
        let mut sheet = Sheet::picker(d.path(), Mode::Photo);
        assert_eq!(sheet.title(), "Add a Photo");
        assert_eq!(sheet.action_title(), "Add");
        assert_eq!(
            sheet.key(SheetKey::Return),
            Outcome::Stay,
            "nothing is highlighted"
        );
        assert_eq!(
            sheet.key(SheetKey::Choose),
            Outcome::Stay,
            "nothing to choose yet"
        );
        sheet.key(SheetKey::Down);
        assert_eq!(
            sheet.key(SheetKey::Choose),
            Outcome::Stay,
            "a folder is not a picture"
        );
        sheet.key(SheetKey::Return);
        settle(&mut sheet);
        assert_eq!(
            sheet.message(),
            d.path().join("Trips").display().to_string()
        );
        assert_eq!(sheet.key(SheetKey::Parent), Outcome::Stay);
        settle(&mut sheet);
        assert_eq!(sheet.message(), d.path().display().to_string());
        sheet.key(SheetKey::End);
        assert_eq!(
            sheet.key(SheetKey::Return),
            picked(d.path().join("b.png"), false)
        );
        assert_eq!(sheet.key(SheetKey::Escape), Outcome::Cancel);
        let mut folder = Sheet::picker(d.path(), Mode::Folder);
        assert_eq!(folder.action_title(), "Choose");
        assert_eq!(
            folder.key(SheetKey::Choose),
            picked(d.path().to_path_buf(), true)
        );
    }

    #[test]
    fn the_chooser_sheet_takes_presses_on_its_rows_up_and_the_buttons() {
        let d = pictures_dir();
        let content = Rect::new(0.0, 0.0, 600.0, 600.0);
        let mut sheet = Sheet::picker(&d.path().join("Trips"), Mode::Photo);
        let l = sheet.layout(&content);
        let (list, up) = (l.list.unwrap(), l.up.unwrap());
        assert_eq!(sheet.press(&l, up.center()), Outcome::Stay);
        settle(&mut sheet);
        assert_eq!(
            sheet.message(),
            d.path().display().to_string(),
            "Up opens the parent"
        );
        let row = |i: usize| Point {
            x: list.x + 10.0,
            y: list.y + (i as f64 + 0.5) * m::PICKER_ROW,
        };
        assert_eq!(
            sheet.press(&l, row(1)),
            Outcome::Stay,
            "a press highlights a row"
        );
        assert_eq!(
            sheet.press(&l, l.action.center()),
            picked(d.path().join("a.jpg"), false)
        );
        assert_eq!(
            sheet.press(&l, row(1)),
            picked(d.path().join("a.jpg"), false),
            "the second press on a picture chooses it"
        );
        assert_eq!(sheet.press(&l, l.cancel.center()), Outcome::Cancel);
        assert!(l.frame.w <= content.w && l.frame.bottom() <= content.bottom());
    }

    #[test]
    fn the_chooser_list_scrolls_with_the_wheel_and_shows_the_row_under_the_pointer() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..30 {
            std::fs::write(d.path().join(format!("p{i:02}.jpg")), b"x").unwrap();
        }
        let content = Rect::new(0.0, 0.0, 600.0, 600.0);
        let mut sheet = Sheet::picker(d.path(), Mode::Photo);
        let l = sheet.layout(&content);
        let list = l.list.unwrap();
        let at = |row: usize| Point {
            x: list.x + 10.0,
            y: list.y + (row as f64 + 0.5) * m::PICKER_ROW,
        };
        let outside = Point {
            x: list.x - 5.0,
            y: list.y,
        };
        assert!(
            !sheet.scroll(&l, outside, 40.0),
            "the wheel only scrolls over the list"
        );
        assert!(
            !sheet.scroll(&l, at(0), -40.0),
            "nothing above the first row"
        );
        assert!(sheet.scroll(&l, at(0), 2.0 * m::PICKER_ROW));
        assert!(sheet.hover(&l, Some(at(0))));
        assert!(
            !sheet.hover(&l, Some(at(0))),
            "the same row again changes nothing"
        );
        // the pointer is now on the third entry
        assert_eq!(sheet.press(&l, at(0)), Outcome::Stay);
        assert_eq!(
            sheet.key(SheetKey::Return),
            picked(d.path().join("p02.jpg"), false)
        );
        assert!(
            sheet.scroll(&l, at(0), 1000.0),
            "a long scroll stops at the last row"
        );
        assert!(!sheet.scroll(&l, at(0), 1000.0));
        assert_eq!(sheet.press(&l, at(m::PICKER_ROWS - 1)), Outcome::Stay);
        assert_eq!(
            sheet.key(SheetKey::Return),
            picked(d.path().join("p29.jpg"), false)
        );
        assert!(sheet.hover(&l, None), "the pointer left");
    }

    #[test]
    fn a_pending_chooser_shows_loading_until_its_listing_is_applied_and_ignores_a_stale_one() {
        let d = pictures_dir();
        let mut sheet = Sheet::picker_pending(d.path(), Mode::Photo);
        let Kind::Picker { chooser, .. } = &sheet.kind else {
            panic!("a chooser");
        };
        assert!(chooser.0.loading());
        assert_eq!(
            chooser.0.entries().len(),
            0,
            "the constructor read no folder"
        );
        let request = sheet
            .picker_request()
            .expect("the first listing is asked for");
        assert!(sheet.picker_request().is_none());
        // a key before the answer: the chooser stays responsive
        assert_eq!(sheet.key(SheetKey::Escape), Outcome::Cancel);
        sheet.apply_listing(request.run());
        let Kind::Picker { chooser, .. } = &sheet.kind else {
            panic!("a chooser");
        };
        assert!(!chooser.0.loading());
        assert_eq!(chooser.0.entries().len(), 3, "Trips, a.jpg, b.png");
    }
}
