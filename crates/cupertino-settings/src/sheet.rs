//! The window's sheets (§6.4, §7.2): the Wi-Fi passphrase, the Bluetooth Forget confirmation and the pairing agent's
//! prompts. A sheet covers the
//! content column; it takes the keyboard while it is up. Its look and metrics are approximate (the platform sheet,
//! slides; here it appears at once).

use zeroize::{Zeroize, Zeroizing};

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

/// What a sheet asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The passphrase of a secured network that is not saved. The passphrase is never stored by the application.
    Passphrase { ssid: String, text: Secret, failure: Option<String> },
    /// Forget a Bluetooth device: Cancel is the default, "Forget Device" is destructive (wording approximate).
    Forget { id: String, name: String },
    /// A question of the Bluetooth daemon to the pairing agent (`org.cupertino.Bluetooth1.Agent`); `id` is the pairing.
    /// The wording is approximate (the design's pairing dialog is not known).
    Pairing { id: u64, device: String, prompt: Prompt, text: Secret },
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

/// An open sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sheet {
    pub kind: Kind,
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
}

impl Sheet {
    pub fn passphrase(ssid: &str) -> Self {
        Sheet { kind: Kind::Passphrase { ssid: ssid.to_string(), text: Secret::new(), failure: None } }
    }

    pub fn forget(id: &str, name: &str) -> Self {
        Sheet { kind: Kind::Forget { id: id.to_string(), name: name.to_string() } }
    }

    pub fn pairing(id: u64, device: &str, prompt: Prompt) -> Self {
        Sheet { kind: Kind::Pairing { id, device: device.to_string(), prompt, text: Secret::new() } }
    }

    /// The pairing this sheet answers, if it is a pairing prompt.
    pub fn pairing_id(&self) -> Option<u64> {
        match &self.kind {
            Kind::Pairing { id, .. } => Some(*id),
            Kind::Passphrase { .. } | Kind::Forget { .. } => None,
        }
    }

    pub fn title(&self) -> String {
        match &self.kind {
            Kind::Passphrase { ssid, .. } => format!("Enter the password for \u{201c}{ssid}\u{201d}"),
            Kind::Forget { name, .. } => format!("Forget \u{201c}{name}\u{201d}?"),
            Kind::Pairing { device, prompt: Prompt::Confirm { .. }, .. } => format!("Pair with \u{201c}{device}\u{201d}?"),
            Kind::Pairing { device, prompt: Prompt::Request, .. } => format!("Enter the code shown on \u{201c}{device}\u{201d}"),
            Kind::Pairing { device, prompt: Prompt::Display { .. }, .. } => format!("Pairing with \u{201c}{device}\u{201d}"),
        }
    }

    pub fn message(&self) -> String {
        match &self.kind {
            Kind::Passphrase { failure: Some(f), .. } => f.clone(),
            Kind::Passphrase { .. } => "The network needs a password to join.".to_string(),
            Kind::Forget { .. } => "The device will need to be paired again to be used.".to_string(),
            Kind::Pairing { prompt: Prompt::Confirm { passkey: Some(p) }, .. } => format!("Check that the device shows {p:06}."),
            Kind::Pairing { prompt: Prompt::Confirm { passkey: None }, .. } => "The device asks to pair with this computer.".to_string(),
            Kind::Pairing { prompt: Prompt::Request, .. } => "Type the six-digit code.".to_string(),
            Kind::Pairing { prompt: Prompt::Display { passkey, entered }, .. } => format!("Type {passkey:06} on the device, then press Return on it. {entered} of 6 digits typed."),
        }
    }

    pub fn action_title(&self) -> &'static str {
        match &self.kind {
            Kind::Passphrase { .. } => "Join",
            Kind::Forget { .. } => "Forget Device",
            Kind::Pairing { prompt: Prompt::Confirm { .. } | Prompt::Request, .. } => "Pair",
            // the daemon asks nothing back: the sheet only goes away
            Kind::Pairing { prompt: Prompt::Display { .. }, .. } => "OK",
        }
    }

    /// The text of the sheet's field: bullets for the typed passphrase, the digits of a passkey; `None` without a field.
    pub fn field_text(&self) -> Option<String> {
        match &self.kind {
            Kind::Passphrase { text, .. } => Some("\u{2022}".repeat(text.chars().count())),
            Kind::Pairing { prompt: Prompt::Request, text, .. } => Some(text.to_string()),
            Kind::Forget { .. } | Kind::Pairing { .. } => None,
        }
    }

    /// The sheet's call: Join with the passphrase (moved out of the field, not copied), the confirmed removal, or the
    /// pairing answer.
    fn action(&mut self) -> Outcome {
        match &mut self.kind {
            Kind::Passphrase { text, .. } if text.is_empty() => Outcome::Stay,
            Kind::Passphrase { ssid, text, .. } => Outcome::Do(Call::Join { ssid: ssid.clone(), passphrase: Some(std::mem::take(text)) }),
            Kind::Forget { id, .. } => Outcome::Do(Call::Remove { id: id.clone(), confirmed: true }),
            Kind::Pairing { prompt: Prompt::Confirm { .. }, .. } => Outcome::Answer(Answer::Confirm),
            Kind::Pairing { prompt: Prompt::Request, text, .. } if text.len() < PASSKEY_DIGITS => Outcome::Stay,
            Kind::Pairing { prompt: Prompt::Request, text, .. } => Outcome::Answer(Answer::Passkey(text.parse().expect("six decimal digits"))),
            Kind::Pairing { prompt: Prompt::Display { .. }, .. } => Outcome::Cancel,
        }
    }

    /// Cancel: a pairing question is refused; every other sheet just closes.
    fn cancel(&self) -> Outcome {
        match &self.kind {
            Kind::Pairing { prompt: Prompt::Confirm { .. } | Prompt::Request, .. } => Outcome::Answer(Answer::Reject),
            Kind::Passphrase { .. } | Kind::Forget { .. } | Kind::Pairing { prompt: Prompt::Display { .. }, .. } => Outcome::Cancel,
        }
    }

    /// A typed character, Backspace, Return or Escape. Return is the default button: Join or Pair, or Cancel when
    /// forgetting.
    pub fn key(&mut self, key: SheetKey) -> Outcome {
        match (key, &mut self.kind) {
            (SheetKey::Escape, _) | (SheetKey::Return, Kind::Forget { .. }) => self.cancel(),
            (SheetKey::Return, Kind::Passphrase { .. } | Kind::Pairing { .. }) => self.action(),
            (SheetKey::Type(c), Kind::Passphrase { text, .. }) => {
                if text.len() + c.len_utf8() <= SECRET_BYTES {
                    text.push(c);
                }
                Outcome::Stay
            }
            (SheetKey::Type(c), Kind::Pairing { prompt: Prompt::Request, text, .. }) => {
                if c.is_ascii_digit() && text.len() < PASSKEY_DIGITS {
                    text.push(c);
                }
                Outcome::Stay
            }
            (SheetKey::Backspace, Kind::Passphrase { text, .. } | Kind::Pairing { prompt: Prompt::Request, text, .. }) => {
                text.pop();
                Outcome::Stay
            }
            (SheetKey::Type(_) | SheetKey::Backspace, Kind::Forget { .. } | Kind::Pairing { .. }) => Outcome::Stay,
        }
    }

    /// The daemon refused the passphrase: the sheet stays with the error and an empty field.
    pub fn failed(&mut self, error: &str) {
        if let Kind::Passphrase { text, failure, .. } = &mut self.kind {
            // zeroes the typed bytes, not only the length
            text.zeroize();
            *failure = Some(error.to_string());
        }
    }

    /// The layout of the sheet centred in the content column `content`, near its top.
    pub fn layout(&self, content: &Rect) -> SheetLayout {
        let field = self.field_text().is_some();
        let h = m::PADDING * 2.0 + m::TITLE_HEIGHT + m::GAP / 2.0 + m::MESSAGE_HEIGHT + if field { m::FIELD_HEIGHT + m::GAP } else { 0.0 } + m::GAP + m::BUTTON.1;
        let frame = Rect::new(content.x + (content.w - m::WIDTH) / 2.0, content.y + m::PADDING, m::WIDTH, h);
        let inner = frame.inset(m::PADDING, m::PADDING);
        let title = Rect::new(inner.x, inner.y, inner.w, m::TITLE_HEIGHT);
        let message = Rect::new(inner.x, title.bottom() + m::GAP / 2.0, inner.w, m::MESSAGE_HEIGHT);
        let field_rect = field.then(|| Rect::new(inner.x, message.bottom(), inner.w, m::FIELD_HEIGHT));
        let by = inner.bottom() - m::BUTTON.1;
        let action = Rect::new(inner.right() - m::BUTTON.0, by, m::BUTTON.0, m::BUTTON.1);
        let cancel = Rect::new(action.x - m::GAP - m::BUTTON.0, by, m::BUTTON.0, m::BUTTON.1);
        SheetLayout { frame, title, message, field: field_rect, cancel, action }
    }

    /// A press at `p` in a sheet laid out as `layout`.
    pub fn press(&mut self, layout: &SheetLayout, p: Point) -> Outcome {
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
            assert!(!shown.contains("hunter22") && !shown.contains("482916") && shown.contains("[secret]"), "{shown}");
        }
        let call = s.key(SheetKey::Return);
        let shown = format!("{call:?}");
        assert!(!shown.contains("hunter22") && shown.contains("[secret]"), "{shown}");
        assert_eq!(s.field_text().as_deref(), Some(""), "the passphrase moved into the call");
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
        assert_eq!(s.key(SheetKey::Return), Outcome::Do(Call::Join { ssid: "office".into(), passphrase: Some(typed("secre")) }));
        s.failed("The password is not correct.");
        assert_eq!((s.message().as_str(), s.field_text().as_deref()), ("The password is not correct.", Some("")));
        assert_eq!(s.key(SheetKey::Escape), Outcome::Cancel);
    }

    #[test]
    fn forget_needs_its_destructive_button_and_return_cancels() {
        let mut s = Sheet::forget("dev-1", "Headphones");
        let l = s.layout(&Rect::new(216.0, 52.0, 499.0, 573.0));
        assert_eq!(l.frame.w, 340.0);
        assert!(l.field.is_none());
        assert_eq!(s.press(&l, l.cancel.center()), Outcome::Cancel);
        assert_eq!(s.press(&l, l.action.center()), Outcome::Do(Call::Remove { id: "dev-1".into(), confirmed: true }));
        assert_eq!(s.clone().key(SheetKey::Return), Outcome::Cancel, "Cancel is the default");
        assert_eq!(s.action_title(), "Forget Device");
    }

    #[test]
    fn pairing_prompts_answer_the_agent_and_cancel_refuses() {
        let mut confirm = Sheet::pairing(7, "Keyboard", Prompt::Confirm { passkey: Some(42) });
        assert_eq!(confirm.message(), "Check that the device shows 000042.");
        assert_eq!(confirm.pairing_id(), Some(7));
        assert_eq!(confirm.key(SheetKey::Return), Outcome::Answer(Answer::Confirm));
        assert_eq!(confirm.key(SheetKey::Escape), Outcome::Answer(Answer::Reject));
        let mut request = Sheet::pairing(8, "Keyboard", Prompt::Request);
        for c in "12a34567".chars() {
            request.key(SheetKey::Type(c));
        }
        assert_eq!(request.field_text().as_deref(), Some("123456"), "digits only, six at most");
        request.key(SheetKey::Backspace);
        assert_eq!(request.key(SheetKey::Return), Outcome::Stay, "five digits are not a passkey");
        request.key(SheetKey::Type('0'));
        assert_eq!(request.key(SheetKey::Return), Outcome::Answer(Answer::Passkey(123450)));
        let mut display = Sheet::pairing(9, "Keyboard", Prompt::Display { passkey: 987654, entered: 2 });
        let l = display.layout(&Rect::new(216.0, 52.0, 499.0, 573.0));
        assert!(l.field.is_none());
        assert_eq!((display.press(&l, l.cancel.center()), display.press(&l, l.action.center())), (Outcome::Cancel, Outcome::Cancel), "nothing to answer");
    }
}
