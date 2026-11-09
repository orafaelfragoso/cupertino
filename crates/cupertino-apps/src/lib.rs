//! Application registry shared by the Cupertino processes, without a C library.
//!
//! * [`activate`] (feature `dbus`): asking a running D-Bus activatable application for a window
//!   (`org.freedesktop.Application.Activate`);
//! * [`entry`]: desktop entries (freedesktop Desktop Entry Specification): parsing, visibility, indexing;
//! * [`exec`]: `Exec` quoting and field codes, the command a launch turns into;
//! * [`launch`]: what a shell process starts (a program, an application, a file with its default application),
//!   its command with an activation token, and running it detached;
//! * [`mime`]: the shared MIME database: file names to types (`globs2`), aliases, the subclass tree and
//!   the icon names of a type;
//! * [`mimeapps`]: default applications and the Open With list from `mimeapps.list`;
//! * [`recent`]: the session's recently-used files (`recently-used.xbel`), read only;
//! * [`frecency`]: the launcher's usage store, its format and ranking.

#[cfg(feature = "dbus")]
pub mod activate;
pub mod entry;
pub mod exec;
pub mod frecency;
pub mod launch;
pub mod mime;
pub mod mimeapps;
pub mod recent;

pub use entry::{DesktopAction, DesktopEntry, Handler, Locale, Visibility};
pub use exec::CommandSpec;
pub use launch::Launch;
pub use mime::MimeDatabase;
pub use mimeapps::{MimeApps, OpenWithItem, XdgDirs};
