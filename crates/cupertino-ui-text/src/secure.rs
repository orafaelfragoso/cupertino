//! The password buffer: the text lives in one fixed block that is zeroed whenever bytes leave it, grown by copying
//! into a new block and zeroing the old one, and zeroed when the buffer drops. It has no history, no clipboard
//! access and no accessor that returns the text outside [`SecureBuffer::with_secret`].

use std::fmt;

use unicode_segmentation::UnicodeSegmentation;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::TextError;

const MIN_CAPACITY: usize = 32;

/// Zeroes the whole block `bytes` holds, what lies past its length included, and empties it: for bytes that may be a
/// secret on its way into a [`SecureBuffer`], such as what a paste read, before their block is freed.
pub fn wipe_bytes(bytes: &mut Vec<u8>) {
    bytes.zeroize();
}

/// [`wipe_bytes`] for text.
pub fn wipe_text(text: &mut String) {
    text.zeroize();
}

/// The editable text of a password field: zeroed when dropped, never printed, copied or exposed to accessibility.
#[derive(ZeroizeOnDrop)]
pub struct SecureBuffer {
    bytes: Box<[u8]>,
    len: usize,
    caret: usize,
    anchor: usize,
    #[zeroize(skip)]
    max_len: Option<usize>,
}

impl fmt::Debug for SecureBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecureBuffer(..)")
    }
}

impl Default for SecureBuffer {
    fn default() -> Self {
        SecureBuffer::new()
    }
}

impl SecureBuffer {
    pub fn new() -> Self {
        SecureBuffer {
            bytes: vec![0; MIN_CAPACITY].into_boxed_slice(),
            len: 0,
            caret: 0,
            anchor: 0,
            max_len: None,
        }
    }

    /// A buffer that holds at most `max_len` bytes: an insertion past it is cut at a grapheme boundary.
    pub fn with_max_len(max_len: usize) -> Self {
        let mut buffer = SecureBuffer::new();
        buffer.max_len = Some(max_len);
        buffer
    }

    /// Change the limit of [`with_max_len`](Self::with_max_len), or lift it with `None`. Text already past a lower
    /// limit stays; only later insertions are cut.
    pub fn set_max_len(&mut self, max_len: Option<usize>) {
        self.max_len = max_len;
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The number of characters a mask shows: the graphemes of the text.
    pub fn grapheme_count(&self) -> usize {
        self.text().graphemes(true).count()
    }

    /// The caret, as a byte offset on a grapheme boundary.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected bytes, empty when there is only a caret.
    pub fn selection(&self) -> std::ops::Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    /// Lends the text to `f`, which must not keep it. The only way out of the buffer.
    pub fn with_secret<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        f(self.text())
    }

    /// The caret and the selection as grapheme indices, which is where they fall in [`mask`](Self::mask).
    pub fn masked_selection(&self) -> (usize, std::ops::Range<usize>) {
        let text = self.text();
        let index = |byte: usize| text[..byte].graphemes(true).count();
        let selection = self.selection();
        (
            index(self.caret),
            index(selection.start)..index(selection.end),
        )
    }

    /// One bullet per grapheme, for drawing; it reveals the length, as a password field does.
    pub fn mask(&self, bullet: char) -> String {
        std::iter::repeat_n(bullet, self.grapheme_count()).collect()
    }

    /// Insert typed text in place of the selection. Control characters are dropped, line breaks and tabs included.
    pub fn insert(&mut self, s: &str) {
        let mut typed = String::with_capacity(s.len());
        typed.extend(s.chars().filter(|c| !c.is_control()));
        let kept = self.len - self.selection().len();
        let fitted = self.fit(&typed, kept);
        self.replace_selection(fitted);
        typed.zeroize();
    }

    /// Delete the selection, or the grapheme before the caret.
    pub fn delete_backward(&mut self) {
        if self.selection().is_empty() {
            self.anchor = self.prev_boundary(self.caret);
        }
        self.replace_selection("");
    }

    /// Delete the selection, or the grapheme after the caret.
    pub fn delete_forward(&mut self) {
        if self.selection().is_empty() {
            self.anchor = self.caret;
            self.caret = self.next_boundary(self.caret);
        }
        self.replace_selection("");
    }

    /// One grapheme left or right; without `extend` a selection collapses to its start or end instead.
    pub fn move_char(&mut self, forward: bool, extend: bool) {
        let selection = self.selection();
        let to = match (extend || selection.is_empty(), forward) {
            (true, false) => self.prev_boundary(self.caret),
            (true, true) => self.next_boundary(self.caret),
            (false, false) => selection.start,
            (false, true) => selection.end,
        };
        self.move_to(to, extend);
    }

    pub fn move_to_start(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn move_to_end(&mut self, extend: bool) {
        self.move_to(self.len, extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.len;
    }

    /// Put the anchor and the caret at grapheme indices of the [`mask`](Self::mask) (where a press and a drag land on
    /// its bullets), each clamped to the text's end.
    pub fn select_masked(&mut self, anchor: usize, caret: usize) {
        let byte = |index: usize| {
            self.text()
                .grapheme_indices(true)
                .nth(index)
                .map_or(self.len, |(i, _)| i)
        };
        let (anchor, caret) = (byte(anchor), byte(caret));
        self.anchor = anchor;
        self.caret = caret;
    }

    /// Empty the buffer and zero what it held.
    pub fn clear(&mut self) {
        self.bytes[..self.len].zeroize();
        self.len = 0;
        self.caret = 0;
        self.anchor = 0;
    }

    /// Replace the whole text; the caret goes to the end.
    pub fn set(&mut self, text: &str) -> Result<(), TextError> {
        if text.chars().any(char::is_control) {
            return Err(TextError::InvalidText);
        }
        self.clear();
        self.insert(text);
        Ok(())
    }

    fn text(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.text()[..at]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.text()[at..]
            .graphemes(true)
            .next()
            .map_or(at, |g| at + g.len())
    }

    /// The longest grapheme prefix of `s` that fits with `kept` bytes of the text staying.
    fn fit<'a>(&self, s: &'a str, kept: usize) -> &'a str {
        let Some(room) = self.max_len.map(|max| max.saturating_sub(kept)) else {
            return s;
        };
        if s.len() <= room {
            return s;
        }
        let end = s
            .grapheme_indices(true)
            .map(|(i, g)| i + g.len())
            .take_while(|end| *end <= room)
            .last()
            .unwrap_or(0);
        &s[..end]
    }

    fn replace_selection(&mut self, s: &str) {
        let range = self.selection();
        if s.is_empty() && range.is_empty() {
            return;
        }
        let new_len = self.len - range.len() + s.len();
        if new_len > self.bytes.len() {
            self.grow(new_len);
        }
        let tail = self.len - range.end;
        self.bytes
            .copy_within(range.end..self.len, range.start + s.len());
        self.bytes[range.start..range.start + s.len()].copy_from_slice(s.as_bytes());
        if new_len < self.len {
            self.bytes[new_len..self.len].zeroize();
        }
        debug_assert_eq!(range.start + s.len() + tail, new_len);
        self.len = new_len;
        self.caret = range.start + s.len();
        self.anchor = self.caret;
    }

    /// Move the text into a larger block and zero the old one.
    fn grow(&mut self, needed: usize) {
        let capacity = needed.max(self.bytes.len() * 2);
        let mut grown = vec![0; capacity].into_boxed_slice();
        grown[..self.len].copy_from_slice(&self.bytes[..self.len]);
        self.bytes.zeroize();
        self.bytes = grown;
    }
}
