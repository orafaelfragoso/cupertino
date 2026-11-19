//! Overlay key and notice value types shared between input routing and shell overlay rendering.

use smithay::input::keyboard::keysyms;

use crate::input::Mods;

/// A key press the overlays interpret: the keysym of the key without modifiers (letters lower case),
/// the character the modifiers produce, and the physical modifiers (Super is `logo`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayKey {
    /// Raw xkb keycode (identifies the key for repeat and release).
    pub code: u32,
    pub sym: u32,
    pub text: Option<char>,
    pub mods: Mods,
    /// The Alt of `mods` is AltGr alone ([`crate::kbd::KeyboardSettings::alt_gr`]).
    pub alt_gr: bool,
}

/// What the notice that an application holds the shortcuts or the pointer says, and where and until when it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ShortcutNotice {
    pub text: String,
    /// What shows instead of `text` on an output too narrow for it: the sentence that names the release chord.
    pub clause: String,
    /// What shows instead of `clause` below the supported output width: the release chord alone.
    pub short: String,
    pub output: cupertino_wm::OutputId,
    /// The toplevel surface whose inhibitor or pointer constraint the notice is for; `None` for a notice no deactivation
    /// takes down.
    pub owner: Option<smithay::reexports::wayland_server::backend::ObjectId>,
    pub until: f64,
}

/// Keysyms that are modifiers (their presses carry no text and are ignored by the overlays' repeat).
pub fn is_modifier_sym(sym: u32) -> bool {
    matches!(
        sym,
        keysyms::KEY_Shift_L
            | keysyms::KEY_Shift_R
            | keysyms::KEY_Control_L
            | keysyms::KEY_Control_R
            | keysyms::KEY_Alt_L
            | keysyms::KEY_Alt_R
            | keysyms::KEY_Super_L
            | keysyms::KEY_Super_R
            | keysyms::KEY_Meta_L
            | keysyms::KEY_Meta_R
            | keysyms::KEY_Caps_Lock
            | keysyms::KEY_ISO_Level3_Shift
            | keysyms::KEY_Num_Lock
    )
}
