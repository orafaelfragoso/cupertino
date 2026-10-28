//! Single-line text field model shared by the Cupertino apps and CupertinoKit (no drawing, no I/O).
//!
//! * [`TextField`]: the text, a caret and a selection anchor (byte offsets on grapheme boundaries), a composition
//!   range for input methods, undo and redo, the kill buffer, click counting, the caret blink and the horizontal
//!   scroll that keeps the caret in view.
//! * [`Keymap`]: a modifier profile and the editing chords (`cupertino-key-bindings`); the primary modifier's A, C, X,
//!   V and Z are the field's own commands. `cupertino-keyboard` reads the user's profile (`user_profile`) and
//!   chords (`binding::load_default`).
//!
//! The application feeds key presses ([`TextField::key`]) and pointer presses ([`TextField::press`]) with
//! [`std::time::Instant`]s, draws the text with the caret offsets of [`TextField::boundaries`], and carries out the
//! clipboard requests of [`Response`].

mod field;
mod keymap;

pub use cupertino_ui_text::{CARET_BLINK_OFF, CARET_BLINK_ON, MULTI_CLICK_INTERVAL};
pub use field::{Response, TextField, TextFieldKeys};
pub use keymap::{KeyInput, Keymap};

#[cfg(test)]
mod tests;
