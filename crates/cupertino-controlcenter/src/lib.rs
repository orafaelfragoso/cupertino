//! The control panel of the Cupertino shell, a component of the shell host (`cupertino-shell`): the glass tiles of
//! toggles, levels and detail views that open below the menu bar.
//!
//! * [`ControlCenter`]: the component, which the host lists and runs on its UI loop.
//! * [`model`]: the modules in reading order, the reading behind each tile (`Reading`), toggles, levels, media and the
//!   lists of the detail views.
//! * [`arrangement`]: which modules show and in which order, as the person edits and stores it, and where that order
//!   puts each tile.
//! * [`layout`]: the geometry of the tiles, of the editing state and of an open detail view in points.
//! * [`lifecycle`]: the open / close state machine with injected time, on the catalogue's fades.
//! * [`motion`]: the opening and closing of a detail view with injected time, on the catalogue's spring.
//! * [`controller`]: what presses, level drags, detail views and the services' answers do to the readings, and the
//!   reads and changes they ask for, with injected time and no service of its own.
//! * [`view`]: the tiles and the outside-click catcher as CupertinoKit windows.
//! * [`control`]: the `org.cupertino.ControlPanel1` bus interface.

#![forbid(unsafe_code)]

pub mod arrangement;
mod component;
pub mod control;
pub mod controller;
mod effects;
pub mod layout;
pub mod lifecycle;
pub mod model;
pub mod motion;
pub mod view;

pub use component::{ControlCenter, Message, Model};
pub use controller::{Change, Controller, Effect, OverlayKind, Read, Source};
pub use lifecycle::{Lifecycle, Phase};
pub use model::{Controls, Detail, Module, Reading, Slider, TileState, VolumeState, WifiStatus};
