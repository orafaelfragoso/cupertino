//! The session's xdg-desktop-portal backend. See the README.

pub mod app_chooser;
#[cfg(feature = "app")]
pub mod capture;
pub mod chooser;
pub mod consent;
#[cfg(feature = "app")]
pub mod dialog;
pub mod file_chooser;
pub mod geometry;
pub mod pick;
#[cfg(feature = "app")]
pub mod pick_overlay;
pub mod request;
pub mod screen_cast;
pub mod screenshot;
pub mod settings;
pub mod sharing;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_wayland_parent_window_names_a_handle() {
        assert_eq!(wayland_parent("wayland:ab12-cd"), Some("ab12-cd"));
        for none in ["", "wayland:", "x11:3a00004", "ab12-cd", "Wayland:ab12"] {
            assert_eq!(wayland_parent(none), None, "{none:?}");
        }
    }
}
