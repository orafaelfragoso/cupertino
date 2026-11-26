//! StatusNotifier support (the open freedesktop / KDE protocol for tray icons).
//!
//! * [`WatcherService`] serves the watcher at [`WATCHER_PATH`] under both interface names in use,
//!   `org.kde.StatusNotifierWatcher` and `org.freedesktop.StatusNotifierWatcher`, and claims both names (the first is
//!   required, the second taken when free). The component is the host, so `IsStatusNotifierHostRegistered` is true. It
//!   keeps the items registered, drops those whose service or registering connection leaves the bus, tracks other
//!   hosts, and reports item changes on a channel.
//! * [`fetch_item`] reads an item (`org.kde.StatusNotifierItem`, else `org.freedesktop.StatusNotifierItem`) into a
//!   [`StatusItem`] snapshot with one `GetAll`, icon pixmaps and the tooltip included; [`item_changes`] reports its
//!   `New…` signals as [`ItemChange`]s and [`refresh_item`] reads again what one names; [`load_menu`] fetches its
//!   DBusMenu; [`send_action`] sends `Activate`, `SecondaryActivate`, `ContextMenu` and `Scroll`.
//! * [`WatcherClient`] attaches to an existing watcher instead (when another process owns the name).
//!
//! Registered item strings are `service + path` (for example `:1.52/StatusNotifierItem`); a bare
//! service name means the default path `/StatusNotifierItem`, a bare path means the sender.

mod item;
mod watcher;

pub use item::*;
pub use watcher::*;

pub const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
/// The watcher's name in the freedesktop draft of the specification; served beside [`WATCHER_NAME`].
pub const FREEDESKTOP_WATCHER_NAME: &str = "org.freedesktop.StatusNotifierWatcher";
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
pub const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

/// Split a registered item string into `(service, path)`.
pub fn parse_item_id(registered: &str, sender: Option<&str>) -> Option<(String, String)> {
    if registered.starts_with('/') {
        return sender.map(|s| (s.to_string(), registered.to_string()));
    }
    if registered.is_empty() {
        return None;
    }
    match registered.find('/') {
        Some(i) => Some((registered[..i].to_string(), registered[i..].to_string())),
        None => Some((registered.to_string(), DEFAULT_ITEM_PATH.to_string())),
    }
}
