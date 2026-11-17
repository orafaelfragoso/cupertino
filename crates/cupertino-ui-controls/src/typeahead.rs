//! Type-to-select for the items of a menu or a pop-up list. See the crate README, *Type-to-select*.

/// How long a pause ends the typed text (seconds).
pub const RESET_AFTER: f64 = 1.0;

/// The text typed so far to find an item, and when its last character came.
#[derive(Clone, Debug, Default)]
pub struct TypeAhead {
    text: String,
    at: f64,
}

impl TypeAhead {
    /// Whether a search is under way at `now`: a typed character is less than [`RESET_AFTER`] old.
    pub fn typing(&self, now: f64) -> bool {
        !self.text.is_empty() && now - self.at <= RESET_AFTER
    }

    /// Add `c` typed at `now` and find the item it leads to: `titles` are the items in order (`None` for one that
    /// cannot be chosen) and `from` the highlighted one. `None` when no item starts with the text.
    pub fn find(
        &mut self,
        c: char,
        now: f64,
        from: usize,
        titles: &[Option<&str>],
    ) -> Option<usize> {
        if !self.typing(now) {
            self.text.clear();
        }
        self.at = now;
        self.text.extend(c.to_lowercase());
        let mut chars = self.text.chars();
        let first = chars.next().map(String::from).unwrap_or_default();
        let repeated = chars.clone().next().is_some() && chars.all(|c| first.starts_with(c));
        let found = if self.text.chars().count() == 1 {
            next(titles, from + 1, &self.text)
        } else {
            next(titles, from, &self.text)
        };
        // the same letter again with nothing starting with it twice steps on to the next item it starts
        found.or_else(|| repeated.then(|| next(titles, from + 1, &first)).flatten())
    }
}

/// The first item from `start` on, wrapping, whose title starts with `text`.
fn next(titles: &[Option<&str>], start: usize, text: &str) -> Option<usize> {
    (0..titles.len())
        .map(|i| (start + i) % titles.len())
        .find(|i| titles[*i].is_some_and(|t| t.to_lowercase().starts_with(text)))
}
