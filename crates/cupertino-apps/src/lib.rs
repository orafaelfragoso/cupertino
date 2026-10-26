//! Application registry shared by the Cupertino processes, without a C library.
//!
//! * [`entry`]: desktop entries (freedesktop Desktop Entry Specification): parsing, visibility, indexing;
//! * [`exec`]: `Exec` quoting and field codes, the command a launch turns into;
//! * [`mime`]: the shared MIME database: file names to types (`globs2`), aliases, the subclass tree and
//!   the icon names of a type;
//! * [`mimeapps`]: default applications and the Open With list from `mimeapps.list`.

pub mod entry;
pub mod exec;
pub mod mime;
pub mod mimeapps;

pub use entry::{DesktopAction, DesktopEntry, Handler, Locale, Visibility};
pub use exec::CommandSpec;
pub use mime::MimeDatabase;
pub use mimeapps::{MimeApps, OpenWithItem, XdgDirs};
