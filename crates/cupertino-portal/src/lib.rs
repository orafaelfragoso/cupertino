//! The session's xdg-desktop-portal backend. See the README.

pub mod settings;

/// The name xdg-desktop-portal reaches the backend at, as `data/cupertino.portal` declares it.
pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.cupertino";
/// Where every portal interface lives.
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
