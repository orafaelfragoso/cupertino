//! What the consent windows share (screen sharing, screenshots and the colour picker): the application that asks, named
//! as its desktop entry names it, and the arming delay before a window takes an answer that grants anything.
//!
//! The app id is the one xdg-desktop-portal passes the backend, which only the frontend may call
//! ([`crate::request::from_frontend`]): the frontend reads it from the caller's sandbox (Flatpak or Snap metadata) or, for
//! an application outside a sandbox, from the systemd unit it runs in, and an application cannot pass its own. A host
//! application runs as the person and could name itself after another one, so the windows show who asks; they never
//! trust the name to grant anything by itself.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_apps::entry::{parse_handler, Locale, Visibility};

use crate::file_chooser::plain;

/// How long a consent window waits after it opened before a key, a click or a held key's repeats can grant. A key typed
/// for the application just before the window took the focus lands in this time and grants nothing.
pub const ARMING: Duration = Duration::from_millis(500);
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

/// When a consent window may grant: [`ARMING`] after it opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arming {
    opened: Instant,
}

impl Arming {
    pub fn new(opened: Instant) -> Self {
        Self { opened }
    }

    /// Whether an answer that grants is taken at `now`.
    pub fn armed(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.opened) >= ARMING
    }

    /// How long until it is armed, from `now`.
    pub fn remaining(&self, now: Instant) -> Duration {
        ARMING.saturating_sub(now.saturating_duration_since(self.opened))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_grants_nothing_until_the_arming_delay_has_passed() {
        let opened = Instant::now();
        let arming = Arming::new(opened);
        assert!(!arming.armed(opened));
        assert!(!arming.armed(opened + ARMING - Duration::from_millis(1)));
        assert!(arming.armed(opened + ARMING));
        assert_eq!(arming.remaining(opened), ARMING);
        assert_eq!(arming.remaining(opened + ARMING * 2), Duration::ZERO);
    }

    #[test]
    fn an_app_is_named_and_drawn_by_its_entry_else_shown_as_its_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("org.example.Meeting.desktop"),
            "[Desktop Entry]\nType=Application\nName=Meeting\u{202e}\nIcon=org.example.Meeting\nExec=meeting\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("org.example.Path.desktop"),
            "[Desktop Entry]\nType=Application\nName=Path\nIcon=/opt/path.png\nExec=path\n",
        )
        .unwrap();
        let dirs = [dir.path().to_path_buf()];
        assert_eq!(
            identify(&dirs, "org.example.Meeting"),
            Identity {
                name: Some("Meeting".to_owned()),
                icon: Some("org.example.Meeting".to_owned()),
            },
            "the name loses its direction override"
        );
        assert_eq!(
            identify(&dirs, "org.example.Path").icon,
            None,
            "an icon path"
        );
        assert_eq!(
            identify(&dirs, "org.example.Other"),
            Identity {
                name: Some("org.example.Other".to_owned()),
                icon: None,
            }
        );
        assert_eq!(identify(&dirs, "../org.example.Meeting").icon, None);
        assert_eq!(identify(&dirs, ""), Identity::default());
    }
}
