//! The single-line editor of an item's name while it is renamed in place: a shared text field
//! (`cupertino-ui-text`'s `TextBuffer`) that starts with the name selected without its extension (`rename::initial_selection`);
//! the window draws it over the item's label and feeds it keys and presses.

use crate::rename::{initial_selection, validate_name, NameError};
use cupertino_ui_text::TextBuffer;

/// The most bytes a file name holds (the file system's limit).
pub const NAME_MAX_LEN: usize = 255;

#[derive(Clone, Debug)]
pub struct NameEdit {
    index: usize,
    original: String,
    field: TextBuffer,
}

impl NameEdit {
    /// Edit the name of item `index`.
    pub fn new(index: usize, name: &str, is_dir: bool) -> Self {
        let chars = initial_selection(name, is_dir);
        let byte = |c: usize| name.char_indices().nth(c).map_or(name.len(), |(b, _)| b);
        let mut field = TextBuffer::with_max_len(NAME_MAX_LEN);
        field.set_text(name);
        field.select(byte(chars.start)..byte(chars.end));
        NameEdit {
            index,
            original: name.to_string(),
            field,
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

    pub fn text(&self) -> &str {
        self.field.text()
    }

    pub fn field(&self) -> &TextBuffer {
        &self.field
    }

    pub fn field_mut(&mut self) -> &mut TextBuffer {
        &mut self.field
    }

    /// The new name to rename the item to: `None` when the text is the original, an error when it is not a valid name.
    pub fn commit(&self) -> Result<Option<String>, NameError> {
        let text = self.text();
        validate_name(text)?;
        Ok((text != self.original).then(|| text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_starts_with_the_name_selected_without_its_extension() {
        let mut edit = NameEdit::new(2, "café.pdf", false);
        assert_eq!(edit.field().selection(), 0..5, "the bytes of café");
        edit.field_mut().insert("summary");
        assert_eq!((edit.text(), edit.field().caret()), ("summary.pdf", 7));
        let folder = NameEdit::new(0, "a.b", true);
        assert_eq!(folder.field().selection(), 0..3, "a folder's whole name");
    }

    #[test]
    fn a_name_holds_at_most_255_bytes() {
        let mut edit = NameEdit::new(0, "a", false);
        edit.field_mut().select_all();
        edit.field_mut().paste(&"é".repeat(200));
        assert_eq!(
            edit.text(),
            "é".repeat(127),
            "254 bytes: half an é does not fit"
        );
    }

    #[test]
    fn commit_reports_no_change_and_refuses_invalid_names() {
        let mut edit = NameEdit::new(0, "a.txt", false);
        assert_eq!(edit.commit(), Ok(None));
        edit.field_mut().select_all();
        edit.field_mut().insert("b/c");
        assert_eq!(edit.commit(), Err(NameError::ContainsSlash));
        edit.field_mut().select_all();
        edit.field_mut().insert("b.txt");
        assert_eq!(edit.commit(), Ok(Some("b.txt".to_string())));
    }
}
