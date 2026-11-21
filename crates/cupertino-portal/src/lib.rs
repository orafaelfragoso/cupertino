//! The session's xdg-desktop-portal backend. See the README.

#![forbid(unsafe_code)]

pub mod app_chooser;
pub mod capture;
pub mod chooser;
pub mod consent;
pub mod dialog;
pub mod file_chooser;
pub mod geometry;
pub mod global_shortcuts;
pub mod prompts;
pub mod request;
pub mod screen_cast;
pub mod screenshot;
pub mod screenshot_tool;
pub mod session;
pub mod settings;
pub mod sharing;
pub mod shortcut_keys;
pub mod toolkits;

/// The name xdg-desktop-portal reaches the backend at, as `data/cupertino.portal` declares it.
pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.cupertino";
/// Where every portal interface lives.
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// The xdg-foreign handle of a request's `parent_window` (`wayland:<handle>`), which a dialog opens as a child of. An
/// X11 window (`x11:<xid>`), an empty one or an empty handle names none, and the dialog opens on its own.
pub fn wayland_parent(parent_window: &str) -> Option<&str> {
    parent_window
        .strip_prefix("wayland:")
        .filter(|handle| !handle.is_empty())
}
