//! The dock's settings: the options of `dock.conf` (read by `cupertino_defaults::dock`) applied to the
//! dock's configuration, the pins the dock stores itself when the user changes them, and the pins
//! offered when there is no file.

use std::io;
use std::path::Path;

use cupertino_defaults::dock::{DockSettings, Position};

use crate::config::{sanitize_large_size, sanitize_tile_size, DockConfig, Orientation};
use crate::desktop::DesktopIndex;

/// The session's file manager (desktop file ID): the dock opens the trash and folders in it.
pub const FILE_MANAGER: &str = "cupertino-files";

/// File managers, the session's own first. The first installed one is the dock's built-in first tile
/// ([`session_file_manager`], [`crate::dock::Dock::set_built_in`]): the design's
/// default pins leave the file manager out and show it first. The others are an approximate selection.
pub const FILE_MANAGERS: &[&str] = &[
    FILE_MANAGER,
    "org.gnome.Nautilus",
    "org.kde.dolphin",
    "thunar",
];

/// Applications offered as dock defaults after the file manager and the terminals when installed, in
/// order (desktop file IDs or commands). approximate selection of common applications.
pub const DEFAULT_CANDIDATES: &[&str] = &[
    "firefox",
    "org.mozilla.firefox",
    "chromium",
    "google-chrome",
    "org.gnome.Epiphany",
    "foot",
    "alacritty",
    "kitty",
    "org.wezfurlong.wezterm",
    "org.gnome.TextEditor",
    "code",
    "mpv",
];

/// Terminal emulators in order of preference, for the default pins and for entries that ask for a
/// terminal. approximate selection of common terminals.
pub const TERMINALS: &[&str] = &["foot", "alacritty", "kitty", "wezterm", "xterm"];

/// The dock's configuration: the design defaults for a display `display_height` points tall,
/// with whatever `file` sets (sizes pass the design validation).
pub fn dock_config(file: Option<&DockSettings>, display_height: f64) -> DockConfig {
    let mut cfg = DockConfig::for_display_height(display_height);
    let Some(f) = file else { return cfg };
    if let Some(t) = f.tile_size {
        cfg.tile_size = sanitize_tile_size(Some(t), display_height);
    }
    if let Some(l) = f.large_size {
        cfg.large_size = sanitize_large_size(Some(l));
    }
    cfg.magnification = f.magnification.unwrap_or(cfg.magnification);
    cfg.autohide = f.autohide.unwrap_or(cfg.autohide);
    cfg.orientation = f.position.map_or(cfg.orientation, |p| match p {
        Position::Bottom => Orientation::Bottom,
        Position::Left => Orientation::Left,
        Position::Right => Orientation::Right,
    });
    cfg.show_recents = f.recents.unwrap_or(cfg.show_recents);
    cfg.minimize_to_app = f.minimize_to_app.unwrap_or(cfg.minimize_to_app);
    cfg.launch_animation = f.bounce.unwrap_or(cfg.launch_animation);
    cfg.show_running_indicators = f.indicators.unwrap_or(cfg.show_running_indicators);
    cfg
}

/// The pins file the dock writes: one key per line, `#` starts a comment. Unlike `dock.conf`, which
/// the user writes and the dock never rewrites, this file holds the pins as the user left them in
/// the dock (a drag, Keep in Dock, Remove from Dock) and takes their place at the next start.
pub fn parse_pins(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Read the pins file at `path`; `None` when it does not exist (a file with no pins is `Some`, the
/// user removed them all).
pub fn load_pins(path: &Path) -> io::Result<Option<Vec<String>>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(parse_pins(&text))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Write the pins to `path` atomically (`cupertino_defaults::atomic`), creating its directory.
pub fn store_pins(path: &Path, pins: &[String]) -> io::Result<()> {
    let mut text = String::from("# written by cupertino-dock; replaces the pins of dock.conf\n");
    for pin in pins {
        text.push_str(pin);
        text.push('\n');
    }
    cupertino_defaults::atomic::write_atomic(path, text.as_bytes())
}

/// The desktop file ID of the session file manager, the dock's built-in first tile: the first
/// installed of [`FILE_MANAGERS`] (so [`FILE_MANAGER`] when it is installed); `None` when none is.
pub fn session_file_manager(index: &DesktopIndex) -> Option<String> {
    FILE_MANAGERS
        .iter()
        .find_map(|name| index.find(name).filter(|e| !e.no_display))
        .map(|e| e.id.clone())
}

/// Up to `max` installed applications, after the built-in file manager: `terminals` then
/// [`DEFAULT_CANDIDATES`] (one per desktop entry).
pub fn default_pins(index: &DesktopIndex, terminals: &[&str], max: usize) -> Vec<String> {
    let mut pins: Vec<String> = Vec::new();
    for name in terminals.iter().chain(DEFAULT_CANDIDATES) {
        if pins.len() >= max {
            break;
        }
        if let Some(e) = index
            .find(name)
            .filter(|e| !e.no_display && !pins.contains(&e.id))
        {
            pins.push(e.id.clone());
        }
    }
    pins
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::parse_entry;

    #[test]
    fn the_file_overrides_the_defaults_and_sizes_are_validated() {
        let defaults = dock_config(None, 1080.0);
        assert_eq!(
            (
                defaults.tile_size,
                defaults.large_size,
                defaults.magnification,
                defaults.autohide
            ),
            (64.0, 128.0, false, false)
        );
        assert_eq!(
            dock_config(None, 768.0).tile_size,
            52.0,
            "small displays get the small default"
        );
        let f = DockSettings::parse("tile_size = 40\nlarge_size = 96\nmagnification = on\nautohide = on\nposition = right\nrecents = no\n", None).0;
        let cfg = dock_config(Some(&f), 1080.0);
        assert_eq!(
            (
                cfg.tile_size,
                cfg.large_size,
                cfg.magnification,
                cfg.autohide,
                cfg.orientation,
                cfg.show_recents
            ),
            (40.0, 96.0, true, true, Orientation::Right, false)
        );
        let silly = dock_config(
            Some(&DockSettings::parse("tile_size = 4000\n", None).0),
            1080.0,
        );
        assert_eq!(silly.tile_size, sanitize_tile_size(Some(4000.0), 1080.0));
        assert!(silly.tile_size < 4000.0);
    }

    #[test]
    fn stored_pins_come_back_in_order_and_an_empty_file_is_no_pins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("dock-pins");
        assert_eq!(load_pins(&path).unwrap(), None);
        let pins = vec![
            "mail".to_string(),
            "org.example.files".to_string(),
            "foot".to_string(),
        ];
        store_pins(&path, &pins).unwrap();
        assert_eq!(load_pins(&path).unwrap(), Some(pins));
        store_pins(&path, &[]).unwrap();
        assert_eq!(load_pins(&path).unwrap(), Some(Vec::new()));
        let leftovers = std::fs::read_dir(dir.path().join("state"))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0, "the temporary file is renamed away");
    }

    fn entry(id: &str, exec: &str) -> crate::desktop::DesktopEntry {
        parse_entry(
            id,
            Path::new("x"),
            &format!("[Desktop Entry]\nName={id}\nExec={exec}\n"),
        )
        .unwrap()
    }

    #[test]
    fn defaults_pick_installed_applications_once() {
        let idx = DesktopIndex {
            entries: vec![
                entry("foot", "foot"),
                entry("firefox", "firefox %u"),
                entry("org.gnome.Nautilus", "nautilus"),
            ],
        };
        assert_eq!(
            default_pins(&idx, &["foot", "xterm"], 4),
            ["foot", "firefox"],
            "the file manager is built in, not a pin"
        );
        assert_eq!(default_pins(&idx, &["foot"], 1), ["foot"]);
        assert!(default_pins(&DesktopIndex::default(), &["foot"], 4).is_empty());
    }

    #[test]
    fn the_session_file_manager_is_the_own_one_when_installed_else_the_first_installed() {
        let mut idx = DesktopIndex {
            entries: vec![
                entry("foot", "foot"),
                entry("org.kde.dolphin", "dolphin"),
                entry("org.gnome.Nautilus", "nautilus"),
            ],
        };
        assert_eq!(
            session_file_manager(&idx).as_deref(),
            Some("org.gnome.Nautilus"),
            "in the order of FILE_MANAGERS"
        );
        idx.entries
            .push(entry("cupertino-files", "cupertino-files %U"));
        assert_eq!(
            session_file_manager(&idx).as_deref(),
            Some("cupertino-files")
        );
        assert_eq!(
            session_file_manager(&DesktopIndex {
                entries: vec![entry("foot", "foot")]
            }),
            None
        );
    }
}
