//! Screenshot tool for a Wayland compositor: the model, and the windows CupertinoKit draws, of the
//! `cupertino-screenshot` process.
//!
//! * [`values`]: the tool's design values.
//! * [`session`]: the overlay as a state machine: modes, the selection, the window pick, the timer, the keys, the
//!   outcome.
//! * [`selection`], [`pick`]: the selection rectangle with its handles and readout; the window under the pointer.
//! * [`thumbnail`]: the floating thumbnail's size, expiry, dismiss animation and swipe.
//! * [`filename`], [`settings`]: `Screenshot <date> at <time>.png` and where it goes; `screenshot.conf`.
//! * [`toolbar`]: the Options menu's rows and what they change.
//! * [`image`]: captured pixels to upright images and PNG files.
//! * feature `app`: `view`, the overlay, toolbar, Options menu, thumbnail and clipboard pixel as CupertinoKit windows;
//!   `controller`, one run from the overlay to the delivered capture; and the `cupertino-screenshot` process, a client
//!   the server starts on a privileged connection of its own for each screenshot shortcut (see the README).

#[cfg(feature = "app")]
pub mod controller;
pub mod filename;
pub mod image;
pub mod pick;
pub mod selection;
pub mod session;
pub mod settings;
pub mod thumbnail;
pub mod toolbar;
pub mod values;
#[cfg(feature = "app")]
pub mod view;
