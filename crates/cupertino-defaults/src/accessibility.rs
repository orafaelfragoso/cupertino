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
//!
//! Every option is off when its key is absent, as the design reads a missing preference as false.
//! The contrast slider is 0 by default and accepts 0 to 1. Filter intensities are clamped to 0.25 to 1
//! by the design's reader; this file rejects values outside that range instead. The defaults of the intensities are
//! 0.5 for the three colour-blindness corrections, 1.0 for grayscale and 0.4 for the tint. The
//! reference stores one intensity per filter; this file keeps the intensity of the selected filter only.

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
/// The keys the machine's file may set: display options that cannot make the login screen unusable for someone else.
/// A key joins only after a security review.
pub const SYSTEM_KEYS: &[&str] = &["reduce_motion"];

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

    /// The settings in `path` and every line that was not understood as `(line, message)`. A file that does not exist
    /// gives the defaults; one that cannot be read is an error.
    pub fn load(path: &Path) -> io::Result<(Self, Vec<(usize, String)>)> {
        let mut settings = Self::default();
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let errors = settings.apply_text(&text);
                Ok((settings, errors))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((settings, Vec::new())),
            Err(e) => Err(e),
        }
    }

    /// Write `path` atomically ([`crate::atomic::write_atomic`]), creating its directory.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        crate::atomic::write_atomic(path, self.to_text().as_bytes())
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
        entries
    }

    /// The machine's file: the [`SYSTEM_KEYS`] with their values, which [`Self::from_system_text`] reads back.
    pub fn system_text(&self) -> String {
        self.entries()
            .into_iter()
            .filter(|(key, _)| SYSTEM_KEYS.contains(key))
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
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
        self.apply_lines(text, |_| true)
    }

    /// The machine's defaults from the text of [`SYSTEM_ACCESSIBILITY`]: only the [`SYSTEM_KEYS`] apply, every other
    /// key is reported and stays off.
    pub fn from_system_text(text: &str) -> (Self, Vec<(usize, String)>) {
        let mut settings = Self::default();
        let errors = settings.apply_lines(text, |key| SYSTEM_KEYS.contains(&key));
        (settings, errors)
    }

    fn apply_lines(&mut self, text: &str, allowed: impl Fn(&str) -> bool) -> Vec<(usize, String)> {
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
            let key = key.trim();
            if !allowed(key) {
                errors.push((i + 1, format!("{key}: not a setting of the machine's file")));
                continue;
            }
            if let Err(e) = self.set(key, value.trim()) {
                errors.push((i + 1, e));
            }
        }
        errors
    }

    /// Set one key to a value written as in the file; `Err` names the problem and changes nothing.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let on = |v: &str| match v.to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => Ok(true),
            "off" | "false" | "no" | "0" => Ok(false),
            _ => Err(format!("{key}: expected on or off, got {v:?}")),
        };
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
            other => return Err(format!("unknown setting {other:?}")),
        }
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_key_is_off_and_the_filter_defaults_are_the_reference_ones() {
        let s = AccessibilitySettings::default();
        assert_eq!(s.display, DisplayOptions::default());
        assert!(!s.invert_colors);
        assert_eq!((s.display_contrast, s.filter()), (0.0, None));
        assert_eq!(ColorFilter::Grayscale.default_intensity(), 1.0);
        assert_eq!(ColorFilter::Deuteranopia.default_intensity(), 0.5);
        assert_eq!(ColorFilter::Tint { hue: 0.3 }.default_intensity(), 0.4);
    }

    #[test]
    fn lines_set_the_options_and_bad_lines_are_reported_by_number() {
        let mut s = AccessibilitySettings::default();
        let errors = s.apply_text(
            "reduce_motion = on\nreduce_transparency = yes # opaque menus\n\ncolor_filter = tint 0.6\ncolor_filter_intensity = 0.1\nincrease_contrast = maybe\ncolor_filter = sepia\ndisplay_contrast = 0.25\nbogus = on\n",
        );
        assert_eq!(
            errors.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
            vec![5, 6, 7, 9]
        );
        assert!(
            s.display.reduce_motion
                && s.display.reduce_transparency
                && !s.display.increase_contrast
        );
        assert_eq!(
            s.filter(),
            Some((ColorFilter::Tint { hue: 0.6 }, 0.4)),
            "a rejected intensity leaves the filter's own"
        );
        assert_eq!(s.display_contrast, 0.25);
    }

    #[test]
    fn the_text_reads_back_unchanged() {
        let s = AccessibilitySettings {
            display: DisplayOptions {
                reduce_motion: true,
                reduce_transparency: false,
                increase_contrast: true,
                differentiate_without_color: true,
            },
            invert_colors: true,
            display_contrast: 0.35,
            color_filter: Some(ColorFilter::Tint { hue: 0.125 }),
            color_filter_intensity: Some(0.75),
        };
        let mut back = AccessibilitySettings::default();
        assert!(back.apply_text(&s.to_text()).is_empty());
        assert_eq!(back, s);
        let mut plain = s;
        assert!(plain
            .apply_text(&AccessibilitySettings::default().to_text())
            .is_empty());
        assert_eq!(plain, AccessibilitySettings::default());
    }

    #[test]
    fn store_writes_a_file_that_load_reads() {
        let dir =
            std::env::temp_dir().join(format!("cupertino-accessibility-{}", std::process::id()));
        let path = dir.join("nested").join(ACCESSIBILITY_FILE);
        let mut s = AccessibilitySettings::default();
        s.display.set(DisplayOption::ReduceTransparency, true);
        s.color_filter = Some(ColorFilter::Protanopia);
        s.store(&path).unwrap();
        let (back, errors) = AccessibilitySettings::load(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(errors.is_empty());
        assert_eq!(back, s);
        assert_eq!(
            AccessibilitySettings::load(&path).unwrap().0,
            AccessibilitySettings::default(),
            "a missing file is the defaults"
        );
    }

    #[test]
    fn the_machines_file_sets_only_its_keys() {
        let (settings, errors) = AccessibilitySettings::from_system_text(
            "reduce_motion = on\ninvert_colors = on\ncolor_filter = grayscale\nreduce_motion = maybe\n",
        );
        assert!(settings.display.reduce_motion);
        assert!(!settings.invert_colors && settings.color_filter.is_none());
        assert_eq!(
            errors.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
            [2, 3, 4],
            "{errors:?}"
        );
    }

    #[test]
    fn one_key_is_set_as_the_file_writes_it() {
        let mut s = AccessibilitySettings::default();
        s.set("reduce_motion", "on").unwrap();
        s.set("color_filter", "tint 0.5").unwrap();
        assert!(s.set("display_contrast", "2").is_err());
        assert!(s.set("zoom", "on").is_err());
        let entries = s.entries();
        assert_eq!(entries[0], ("reduce_motion", "on".to_string()));
        assert!(entries.contains(&("color_filter", "tint 0.5".to_string())));
        assert!(
            entries.contains(&("display_contrast", "0".to_string())),
            "the refused value changed nothing"
        );
    }
}
