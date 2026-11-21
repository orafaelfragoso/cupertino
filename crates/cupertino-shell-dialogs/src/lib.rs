//! The portal's dialogs, a component of the shell host (`cupertino-shell`): the file chooser, the family's open and
//! save panel, and the app chooser, which the portal asks for over `org.cupertino.Dialogs1`.
//!
//! * [`ask`]: what the portal asks for, bounded once more, and how each dialog ends.
//! * [`chooser`]: one file chooser without drawing: its folder, filter, selection, name and choices.
//! * [`service`]: `org.cupertino.Dialogs1`, served on the component's own bus connection to the portal alone.
//! * [`file`], [`apps`]: what an open file chooser and app chooser keep, and what each event does to them.
//! * [`view`]: their windows in CupertinoKit.
//! * [`Dialogs`]: the component.

#![forbid(unsafe_code)]

pub mod apps;
pub mod ask;
pub mod chooser;
mod component;
pub mod file;
pub mod service;
pub mod view;

pub use component::{
    item_icons, load, read, Dialogs, Loaded, Message, Model, Taken, LISTING_DEADLINE,
};
