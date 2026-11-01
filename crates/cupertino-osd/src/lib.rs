//! Volume, brightness and keyboard-backlight overlay for a Wayland compositor.
//!
//! * [`model`]: the overlay kinds, the 16-segment fill rule and the show / hold / fade lifetime (`Hud`).
//! * [`sender`]: the design sender's level steps and hold times (one place) and the display floor deviation.
//! * [`backend`]: the output volume (the default PipeWire sink through `cupertino-pipewire`, else the ALSA master) and
//!   the backlights, behind traits, with in-memory stand-ins.
//! * [`controller`]: carries out a `Command` on the backends and shows the result.
//! * [`layout`]: the geometry of the 200 x 200 window.
//! * feature `dbus`: [`control`] (the `org.cupertino.Osd1` bus interface) and [`system`] (the display and keyboard
//!   backlights through the power daemon on the system bus).
//! * feature `app`: [`view`] (the overlay's window in CupertinoKit) and the `cupertino-osd` process (that window on a
//!   layer surface as a client of the server).
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
pub mod sender;
#[cfg(feature = "dbus")]
pub mod system;
#[cfg(feature = "app")]
pub mod view;

pub use backend::{
    AlsaVolume, BackendError, Backlight, MemoryBacklight, MemoryVolume, SessionMicrophone,
    SessionVolume, Volume, VolumeState,
};
pub use controller::{Command, Controller, EffectError};
pub use model::{segments, Fill, Hud, Kind, Overlay, Phase, View};
