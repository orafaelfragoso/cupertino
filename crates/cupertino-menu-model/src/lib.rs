//! Typed application menus, shared by CupertinoKit, which exports them, and the menu bar, which shows them.
//!
//! Chords are written in logical modifiers ([`cupertino_modifiers::Modifier`]) and become text or accelerators only
//! through a [`cupertino_modifiers::ModifierMap`], the active modifier profile. [`Role`] names the items the menu bar rebinds, by action.
//! [`standard::menu_bar`] builds the App, File, Edit and View menus every application has, and
//! [`MenuBar::exported`] is what an application publishes: everything but the commands the desktop runs itself.

mod chord;
mod menu;
pub mod standard;

pub use chord::{Chord, Key};
pub use menu::{ActionName, Command, Entry, Item, Menu, MenuBar, Role, Scope, SystemCommand};
