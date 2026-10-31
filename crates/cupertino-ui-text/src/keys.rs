//! The key handling of a text field: a [`TextBuffer`] driven by the keys the [`Keymap`] resolves.

use std::time::Instant;

use crate::TextBuffer;
use cupertino_key_bindings::TextAction;

use crate::keymap::{Command, KeyInput, Keymap};
use cupertino_modifiers::Mods;

/// What a key or pointer input did; the window redraws for every value but [`Response::Ignored`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// Not a field key (Return, Escape, Tab, a window shortcut): the window handles it.
    Ignored,
    /// The caret or the selection moved; the text is the same.
    Moved,
    /// The text changed.
    Edited,
    /// Put this text on the clipboard; the text is the same.
    Copy(String),
    /// Put this text on the clipboard; it was cut, so the text changed.
    Cut(String),
    /// Read the clipboard and pass its text to [`TextBuffer::paste`].
    Paste,
}

/// A field that takes key presses.
pub trait TextFieldKeys {
    /// A key press at `now`.
    fn key(&mut self, keymap: &mut Keymap, key: KeyInput, now: Instant) -> Response;
    /// A key press with logical modifiers: the evdev `code`, the roles held and the character it types. The caret
    /// blink is the caller's.
    fn logical_key(
        &mut self,
        keymap: &mut Keymap,
        code: u32,
        mods: Mods,
        character: Option<char>,
    ) -> Response;
}

impl TextFieldKeys for TextBuffer {
    fn key(&mut self, keymap: &mut Keymap, key: KeyInput, now: Instant) -> Response {
        let response = run(self, keymap.command(key));
        if response != Response::Ignored {
            self.touch(now);
        }
        response
    }

    fn logical_key(
        &mut self,
        keymap: &mut Keymap,
        code: u32,
        mods: Mods,
        character: Option<char>,
    ) -> Response {
        run(self, keymap.logical_command(code, mods, character))
    }
}

fn run(buffer: &mut TextBuffer, command: Command) -> Response {
    match command {
        Command::Ignored => Response::Ignored,
        Command::Pending => Response::Moved,
        Command::SelectAll => {
            buffer.select_all();
            Response::Moved
        }
        Command::Copy if buffer.selection().is_empty() => Response::Moved,
        Command::Copy => Response::Copy(buffer.selected_text().to_string()),
        Command::Cut if buffer.selection().is_empty() => Response::Moved,
        Command::Cut => Response::Cut(buffer.cut()),
        Command::Paste => Response::Paste,
        Command::Undo => edited(buffer.undo()),
        Command::Redo => edited(buffer.redo()),
        Command::Insert(c) => {
            buffer.insert(c.encode_utf8(&mut [0; 4]));
            Response::Edited
        }
        Command::Actions(actions) => {
            let before = buffer.text().to_string();
            for action in actions {
                if !apply(buffer, action) {
                    return Response::Ignored;
                }
            }
            if buffer.text() == before {
                Response::Moved
            } else {
                Response::Edited
            }
        }
    }
}

/// Carry out a standard editing action; `false` for an action the window handles (Return, Tab, Escape).
fn apply(buffer: &mut TextBuffer, action: TextAction) -> bool {
    use TextAction as A;
    match action {
        A::InsertNewline
        | A::InsertNewlineIgnoringFieldEditor
        | A::InsertLineBreak
        | A::InsertTab
        | A::InsertTabIgnoringFieldEditor
        | A::InsertBacktab
        | A::CancelOperation
        | A::Complete
        | A::SelectNextKeyView
        | A::SelectPreviousKeyView => return false,
        A::MoveLeft => buffer.move_visual(false, false),
        A::MoveBackward => buffer.move_char(false, false),
        A::MoveRight => buffer.move_visual(true, false),
        A::MoveForward => buffer.move_char(true, false),
        A::MoveLeftAndModifySelection => buffer.move_visual(false, true),
        A::MoveBackwardAndModifySelection => buffer.move_char(false, true),
        A::MoveRightAndModifySelection => buffer.move_visual(true, true),
        A::MoveForwardAndModifySelection => buffer.move_char(true, true),
        A::MoveWordLeft | A::MoveWordBackward => buffer.move_word(false, false),
        A::MoveWordRight | A::MoveWordForward => buffer.move_word(true, false),
        A::MoveWordLeftAndModifySelection | A::MoveWordBackwardAndModifySelection => {
            buffer.move_word(false, true)
        }
        A::MoveWordRightAndModifySelection | A::MoveWordForwardAndModifySelection => {
            buffer.move_word(true, true)
        }
        A::MoveToLeftEndOfLine
        | A::MoveToBeginningOfParagraph
        | A::MoveToBeginningOfDocument
        | A::MoveUp
        | A::PageUp
        | A::ScrollToBeginningOfDocument => buffer.move_to_start(false),
        A::MoveToRightEndOfLine
        | A::MoveToEndOfParagraph
        | A::MoveToEndOfDocument
        | A::MoveDown
        | A::PageDown
        | A::ScrollToEndOfDocument => buffer.move_to_end(false),
        A::MoveToLeftEndOfLineAndModifySelection
        | A::MoveToBeginningOfParagraphAndModifySelection
        | A::MoveToBeginningOfDocumentAndModifySelection
        | A::MoveParagraphBackwardAndModifySelection
        | A::MoveUpAndModifySelection
        | A::PageUpAndModifySelection => buffer.move_to_start(true),
        A::MoveToRightEndOfLineAndModifySelection
        | A::MoveToEndOfParagraphAndModifySelection
        | A::MoveToEndOfDocumentAndModifySelection
        | A::MoveParagraphForwardAndModifySelection
        | A::MoveDownAndModifySelection
        | A::PageDownAndModifySelection => buffer.move_to_end(true),
        A::DeleteBackward | A::DeleteBackwardByDecomposingPreviousCharacter => {
            buffer.delete_backward()
        }
        A::DeleteForward => buffer.delete_forward(),
        A::DeleteWordBackward => buffer.delete_word_backward(),
        A::DeleteWordForward => buffer.delete_word_forward(),
        A::DeleteToBeginningOfLine => buffer.delete_to_line_start(),
        A::Delete => buffer.delete_selection(),
        A::DeleteToEndOfParagraph => buffer.kill_to_end(),
        A::Yank => buffer.yank(),
        A::Transpose => buffer.transpose(),
        A::InsertDoubleQuoteIgnoringSubstitution => buffer.insert("\""),
        A::InsertSingleQuoteIgnoringSubstitution => buffer.insert("'"),
        A::Noop
        | A::CenterSelectionInVisibleArea
        | A::CycleToNextInputKeyboardLayout
        | A::CycleToNextInputScript
        | A::InsertRightToLeftSlash
        | A::MakeBaseWritingDirectionLeftToRight
        | A::MakeBaseWritingDirectionNatural
        | A::MakeBaseWritingDirectionRightToLeft
        | A::MakeTextWritingDirectionLeftToRight
        | A::MakeTextWritingDirectionNatural
        | A::MakeTextWritingDirectionRightToLeft
        | A::ScrollPageDown
        | A::ScrollPageUp
        | A::TogglePlatformInputSystem => {}
    }
    true
}

fn edited(changed: bool) -> Response {
    if changed {
        Response::Edited
    } else {
        Response::Moved
    }
}

#[cfg(test)]
mod tests;
