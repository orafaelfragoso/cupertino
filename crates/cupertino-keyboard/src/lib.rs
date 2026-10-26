//! Keyboard model for the Cupertino compositor (no windowing-library or Wayland dependency).
//!
//! * [`repeat`]: key repeat timing (preference units, clamping, `wl_keyboard.repeat_info`, caps-lock delay, repeater).
//! * [`modifiers`]: physical → logical modifier profiles (including a `cupertino` profile with the super key
//!   as the primary shortcut modifier), shortcut symbol selection, fn / globe key usage and tap debounce.
//! * [`rewrite`]: which Super and Alt chords the compositor rewrites for third-party clients.
//! * [`binding`]: the text-editing binding table, chord sequences, user override file, resolver.
//! * [`shortcuts`]: desktop shortcut defaults with neutral action names.
//! * [`accent`]: accent picker state machine and popup layout.
//! * [`slow_keys`], [`sticky_keys`], [`mouse_keys`]: the accessibility keyboard filters.
//! * [`switcher`]: input-source MRU switcher and HUD interaction / timing.
//! * [`xkb`]: XKB keymap + compose generator for decoded layout JSON; [`keycodes`]: layout key ↔ evdev ↔ XKB names.
//!
//! Every value that could not be is unknown or approximate and configurable.

pub mod accent;
pub mod binding;
mod binding_table;
pub mod keycodes;
pub mod keys;
pub mod modifiers;
pub mod mouse_keys;
pub mod repeat;
pub mod rewrite;
mod shortcut_table;
pub mod shortcuts;
pub mod slow_keys;
pub mod sticky_keys;
pub mod switcher;
pub mod xkb;

pub use accent::{AccentConfig, AccentEvent, AccentPicker, AccentTable};
pub use binding::{BindingTable, Resolution, Resolver, TextAction};
pub use keys::{Chord, ChordError, Key, Mods, NamedKey};
pub use modifiers::{FnUsage, GlobeTap, ModifierProfile, RawMods};
pub use mouse_keys::{MouseKeys, PointerAction};
pub use repeat::{CapsLockDelay, KeyRepeater, RepeatConfig, RepeatInfo};
pub use rewrite::{decide, is_printable, ClientClass, ClientTarget, KeyPress, Rewrite};
pub use shortcuts::{ShortcutSet, SystemAction};
pub use slow_keys::{SlowKeys, SlowStep};
pub use sticky_keys::{StickyKeys, StickyOutcome};
pub use switcher::{Step, SwitchEvent, Switcher};
pub use xkb::{generate, GenOptions, Keyboard, Layout, Level3Keys, XkbOutput};
