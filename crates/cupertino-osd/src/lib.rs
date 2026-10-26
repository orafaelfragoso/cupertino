//! Volume, brightness and keyboard-backlight overlay for a Wayland compositor.
//!
//! * [`model`]: the overlay kinds, the 16-segment fill rule and the show / hold / fade lifetime (`Hud`).
//! * [`sender`]: the design sender's level steps and hold times (one place) and the display floor deviation.
//! * [`backend`]: the output volume (through `cupertino-alsa`) and the backlights, behind traits, with in-memory stand-ins.
//! * [`controller`]: carries out a `Command` on the backends and shows the result.
//! * [`layout`], [`render`]: the geometry of the 200 x 200 window and the CPU drawing of its content.
//! * feature `dbus`: [`control`] (the `org.cupertino.Osd1` bus interface) and [`system`] (the display and keyboard
//!   backlights through the power daemon on the system bus).
//! * feature `app`: the `cupertino-osd` process (the overlay on a layer surface as a client of the server).
//!
//! Window size, corner radius, position, meter geometry, the partial-segment rule and the fade are taken from the
//! reference analysis; colours, glyph placement and the
//! material recipe are approximate.

pub mod backend;
#[cfg(feature = "dbus")]
pub mod control;
pub mod controller;
pub mod layout;
pub mod model;
pub mod render;
pub mod sender;
#[cfg(feature = "dbus")]
pub mod system;

pub use backend::{
    AlsaVolume, BackendError, Backlight, MemoryBacklight, MemoryVolume, Volume, VolumeState,
};
pub use controller::{Command, Controller, EffectError};
pub use model::{segments, Fill, Hud, Kind, Overlay, Phase, View};
