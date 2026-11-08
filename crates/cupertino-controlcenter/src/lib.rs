//! Control panel for a Wayland compositor: the glass tiles of toggles, levels and detail views that open below the
//! menu bar.
//!
//! * [`model`]: the modules in reading order, the reading behind each tile (`Reading`), toggles, levels, media and the
//!   lists of the detail views.
//! * [`layout`]: the geometry of the tiles and of an open detail view in points.
//! * [`lifecycle`]: the open / close state machine with injected time.
//! * [`motion`]: the opening and closing of a detail view with injected time.
//! * [`backend`]: what the panel reads and changes (Wi-Fi, Bluetooth, the microphone, dock hiding, the keyboard
//!   backlight, focus mode, brightness, volume and outputs, the media player, the screen lock, the settings application,
//!   the overlay), behind traits, with in-memory stand-ins; the volume goes through `cupertino-pipewire` and
//!   `cupertino-alsa`.
//! * [`controller`]: carries out presses, level drags and detail views on the backends and disables a tile whose backend
//!   fails.
//! * feature `dbus`: [`control`] (the `org.cupertino.ControlPanel1` bus interface) and [`system`] (the backends over the
//!   system and session buses).
//! * feature `app`: [`view`] (the tiles and the outside-click catcher as CupertinoKit windows) and the
//!   `cupertino-controlcenter` process (the panel on a layer surface as a client of the server).

pub mod backend;
#[cfg(feature = "dbus")]
pub mod control;
pub mod controller;
pub mod layout;
pub mod lifecycle;
pub mod model;
pub mod motion;
#[cfg(feature = "dbus")]
pub mod system;
#[cfg(feature = "app")]
pub mod view;

pub use backend::{BackendError, OverlayKind};
pub use controller::{Backends, Controller};
pub use lifecycle::{Lifecycle, Phase};
pub use model::{Controls, Detail, Module, Reading, Slider, TileState, VolumeState, WifiStatus};
