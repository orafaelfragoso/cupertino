//! The numeric constants of the C API that Smithay imports by their C names.
//!
//! There is no foreign function here: the module only keeps the path `xkb::ffi::XKB_STATE_*` valid.

/// Depressed modifiers, i.e. a key is physically holding them.
pub const XKB_STATE_MODS_DEPRESSED: u32 = 1 << 0;
/// Latched modifiers: cleared after the next non-modifier key press.
pub const XKB_STATE_MODS_LATCHED: u32 = 1 << 1;
/// Locked modifiers: cleared when the key that locked them is pressed again.
pub const XKB_STATE_MODS_LOCKED: u32 = 1 << 2;
/// Effective modifiers, derived from the other components.
pub const XKB_STATE_MODS_EFFECTIVE: u32 = 1 << 3;
/// Depressed layout.
pub const XKB_STATE_LAYOUT_DEPRESSED: u32 = 1 << 4;
/// Latched layout.
pub const XKB_STATE_LAYOUT_LATCHED: u32 = 1 << 5;
/// Locked layout.
pub const XKB_STATE_LAYOUT_LOCKED: u32 = 1 << 6;
/// Effective layout, derived from the other components.
pub const XKB_STATE_LAYOUT_EFFECTIVE: u32 = 1 << 7;
/// LEDs, derived from the other components.
pub const XKB_STATE_LEDS: u32 = 1 << 8;
