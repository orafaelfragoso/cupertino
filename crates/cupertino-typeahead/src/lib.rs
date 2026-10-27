//! Type-to-select for the items of a menu or a pop-up list. See the README.

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
        let start = if self.text.chars().count() == 1 {
            from + 1
        } else {
            from
        };
        (0..titles.len())
            .map(|i| (start + i) % titles.len())
            .find(|i| titles[*i].is_some_and(|t| t.to_lowercase().starts_with(&self.text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TITLES: [Option<&str>; 6] = [
        Some("Automatic"),
        Some("400"),
        None,
        Some("800"),
        Some("Custom"),
        Some("Cursor"),
    ];

    #[test]
    fn a_letter_finds_the_next_item_and_repeating_it_steps_through_the_ones_that_share_it() {
        let mut t = TypeAhead::default();
        assert_eq!(t.find('c', 0.0, 0, &TITLES), Some(4));
        let mut t = TypeAhead::default();
        assert_eq!(t.find('c', 0.0, 4, &TITLES), Some(5));
        let mut t = TypeAhead::default();
        assert_eq!(t.find('c', 0.0, 5, &TITLES), Some(4), "wraps");
    }

    #[test]
    fn more_letters_narrow_the_search_from_the_highlighted_item() {
        let mut t = TypeAhead::default();
        assert_eq!(t.find('C', 0.0, 0, &TITLES), Some(4));
        assert_eq!(t.find('u', 0.3, 4, &TITLES), Some(4));
        assert_eq!(t.find('r', 0.6, 4, &TITLES), Some(5));
        assert_eq!(t.find('x', 0.9, 5, &TITLES), None);
    }

    #[test]
    fn an_item_that_cannot_be_chosen_never_matches() {
        let mut t = TypeAhead::default();
        let titles = [Some("one"), None, Some("two")];
        assert_eq!(t.find('t', 0.0, 0, &titles), Some(2));
        let mut t = TypeAhead::default();
        assert_eq!(t.find('z', 0.0, 0, &titles), None);
    }

    #[test]
    fn a_pause_starts_a_new_search_and_typing_reports_a_search_under_way() {
        let mut t = TypeAhead::default();
        assert!(!t.typing(0.0));
        t.find('c', 0.0, 0, &TITLES);
        assert!(t.typing(0.5));
        assert!(!t.typing(2.0));
        assert_eq!(t.find('a', 2.0, 4, &TITLES), Some(0));
    }
}
