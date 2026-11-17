//! What the input method may learn: debug output that never prints the composed or surrounding text, the bounded
//! window around the caret, and the state a session sends.

use std::fmt;

use cupertino_geometry::Rect;
use unicode_segmentation::GraphemeCursor;

use crate::{
    ChangeCause, ContentHints, ImeBatch, Preedit, Selection, Surrounding, TextInputSession,
    TextInputState,
};

/// The most bytes of text sent around the caret (the transport cuts at 4000; this keeps both sides inside it).
pub const SURROUNDING_MAX_BYTES: usize = 4000;

impl fmt::Debug for Preedit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Preedit")
            .field("text_len", &self.text.len())
            .field("cursor", &self.cursor)
            .finish()
    }
}

impl fmt::Debug for ImeBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImeBatch")
            .field("preedit", &self.preedit)
            .field("commit_len", &self.commit.as_ref().map(String::len))
            .field("delete", &self.delete)
            .field("current", &self.current)
            .finish()
    }
}

impl fmt::Debug for Surrounding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Surrounding")
            .field("text_len", &self.text.len())
            .field("cursor", &self.cursor)
            .field("anchor", &self.anchor)
            .finish()
    }
}

impl Surrounding {
    /// The part of `text` within `max_bytes` around `cursor` and `anchor` (byte offsets on grapheme boundaries),
    /// cut at grapheme boundaries, with both offsets relative to the window. A selection that fits stays whole; a
    /// longer one is cut around the cursor.
    pub fn around(text: &str, cursor: usize, anchor: usize, max_bytes: usize) -> Surrounding {
        let (low, high) = (cursor.min(anchor), cursor.max(anchor));
        let slack = max_bytes.saturating_sub(high - low);
        let first = if high - low > max_bytes {
            cursor.saturating_sub(max_bytes / 2)
        } else {
            low.saturating_sub(slack / 2)
        };
        let last = (first + max_bytes).min(text.len());
        let first = last.saturating_sub(max_bytes).min(first);
        let start = boundary_at_or_after(text, first);
        let end = boundary_at_or_before(text, last).max(start);
        Surrounding {
            text: text[start..end].to_string(),
            cursor: cursor.clamp(start, end) - start,
            anchor: anchor.clamp(start, end) - start,
        }
    }
}

/// The first grapheme boundary at or after `at`, found from `at` alone: the cost does not grow with the text.
fn boundary_at_or_after(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    let mut cursor = GraphemeCursor::new(at, text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(true) => at,
        _ => cursor
            .next_boundary(text, 0)
            .ok()
            .flatten()
            .unwrap_or(text.len()),
    }
}

/// The last grapheme boundary at or before `at`, found from `at` alone.
fn boundary_at_or_before(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let mut cursor = GraphemeCursor::new(at, text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(true) => at,
        _ => cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0),
    }
}

impl TextInputSession {
    /// The state to send. A protected session sends no text and no caret: the input method gets the field's own
    /// rectangle so its candidate window still has a place, and learns nothing about the secret.
    pub fn state(
        &self,
        surrounding: Option<Surrounding>,
        cause: ChangeCause,
        caret_rect: Option<Rect>,
        field_rect: Option<Rect>,
    ) -> TextInputState {
        let (surrounding, cursor_rect) = if self.protected {
            (None, field_rect)
        } else {
            (surrounding, caret_rect)
        };
        TextInputState {
            surrounding,
            purpose: self.purpose,
            hints: self.hints,
            cause,
            cursor_rect,
            reset: false,
        }
    }

    /// Whether a batch answers the field's latest state, so the field answers it with its new one; a stale batch still
    /// applies (the input method answers the newer state next).
    pub fn accepts(&self, batch: &ImeBatch) -> bool {
        batch.current
    }

    /// The content hints of a protected field: hidden, sensitive text, without completion, spell checking or
    /// automatic capitalisation, so nothing is predicted, learned or stored.
    pub fn protected_hints() -> ContentHints {
        ContentHints {
            hidden_text: true,
            sensitive_data: true,
            ..ContentHints::default()
        }
    }

    /// The selection the session holds, as the buffer reports it.
    pub fn select(&mut self, selection: Selection) {
        if self.selection != selection {
            self.selection = selection;
            self.revision += 1;
        }
    }
}
