//! Control panel for a Wayland compositor: the card of toggles and sliders that opens below the menu bar.
//!
//! * [`model`]: the modules in grid order, the reading behind each tile (`Reading`), toggles, sliders and the battery.
//! * [`layout`]: the frames of the card in points and the hit test over them.
//! * [`lifecycle`]: the open / close state machine with injected time.
//! * [`backend`]: what the panel reads and changes (Wi-Fi, Bluetooth, focus mode, brightness, volume, battery, the
//!   overlay), behind traits, with in-memory stand-ins; the volume goes through `cupertino-alsa`.
//! * [`controller`]: carries out presses and slider drags on the backends and disables a tile whose backend fails.
//! * [`render`]: the CPU drawing of the card.
//! * feature `dbus`: [`control`] (the `org.cupertino.ControlPanel1` bus interface) and [`system`] (the backends over the
//!   system bus: network, Bluetooth and power daemons, and the overlay on the session bus).
//! * feature `app`: the `cupertino-controlcenter` process (the panel on a layer surface as a client of the server).
//!
//! Sizes, the open / close timings, the slider rules and the battery glyph come from the analysis; colours, icon names, the column gap and the behaviour of the modules without a
//! backend are or approximate, each labelled where it is defined.

pub mod backend;
#[cfg(feature = "dbus")]
pub mod control;
pub mod controller;
pub mod layout;
pub mod lifecycle;
pub mod model;
pub mod render;
#[cfg(feature = "dbus")]
pub mod system;

pub use backend::{BackendError, OverlayKind};
pub use controller::{Backends, Controller};
pub use layout::{Hit, Layout};
pub use lifecycle::{Lifecycle, Phase};
pub use model::{Battery, Controls, Module, Reading, Slider, TileState, VolumeState, WifiStatus};
