//! The screenshot tool of the Cupertino desktop: the model, the windows CupertinoKit draws and the program of the
//! `cupertino-screenshot` process, which the server starts on a connection with the capture role and which reaches the
//! screen through `cupertino-kit-shell`.
//!
//! * [`values`]: the tool's design values.
//! * [`session`]: the overlay as a state machine: modes, the selection, the window pick, the timer, the keys, the
//!   outcome.
//! * [`selection`], [`pick`]: the selection rectangle with its handles and readout; the window under the pointer.
//! * [`thumbnail`]: the floating thumbnail's size, expiry, dismiss animation and swipe.
//! * [`filename`], [`settings`]: `Screenshot <date> at <time>.png` and where it goes; `screenshot.conf`.
//! * [`toolbar`]: the Options menu's rows and what they change.
//! * [`view`]: the overlay, toolbar, Options menu, thumbnail and clipboard pixel as CupertinoKit windows.
//! * [`controller`]: one run from the overlay to the delivered capture.
//! * [`files`]: the settings file and the captures written as PNG files, on the I/O pool.
//! * [`shot`]: one capture from a shortcut, the program `cupertino-screenshot --mode` runs.

#![forbid(unsafe_code)]

pub mod controller;
pub mod filename;
pub mod files;
pub mod pick;
pub mod selection;
pub mod session;
pub mod settings;
pub mod shot;
pub mod thumbnail;
pub mod toolbar;
pub mod values;
pub mod view;
