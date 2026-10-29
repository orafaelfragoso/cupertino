//! What the input method may learn: debug output that never prints the composed or surrounding text, the bounded
//! window around the caret, and the state a session sends.

use std::fmt;

use cupertino_geometry::Rect;
use unicode_segmentation::GraphemeCursor;

use crate::{
    ChangeCause, ImeBatch, Preedit, Selection, Surrounding, TextInputSession, TextInputState,
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
        }
    }

    /// Whether a batch answers the field's latest state; a stale one resets the composition instead of applying.
    pub fn accepts(&self, batch: &ImeBatch) -> bool {
        batch.current
    }

    /// The selection the session holds, as the buffer reports it.
    pub fn select(&mut self, selection: Selection) {
        if self.selection != selection {
            self.selection = selection;
            self.revision += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentHints, ContentPurpose};

    fn session(protected: bool) -> TextInputSession {
        TextInputSession {
            revision: 0,
            purpose: ContentPurpose::Password,
            hints: ContentHints::default(),
            selection: Selection::default(),
            preedit: None,
            protected,
        }
    }

    fn rect(x: f64) -> Rect {
        Rect::new(x, 0.0, 10.0, 10.0)
    }

    #[test]
    fn debug_of_composed_text_prints_lengths_only() {
        let batch = ImeBatch {
            preedit: Some(Preedit {
                text: "secret-preedit".into(),
                cursor: Some(0..1),
            }),
            commit: Some("secret-commit".into()),
            delete: (0, 0),
            current: true,
        };
        let text = format!("{batch:?} {:?}", session(false).preedit);
        assert!(!text.contains("secret"), "{text}");
        assert!(text.contains("commit_len: Some(13)"), "{text}");
        let surrounding = Surrounding {
            text: "secret-around".into(),
            cursor: 0,
            anchor: 0,
        };
        assert!(!format!("{surrounding:?}").contains("secret"));
    }

    #[test]
    fn a_protected_session_sends_the_field_rect_and_no_text() {
        let around = Surrounding::around("hunter2", 7, 7, 100);
        let state = session(true).state(
            Some(around),
            ChangeCause::Other,
            Some(rect(40.0)),
            Some(rect(0.0)),
        );
        assert_eq!(state.surrounding, None);
        assert_eq!(state.cursor_rect, Some(rect(0.0)));
        assert_eq!(state.purpose, ContentPurpose::Password);
    }

    #[test]
    fn an_unprotected_session_sends_the_caret_rect() {
        let around = Surrounding::around("abc", 1, 1, 100);
        let state = session(false).state(
            Some(around.clone()),
            ChangeCause::InputMethod,
            Some(rect(40.0)),
            Some(rect(0.0)),
        );
        assert_eq!(state.surrounding, Some(around));
        assert_eq!(state.cursor_rect, Some(rect(40.0)));
        assert_eq!(state.cause, ChangeCause::InputMethod);
    }

    #[test]
    fn a_window_keeps_short_text_whole() {
        let around = Surrounding::around("héllo", 3, 3, 100);
        assert_eq!(
            around,
            Surrounding {
                text: "héllo".into(),
                cursor: 3,
                anchor: 3
            }
        );
    }

    #[test]
    fn a_window_stays_inside_the_bound_on_grapheme_boundaries() {
        let text = "👍🏽".repeat(500);
        let cursor = text.len() / 2;
        let around = Surrounding::around(&text, cursor, cursor, 100);
        assert!(around.text.len() <= 100, "{}", around.text.len());
        assert!(around.text.len() >= 90);
        assert!(
            around.text.chars().count().is_multiple_of(2),
            "cut inside a grapheme"
        );
        assert_eq!(
            &text[cursor - around.cursor..][..around.text.len()],
            around.text
        );
    }

    #[test]
    fn a_window_at_the_start_and_the_end_fills_from_the_other_side() {
        let text = "a".repeat(1000);
        let first = Surrounding::around(&text, 0, 0, 100);
        assert_eq!((first.text.len(), first.cursor), (100, 0));
        let last = Surrounding::around(&text, 1000, 1000, 100);
        assert_eq!((last.text.len(), last.cursor), (100, 100));
    }

    #[test]
    fn the_cost_of_a_window_does_not_grow_with_the_text() {
        use std::time::Instant;
        let small = "ab👍🏽 ".repeat(900);
        let large = "ab👍🏽 ".repeat(120_000);
        assert!(small.len() < 11_000 && large.len() > 1_000_000);
        let best = |text: &str| {
            let at = text.len() / 2 / 4 * 4;
            let at = (0..=at).rev().find(|i| text.is_char_boundary(*i)).unwrap();
            (0..15)
                .map(|_| {
                    let started = Instant::now();
                    for _ in 0..200 {
                        std::hint::black_box(Surrounding::around(text, at, at, 4000));
                    }
                    started.elapsed()
                })
                .min()
                .unwrap()
        };
        let (small_cost, large_cost) = (best(&small), best(&large));
        assert!(
            large_cost < small_cost * 4,
            "10 KB took {small_cost:?}, 1 MB took {large_cost:?}"
        );
    }

    #[test]
    fn a_window_cut_never_splits_a_regional_indicator_pair() {
        let text = "🇳🇱".repeat(300);
        let cursor = 8 * 150;
        let around = Surrounding::around(&text, cursor, cursor, 104);
        assert!(
            (cursor - around.cursor).is_multiple_of(8),
            "starts mid-pair"
        );
        assert!(around.text.len().is_multiple_of(8), "ends mid-pair");
    }

    #[test]
    fn a_selection_that_fits_stays_whole() {
        let text = "a".repeat(1000);
        let around = Surrounding::around(&text, 600, 560, 100);
        assert_eq!(around.cursor.abs_diff(around.anchor), 40);
        assert_eq!(around.text.len(), 100);
    }

    #[test]
    fn a_selection_longer_than_the_bound_is_cut_around_the_cursor() {
        let text = "a".repeat(1000);
        let around = Surrounding::around(&text, 800, 100, 100);
        assert_eq!(around.text.len(), 100);
        assert_eq!(around.cursor, 50);
        assert_eq!(around.anchor, 0);
    }

    #[test]
    fn a_session_bumps_its_revision_only_when_the_selection_changes() {
        let mut s = session(false);
        let moved = Selection {
            anchor: Default::default(),
            focus: crate::TextPosition {
                byte: 3,
                ..Default::default()
            },
        };
        s.select(moved);
        s.select(moved);
        assert_eq!(s.revision, 1);
    }
}
