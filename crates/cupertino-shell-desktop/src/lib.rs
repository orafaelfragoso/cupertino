//! The desktop items of the Cupertino desktop, a component of the shell host (`cupertino-shell`): the items of the
//! Desktop folder as icons on the main display, drawn with Kit's file family (`cupertino_kit::files`) and the desktop's
//! own item view.
//!
//! * [`arrangement`]: where the items sit in the grid, and what keeps their places between sessions.
//! * [`metrics`]: the measured sizes of the items and the grid.
//! * [`Desktop`]: the component; [`prepare`] gives a driver of the headless window system the desktop of a folder.
//! * `view`: what the desktop draws on each output, and what its presses mean.

#![forbid(unsafe_code)]

pub mod arrangement;
mod component;
pub mod metrics;
mod view;

pub use component::{prepare, DeskCommand, Desktop, Message, Model, APP_ID};
