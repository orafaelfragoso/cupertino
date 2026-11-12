//! `appearance.conf` (`$CUPERTINO_APPEARANCE_CONFIG` overrides the path): what a person chose about how the desktop
//! looks and how windows behave under the pointer. Every program that draws with the toolkit reads it and follows
//! changes while it runs; the settings application writes it. A key the file does not set reads as its default.
//!
//! | Key | Values | Unset |
//! |---|---|---|
//! | `auto` | `on`, `off`: light by day and dark by night, on the schedule below | `off` |
//! | `auto_dark_from` | `HH:MM`: when the dark look starts | `19:00` |
//! | `auto_light_from` | `HH:MM`: when the light look starts | `07:00` |
//! | `accent` | `multicolor`, `blue`, `purple`, `pink`, `red`, `orange`, `yellow`, `green`, `graphite` | `multicolor` |
//! | `highlight` | `automatic`, or one of the accent names but `multicolor` | `automatic` |
//! | `sidebar_icons` | `small`, `medium`, `large` | `medium` |
//! | `scroll_click` | `next-page`, `jump-to-spot`: what a click in a scroll bar's track does | `next-page` |
//! | `tint_windows` | `on`, `off`: windows take a tint of the wallpaper's colour | `on` |
//! | `titlebar_double_click` | `zoom`, `minimize`, `none` | `zoom` |

use std::io;
use std::path::{Path, PathBuf};

use crate::conf::{self, parse_switch, switch_text, Key, Problem};

/// Environment variable that redirects the settings file.
pub const APPEARANCE_CONF_ENV: &str = "CUPERTINO_APPEARANCE_CONFIG";

/// The colour controls show a selection or a default choice in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Accent {
    /// The system's own blue.
    #[default]
    Multicolor,
    Blue,
    Purple,
    Pink,
    Red,
    Orange,
    Yellow,
    Green,
    Graphite,
}

impl Accent {
    pub const ALL: [Accent; 9] = [
        Accent::Multicolor,
        Accent::Blue,
        Accent::Purple,
        Accent::Pink,
        Accent::Red,
        Accent::Orange,
        Accent::Yellow,
        Accent::Green,
        Accent::Graphite,
    ];

    /// How the file writes it.
    pub fn name(self) -> &'static str {
        match self {
            Accent::Multicolor => "multicolor",
            Accent::Blue => "blue",
            Accent::Purple => "purple",
            Accent::Pink => "pink",
            Accent::Red => "red",
            Accent::Orange => "orange",
            Accent::Yellow => "yellow",
            Accent::Green => "green",
            Accent::Graphite => "graphite",
        }
    }

    pub fn from_name(name: &str) -> Option<Accent> {
        Accent::ALL.into_iter().find(|a| a.name() == name)
    }

    /// The colour as sRGB, in the light or the dark appearance.
    pub fn rgb(self, dark: bool) -> [u8; 3] {
        match (self, dark) {
            (Accent::Multicolor | Accent::Blue, _) => [0, 122, 255],
            (Accent::Purple, false) => [149, 61, 150],
            (Accent::Purple, true) => [165, 80, 167],
            (Accent::Pink, _) => [247, 79, 158],
            (Accent::Red, false) => [224, 56, 62],
            (Accent::Red, true) => [255, 82, 87],
            (Accent::Orange, _) => [247, 130, 27],
            (Accent::Yellow, false) => [255, 199, 38],
            (Accent::Yellow, true) => [255, 198, 0],
            (Accent::Green, _) => [98, 186, 70],
            (Accent::Graphite, false) => [152, 152, 152],
            (Accent::Graphite, true) => [140, 140, 140],
        }
    }
}

/// The colour of selected text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Highlight {
    /// A light tint of the accent.
    #[default]
    Automatic,
    /// A light tint of this colour (never [`Accent::Multicolor`]).
    Color(Accent),
}

/// How large a sidebar's icons and rows are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SidebarIcons {
    Small,
    #[default]
    Medium,
    Large,
}

impl SidebarIcons {
    pub const ALL: [SidebarIcons; 3] = [
        SidebarIcons::Small,
        SidebarIcons::Medium,
        SidebarIcons::Large,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SidebarIcons::Small => "small",
            SidebarIcons::Medium => "medium",
            SidebarIcons::Large => "large",
        }
    }
}

/// What a click in a scroll bar's track, off the knob, does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollClick {
    /// Scroll by one page towards the click.
    #[default]
    NextPage,
    /// Move the knob to the click.
    JumpToSpot,
}

/// What a double click on a window's title area does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TitlebarAction {
    #[default]
    Zoom,
    Minimize,
    None,
}

impl TitlebarAction {
    pub const ALL: [TitlebarAction; 3] = [
        TitlebarAction::Zoom,
        TitlebarAction::Minimize,
        TitlebarAction::None,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TitlebarAction::Zoom => "zoom",
            TitlebarAction::Minimize => "minimize",
            TitlebarAction::None => "none",
        }
    }
}

/// A time of day in minutes after midnight.
pub type Minutes = u32;

/// `HH:MM` as minutes after midnight.
pub fn parse_time(value: &str) -> Result<Minutes, String> {
    let bad = || format!("expected HH:MM, got {value:?}");
    let (h, m) = value.split_once(':').ok_or_else(bad)?;
    let (h, m): (u32, u32) = (
        h.trim().parse().map_err(|_| bad())?,
        m.trim().parse().map_err(|_| bad())?,
    );
    if h > 23 || m > 59 {
        return Err(bad());
    }
    Ok(h * 60 + m)
}

/// Minutes after midnight as `HH:MM`.
pub fn time_text(minutes: Minutes) -> String {
    format!("{:02}:{:02}", minutes / 60 % 24, minutes % 60)
}

/// Everything in `appearance.conf`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppearanceSettings {
    pub auto: bool,
    pub auto_dark_from: Minutes,
    pub auto_light_from: Minutes,
    pub accent: Accent,
    pub highlight: Highlight,
    pub sidebar_icons: SidebarIcons,
    pub scroll_click: ScrollClick,
    pub tint_windows: bool,
    pub titlebar_double_click: TitlebarAction,
}

impl AppearanceSettings {
    /// What an empty file reads as.
    pub const UNSET: Self = Self {
        auto: false,
        auto_dark_from: 19 * 60,
        auto_light_from: 7 * 60,
        accent: Accent::Multicolor,
        highlight: Highlight::Automatic,
        sidebar_icons: SidebarIcons::Medium,
        scroll_click: ScrollClick::NextPage,
        tint_windows: true,
        titlebar_double_click: TitlebarAction::Zoom,
    };

    /// Whether the dark look is due at `minutes` after midnight on the schedule: from `auto_dark_from` until
    /// `auto_light_from`, across midnight when the dark start is the later. Equal times mean always light.
    pub fn dark_at(&self, minutes: Minutes) -> bool {
        let (dark, light) = (self.auto_dark_from, self.auto_light_from);
        if dark == light {
            false
        } else if dark < light {
            (dark..light).contains(&minutes)
        } else {
            minutes >= dark || minutes < light
        }
    }
}

const KEYS: &[Key<AppearanceSettings>] = &[
    Key {
        name: "auto",
        read: |s, v| parse_switch(v).map(|on| s.auto = on),
        write: |s| Some(switch_text(s.auto)),
        keep: |s, g| s.auto = g.auto,
    },
    Key {
        name: "auto_dark_from",
        read: |s, v| parse_time(v).map(|t| s.auto_dark_from = t),
        write: |s| Some(time_text(s.auto_dark_from)),
        keep: |s, g| s.auto_dark_from = g.auto_dark_from,
    },
    Key {
        name: "auto_light_from",
        read: |s, v| parse_time(v).map(|t| s.auto_light_from = t),
        write: |s| Some(time_text(s.auto_light_from)),
        keep: |s, g| s.auto_light_from = g.auto_light_from,
    },
    Key {
        name: "accent",
        read: |s, v| {
            s.accent = Accent::from_name(v)
                .ok_or_else(|| format!("expected an accent colour name, got {v:?}"))?;
            Ok(())
        },
        write: |s| Some(s.accent.name().to_string()),
        keep: |s, g| s.accent = g.accent,
    },
    Key {
        name: "highlight",
        read: |s, v| {
            s.highlight = match (v, Accent::from_name(v)) {
                ("automatic", _) => Highlight::Automatic,
                (_, Some(Accent::Multicolor)) | (_, None) => {
                    return Err(format!("expected automatic or a colour name, got {v:?}"))
                }
                (_, Some(accent)) => Highlight::Color(accent),
            };
            Ok(())
        },
        write: |s| {
            Some(match s.highlight {
                Highlight::Automatic => "automatic".to_string(),
                Highlight::Color(a) => a.name().to_string(),
            })
        },
        keep: |s, g| s.highlight = g.highlight,
    },
    Key {
        name: "sidebar_icons",
        read: |s, v| {
            s.sidebar_icons = SidebarIcons::ALL
                .into_iter()
                .find(|i| i.name() == v)
                .ok_or_else(|| format!("expected small, medium or large, got {v:?}"))?;
            Ok(())
        },
        write: |s| Some(s.sidebar_icons.name().to_string()),
        keep: |s, g| s.sidebar_icons = g.sidebar_icons,
    },
    Key {
        name: "scroll_click",
        read: |s, v| {
            s.scroll_click = match v {
                "next-page" => ScrollClick::NextPage,
                "jump-to-spot" => ScrollClick::JumpToSpot,
                other => return Err(format!("expected next-page or jump-to-spot, got {other:?}")),
            };
            Ok(())
        },
        write: |s| {
            Some(
                match s.scroll_click {
                    ScrollClick::NextPage => "next-page",
                    ScrollClick::JumpToSpot => "jump-to-spot",
                }
                .to_string(),
            )
        },
        keep: |s, g| s.scroll_click = g.scroll_click,
    },
    Key {
        name: "tint_windows",
        read: |s, v| parse_switch(v).map(|on| s.tint_windows = on),
        write: |s| Some(switch_text(s.tint_windows)),
        keep: |s, g| s.tint_windows = g.tint_windows,
    },
    Key {
        name: "titlebar_double_click",
        read: |s, v| {
            s.titlebar_double_click = TitlebarAction::ALL
                .into_iter()
                .find(|a| a.name() == v)
                .ok_or_else(|| format!("expected zoom, minimize or none, got {v:?}"))?;
            Ok(())
        },
        write: |s| Some(s.titlebar_double_click.name().to_string()),
        keep: |s, g| s.titlebar_double_click = g.titlebar_double_click,
    },
];

impl AppearanceSettings {
    /// Where the file is: `$CUPERTINO_APPEARANCE_CONFIG`, else `appearance.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(APPEARANCE_CONF_ENV, "appearance.conf")
    }

    /// The settings of `text` and the problems of its lines; with `last_good`, a key whose line is wrong keeps its
    /// last good value.
    pub fn parse(text: &str, last_good: Option<&Self>) -> (Self, Vec<Problem>) {
        conf::read(
            &conf::ConfText::parse(text),
            KEYS,
            &Self::UNSET,
            last_good,
            None,
        )
    }

    /// [`Self::parse`] of the file at `path`; a missing file is every key unset.
    pub fn load(path: &Path, last_good: Option<&Self>) -> io::Result<(Self, Vec<Problem>)> {
        conf::load(path, KEYS, &Self::UNSET, last_good, None)
    }

    /// The settings in effect now: the file as it is, else every key unset; lines it does not understand are skipped.
    pub fn current() -> Self {
        Self::path()
            .and_then(|p| Self::load(&p, None).ok())
            .map_or(Self::UNSET, |(s, _)| s)
    }

    /// Write the keys whose value differs from the file's into `path` atomically; comments and other lines stay.
    pub fn store(&self, path: &Path) -> io::Result<Vec<Problem>> {
        conf::store(path, KEYS, &Self::UNSET, None, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schedule_runs_across_midnight() {
        let s = AppearanceSettings::UNSET;
        assert!(s.dark_at(23 * 60));
        assert!(s.dark_at(3 * 60));
        assert!(!s.dark_at(12 * 60));
        assert!(!s.dark_at(7 * 60));
        assert!(s.dark_at(19 * 60));
    }
}
