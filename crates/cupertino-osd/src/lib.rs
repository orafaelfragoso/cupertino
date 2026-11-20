//! The on-screen displays of the Cupertino desktop: the volume, brightness and keyboard-backlight overlay, a component
//! of the shell host (`cupertino-shell`).
//!
//! * [`model`]: the overlay kinds, the 16-segment fill rule and the show / hold / fade lifetime (`Hud`).
//! * [`sender`]: the design sender's level steps and hold times (one place) and the display floor deviation.
//! * [`controller`]: what a command asks of the levels and what the overlay shows, as pure functions.
//! * [`effects`]: the levels changed through the services that own them, `org.cupertino.Audio1` on the session bus and
//!   the backlights on the system bus.
//! * [`control`]: `org.cupertino.Osd1`, served on the component's own bus connection.
//! * [`layout`]: the geometry of the 200 x 200 window.
//! * [`view`]: the overlay's window in CupertinoKit.
//! * [`Osd`]: the component.
//!
//! Window size, corner radius, position, meter geometry, the partial-segment rule and the fade are taken from the
//! reference analysis; colours, glyph placement and the
//! material recipe are approximate.

#![forbid(unsafe_code)]

mod component;
pub mod control;
pub mod controller;
pub mod effects;
pub mod layout;
pub mod model;
pub mod sender;
pub mod view;

pub use component::{Message, Model, Osd};
pub use controller::{Command, EffectError, Shown, VolumeState};
pub use model::{segments, Fill, Hud, Kind, Overlay, Phase, View};
