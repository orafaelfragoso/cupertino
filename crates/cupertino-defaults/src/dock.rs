//! `dock.conf` (`$CUPERTINO_DOCK_CONFIG` overrides the path): the dock's size, magnification, auto-hide, position and
//! recent applications, the actions of the hot corners and gestures, and pinned applications. The dock reads it; the
//! settings application writes it. The dock keeps the pins as the user leaves them in its own `dock-pins` file, which
//! wins over the pins here; this module only reads pins.
//!
//! | Key | Values | Unset |
//! |---|---|---|
//! | bare lines, `pin`, `pinned` | an application: desktop file ID, `app_id` or command | — |
//! | `tile_size` | points; valid `0 < v < 257`, rounded up ([`sanitize_tile_size`]) | 64, 52 on displays at most 800 pt tall |
//! | `large_size` | points; valid `0 < v < 513`, rounded up ([`sanitize_large_size`]) | 128 |
//! | `magnification`, `autohide`, `recents` | `on`, `off` | off, off, on |
//! | `position` | `bottom`, `left`, `right` | `bottom` |
//! | `corner_top_left` … `spread` ([`TRIGGER_KEYS`]) | `none`, `app-grid`, `exec:<command line>`, a server action name | [`ActionMap::default`] |
//!
//! Size defaults and validation are the design's; out-of-range sizes fall back to the default as
//! there. The defaults of the other options are the dock's (`DockConfig`).

use std::io;
use std::path::{Path, PathBuf};

use crate::conf::{self, parse_switch, switch_text, Key, Problem};

/// Environment variable that redirects the settings file.
pub const DOCK_CONF_ENV: &str = "CUPERTINO_DOCK_CONFIG";

/// Smallest tile size default, used when the main display is at most this tall (points)..
pub const SMALL_DISPLAY_HEIGHT: f64 = 800.0;
pub const DEFAULT_TILE_SIZE: f64 = 64.0;
pub const DEFAULT_TILE_SIZE_SMALL_DISPLAY: f64 = 52.0;
pub const DEFAULT_LARGE_SIZE: f64 = 128.0;

/// Tile size default for a display of the given height in points.
pub fn default_tile_size(display_height: f64) -> f64 {
    if display_height <= SMALL_DISPLAY_HEIGHT {
        DEFAULT_TILE_SIZE_SMALL_DISPLAY
    } else {
        DEFAULT_TILE_SIZE
    }
}

/// Tile size preference validation: valid when `0 < v < 257`, rounded up; otherwise the default for the display.
pub fn sanitize_tile_size(value: Option<f64>, display_height: f64) -> f64 {
    match value {
        Some(v) if v > 0.0 && v < 257.0 => v.ceil(),
        _ => default_tile_size(display_height),
    }
}

/// Large size preference validation: valid when `0 < v < 513`, rounded up; otherwise [`DEFAULT_LARGE_SIZE`].
pub fn sanitize_large_size(value: Option<f64>) -> f64 {
    match value {
        Some(v) if v > 0.0 && v < 513.0 => v.ceil(),
        _ => DEFAULT_LARGE_SIZE,
    }
}

/// The screen edge the dock sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Bottom,
    Left,
    Right,
}

/// How a folder tile's stack opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StackStyle {
    /// A fan for a small folder, a grid for a large one.
    #[default]
    Auto,
    /// Always a fan while the entries fit it; a grid for more.
    Fan,
    /// Always the grid.
    Grid,
}

/// The names of the eight triggers, as the keys of `dock.conf`.
pub const TRIGGER_KEYS: [&str; 8] = [
    "corner_top_left",
    "corner_top_right",
    "corner_bottom_left",
    "corner_bottom_right",
    "swipe_up",
    "swipe_down",
    "pinch",
    "spread",
];

/// A corner or gesture the server reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    CornerTopLeft,
    CornerTopRight,
    CornerBottomLeft,
    CornerBottomRight,
    SwipeUp,
    SwipeDown,
    Pinch,
    Spread,
}

impl Trigger {
    pub const ALL: [Trigger; 8] = [
        Trigger::CornerTopLeft,
        Trigger::CornerTopRight,
        Trigger::CornerBottomLeft,
        Trigger::CornerBottomRight,
        Trigger::SwipeUp,
        Trigger::SwipeDown,
        Trigger::Pinch,
        Trigger::Spread,
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|t| *t == self)
            .expect("every trigger is in ALL")
    }

    /// The trigger a `dock.conf` key names.
    pub fn from_key(key: &str) -> Option<Trigger> {
        TRIGGER_KEYS
            .iter()
            .position(|k| *k == key)
            .map(|i| Self::ALL[i])
    }
}

/// What a trigger runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionTarget {
    None,
    /// The application browser (the launcher's: the dock asks the server for its `app-grid` action).
    AppGrid,
    /// A command line, run detached.
    Exec(String),
    /// One of the server's named actions (the names of the shortcut file: `overview`, `app-overview`, `show-desktop`,
    /// `launcher`, `control-panel`, `notification-center`, `lock`, ...).
    Server(String),
}

/// Parse an action value: `none`, `app-grid`, `exec:<command line>`, else the name of a server action. Empty, and
/// `exec:` without a program, are invalid.
pub fn parse_action(value: &str) -> Option<ActionTarget> {
    let v = value.trim();
    Some(match v {
        "" => return None,
        "none" => ActionTarget::None,
        "app-grid" => ActionTarget::AppGrid,
        _ => match v.strip_prefix("exec:") {
            Some(cmd) if !cmd.trim().is_empty() => ActionTarget::Exec(cmd.trim().to_string()),
            Some(_) => return None,
            None => ActionTarget::Server(v.to_string()),
        },
    })
}

/// How an action is written.
pub fn action_text(action: &ActionTarget) -> String {
    match action {
        ActionTarget::None => "none".to_string(),
        ActionTarget::AppGrid => "app-grid".to_string(),
        ActionTarget::Exec(cmd) => format!("exec:{cmd}"),
        ActionTarget::Server(name) => name.clone(),
    }
}

/// The eight triggers and their actions.
///
/// Defaults: no corner does anything (corner action 1, "none", for all four). The gesture defaults
/// are the design's trackpad roles (roles on gestures,:
/// the four-finger pinch and spread are the application-grid and show-desktop gestures of the dock process): swipe up
/// opens the overview, swipe down the application overview, a pinch the application grid and a spread shows the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionMap {
    actions: [ActionTarget; 8],
}

impl Default for ActionMap {
    fn default() -> Self {
        Self {
            actions: [
                ActionTarget::None,
                ActionTarget::None,
                ActionTarget::None,
                ActionTarget::None,
                ActionTarget::Server("overview".to_string()),
                ActionTarget::Server("app-overview".to_string()),
                ActionTarget::AppGrid,
                ActionTarget::Server("show-desktop".to_string()),
            ],
        }
    }
}

impl ActionMap {
    pub fn set(&mut self, trigger: Trigger, action: ActionTarget) {
        self.actions[trigger.index()] = action;
    }

    pub fn get(&self, trigger: Trigger) -> &ActionTarget {
        &self.actions[trigger.index()]
    }
}

/// Everything in `dock.conf`; `None` is a key the file does not set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DockSettings {
    /// Pinned applications, in order.
    pub pinned: Vec<String>,
    pub tile_size: Option<f64>,
    pub large_size: Option<f64>,
    pub magnification: Option<bool>,
    pub autohide: Option<bool>,
    pub position: Option<Position>,
    pub recents: Option<bool>,
    pub stack: Option<StackStyle>,
    pub actions: ActionMap,
}

fn parse_size(value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("expected points, got {value:?}"))
}

fn pin(s: &mut DockSettings, value: &str) -> Result<(), String> {
    s.pinned.push(value.to_string());
    Ok(())
}

/// The key of trigger `i` (a `const` table entry per trigger).
macro_rules! trigger_key {
    ($i:expr) => {
        Key {
            name: TRIGGER_KEYS[$i],
            read: |s, v| {
                parse_action(v)
                    .map(|a| s.actions.set(Trigger::ALL[$i], a))
                    .ok_or_else(|| {
                        format!(
                            "expected none, app-grid, exec:<command> or an action name, got {v:?}"
                        )
                    })
            },
            write: |s| Some(action_text(s.actions.get(Trigger::ALL[$i]))),
            keep: |s, g| {
                s.actions
                    .set(Trigger::ALL[$i], g.actions.get(Trigger::ALL[$i]).clone())
            },
        }
    };
}

const KEYS: &[Key<DockSettings>] = &[
    // pins are read here and written by the dock to `dock-pins`, so they are never rewritten
    Key {
        name: "pin",
        read: pin,
        write: |_| None,
        keep: |_, _| {},
    },
    Key {
        name: "pinned",
        read: pin,
        write: |_| None,
        keep: |_, _| {},
    },
    Key {
        name: "tile_size",
        read: |s, v| parse_size(v).map(|n| s.tile_size = Some(n)),
        write: |s| s.tile_size.map(|n| n.to_string()),
        keep: |s, g| s.tile_size = g.tile_size,
    },
    Key {
        name: "large_size",
        read: |s, v| parse_size(v).map(|n| s.large_size = Some(n)),
        write: |s| s.large_size.map(|n| n.to_string()),
        keep: |s, g| s.large_size = g.large_size,
    },
    Key {
        name: "magnification",
        read: |s, v| parse_switch(v).map(|b| s.magnification = Some(b)),
        write: |s| s.magnification.map(switch_text),
        keep: |s, g| s.magnification = g.magnification,
    },
    Key {
        name: "autohide",
        read: |s, v| parse_switch(v).map(|b| s.autohide = Some(b)),
        write: |s| s.autohide.map(switch_text),
        keep: |s, g| s.autohide = g.autohide,
    },
    Key {
        name: "recents",
        read: |s, v| parse_switch(v).map(|b| s.recents = Some(b)),
        write: |s| s.recents.map(switch_text),
        keep: |s, g| s.recents = g.recents,
    },
    Key {
        name: "stack",
        read: |s, v| {
            s.stack = Some(match v {
                "auto" => StackStyle::Auto,
                "fan" => StackStyle::Fan,
                "grid" => StackStyle::Grid,
                other => return Err(format!("expected auto, fan or grid, got {other:?}")),
            });
            Ok(())
        },
        write: |s| {
            s.stack.map(|p| match p {
                StackStyle::Auto => "auto".to_string(),
                StackStyle::Fan => "fan".to_string(),
                StackStyle::Grid => "grid".to_string(),
            })
        },
        keep: |s, g| s.stack = g.stack,
    },
    Key {
        name: "position",
        read: |s, v| {
            s.position = Some(match v {
                "bottom" => Position::Bottom,
                "left" => Position::Left,
                "right" => Position::Right,
                other => return Err(format!("expected bottom, left or right, got {other:?}")),
            });
            Ok(())
        },
        write: |s| {
            s.position.map(|p| match p {
                Position::Bottom => "bottom".to_string(),
                Position::Left => "left".to_string(),
                Position::Right => "right".to_string(),
            })
        },
        keep: |s, g| s.position = g.position,
    },
    trigger_key!(0),
    trigger_key!(1),
    trigger_key!(2),
    trigger_key!(3),
    trigger_key!(4),
    trigger_key!(5),
    trigger_key!(6),
    trigger_key!(7),
];

fn bare_pin(s: &mut DockSettings, word: &str) {
    s.pinned.push(word.to_string());
}

impl DockSettings {
    /// Where the file is: `$CUPERTINO_DOCK_CONFIG`, else `dock.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(DOCK_CONF_ENV, "dock.conf")
    }

    /// The settings of `text` and the problems of its lines; with `last_good`, a key whose line is wrong keeps its
    /// last good value.
    pub fn parse(text: &str, last_good: Option<&Self>) -> (Self, Vec<Problem>) {
        conf::read(
            &conf::ConfText::parse(text),
            KEYS,
            &Self::default(),
            last_good,
            Some(bare_pin),
        )
    }

    /// [`Self::parse`] of the file at `path`; a missing file sets nothing.
    pub fn load(path: &Path, last_good: Option<&Self>) -> io::Result<(Self, Vec<Problem>)> {
        conf::load(path, KEYS, &Self::default(), last_good, Some(bare_pin))
    }

    /// Write the keys whose value differs from the file's into `path` atomically; pins, comments and other lines stay.
    pub fn store(&self, path: &Path) -> io::Result<Vec<Problem>> {
        conf::store(path, KEYS, &Self::default(), Some(bare_pin), self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_and_options_are_read() {
        let (f, problems) = DockSettings::parse("# comment\nfoot\npin = firefox\ntile_size = 48\nlarge_size=96\nmagnification = true\nautohide = no\nposition = left\nrecents=false\n", None);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(f.pinned, ["foot", "firefox"]);
        assert_eq!((f.tile_size, f.large_size), (Some(48.0), Some(96.0)));
        assert_eq!(
            (f.magnification, f.autohide, f.recents),
            (Some(true), Some(false), Some(false))
        );
        assert_eq!(f.position, Some(Position::Left));
        assert_eq!(f.actions, ActionMap::default());
        assert_eq!(DockSettings::parse("", None).0, DockSettings::default());
    }

    #[test]
    fn trigger_keys_set_the_actions_and_bad_values_are_problems() {
        let (f, problems) = DockSettings::parse("corner_bottom_right = show-desktop\nswipe_up = none\npinch = exec:foot\nspread = exec:\nautohide = maybe\nposition = top\nunknown = 1\n", None);
        assert_eq!(
            f.actions.get(Trigger::CornerBottomRight),
            &ActionTarget::Server("show-desktop".into())
        );
        assert_eq!(f.actions.get(Trigger::SwipeUp), &ActionTarget::None);
        assert_eq!(
            f.actions.get(Trigger::Pinch),
            &ActionTarget::Exec("foot".into())
        );
        assert_eq!(
            f.actions.get(Trigger::Spread),
            ActionMap::default().get(Trigger::Spread),
            "an empty program is invalid"
        );
        assert_eq!((f.autohide, f.position), (None, None));
        assert_eq!(
            problems.iter().map(|p| p.line).collect::<Vec<_>>(),
            vec![4, 5, 6, 7]
        );
        assert!(f.pinned.is_empty(), "trigger lines are settings, not pins");
    }

    #[test]
    fn values_parse_to_their_targets() {
        assert_eq!(parse_action("none"), Some(ActionTarget::None));
        assert_eq!(parse_action(" app-grid "), Some(ActionTarget::AppGrid));
        assert_eq!(
            parse_action("exec:foot -e htop"),
            Some(ActionTarget::Exec("foot -e htop".into()))
        );
        assert_eq!(
            parse_action("show-desktop"),
            Some(ActionTarget::Server("show-desktop".into()))
        );
        assert_eq!(parse_action("exec:"), None, "a program is needed");
        assert_eq!(parse_action(""), None);
        for a in [
            ActionTarget::None,
            ActionTarget::AppGrid,
            ActionTarget::Exec("foot -e htop".into()),
            ActionTarget::Server("lock".into()),
        ] {
            assert_eq!(parse_action(&action_text(&a)), Some(a));
        }
    }

    #[test]
    fn the_defaults_bind_no_corner_and_the_reference_gestures() {
        let m = ActionMap::default();
        for t in &Trigger::ALL[..4] {
            assert_eq!(m.get(*t), &ActionTarget::None, "{t:?}");
        }
        assert_eq!(
            m.get(Trigger::SwipeUp),
            &ActionTarget::Server("overview".into())
        );
        assert_eq!(
            m.get(Trigger::SwipeDown),
            &ActionTarget::Server("app-overview".into())
        );
        assert_eq!(m.get(Trigger::Pinch), &ActionTarget::AppGrid);
        assert_eq!(
            m.get(Trigger::Spread),
            &ActionTarget::Server("show-desktop".into())
        );
        assert_eq!(
            Trigger::from_key("corner_bottom_right"),
            Some(Trigger::CornerBottomRight)
        );
        assert_eq!(Trigger::from_key("corner"), None);
    }

    #[test]
    fn sizes_follow_the_reference_validation() {
        assert_eq!(sanitize_tile_size(Some(47.2), 1080.0), 48.0);
        assert_eq!(sanitize_tile_size(Some(4000.0), 1080.0), 64.0);
        assert_eq!(
            sanitize_tile_size(None, 768.0),
            52.0,
            "small displays get the small default"
        );
        assert_eq!(sanitize_large_size(Some(512.5)), 513.0);
        assert_eq!(sanitize_large_size(Some(513.0)), 128.0);
        assert_eq!(sanitize_large_size(Some(96.0)), 96.0);
    }

    #[test]
    fn a_store_keeps_pins_corner_lines_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dock.conf");
        std::fs::write(&path, "# my dock\nfoot\npin = firefox\ncorner_top_left = lock # hot corner\nswipe_up = none\ntile_size = 48\n").unwrap();
        let (mut s, _) = DockSettings::load(&path, None).unwrap();
        s.tile_size = Some(36.0);
        s.magnification = Some(true);
        s.large_size = None;
        assert!(s.store(&path).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# my dock\nfoot\npin = firefox\ncorner_top_left = lock # hot corner\nswipe_up = none\ntile_size = 36\nmagnification = on\n");
        assert_eq!(DockSettings::load(&path, None).unwrap().0, s);
    }

    #[test]
    fn a_bad_value_keeps_the_last_good_one() {
        let good = DockSettings {
            autohide: Some(true),
            tile_size: Some(40.0),
            ..DockSettings::default()
        };
        let (s, problems) =
            DockSettings::parse("autohide = sometimes\ntile_size = huge\n", Some(&good));
        assert_eq!((s.autohide, s.tile_size), (Some(true), Some(40.0)));
        assert_eq!(problems.len(), 2);
    }
}
