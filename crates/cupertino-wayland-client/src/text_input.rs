//! Input methods (`zwp_text_input_v3`): the state of the focused text field, sent whenever its surface has the
//! text-input focus, and the composing, committed and deleted text the input method answers with, one batch per
//! `done`.

use std::fmt;

use cupertino_geometry::Rect;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    self, ChangeCause, ContentHint, ContentPurpose, ZwpTextInputV3,
};

use crate::connection::{round_out, State, SurfaceId};
use crate::Event;

/// The most bytes of surrounding text one request carries (the protocol's limit).
pub const MAX_SURROUNDING: usize = 4000;

/// Typed text in a `Debug` print: its length only, so logging an event never logs what was typed.
struct Redacted(usize);

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0)
    }
}

/// The plain text around the cursor of a text field, without its preedit, at most [`MAX_SURROUNDING`] bytes. Its
/// `Debug` shows the length, not the text.
#[derive(Clone, PartialEq, Eq)]
pub struct SurroundingText {
    text: String,
    cursor: usize,
    anchor: usize,
}

impl SurroundingText {
    /// `cursor` and `anchor` are byte offsets into `text`, equal without a selection. A longer text is cut to the
    /// window around the selection, or around the cursor when the selection alone is too long, at character boundaries.
    /// `None` when an offset is past the end or inside a character.
    pub fn new(text: &str, cursor: usize, anchor: usize) -> Option<SurroundingText> {
        if !text.is_char_boundary(cursor) || !text.is_char_boundary(anchor) {
            return None;
        }
        let (low, high) = (cursor.min(anchor), cursor.max(anchor));
        let (low, high) = if high - low <= MAX_SURROUNDING {
            (low, high)
        } else {
            (cursor, cursor)
        };
        let spare = MAX_SURROUNDING - (high - low);
        let mut end = (low.saturating_sub(spare / 2) + MAX_SURROUNDING).min(text.len());
        let mut start = end.saturating_sub(MAX_SURROUNDING);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Some(SurroundingText {
            text: text[start..end].to_owned(),
            cursor: cursor - start,
            anchor: anchor.clamp(start, end) - start,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The cursor's byte offset into [`SurroundingText::text`].
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selection anchor's byte offset; the cursor's without a selection.
    pub fn anchor(&self) -> usize {
        self.anchor
    }
}

impl fmt::Debug for SurroundingText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SurroundingText")
            .field("text", &Redacted(self.text.len()))
            .field("cursor", &self.cursor)
            .field("anchor", &self.anchor)
            .finish()
    }
}

/// What the input method is told about the focused text field. Protected content (a password or PIN purpose, or the
/// hidden-text or sensitive-data hint) never sends its text and always carries the sensitive-data hint.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInputState {
    /// `None` when the field does not know its text. Never sent for protected content (see [`TextInputState`]).
    pub surrounding: Option<SurroundingText>,
    /// Whether the last change came from the input method or from elsewhere (typing, the application).
    pub cause: ChangeCause,
    pub hint: ContentHint,
    pub purpose: ContentPurpose,
    /// Around the cursor, in surface-local logical points (rounded outwards), so candidates appear beside it; `None`
    /// when unknown.
    pub cursor_rect: Option<Rect>,
    /// The field dropped, or kept as typed, a composition the input method still holds: the state is sent after
    /// enabling afresh, which resets the input method. Sent once; the state is kept without it.
    pub reset: bool,
}

impl Default for TextInputState {
    fn default() -> Self {
        TextInputState {
            surrounding: None,
            cause: ChangeCause::Other,
            hint: ContentHint::None,
            purpose: ContentPurpose::Normal,
            cursor_rect: None,
            reset: false,
        }
    }
}

impl TextInputState {
    /// The state as sent: protected text is never shown to the input method, and its typing is never stored.
    fn guarded(&self) -> TextInputState {
        if matches!(self.purpose, ContentPurpose::Password | ContentPurpose::Pin)
            || self
                .hint
                .intersects(ContentHint::HiddenText | ContentHint::SensitiveData)
        {
            TextInputState {
                surrounding: None,
                hint: self.hint | ContentHint::SensitiveData,
                ..self.clone()
            }
        } else {
            self.clone()
        }
    }
}

/// Composing text the input method shows at the cursor, not yet part of the field. Its `Debug` shows the length, not
/// the text.
#[derive(Clone, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    /// The byte range of [`Preedit::text`] to show as the cursor (a caret when empty, a highlight otherwise); `None`
    /// hides the cursor.
    pub cursor: Option<(usize, usize)>,
}

/// One batch from the input method. Apply it in this order: remove the shown preedit, delete around the cursor, insert
/// the commit with the cursor after it, send the new state, then show the new preedit. Its `Debug` shows lengths, not
/// text.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TextInputDone {
    pub preedit: Option<Preedit>,
    pub commit: Option<String>,
    /// Bytes to delete before and after the cursor, the selection excluded; clamp them to character boundaries.
    pub delete: (u32, u32),
    /// `false` when the input method answered an older state than the latest one sent: apply it all the same.
    pub current: bool,
}

impl fmt::Debug for Preedit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Preedit")
            .field("text", &Redacted(self.text.len()))
            .field("cursor", &self.cursor)
            .finish()
    }
}

impl fmt::Debug for TextInputDone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextInputDone")
            .field("preedit", &self.preedit)
            .field("commit", &self.commit.as_ref().map(|c| Redacted(c.len())))
            .field("delete", &self.delete)
            .field("current", &self.current)
            .finish()
    }
}

/// Input method activity on a surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The surface has the text-input focus; the state set for it is sent now.
    Enter,
    /// The surface lost the text-input focus; drop any shown preedit.
    Leave,
    Done(TextInputDone),
}

#[derive(Debug, Default)]
pub(crate) struct TextInput {
    object: Option<ZwpTextInputV3>,
    focus: Option<SurfaceId>,
    wanted: Vec<(SurfaceId, TextInputState)>,
    /// What the compositor holds as of the last commit; `None` while disabled.
    sent: Option<TextInputState>,
    commits: u32,
    pending: TextInputDone,
}

impl TextInput {
    pub(crate) fn new(
        manager: Option<&ZwpTextInputManagerV3>,
        seat: Option<&WlSeat>,
        qh: &QueueHandle<State>,
    ) -> Self {
        TextInput {
            object: manager.zip(seat).map(|(m, s)| m.get_text_input(s, qh, ())),
            ..TextInput::default()
        }
    }

    /// The state for `surface`'s focused field, `None` when no field there takes text.
    pub(crate) fn set(&mut self, surface: SurfaceId, state: Option<&TextInputState>) {
        self.wanted.retain(|(s, _)| *s != surface);
        if let Some(state) = state {
            self.wanted.push((surface, state.guarded()));
        }
        if self.focus == Some(surface) {
            self.sync();
        }
    }

    /// The surface is destroyed.
    pub(crate) fn forget(&mut self, surface: SurfaceId) {
        self.set(surface, None);
        if self.focus == Some(surface) {
            self.focus = None;
        }
    }

    /// Send what the focused surface wants, when it differs from what the compositor holds.
    fn sync(&mut self) {
        let Some(object) = &self.object else {
            return;
        };
        let wanted = self
            .focus
            .and_then(|f| self.wanted.iter().find(|(s, _)| *s == f))
            .map(|(_, state)| state);
        match (&self.sent, wanted) {
            (None, None) => return,
            (Some(sent), Some(wanted)) if sent == wanted && !wanted.reset => return,
            (Some(_), None) => object.disable(),
            (sent, Some(wanted)) => {
                // a value once sent can only be withdrawn by enabling afresh
                let withdrawn = sent.as_ref().is_some_and(|s| {
                    s.surrounding.is_some() && wanted.surrounding.is_none()
                        || s.cursor_rect.is_some() && wanted.cursor_rect.is_none()
                });
                if sent.is_none() || withdrawn || wanted.reset {
                    object.enable();
                }
                if let Some(s) = &wanted.surrounding {
                    object.set_surrounding_text(s.text.clone(), s.cursor as i32, s.anchor as i32);
                }
                object.set_text_change_cause(wanted.cause);
                object.set_content_type(wanted.hint, wanted.purpose);
                if let Some(r) = &wanted.cursor_rect {
                    let (x, y, width, height) = round_out(r);
                    object.set_cursor_rectangle(x, y, width, height);
                }
            }
        }
        object.commit();
        self.commits = self.commits.wrapping_add(1);
        let sent = wanted.cloned().map(|state| TextInputState {
            reset: false,
            ..state
        });
        // a reset is sent once: what stays wanted is the state itself
        if let Some((_, wanted)) = self
            .focus
            .and_then(|f| self.wanted.iter_mut().find(|(s, _)| *s == f))
        {
            wanted.reset = false;
        }
        self.sent = sent;
    }
}

/// The byte range `[begin, end)` of a preedit cursor when it lies on character boundaries of `text`; the protocol hides
/// the cursor with -1, and any other range the client cannot place hides it too.
fn preedit_cursor(text: &str, begin: i32, end: i32) -> Option<(usize, usize)> {
    let (begin, end) = (usize::try_from(begin).ok()?, usize::try_from(end).ok()?);
    (begin <= end && text.is_char_boundary(begin) && text.is_char_boundary(end))
        .then_some((begin, end))
}

impl Dispatch<ZwpTextInputV3, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let entered = match &event {
            zwp_text_input_v3::Event::Enter { surface } => state.surface_of(surface),
            _ => None,
        };
        let input = &mut state.text_input;
        let focus = input.focus;
        let found = match event {
            zwp_text_input_v3::Event::Enter { .. } => {
                input.focus = entered;
                input.sync();
                input.focus.map(|s| (s, TextInputEvent::Enter))
            }
            zwp_text_input_v3::Event::Leave { .. } => {
                input.focus = None;
                input.sent = None;
                input.pending = TextInputDone::default();
                focus.map(|s| (s, TextInputEvent::Leave))
            }
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                input.pending.preedit = text.filter(|t| !t.is_empty()).map(|text| Preedit {
                    cursor: preedit_cursor(&text, cursor_begin, cursor_end),
                    text,
                });
                None
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                input.pending.commit = text.filter(|t| !t.is_empty());
                None
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                input.pending.delete = (before_length, after_length);
                None
            }
            zwp_text_input_v3::Event::Done { serial } => {
                let done = TextInputDone {
                    current: serial == input.commits,
                    ..std::mem::take(&mut input.pending)
                };
                focus.map(|s| (s, TextInputEvent::Done(done)))
            }
            _ => None,
        };
        if let Some((surface, event)) = found {
            state.events.push(Event::TextInput { surface, event });
        }
    }
}

delegate_noop!(State: ZwpTextInputManagerV3);

#[cfg(test)]
mod tests {
    use super::*;

    fn window(text: &str, cursor: usize, anchor: usize) -> (String, usize, usize) {
        let s = SurroundingText::new(text, cursor, anchor).unwrap();
        assert!(s.text.len() <= MAX_SURROUNDING);
        (s.text.clone(), s.cursor, s.anchor)
    }

    #[test]
    fn a_short_text_is_sent_whole_with_its_offsets() {
        assert_eq!(window("héllo", 3, 1), ("héllo".into(), 3, 1));
    }

    #[test]
    fn an_offset_inside_a_character_or_past_the_end_is_refused() {
        assert_eq!(SurroundingText::new("é", 1, 1), None);
        assert_eq!(SurroundingText::new("ab", 3, 3), None);
    }

    #[test]
    fn a_long_text_is_cut_to_the_limit_around_the_selection() {
        let text = format!("{}[sel]{}", "a".repeat(6000), "b".repeat(6000));
        let (cut, cursor, anchor) = window(&text, 6005, 6000);
        assert_eq!(cut.len(), MAX_SURROUNDING);
        assert_eq!(&cut[anchor..cursor], "[sel]");
        assert!(cut.starts_with('a') && cut.ends_with('b'));
    }

    #[test]
    fn a_cursor_near_either_end_keeps_the_whole_budget() {
        let text = "x".repeat(9000);
        assert_eq!(window(&text, 10, 10).1, 10);
        let (cut, cursor, _) = window(&text, 8990, 8990);
        assert_eq!((cut.len(), cursor), (MAX_SURROUNDING, MAX_SURROUNDING - 10));
    }

    #[test]
    fn a_selection_longer_than_the_limit_keeps_the_cursor_side() {
        let text = "y".repeat(10_000);
        assert_eq!(
            window(&text, 9000, 100),
            ("y".repeat(MAX_SURROUNDING), 3000, 0)
        );
        assert_eq!(window(&text, 100, 9000).2, MAX_SURROUNDING);
    }

    #[test]
    fn the_cut_never_splits_a_character() {
        let text = "é".repeat(5000);
        let (cut, cursor, anchor) = window(&text, 5002, 5000);
        assert_eq!(cut, "é".repeat(1999));
        assert_eq!((cursor, anchor), (2000, 1998));
    }

    #[test]
    fn password_and_pin_content_hide_the_text_and_mark_it_sensitive() {
        for purpose in [ContentPurpose::Password, ContentPurpose::Pin] {
            let state = TextInputState {
                surrounding: SurroundingText::new("hunter2", 7, 7),
                purpose,
                ..TextInputState::default()
            }
            .guarded();
            assert_eq!(state.surrounding, None);
            assert!(state.hint.contains(ContentHint::SensitiveData));
        }
        let plain = TextInputState {
            surrounding: SurroundingText::new("hello", 5, 5),
            ..TextInputState::default()
        };
        assert_eq!(plain.guarded(), plain);
    }

    #[test]
    fn hidden_or_sensitive_hints_protect_any_purpose() {
        for hint in [ContentHint::HiddenText, ContentHint::SensitiveData] {
            let state = TextInputState {
                surrounding: SurroundingText::new("123", 3, 3),
                hint,
                ..TextInputState::default()
            }
            .guarded();
            assert_eq!(state.surrounding, None);
            assert!(state.hint.contains(hint | ContentHint::SensitiveData));
        }
    }

    #[test]
    fn debug_prints_never_show_typed_text() {
        let done = TextInputDone {
            preedit: Some(Preedit {
                text: "secret".into(),
                cursor: Some((0, 6)),
            }),
            commit: Some("hunter2".into()),
            ..TextInputDone::default()
        };
        let state = TextInputState {
            surrounding: SurroundingText::new("swordfish", 9, 9),
            ..TextInputState::default()
        };
        let printed = format!(
            "{:?} {:?} {state:?}",
            TextInputEvent::Done(done),
            state.guarded()
        );
        for secret in ["secret", "hunter2", "swordfish"] {
            assert!(!printed.contains(secret), "{printed}");
        }
        assert!(
            printed.contains("<6 bytes>")
                && printed.contains("<7 bytes>")
                && printed.contains("<9 bytes>")
        );
    }

    #[test]
    fn a_preedit_cursor_off_a_character_boundary_is_hidden() {
        assert_eq!(preedit_cursor("añb", 1, 3), Some((1, 3)));
        assert_eq!(preedit_cursor("añb", 2, 2), None);
        assert_eq!(preedit_cursor("añb", 2, 3), None);
        assert_eq!(preedit_cursor("añb", 1, 2), None);
        assert_eq!(preedit_cursor("ab", -1, -1), None);
        assert_eq!(preedit_cursor("ab", 2, 1), None);
        assert_eq!(preedit_cursor("ab", 0, 3), None);
    }
}
