//! Accessibility display settings (`accessibility.conf`, `$CUPERTINO_ACCESSIBILITY_CONF` overrides the path). The
//! compositor is the only reader and writer: it applies the settings to its own drawing and tells the shell processes the
//! four display options through the session object, so no other process opens the file. One `key = value` per line, `#
//! starts a comment.
//!
//! | Key | Value | Default |
//! |---|---|---|
//! | `reduce_motion`, `reduce_transparency`, `increase_contrast`, `differentiate_without_color` | `on` or `off` | off |
//! | `invert_colors` | `on` or `off` | off |
//! | `display_contrast` | 0 to 1 | 0 |
//! | `color_filter` | `off`, `grayscale`, `protanopia`, `deuteranopia`, `tritanopia` or `tint <hue 0…1> | off |
//! | `color_filter_intensity` | 0.25 to 1, or `default` | the filter's own |
//! | `screen_reader` | `on` or `off`: the session's screen reader runs ([`installed_screen_reader`]) | off |
//! | `steady_caret` | `on` or `off`: text fields and editors show the caret without blinking | off |
//!
//! Every option is off when its key is absent, as the design reads a missing preference as false.
//! The contrast slider is 0 by default and accepts 0 to 1. Filter intensities are clamped to 0.25 to 1
//! by the design's reader; this file rejects values outside that range instead. The defaults of the intensities are
//! 0.5 for the three colour-blindness corrections, 1.0 for grayscale and 0.4 for the tint. The
//! reference stores one intensity per filter; this file keeps the intensity of the selected filter only.
//!
//! A person's file records only the keys they changed ([`PersonalAccessibility`]); the machine's file
//! ([`SYSTEM_ACCESSIBILITY`]) fills every other key. A file with every key, as earlier versions wrote it, stays valid and
//! sets them all.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::Path;

/// Environment variable that redirects `accessibility.conf`.
pub const ACCESSIBILITY_ENV: &str = "CUPERTINO_ACCESSIBILITY_CONF";
/// File name under the configuration directory.
pub const ACCESSIBILITY_FILE: &str = "accessibility.conf";
/// The machine's defaults, written by an administrator: the login screen's settings, and those of every account without
/// an `accessibility.conf` of its own. Same format, owned by root, readable by everyone (it names no one).
pub const SYSTEM_ACCESSIBILITY: &str = "/etc/cupertino/accessibility.conf";
/// The key of the machine's file that starts the screen reader with the login screen. No person's file has it, so one
/// person's choice never reaches another's login.
pub const SCREEN_READER_AT_LOGIN: &str = "screen_reader_at_login";
/// The key of a person's file that runs the screen reader in their session. The machine's file cannot set it.
pub const SCREEN_READER: &str = "screen_reader";
/// The key that keeps the text caret of every application from blinking.
pub const STEADY_CARET: &str = "steady_caret";
/// The keys the machine's file may set: display options that cannot make the login screen unusable for someone else,
/// and [`SCREEN_READER_AT_LOGIN`]. A key joins only after a security review.
pub const SYSTEM_KEYS: &[&str] = &["reduce_motion", SCREEN_READER_AT_LOGIN];

/// The screen readers a session may start, by absolute path, in order of preference: the login screen's and a person's
/// session's alike. Never one from `$PATH` or from a setting.
pub const SCREEN_READERS: &[&str] = &["/usr/bin/orca"];

/// The first of [`SCREEN_READERS`] that only root can change, every directory and link on the way included
/// ([`crate::system::root_only_target`]); `None` when no reader is installed.
pub fn installed_screen_reader() -> Option<std::path::PathBuf> {
    SCREEN_READERS
        .iter()
        .find_map(|path| crate::system::root_only_target(Path::new(path)))
}

/// Lowest filter intensity the design's reader returns.
pub const MIN_FILTER_INTENSITY: f64 = 0.25;

/// The display options every process draws with. The compositor sends them to the shell processes on the session object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DisplayOptions {
    /// Replace spatial transitions with fades.
    pub reduce_motion: bool,
    /// Draw materials with their opaque variants.
    pub reduce_transparency: bool,
    /// Use the high-contrast catalogs and the contrast shadow table.
    pub increase_contrast: bool,
    /// Do not carry state by colour alone.
    pub differentiate_without_color: bool,
}

/// One of the four display options, to toggle it by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DisplayOption {
    ReduceMotion,
    ReduceTransparency,
    IncreaseContrast,
    DifferentiateWithoutColor,
}

impl DisplayOption {
    pub const ALL: [DisplayOption; 4] = [
        DisplayOption::ReduceMotion,
        DisplayOption::ReduceTransparency,
        DisplayOption::IncreaseContrast,
        DisplayOption::DifferentiateWithoutColor,
    ];

    /// The settings key of the option.
    pub fn key(self) -> &'static str {
        match self {
            DisplayOption::ReduceMotion => "reduce_motion",
            DisplayOption::ReduceTransparency => "reduce_transparency",
            DisplayOption::IncreaseContrast => "increase_contrast",
            DisplayOption::DifferentiateWithoutColor => "differentiate_without_color",
        }
    }
}

impl DisplayOptions {
    pub fn get(&self, option: DisplayOption) -> bool {
        match option {
            DisplayOption::ReduceMotion => self.reduce_motion,
            DisplayOption::ReduceTransparency => self.reduce_transparency,
            DisplayOption::IncreaseContrast => self.increase_contrast,
            DisplayOption::DifferentiateWithoutColor => self.differentiate_without_color,
        }
    }

    pub fn set(&mut self, option: DisplayOption, on: bool) {
        match option {
            DisplayOption::ReduceMotion => self.reduce_motion = on,
            DisplayOption::ReduceTransparency => self.reduce_transparency = on,
            DisplayOption::IncreaseContrast => self.increase_contrast = on,
            DisplayOption::DifferentiateWithoutColor => self.differentiate_without_color = on,
        }
    }
}

/// The colour filter of the display (the settings pane's filter popup, type bits).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorFilter {
    Grayscale,
    /// Red/green, the protanopia correction.
    Protanopia,
    /// Green/red, the deuteranopia correction.
    Deuteranopia,
    /// Blue/yellow, the tritanopia correction.
    Tritanopia,
    /// Colour tint at `hue` (0 to 1 around the colour wheel).
    Tint {
        hue: f64,
    },
}

impl ColorFilter {
    /// Intensity used when the file names none.
    pub fn default_intensity(self) -> f64 {
        match self {
            ColorFilter::Grayscale => 1.0,
            ColorFilter::Protanopia | ColorFilter::Deuteranopia | ColorFilter::Tritanopia => 0.5,
            ColorFilter::Tint { .. } => 0.4,
        }
    }
}

impl fmt::Display for ColorFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ColorFilter::Grayscale => write!(f, "grayscale"),
            ColorFilter::Protanopia => write!(f, "protanopia"),
            ColorFilter::Deuteranopia => write!(f, "deuteranopia"),
            ColorFilter::Tritanopia => write!(f, "tritanopia"),
            ColorFilter::Tint { hue } => write!(f, "tint {hue}"),
        }
    }
}

/// Everything in `accessibility.conf`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AccessibilitySettings {
    pub display: DisplayOptions,
    /// Classic invert: the display's encoded values become `1 - value`.
    pub invert_colors: bool,
    /// The display contrast slider, 0 (off) to 1.
    pub display_contrast: f64,
    pub color_filter: Option<ColorFilter>,
    /// Intensity of the colour filter; `None` uses [`ColorFilter::default_intensity`].
    pub color_filter_intensity: Option<f64>,
    /// The session's screen reader runs. The compositor starts and stops it to match; at the login screen the machine's
    /// [`SCREEN_READER_AT_LOGIN`] decides instead.
    pub screen_reader: bool,
    /// Text fields and editors show the caret without blinking. The compositor tells applications through their
    /// appearance observers.
    pub steady_caret: bool,
}

impl AccessibilitySettings {
    /// The selected filter and the intensity it applies with.
    pub fn filter(&self) -> Option<(ColorFilter, f64)> {
        self.color_filter.map(|f| {
            (
                f,
                self.color_filter_intensity.unwrap_or(f.default_intensity()),
            )
        })
    }

    /// Every key with its value as the file writes it, in the order of the table above.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        let mut entries: Vec<(&'static str, String)> = DisplayOption::ALL
            .into_iter()
            .map(|o| (o.key(), on(self.display.get(o))))
            .collect();
        entries.push(("invert_colors", on(self.invert_colors)));
        entries.push(("display_contrast", self.display_contrast.to_string()));
        entries.push((
            "color_filter",
            self.color_filter
                .map_or_else(|| "off".to_string(), |f| f.to_string()),
        ));
        entries.push((
            "color_filter_intensity",
            self.color_filter_intensity
                .map_or_else(|| "default".to_string(), |i| i.to_string()),
        ));
        entries.push((SCREEN_READER, on(self.screen_reader)));
        entries.push((STEADY_CARET, on(self.steady_caret)));
        entries
    }

    /// Every key with its value, in the order of the table above; [`Self::apply_text`] reads it back unchanged.
    pub fn to_text(&self) -> String {
        self.entries()
            .into_iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }

    /// Apply `key = value` lines; returns `(line, message)` for every line that was not understood.
    pub fn apply_text(&mut self, text: &str) -> Vec<(usize, String)> {
        apply_lines(text, |key, value| self.set(key, value))
    }

    /// `self` with the value of `key` taken from `from`; an unknown key changes nothing.
    fn copy_key(&mut self, from: &Self, key: &str) {
        if let Some(option) = DisplayOption::ALL.into_iter().find(|o| o.key() == key) {
            self.display.set(option, from.display.get(option));
            return;
        }
        match key {
            "invert_colors" => self.invert_colors = from.invert_colors,
            "display_contrast" => self.display_contrast = from.display_contrast,
            "color_filter" => self.color_filter = from.color_filter,
            "color_filter_intensity" => self.color_filter_intensity = from.color_filter_intensity,
            SCREEN_READER => self.screen_reader = from.screen_reader,
            STEADY_CARET => self.steady_caret = from.steady_caret,
            _ => {}
        }
    }

    /// Set one key to a value written as in the file; `Err` names the problem and changes nothing.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let on = |v: &str| parse_on(key, v);
        let fraction = |v: &str, min: f64| match v.parse::<f64>() {
            Ok(x) if (min..=1.0).contains(&x) => Ok(x),
            _ => Err(format!(
                "{key}: expected a number from {min} to 1, got {v:?}"
            )),
        };
        if let Some(option) = DisplayOption::ALL.into_iter().find(|o| o.key() == key) {
            self.display.set(option, on(value)?);
            return Ok(());
        }
        match key {
            "invert_colors" => self.invert_colors = on(value)?,
            "display_contrast" => self.display_contrast = fraction(value, 0.0)?,
            "color_filter" => self.color_filter = parse_filter(value)?,
            "color_filter_intensity" => {
                self.color_filter_intensity = if value == "default" {
                    None
                } else {
                    Some(fraction(value, MIN_FILTER_INTENSITY)?)
                }
            }
            SCREEN_READER => self.screen_reader = on(value)?,
            STEADY_CARET => self.steady_caret = on(value)?,
            other => return Err(format!("unknown setting {other:?}")),
        }
        Ok(())
    }
}

/// A person's `accessibility.conf`: the keys they changed, each with its value. Every other key is the machine's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PersonalAccessibility {
    /// The value of each of `keys`; the other keys' values mean nothing.
    values: AccessibilitySettings,
    keys: BTreeSet<&'static str>,
}

impl PersonalAccessibility {
    /// The keys in `path` and every line that was not understood as `(line, message)`. A file that does not exist records
    /// no key; one that cannot be read is an error.
    pub fn load(path: &Path) -> io::Result<(Self, Vec<(usize, String)>)> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::from_text(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((Self::default(), Vec::new())),
            Err(e) => Err(e),
        }
    }

    /// Write `path` atomically ([`crate::atomic::write_atomic`]), creating its directory.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        crate::atomic::write_atomic(path, self.to_text().as_bytes())
    }

    /// The keys a file sets; a line that is not understood records nothing.
    pub fn from_text(text: &str) -> (Self, Vec<(usize, String)>) {
        let mut personal = Self::default();
        let errors = apply_lines(text, |key, value| {
            personal.values.set(key, value)?;
            personal.keys.extend(known_key(key));
            Ok(())
        });
        (personal, errors)
    }

    /// The recorded keys with their values, in the order of the table above; [`Self::from_text`] reads it back.
    pub fn to_text(&self) -> String {
        self.values
            .entries()
            .into_iter()
            .filter(|(key, _)| self.keys.contains(key))
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }

    /// The settings in effect: `system` with the value of every recorded key.
    pub fn over(&self, system: &AccessibilitySettings) -> AccessibilitySettings {
        let mut settings = *system;
        for key in &self.keys {
            settings.copy_key(&self.values, key);
        }
        settings
    }

    /// The person changed the settings in effect from `before` to `after`: every key whose value differs is theirs from
    /// now on, and stays theirs when they set it back.
    pub fn record(&mut self, before: &AccessibilitySettings, after: &AccessibilitySettings) {
        for ((key, was), (_, is)) in before.entries().into_iter().zip(after.entries()) {
            if was != is {
                self.keys.insert(key);
            }
        }
        for key in &self.keys {
            self.values.copy_key(after, key);
        }
    }
}

/// The machine's file, [`SYSTEM_ACCESSIBILITY`]: only the [`SYSTEM_KEYS`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SystemAccessibility {
    /// The login screen's settings, and those of every key a person's file does not record.
    pub settings: AccessibilitySettings,
    /// [`SCREEN_READER_AT_LOGIN`]: the login screen starts with the screen reader on.
    pub screen_reader_at_login: bool,
}

impl SystemAccessibility {
    /// The machine's defaults from the text of [`SYSTEM_ACCESSIBILITY`]: every key that is not one of [`SYSTEM_KEYS`] is
    /// reported and stays off.
    pub fn from_text(text: &str) -> (Self, Vec<(usize, String)>) {
        let mut system = Self::default();
        let errors = apply_lines(text, |key, value| system.set(key, value));
        (system, errors)
    }

    /// The [`SYSTEM_KEYS`] with their values, which [`Self::from_text`] reads back.
    pub fn to_text(&self) -> String {
        let reader = (
            SCREEN_READER_AT_LOGIN,
            if self.screen_reader_at_login {
                "on"
            } else {
                "off"
            }
            .to_string(),
        );
        self.settings
            .entries()
            .into_iter()
            .filter(|(key, _)| SYSTEM_KEYS.contains(key))
            .chain([reader])
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }

    /// Set one of the [`SYSTEM_KEYS`] to a value written as in the file; `Err` names the problem and changes nothing.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        if !SYSTEM_KEYS.contains(&key) {
            return Err(format!("{key}: not a setting of the machine's file"));
        }
        if key == SCREEN_READER_AT_LOGIN {
            self.screen_reader_at_login = parse_on(key, value)?;
            return Ok(());
        }
        self.settings.set(key, value)
    }
}

/// Each `key = value` line of `text` given to `set`; returns `(line, message)` for every line that was not understood.
fn apply_lines(
    text: &str,
    mut set: impl FnMut(&str, &str) -> Result<(), String>,
) -> Vec<(usize, String)> {
    let mut errors = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            errors.push((i + 1, format!("expected `key = value`, got {line:?}")));
            continue;
        };
        if let Err(e) = set(key.trim(), value.trim()) {
            errors.push((i + 1, e));
        }
    }
    errors
}

fn parse_on(key: &str, value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        _ => Err(format!("{key}: expected on or off, got {value:?}")),
    }
}

/// The key of the table above named `name`.
fn known_key(name: &str) -> Option<&'static str> {
    AccessibilitySettings::default()
        .entries()
        .into_iter()
        .map(|(key, _)| key)
        .find(|key| *key == name)
}

fn parse_filter(value: &str) -> Result<Option<ColorFilter>, String> {
    let mut words = value.split_whitespace();
    let filter = match (words.next(), words.next(), words.next()) {
        (Some("off"), None, None) => None,
        (Some("grayscale"), None, None) => Some(ColorFilter::Grayscale),
        (Some("protanopia"), None, None) => Some(ColorFilter::Protanopia),
        (Some("deuteranopia"), None, None) => Some(ColorFilter::Deuteranopia),
        (Some("tritanopia"), None, None) => Some(ColorFilter::Tritanopia),
        (Some("tint"), Some(hue), None) => match hue.parse::<f64>() {
            Ok(h) if (0.0..=1.0).contains(&h) => Some(ColorFilter::Tint { hue: h }),
            _ => return Err(format!("color_filter: expected a tint hue from 0 to 1, got {hue:?}")),
        },
        _ => return Err(format!("color_filter: expected off, grayscale, protanopia, deuteranopia, tritanopia or tint <hue>, got {value:?}")),
    };
    Ok(filter)
}
