//! The window's sheets (§6.4, §7.2): the Wi-Fi passphrase, the Bluetooth Forget confirmation and the pairing agent's
//! prompts. A sheet covers the
//! content column; it takes the keyboard while it is up. Its look and metrics are approximate (the platform sheet,
//! slides; here it appears at once).

use zeroize::{Zeroize, Zeroizing};

use cupertino_defaults::compositor::MOUSE_DPI_RANGE;

use cupertino_kit::typeahead::TypeAhead;

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
    /// The applications sheet: its width, a row of its list and the rows shown. approximate.
    pub const PICKER_WIDTH: f64 = 460.0;
    pub const PICKER_ROW: f64 = 22.0;
    pub const PICKER_ROWS: usize = 12;
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
        /// The network as a label.
        name: String,
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
    /// The application picked to allow to use the clipboard, by its program; the sheet stays until it is allowed.
    Program(std::path::PathBuf),
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
    /// The applications sheet's list of rows.
    pub list: Option<Rect>,
}

impl Sheet {
    /// The passphrase of network `ssid` (the service's key), shown as `name`.
    pub fn passphrase(ssid: &str, name: &str) -> Self {
        Sheet {
            kind: Kind::Passphrase {
                ssid: ssid.to_string(),
                name: name.to_string(),
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
            | Kind::Apps { .. } => None,
        }
    }

    pub fn title(&self) -> String {
        match &self.kind {
            Kind::Passphrase { name, .. } => {
                format!("Enter the password for \u{201c}{name}\u{201d}")
            }
            Kind::Forget {
                what: Forget::Device(_),
                name,
            } => format!("Forget \u{201c}{name}\u{201d}?"),
            Kind::Forget {
                what: Forget::Clipboard(_),
                name,
            } => format!("Remove \u{201c}{name}\u{201d} from Clipboard Access?"),
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
            Kind::Apps {
                failure: Some(f), ..
            } => f.clone(),
            Kind::Apps { .. } => "It can read and change what you copy at any time.".to_string(),
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
            Kind::Forget { .. } | Kind::Pairing { .. } | Kind::Apps { .. } => None,
        }
    }

    /// The sheet's call: Join with the passphrase (moved out of the field, not copied), the confirmed removal, or the
    /// pairing answer.
    pub fn action(&mut self) -> Outcome {
        match &mut self.kind {
            Kind::Passphrase { text, .. } if text.is_empty() => Outcome::Stay,
            Kind::Passphrase { ssid, text, .. } => Outcome::Do(Call::Join {
                ssid: ssid.clone(),
                passphrase: Some(std::mem::take(text)),
            }),
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
    pub fn cancel(&self) -> Outcome {
        match &self.kind {
            Kind::Pairing {
                prompt: Prompt::Confirm { .. } | Prompt::Request,
                ..
            } => Outcome::Answer(Answer::Reject),
            Kind::Passphrase { .. }
            | Kind::Forget { .. }
            | Kind::Resolution { .. }
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

    /// [`Sheet::key`] at `now` seconds: the applications list's type-to-select forgets the typed text after a pause.
    pub fn key_at(&mut self, key: SheetKey, now: f64) -> Outcome {
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
                SheetKey::Return | SheetKey::Backspace => None,
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
            (SheetKey::Return | SheetKey::Type(_) | SheetKey::Backspace, Kind::Apps { .. }) => {
                Outcome::Stay
            }
            (
                SheetKey::Type(_) | SheetKey::Backspace,
                Kind::Forget { .. } | Kind::Pairing { .. },
            ) => Outcome::Stay,
            (SheetKey::Up | SheetKey::Down | SheetKey::Home | SheetKey::End, _) => Outcome::Stay,
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
            Kind::Apps { failure, .. } => *failure = Some(error.to_string()),
            _ => {}
        }
    }

    /// The layout of the sheet centred in the content column `content`, near its top.
    pub fn layout(&self, content: &Rect) -> SheetLayout {
        if let Kind::Apps { .. } = self.kind {
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
        }
    }

    /// The applications sheet: the title, the message under it, the list, then the buttons.
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
        let title = Rect::new(inner.x, inner.y, inner.w, m::TITLE_HEIGHT);
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
        }
    }

    /// The pointer is at `p` (`None`: it left): the applications sheet's row under it shows its hover. `true` when that changed.
    pub fn hover(&mut self, layout: &SheetLayout, p: Option<Point>) -> bool {
        let (rows, top, hovered) = match &mut self.kind {
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

    /// The wheel moved by `dy` points with the pointer at `p`: the applications sheet's list scrolls when `p` is over it. `true`
    /// when it moved.
    pub fn scroll(&mut self, layout: &SheetLayout, p: Point, dy: f64) -> bool {
        let (count, top) = match &mut self.kind {
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

    /// The field now holds `typed`, as far as the sheet takes it: digits only where it asks for a number, and no more
    /// than its field holds.
    pub fn set_text(&mut self, typed: &str) {
        match &mut self.kind {
            Kind::Passphrase { text, .. } | Kind::Pairing { text, .. } => text.zeroize(),
            Kind::Resolution { text, .. } => text.clear(),
            Kind::Forget { .. } | Kind::Apps { .. } => return,
        }
        for c in typed.chars() {
            self.key(SheetKey::Type(c));
        }
    }

    /// The rows of the applications list, and the selected one; `None` for a sheet without a list.
    pub fn rows(&self) -> Option<(Vec<String>, Option<usize>)> {
        match &self.kind {
            Kind::Apps { apps, selected, .. } => {
                Some((apps.iter().map(|a| a.name.clone()).collect(), *selected))
            }
            _ => None,
        }
    }

    /// A press on row `row` of the list: it selects, and a press on the selected row allows the application (the
    /// second press of a double click).
    pub fn press_row(&mut self, row: usize) -> Outcome {
        match &mut self.kind {
            Kind::Apps { apps, selected, .. } => {
                if apps.get(row).is_some_and(|a| a.reason.is_none()) {
                    if *selected == Some(row) {
                        return self.action();
                    }
                    *selected = Some(row);
                }
                Outcome::Stay
            }
            _ => Outcome::Stay,
        }
    }

    /// A press at `p` in a sheet laid out as `layout`.
    pub fn press(&mut self, layout: &SheetLayout, p: Point) -> Outcome {
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
    /// The applications list's keys: the arrows, Home and End.
    Up,
    Down,
    Home,
    End,
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
