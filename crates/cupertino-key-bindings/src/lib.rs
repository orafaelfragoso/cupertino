//! Key chords and the text-editing binding table, free of file watching and desktop settings so the toolkit can
//! use them. `cupertino-keyboard` re-exports both and adds the user's file in the config directory.
//!
//! * [`keys`]: [`Chord`], [`Key`] and [`NamedKey`], with logical modifiers.
//! * [`binding`]: the standard [`BindingTable`], override text, and the [`Resolver`] for multi-chord sequences.

pub mod binding;
mod binding_table;
pub mod keys;

pub use binding::{BindingTable, OverrideError, Resolution, Resolver, TextAction};
pub use keys::{Chord, ChordError, Key, NamedKey};
