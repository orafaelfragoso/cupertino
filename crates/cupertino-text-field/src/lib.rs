//! The single-line text field of the Cupertino apps: CupertinoKit's editing model and its keys, from
//! `cupertino-ui-text`, under the names the apps use. `cupertino-keyboard` reads the user's profile
//! (`binding::user_profile`) and chords (`binding::load_default`) for [`Keymap::new`].

pub use cupertino_ui_text::{
    KeyInput, Keymap, Response, TextFieldKeys, CARET_BLINK_OFF, CARET_BLINK_ON,
    MULTI_CLICK_INTERVAL,
};

/// The editing model of a field.
pub type TextField = cupertino_ui_text::TextBuffer;
