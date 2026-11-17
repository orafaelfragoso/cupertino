//! Type-to-select: incremental prefix search while typing.
//!
//! * The buffer is cleared when more than `max(2.0 s, 2 * key-repeat delay) passed since the last
//!   key (the delay is the system key-repeat threshold in 1/60 s ticks).
//! * A first character that is a plain or ideographic space is ignored.
//! * Control characters are not appended.
//! * Matching is case-insensitive with accents and punctuation significant, digits compared as text
//!   (collation flags). The result is the first item, in display order, whose key is not
//!   smaller than the buffer and which starts with it (falls back to the first item greater than
//!   the buffer, as the design selects the lower bound).
//! * Tab / Shift-Tab move to the next / previous item in display order (derived from
//!   `FindMatchPreviousNext`).

use crate::metrics::timing::TYPE_SELECT_MIN_TIMEOUT;
use crate::natural::type_select_key;

#[derive(Clone, Debug)]
pub struct TypeSelect {
    buffer: String,
    last: f64,
    /// System "delay until repeat" in seconds; the design reads it as ticks of 1/60 s.
    pub key_repeat_delay: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Event consumed without selecting (ignored space, control character).
    Ignored,
    /// Select item at this index.
    Select(usize),
    /// Buffer extended but nothing matched.
    NoMatch,
}

impl Default for TypeSelect {
    fn default() -> Self {
        // 35 ticks of 1/60 s is the shipped default repeat threshold
        TypeSelect {
            buffer: String::new(),
            last: f64::NEG_INFINITY,
            key_repeat_delay: 35.0 / 60.0,
        }
    }
}

impl TypeSelect {
    pub fn timeout(&self) -> f64 {
        (2.0 * self.key_repeat_delay).max(TYPE_SELECT_MIN_TIMEOUT)
    }

    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Clear when expired; true if a search is in progress.
    pub fn in_progress(&mut self, now: f64) -> bool {
        if now - self.last > self.timeout() {
            self.buffer.clear();
        }
        !self.buffer.is_empty()
    }

    /// Feed typed text at time `now` (seconds). `names` are the display names in display order.
    pub fn type_text<S: AsRef<str>>(&mut self, text: &str, now: f64, names: &[S]) -> Outcome {
        self.in_progress(now);
        if self.buffer.is_empty() && matches!(text, " " | "\u{3000}") {
            return Outcome::Ignored;
        }
        let filtered: String = text
            .chars()
            .filter(|c| (*c as u32) > 0x1F && *c as u32 != 0x7F)
            .collect();
        if filtered.is_empty() {
            return Outcome::Ignored;
        }
        self.buffer.push_str(&filtered);
        self.last = now;
        let needle = type_select_key(&self.buffer);
        let keys: Vec<Vec<char>> = names.iter().map(|n| type_select_key(n.as_ref())).collect();
        // the list is in display order, which is almost the key order; search the first prefix match
        if let Some(i) = keys.iter().position(|k| k.starts_with(&needle)) {
            return Outcome::Select(i);
        }
        match keys.iter().position(|k| k.as_slice() > needle.as_slice()) {
            Some(i) => Outcome::Select(i),
            None => Outcome::NoMatch,
        }
    }

    /// Tab (`reverse = false`) or Shift-Tab: neighbour of `current` in display order.
    pub fn tab(&mut self, current: Option<usize>, len: usize, reverse: bool) -> Option<usize> {
        self.buffer.clear();
        if len == 0 {
            return None;
        }
        Some(match (current, reverse) {
            (None, false) => 0,
            (None, true) => len - 1,
            (Some(i), false) => (i + 1).min(len - 1),
            (Some(i), true) => i.saturating_sub(1),
        })
    }
}
