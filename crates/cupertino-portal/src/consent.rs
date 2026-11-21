//! What the consent questions share (screen sharing, screenshots and keyboard shortcuts): the application that asks,
//! named as its desktop entry names it.
//!
//! The app id is the one xdg-desktop-portal passes the backend, which only the frontend may call
//! ([`crate::request::from_frontend`]): the frontend reads it from the caller's sandbox (Flatpak or Snap metadata) or, for
//! an application outside a sandbox, from the systemd unit it runs in, and an application cannot pass its own. A host
//! application runs as the person and could name itself after another one, so the windows show who asks; they never
//! trust the name to grant anything by itself.

use std::path::PathBuf;

use cupertino_apps::entry::{parse_handler, Locale, Visibility};

use crate::file_chooser::plain;

/// Bytes of an icon name the windows look up.
const MAX_ICON: usize = 1024;

/// The application that asks, as a consent window shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    /// The name of the app's desktop entry, else its id; `None` when the id is empty.
    pub name: Option<String>,
    /// The `Icon=` of its desktop entry when that is an icon name: one with no `/` or control character.
    pub icon: Option<String>,
}

/// The application with id `app_id`, from `<app_id>.desktop` in the first of `applications` that has it, read the way
/// the app chooser reads it; an id that is not a file name, or has no entry, is shown as itself without an icon.
pub fn identify(applications: &[PathBuf], app_id: &str) -> Identity {
    if app_id.is_empty() {
        return Identity::default();
    }
    let file_name = !app_id.contains('/') && !app_id.chars().any(char::is_control);
    let entry = file_name
        .then(|| {
            applications
                .iter()
                .find_map(|dir| std::fs::read_to_string(dir.join(format!("{app_id}.desktop"))).ok())
        })
        .flatten()
        .and_then(|text| {
            let (locale, visibility) = (Locale::from_env(), Visibility::from_env());
            parse_handler(app_id, &text, &locale, &visibility).ok()
        })
        .map(|handler| handler.entry);
    let name = entry
        .as_ref()
        .map(|e| plain(&e.name))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| plain(app_id));
    let icon = entry.and_then(|e| e.icon).filter(|icon| {
        !icon.is_empty()
            && icon.len() <= MAX_ICON
            && !icon.contains('/')
            && !icon.chars().any(char::is_control)
    });
    Identity {
        name: Some(name),
        icon,
    }
}
