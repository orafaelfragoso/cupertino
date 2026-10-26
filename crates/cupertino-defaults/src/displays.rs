//! `displays.conf`: the arrangement the user gave each display, keyed by the display's identity,
//! so a display gets it back when it is plugged in again. The compositor writes the file whenever
//! a configuration is applied and reads it at start and at every hot-plug.
//!
//! ```text
//! [GSM LG HDR 4K 0x0001a2b3]
//! enabled = true
//! mode = 3840x2160@60000
//! position = 0,0
//! scale = 1.5
//! transform = normal
//! ```
//!
//! A section header is the display's key ([`display_key`]). `mode` is the pixel size and the refresh
//! rate in millihertz; `position` is the top-left corner in the global logical space; `scale` is
//! pixels per point. Every key is required in a section.

use std::fmt;
use std::io;
use std::path::Path;

/// Environment variable that redirects `displays.conf`.
pub const DISPLAYS_ENV: &str = "CUPERTINO_DISPLAYS_CONFIG";
/// File name under the configuration directory.
pub const DISPLAYS_FILE: &str = "displays.conf";

/// Rotation and mirroring of a display, as the output protocols name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    const NAMES: [(Transform, &'static str); 8] = [
        (Transform::Normal, "normal"),
        (Transform::Rotate90, "90"),
        (Transform::Rotate180, "180"),
        (Transform::Rotate270, "270"),
        (Transform::Flipped, "flipped"),
        (Transform::Flipped90, "flipped-90"),
        (Transform::Flipped180, "flipped-180"),
        (Transform::Flipped270, "flipped-270"),
    ];

    pub fn name(self) -> &'static str {
        Self::NAMES.iter().find(|(t, _)| *t == self).map(|(_, n)| *n).expect("every transform is named")
    }

    pub fn from_name(name: &str) -> Option<Transform> {
        Self::NAMES.iter().find(|(_, n)| *n == name).map(|(t, _)| *t)
    }
}

/// A mode by its pixel size and refresh rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSetting {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

/// What one display was set to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplaySettings {
    pub enabled: bool,
    pub mode: ModeSetting,
    pub position: (i32, i32),
    pub scale: f64,
    pub transform: Transform,
}

/// Every display the user configured, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplaysConfig {
    pub displays: Vec<(String, DisplaySettings)>,
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

/// The key a display's settings are stored under: its maker, model and serial number when its
/// EDID names a serial (so the settings follow the display to another connector), else the
/// connector, since two identical displays without serials cannot be told apart otherwise.
/// Characters that would end the section header become `_`.
pub fn display_key(connector: &str, make: &str, model: &str, serial: Option<&str>) -> String {
    let key = match serial.filter(|s| !s.trim().is_empty()) {
        Some(serial) => format!("{} {} {}", make.trim(), model.trim(), serial.trim()),
        None => format!("connector {}", connector.trim()),
    };
    key.chars().map(|c| if c == ']' || c.is_control() { '_' } else { c }).collect()
}

impl DisplaysConfig {
    pub fn get(&self, key: &str) -> Option<&DisplaySettings> {
        self.displays.iter().find(|(k, _)| k == key).map(|(_, s)| s)
    }

    /// Store `settings` under `key`, replacing what was there.
    pub fn set(&mut self, key: &str, settings: DisplaySettings) {
        match self.displays.iter_mut().find(|(k, _)| k == key) {
            Some((_, s)) => *s = settings,
            None => self.displays.push((key.to_string(), settings)),
        }
    }

    /// Parse the file. Sections with a problem are dropped whole and reported, so a display never
    /// gets half of what was stored for it.
    pub fn parse(text: &str) -> (DisplaysConfig, Vec<Problem>) {
        let mut config = DisplaysConfig::default();
        let mut problems = Vec::new();
        let mut section: Option<Section> = None;
        for (i, raw) in text.lines().enumerate() {
            let line_no = i + 1;
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(header) = line.strip_prefix('[') {
                if let Some(done) = section.take() {
                    done.finish(&mut config, &mut problems);
                }
                match header.strip_suffix(']').map(str::trim).filter(|k| !k.is_empty()) {
                    Some(key) => section = Some(Section::new(key, line_no)),
                    None => problems.push(Problem { line: line_no, problem: format!("expected `[display key]`, got {line:?}") }),
                }
                continue;
            }
            let Some(current) = section.as_mut() else {
                problems.push(Problem { line: line_no, problem: "setting outside a display section".into() });
                continue;
            };
            let Some((key, value)) = line.split_once('=') else {
                current.error(line_no, format!("expected `key = value`, got {line:?}"));
                continue;
            };
            if let Err(e) = current.apply(key.trim(), value.trim()) {
                current.error(line_no, e);
            }
        }
        if let Some(done) = section.take() {
            done.finish(&mut config, &mut problems);
        }
        (config, problems)
    }

    /// The file's text; [`DisplaysConfig::parse`] reads it back unchanged.
    pub fn to_text(&self) -> String {
        let mut out = String::from("# Display arrangement, written by the compositor when a configuration is applied.\n");
        for (key, s) in &self.displays {
            out.push_str(&format!(
                "\n[{key}]\nenabled = {}\nmode = {}x{}@{}\nposition = {},{}\nscale = {}\ntransform = {}\n",
                s.enabled,
                s.mode.width,
                s.mode.height,
                s.mode.refresh_mhz,
                s.position.0,
                s.position.1,
                s.scale,
                s.transform.name()
            ));
        }
        out
    }

    /// Read `path`. A missing file is an empty configuration; problems are returned with it.
    pub fn load(path: &Path) -> io::Result<(DisplaysConfig, Vec<Problem>)> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((DisplaysConfig::default(), Vec::new())),
            Err(e) => Err(e),
        }
    }

    /// Write `path` atomically ([`crate::atomic::write_atomic`]), creating its directory.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        crate::atomic::write_atomic(path, self.to_text().as_bytes())
    }
}

/// A section being read.
struct Section {
    key: String,
    line: usize,
    enabled: Option<bool>,
    mode: Option<ModeSetting>,
    position: Option<(i32, i32)>,
    scale: Option<f64>,
    transform: Option<Transform>,
    errors: Vec<Problem>,
}

impl Section {
    fn new(key: &str, line: usize) -> Section {
        Section { key: key.to_string(), line, enabled: None, mode: None, position: None, scale: None, transform: None, errors: Vec::new() }
    }

    fn error(&mut self, line: usize, problem: String) {
        self.errors.push(Problem { line, problem });
    }

    fn apply(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "enabled" => self.enabled = Some(value.parse().map_err(|_| format!("enabled: expected true or false, got {value:?}"))?),
            "mode" => self.mode = Some(parse_mode(value)?),
            "position" => self.position = Some(parse_position(value)?),
            "scale" => {
                let scale: f64 = value.parse().map_err(|_| format!("scale: expected a number, got {value:?}"))?;
                if !(scale.is_finite() && scale > 0.0) {
                    return Err(format!("scale: must be above zero, got {value}"));
                }
                self.scale = Some(scale);
            }
            "transform" => self.transform = Some(Transform::from_name(value).ok_or_else(|| format!("transform: unknown {value:?}"))?),
            other => return Err(format!("unknown setting {other:?}")),
        }
        Ok(())
    }

    fn finish(self, config: &mut DisplaysConfig, problems: &mut Vec<Problem>) {
        let missing: Vec<&str> = [
            ("enabled", self.enabled.is_none()),
            ("mode", self.mode.is_none()),
            ("position", self.position.is_none()),
            ("scale", self.scale.is_none()),
            ("transform", self.transform.is_none()),
        ]
        .into_iter()
        .filter(|(_, m)| *m)
        .map(|(n, _)| n)
        .collect();
        let dropped = !self.errors.is_empty() || !missing.is_empty();
        problems.extend(self.errors);
        if !missing.is_empty() {
            problems.push(Problem { line: self.line, problem: format!("[{}] lacks {}", self.key, missing.join(", ")) });
        }
        if dropped {
            problems.push(Problem { line: self.line, problem: format!("[{}] ignored", self.key) });
            return;
        }
        let (Some(enabled), Some(mode), Some(position), Some(scale), Some(transform)) = (self.enabled, self.mode, self.position, self.scale, self.transform) else {
            unreachable!("checked for missing settings above")
        };
        config.set(&self.key, DisplaySettings { enabled, mode, position, scale, transform });
    }
}

fn parse_mode(value: &str) -> Result<ModeSetting, String> {
    let bad = || format!("mode: expected WIDTHxHEIGHT@MILLIHERTZ, got {value:?}");
    let (size, refresh) = value.split_once('@').ok_or_else(bad)?;
    let (w, h) = size.split_once('x').ok_or_else(bad)?;
    let mode = ModeSetting {
        width: w.trim().parse().map_err(|_| bad())?,
        height: h.trim().parse().map_err(|_| bad())?,
        refresh_mhz: refresh.trim().parse().map_err(|_| bad())?,
    };
    if mode.width == 0 || mode.height == 0 {
        return Err(bad());
    }
    Ok(mode)
}

fn parse_position(value: &str) -> Result<(i32, i32), String> {
    let bad = || format!("position: expected X,Y, got {value:?}");
    let (x, y) = value.split_once(',').ok_or_else(bad)?;
    Ok((x.trim().parse().map_err(|_| bad())?, y.trim().parse().map_err(|_| bad())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> DisplaySettings {
        DisplaySettings { enabled: true, mode: ModeSetting { width: 3840, height: 2160, refresh_mhz: 59_997 }, position: (-1280, 120), scale: 1.375, transform: Transform::Rotate90 }
    }

    #[test]
    fn the_file_round_trips() {
        let mut config = DisplaysConfig::default();
        config.set("GSM LG HDR 4K 0x0001a2b3", settings());
        config.set("connector HDMI-A-1", DisplaySettings { enabled: false, transform: Transform::Flipped270, ..settings() });
        let (back, problems) = DisplaysConfig::parse(&config.to_text());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(back, config);
    }

    #[test]
    fn store_and_load_round_trip_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("cupertino-displays-{}", std::process::id()));
        let path = dir.join("nested").join(DISPLAYS_FILE);
        assert_eq!(DisplaysConfig::load(&path).unwrap(), (DisplaysConfig::default(), Vec::new()));
        let mut config = DisplaysConfig::default();
        config.set("connector headless-2", settings());
        config.store(&path).unwrap();
        assert_eq!(DisplaysConfig::load(&path).unwrap(), (config, Vec::new()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn setting_a_key_again_replaces_it() {
        let mut config = DisplaysConfig::default();
        config.set("a", settings());
        config.set("a", DisplaySettings { scale: 2.0, ..settings() });
        assert_eq!(config.displays.len(), 1);
        assert_eq!(config.get("a").unwrap().scale, 2.0);
    }

    #[test]
    fn a_section_with_a_problem_is_dropped_whole() {
        let text = "[good]\nenabled = true\nmode = 800x600@60000\nposition = 0,0\nscale = 1\ntransform = normal\n\
                    [bad scale]\nenabled = true\nmode = 800x600@60000\nposition = 0,0\nscale = 0\ntransform = normal\n\
                    [incomplete]\nenabled = false\n";
        let (config, problems) = DisplaysConfig::parse(text);
        assert_eq!(config.displays.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), vec!["good"]);
        assert!(problems.iter().any(|p| p.line == 11 && p.problem.contains("above zero")), "{problems:?}");
        assert!(problems.iter().any(|p| p.problem.contains("[incomplete] lacks mode, position, scale, transform")), "{problems:?}");
    }

    #[test]
    fn keys_prefer_the_serial_and_fall_back_to_the_connector() {
        assert_eq!(display_key("DP-1", "GSM", "LG HDR 4K", Some("0x0001a2b3")), "GSM LG HDR 4K 0x0001a2b3");
        assert_eq!(display_key("DP-1", "GSM", "LG HDR 4K", Some("  ")), "connector DP-1");
        assert_eq!(display_key("DP-1", "X", "Y]", Some("1\n")), "X Y_ 1");
    }
}
