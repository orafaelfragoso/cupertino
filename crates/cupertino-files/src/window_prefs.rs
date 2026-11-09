//! How a browser window shows its folder and which bars it shows, kept between windows and launches. Each window keeps
//! its own choices while it is open; the last choice made in any window is saved, and a window opened later (in this
//! launch or the next) starts with it. The path bar and the status bar are hidden until a window shows them.
//!
//! The file is `$XDG_CONFIG_HOME/cupertino/files/window.json` ([`location`]): `{"view": "icons" | "list" | "columns",
//! "path-bar": bool, "status-bar": bool, "column-width": number}`; the column width is the width the column view's
//! columns start at, the last width a column was given. A missing, unreadable or malformed file, or a field missing or of another
//! type, gives that field's default; a write replaces the file atomically.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::window::ViewMode;

/// What a new browser window starts with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowPrefs {
    pub view: ViewMode,
    pub path_bar: bool,
    pub status_bar: bool,
    /// The width the column view's columns start at ([`crate::columns::ColumnChain::new`]).
    pub column_width: f64,
}

impl Default for WindowPrefs {
    /// The icon view without the path bar and the status bar.
    fn default() -> Self {
        WindowPrefs {
            view: ViewMode::Icons,
            path_bar: false,
            status_bar: false,
            column_width: crate::columns::DEFAULT_WIDTH,
        }
    }
}

/// Where the session keeps the choices: `files/window.json` in Cupertino's configuration directory, `None` when
/// neither `$XDG_CONFIG_HOME` nor `$HOME` is set.
pub fn location() -> Option<PathBuf> {
    cupertino_defaults::config_home().map(|config| config.join("cupertino/files/window.json"))
}

fn view_name(view: ViewMode) -> &'static str {
    match view {
        ViewMode::Icons => "icons",
        ViewMode::List => "list",
        ViewMode::Columns => "columns",
        ViewMode::Gallery => "gallery",
    }
}

impl WindowPrefs {
    /// The choices in the text of a prefs file, each field that is missing or malformed at its default.
    pub fn parse(text: &str) -> WindowPrefs {
        let defaults = WindowPrefs::default();
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return defaults;
        };
        let flag =
            |key: &str, default: bool| value.get(key).and_then(Value::as_bool).unwrap_or(default);
        WindowPrefs {
            view: match value.get("view").and_then(Value::as_str) {
                Some("list") => ViewMode::List,
                Some("icons") => ViewMode::Icons,
                Some("columns") => ViewMode::Columns,
                Some("gallery") => ViewMode::Gallery,
                _ => defaults.view,
            },
            path_bar: flag("path-bar", defaults.path_bar),
            status_bar: flag("status-bar", defaults.status_bar),
            column_width: value
                .get("column-width")
                .and_then(Value::as_f64)
                .filter(|w| w.is_finite())
                .map_or(defaults.column_width, |w| {
                    w.clamp(crate::columns::MIN_WIDTH, crate::columns::MAX_WIDTH)
                }),
        }
    }

    /// The text of the prefs file.
    pub fn text(&self) -> String {
        let value = json!({
            "view": view_name(self.view),
            "path-bar": self.path_bar,
            "status-bar": self.status_bar,
            "column-width": self.column_width,
        });
        format!("{value:#}\n")
    }

    /// The choices kept at `path`; the defaults when it cannot be read.
    pub fn load(path: &Path) -> WindowPrefs {
        match std::fs::read_to_string(path) {
            Ok(text) => WindowPrefs::parse(&text),
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    eprintln!("cupertino-files: cannot read {}: {e}", path.display());
                }
                WindowPrefs::default()
            }
        }
    }

    /// Keep these choices at `path`, replacing what is there.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        cupertino_defaults::atomic::write_atomic(path, self.text().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_icon_view_without_bars() {
        assert_eq!(
            WindowPrefs::default(),
            WindowPrefs {
                view: ViewMode::Icons,
                path_bar: false,
                status_bar: false,
                column_width: 245.0
            }
        );
    }

    #[test]
    fn choices_round_trip_and_a_bad_field_falls_back_alone() {
        let prefs = WindowPrefs {
            view: ViewMode::List,
            path_bar: true,
            status_bar: false,
            column_width: 300.0,
        };
        assert_eq!(WindowPrefs::parse(&prefs.text()), prefs);
        let gallery = WindowPrefs {
            view: ViewMode::Gallery,
            ..prefs
        };
        assert_eq!(WindowPrefs::parse(&gallery.text()), gallery);
        assert_eq!(
            WindowPrefs::parse(r#"{"view": "cover", "path-bar": "yes", "status-bar": true}"#),
            WindowPrefs {
                view: ViewMode::Icons,
                path_bar: false,
                status_bar: true,
                column_width: 245.0
            }
        );
        let columns = WindowPrefs::parse(r#"{"view": "columns", "column-width": 20}"#);
        assert_eq!(
            (columns.view, columns.column_width),
            (ViewMode::Columns, 100.0)
        );
        assert_eq!(WindowPrefs::parse("not json"), WindowPrefs::default());
        assert_eq!(WindowPrefs::parse("[]"), WindowPrefs::default());
    }

    #[test]
    fn a_saved_file_is_read_back_and_a_missing_one_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cupertino/files/window.json");
        assert_eq!(WindowPrefs::load(&path), WindowPrefs::default());
        let prefs = WindowPrefs {
            view: ViewMode::Columns,
            path_bar: false,
            status_bar: true,
            column_width: 260.0,
        };
        prefs.save(&path).unwrap();
        assert_eq!(WindowPrefs::load(&path), prefs);
    }
}
