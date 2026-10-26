//! The single-line editor of an item's name while it is renamed in place: the text, a caret and a selection,
//! both in characters. It starts with the name selected without its extension (`rename::initial_selection`,
//! ); the window draws it over the item's label and feeds it keys.

use crate::rename::{initial_selection, validate_name, NameError};
use std::ops::Range;

#[derive(Clone, Debug)]
pub struct NameEdit {
    index: usize,
    original: String,
    chars: Vec<char>,
    caret: usize,
    anchor: usize,
}

impl NameEdit {
    /// Edit the name of item `index`.
    pub fn new(index: usize, name: &str, is_dir: bool) -> Self {
        let range = initial_selection(name, is_dir);
        NameEdit {
            index,
            original: name.to_string(),
            chars: name.chars().collect(),
            caret: range.end,
            anchor: range.start,
        }
    }

    /// The item being renamed (an index of the listing it was started in).
    pub fn index(&self) -> usize {
        self.index
    }

    /// The item moved in the listing.
    pub fn set_index(&mut self, index: usize) {
        self.index = index;
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected characters, empty when there is only a caret.
    pub fn selection(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    /// Replace the selection with `s`.
    pub fn insert(&mut self, s: &str) {
        let range = self.selection();
        let inserted: Vec<char> = s.chars().collect();
        self.caret = range.start + inserted.len();
        self.chars.splice(range, inserted);
        self.anchor = self.caret;
    }

    /// Delete the selection, or the character before the caret.
    pub fn backspace(&mut self) {
        if self.selection().is_empty() && self.caret > 0 {
            self.anchor = self.caret - 1;
        }
        self.insert("");
    }

    /// Delete the selection, or the character after the caret.
    pub fn delete(&mut self) {
        if self.selection().is_empty() && self.caret < self.chars.len() {
            self.anchor = self.caret + 1;
        }
        self.insert("");
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
    }

    /// Move the caret one character left; with a selection and no `extend` it goes to the selection's start.
    pub fn left(&mut self, extend: bool) {
        let to = if !extend && !self.selection().is_empty() {
            self.selection().start
        } else {
            self.caret.saturating_sub(1)
        };
        self.move_to(to, extend);
    }

    /// Move the caret one character right; with a selection and no `extend` it goes to the selection's end.
    pub fn right(&mut self, extend: bool) {
        let to = if !extend && !self.selection().is_empty() {
            self.selection().end
        } else {
            (self.caret + 1).min(self.chars.len())
        };
        self.move_to(to, extend);
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.chars.len(), extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.chars.len();
    }

    /// The new name to rename the item to: `None` when the text is the original, an error when it is not a valid name.
    pub fn commit(&self) -> Result<Option<String>, NameError> {
        let text = self.text();
        validate_name(&text)?;
        Ok((text != self.original).then_some(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_starts_with_the_name_selected_without_its_extension() {
        let edit = NameEdit::new(2, "report.pdf", false);
        assert_eq!(edit.selection(), 0..6);
        let mut edit = edit;
        edit.insert("summary");
        assert_eq!(edit.text(), "summary.pdf");
        assert_eq!(edit.caret(), 7);
    }

    #[test]
    fn deleting_and_moving_work_in_characters_not_bytes() {
        let mut edit = NameEdit::new(0, "café", true);
        assert_eq!(edit.selection(), 0..4);
        edit.right(false);
        assert_eq!((edit.caret(), edit.selection()), (4, 4..4));
        edit.backspace();
        assert_eq!(edit.text(), "caf");
        edit.home(false);
        edit.delete();
        assert_eq!(edit.text(), "af");
        edit.end(false);
        edit.left(true);
        assert_eq!(edit.selection(), 1..2);
        edit.left(false);
        assert_eq!(edit.caret(), 1, "a selection collapses to its start");
    }

    #[test]
    fn commit_reports_no_change_and_refuses_invalid_names() {
        let mut edit = NameEdit::new(0, "a.txt", false);
        assert_eq!(edit.commit(), Ok(None));
        edit.select_all();
        edit.insert("b/c");
        assert_eq!(edit.commit(), Err(NameError::ContainsSlash));
        edit.select_all();
        edit.insert("b.txt");
        assert_eq!(edit.commit(), Ok(Some("b.txt".to_string())));
    }
}
