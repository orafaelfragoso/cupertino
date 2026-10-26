//! Screenshot tool for a Wayland compositor: the pure model and drawing of the `cupertino-screenshot` process.
//!
//! * [`values`]: every value taken from the analysis, with its evidence class (`SSU-…).
//! * [`session`]: the overlay as a state machine: modes, the selection, the window pick, the timer, the outcome.
//! * [`selection`], [`pick`]: the selection rectangle with its handles and readout; the window under the pointer.
//! * [`thumbnail`]: the floating thumbnail's size, expiry, dismiss animation and swipe.
//! * [`filename`], [`settings`]: `Screenshot <date> at <time>.png` and where it goes; `screenshot.conf`.
//! * [`toolbar`]: the toolbar's controls and the Options menu.
//! * [`image`], [`render`]: captured pixels to PNG; CPU drawing of the overlay, toolbar, menu and thumbnail.
//! * feature `app`: the `cupertino-screenshot` process, a client the server starts on a privileged connection of its
//!   own for each screenshot shortcut (see the README).

pub mod filename;
pub mod image;
pub mod pick;
pub mod render;
pub mod selection;
pub mod session;
pub mod settings;
pub mod thumbnail;
pub mod toolbar;
pub mod values;
