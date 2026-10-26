//! The tool's settings, `screenshot.conf` in the Cupertino settings directory (`CUPERTINO_SCREENSHOT_CONFIG` redirects
//! it): where captures go, the timer, the Options-menu switches, and the remembered mode and selection. The toolbar's
//! Options menu changes them and the file is written back at once.
//!
//! ```text
//! location = desktop          # desktop, documents, pictures, clipboard or an absolute directory
//! timer = 0                   # 0, 5 or 10 seconds
//! show-thumbnail = true
//! show-pointer = false
//! remember-selection = true
//! mode = selection            # screen, window or selection: the toolbar's last mode
//! selection = 100,80,640,400  # x,y,width,height in global points: the last selection
//! ```
//!
//! The design keeps these in its user defaults; the fallbacks of those keys are not verified, so the
//! defaults below are approximate except `remember-selection` (`save-selections` true).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use cupertino_geometry::Rect;

use crate::values::TIMER_CHOICES;

/// Where a capture goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// The XDG desktop directory (the design's default).
    Desktop,
    Documents,
    Pictures,
    Clipboard,
    Directory(PathBuf),
}

impl Location {
    fn parse(value: &str) -> Result<Self, String> {
        Ok(match value {
            "desktop" => Location::Desktop,
            "documents" => Location::Documents,
            "pictures" => Location::Pictures,
            "clipboard" => Location::Clipboard,
            path if path.starts_with('/') => Location::Directory(PathBuf::from(path)),
            other => return Err(format!("location {other:?} is not desktop, documents, pictures, clipboard or an absolute directory")),
        })
    }

    fn name(&self) -> String {
        match self {
            Location::Desktop => "desktop".into(),
            Location::Documents => "documents".into(),
            Location::Pictures => "pictures".into(),
            Location::Clipboard => "clipboard".into(),
            Location::Directory(path) => path.display().to_string(),
        }
    }

    /// The directory files go to, from the session's `user-dirs.dirs` assignments; `None` for the clipboard.
    pub fn directory(&self, home: &Path, user_dirs: &[(String, PathBuf)]) -> Option<PathBuf> {
        let key = match self {
            Location::Desktop => "DESKTOP",
            Location::Documents => "DOCUMENTS",
            Location::Pictures => "PICTURES",
            Location::Clipboard => return None,
            Location::Directory(path) => return Some(path.clone()),
        };
        Some(user_dir(key, home, user_dirs))
    }
}

/// What the toolbar captures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Screen,
    Window,
    Selection,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Screen => "screen",
            Mode::Window => "window",
            Mode::Selection => "selection",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Mode::Screen, Mode::Window, Mode::Selection].into_iter().find(|m| m.name() == name)
    }
}

/// The settings file's contents.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub location: Location,
    /// Seconds before the capture, one of [`TIMER_CHOICES`].
    pub timer: u32,
    pub show_thumbnail: bool,
    /// Whether the pointer is in the image.
    pub show_pointer: bool,
    pub remember_selection: bool,
    pub mode: Mode,
    pub selection: Option<Rect>,
}

impl Default for Settings {
    /// Desktop (the design's default location), no timer, the thumbnail on, the pointer left out, the selection
    /// remembered, selection mode.
    fn default() -> Self {
        Self { location: Location::Desktop, timer: 0, show_thumbnail: true, show_pointer: false, remember_selection: true, mode: Mode::Selection, selection: None }
    }
}

/// A line of the file that could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub line: usize,
    pub problem: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.problem)
    }
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{other:?} is not true or false")),
    }
}

fn parse_rect(value: &str) -> Result<Rect, String> {
    let parts: Vec<f64> = value.split(',').map(|p| p.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|_| format!("{value:?} is not x,y,width,height"))?;
    match parts[..] {
        [x, y, w, h] if w >= 1.0 && h >= 1.0 => Ok(Rect::new(x, y, w, h)),
        _ => Err(format!("{value:?} is not x,y,width,height with a size of at least one point")),
    }
}

impl Settings {
    /// Parse the file; a line with a problem keeps the setting it names at its default and is reported.
    pub fn parse(text: &str) -> (Settings, Vec<Problem>) {
        let mut s = Settings::default();
        let mut problems = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let applied = match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                None => Err(format!("expected `key = value`, got {line:?}")),
                Some(("location", v)) => Location::parse(v).map(|l| s.location = l),
                Some(("timer", v)) => match v.parse::<u32>() {
                    Ok(t) if TIMER_CHOICES.contains(&t) => {
                        s.timer = t;
                        Ok(())
                    }
                    _ => Err(format!("timer {v:?} is not one of {TIMER_CHOICES:?}")),
                },
                Some(("show-thumbnail", v)) => parse_bool(v).map(|b| s.show_thumbnail = b),
                Some(("show-pointer", v)) => parse_bool(v).map(|b| s.show_pointer = b),
                Some(("remember-selection", v)) => parse_bool(v).map(|b| s.remember_selection = b),
                Some(("mode", v)) => Mode::from_name(v).map(|m| s.mode = m).ok_or_else(|| format!("mode {v:?} is not screen, window or selection")),
                Some(("selection", v)) => parse_rect(v).map(|r| s.selection = Some(r)),
                Some((key, _)) => Err(format!("unknown setting {key:?}")),
            };
            if let Err(problem) = applied {
                problems.push(Problem { line: i + 1, problem });
            }
        }
        (s, problems)
    }

    /// The file's text; [`Settings::parse`] reads it back unchanged.
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "# Screenshot settings, written by the screenshot tool's Options menu.\nlocation = {}\ntimer = {}\nshow-thumbnail = {}\nshow-pointer = {}\nremember-selection = {}\nmode = {}\n",
            self.location.name(),
            self.timer,
            self.show_thumbnail,
            self.show_pointer,
            self.remember_selection,
            self.mode.name()
        );
        if let Some(r) = self.selection {
            out.push_str(&format!("selection = {},{},{},{}\n", r.x(), r.y(), r.width(), r.height()));
        }
        out
    }

    /// Read `path`. A missing file is the defaults; problems are returned with the settings.
    pub fn load(path: &Path) -> io::Result<(Settings, Vec<Problem>)> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((Settings::default(), Vec::new())),
            Err(e) => Err(e),
        }
    }

    /// Write `path` atomically (a temporary file renamed over it), creating its directory.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }
}

/// The `XDG_<NAME>_DIR` assignments of a `user-dirs.dirs` file as `(NAME, path)`, `$HOME` expanded against `home`
/// (xdg-user-dirs: `XDG_<NAME>_DIR="$HOME/<path>" or an absolute path). Comments, malformed lines and
/// relative paths without `$HOME/ are skipped, as xdg-user-dirs does. The same parser lives in the file manager
/// (`cupertino-files`); a shell process cannot depend on an application crate.
pub fn parse_user_dirs(text: &str, home: &Path) -> Vec<(String, PathBuf)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            let name = key.trim().strip_prefix("XDG_")?.strip_suffix("_DIR")?;
            let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
            let path = if value == "$HOME" {
                home.to_path_buf()
            } else if let Some(rest) = value.strip_prefix("$HOME/") {
                home.join(rest)
            } else if value.starts_with('/') {
                PathBuf::from(value)
            } else {
                return None;
            };
            Some((name.to_string(), path))
        })
        .collect()
}

/// The user directory `key` (`DESKTOP`, `DOCUMENTS`, `PICTURES`): the last assignment, else the xdg-user-dirs default
/// `$HOME/Desktop`, `$HOME/Documents`, `$HOME/Pictures` (`user-dirs.defaults`).
fn user_dir(key: &str, home: &Path, user_dirs: &[(String, PathBuf)]) -> PathBuf {
    if let Some((_, path)) = user_dirs.iter().rev().find(|(k, _)| k == key) {
        return path.clone();
    }
    let default = match key {
        "DESKTOP" => "Desktop",
        "DOCUMENTS" => "Documents",
        "PICTURES" => "Pictures",
        other => unreachable!("no location uses the user directory {other}"),
    };
    home.join(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_round_trips_and_a_bad_line_is_reported_and_left_at_its_default() {
        let s = Settings {
            location: Location::Directory(PathBuf::from("/data/shots")),
            timer: 5,
            show_thumbnail: false,
            show_pointer: true,
            remember_selection: false,
            mode: Mode::Window,
            selection: Some(Rect::new(10.0, 20.0, 300.0, 200.0)),
        };
        assert_eq!(Settings::parse(&s.to_text()), (s, Vec::new()));
        let (parsed, problems) = Settings::parse("timer = 3\nmode = window\nlocation = shots\nselection = 1,2,0,4\n");
        assert_eq!(parsed, Settings { mode: Mode::Window, ..Settings::default() });
        assert_eq!(problems.iter().map(|p| p.line).collect::<Vec<_>>(), [1, 3, 4]);
    }

    #[test]
    fn locations_resolve_through_the_user_dirs_with_the_xdg_defaults() {
        let home = Path::new("/home/u");
        let dirs = parse_user_dirs("# comment\nXDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"\nXDG_PICTURES_DIR=\"/data/pictures\"\nXDG_MUSIC_DIR=\"relative\"\n", home);
        assert_eq!(Location::Desktop.directory(home, &dirs), Some(PathBuf::from("/home/u/Schreibtisch")));
        assert_eq!(Location::Pictures.directory(home, &dirs), Some(PathBuf::from("/data/pictures")));
        assert_eq!(Location::Documents.directory(home, &dirs), Some(PathBuf::from("/home/u/Documents")));
        assert_eq!(Location::Clipboard.directory(home, &dirs), None);
        assert_eq!(Location::Desktop.directory(home, &[]), Some(PathBuf::from("/home/u/Desktop")));
    }
}
