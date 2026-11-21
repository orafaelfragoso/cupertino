//! The session's xdg-desktop-portal backend. See the README.

// only the PipeWire buffers of [`screencast`] are unsafe
#![deny(unsafe_code)]

pub mod app_chooser;
pub mod capture;
pub mod consent;
pub mod dialogs;
pub mod file_chooser;
pub mod geometry;
pub mod global_shortcuts;
pub mod prompts;
pub mod request;
pub mod screen_cast;
pub mod screencast;
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
