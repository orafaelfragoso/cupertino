//! Unprivileged, one-shot readings for About This Computer. Unavailable values have no row.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Maximum bytes accepted from one system description file, including the optional PCI name database.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Approved About layout in logical points: content padding, mark and typography, then compact data rows.
pub mod metrics {
    pub const WIDTH: u32 = 280;
    pub const PADDING: f64 = 24.0;
    /// Above the mark: the row the window's control buttons lie in, which the content runs under.
    pub const TOP: f64 = 52.0;
    pub const MARK: f64 = 64.0;
    pub const NAME_Y: f64 = TOP + MARK + 12.0;
    pub const NAME_HEIGHT: f64 = 28.0;
    pub const VERSION_Y: f64 = NAME_Y + NAME_HEIGHT + 4.0;
    pub const VERSION_HEIGHT: f64 = 16.0;
    pub const ROWS_Y: f64 = VERSION_Y + VERSION_HEIGHT + 16.0;
    pub const ROW_HEIGHT: f64 = 16.0;
    pub const ROW_GAP: f64 = 0.0;
    /// How far before the window's centre the right-aligned labels end, and how far after it the values start.
    pub const LABEL_END: f64 = 6.5;
    pub const VALUE_START: f64 = 5.0;
    /// The width of the labels' column, from the leading padding to [`LABEL_END`].
    pub const LABEL_COLUMN: f64 = WIDTH as f64 / 2.0 - LABEL_END - PADDING;
    /// The width the values wrap at, from [`VALUE_START`] to the trailing padding.
    pub const VALUE_COLUMN: f64 = WIDTH as f64 / 2.0 - VALUE_START - PADDING;
    pub const BUTTON_GAP: f64 = 20.0;
    pub const BUTTON_WIDTH: f64 = 92.0;
    pub const BUTTON_HEIGHT: f64 = 24.0;
    pub const BOTTOM: f64 = 20.0;
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowLayout {
    pub label: &'static str,
    pub lines: Vec<String>,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub rows: Vec<RowLayout>,
    pub button: crate::Rect,
}

impl Layout {
    pub fn size(&self) -> (u32, u32) {
        (
            metrics::WIDTH,
            (self.button.bottom() + metrics::BOTTOM).ceil() as u32,
        )
    }
}

impl About {
    /// Wrap values at the painter's measured width; painting, hit testing and the window share this layout.
    pub fn layout(&self, mut measure: impl FnMut(&str) -> f64) -> Layout {
        use metrics::*;
        let width = VALUE_COLUMN;
        let mut y = ROWS_Y;
        let mut rows = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            let lines = wrap_value(&row.value, width, &mut measure);
            let label = if i > 0 && self.rows[i - 1].label == row.label {
                ""
            } else {
                row.label
            };
            let height = lines.len() as f64 * ROW_HEIGHT;
            rows.push(RowLayout { label, lines, y });
            y += height + ROW_GAP;
        }
        if !rows.is_empty() {
            y -= ROW_GAP;
        }
        let button = crate::Rect::new(
            (f64::from(WIDTH) - BUTTON_WIDTH) / 2.0,
            y + BUTTON_GAP,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
        );
        Layout { rows, button }
    }
}

/// Whitespace a line may break at: any but the no-break spaces, which hold their neighbours together.
fn breakable(c: char) -> bool {
    c.is_whitespace() && !matches!(c, NO_BREAK_SPACE | NARROW_NO_BREAK_SPACE)
}

/// Prefer whole words, splitting an overlong word only at character boundaries. No value text is elided.
fn wrap_value(text: &str, width: f64, measure: &mut impl FnMut(&str) -> f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(breakable).filter(|word| !word.is_empty()) {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if measure(&candidate) <= width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        for c in word.chars() {
            let mut candidate = line.clone();
            candidate.push(c);
            if !line.is_empty() && measure(&candidate) > width {
                lines.push(std::mem::take(&mut line));
            }
            line.push(c);
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// The longest make or model shown; a display reports both, so a longer one is cut with an ellipsis (approximate).
const MAX_IDENTIFICATION_CHARS: usize = 32;
/// Scales are shown on the 1/120 grid fractional scaling uses, with at most two decimals.
const SCALE_STEPS: f64 = 120.0;
const MM_PER_INCH: f64 = 25.4;
/// The diagonals shown, in inches: a display often reports its aspect ratio (16 × 9 mm) or nothing usable as its size,
/// so one outside this range is left out (approximate).
const PLAUSIBLE_DIAGONAL: std::ops::RangeInclusive<f64> = 10.0..=100.0;
const NO_BREAK_SPACE: char = '\u{a0}';
/// Joins a mode's width, "×" and height, and "@" to the scale, so a wrapped value never splits either.
const NARROW_NO_BREAK_SPACE: char = '\u{202f}';

/// What the Displays row shows of one display, as the compositor announces it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Display<'a> {
    pub make: Option<&'a str>,
    pub model: Option<&'a str>,
    pub description: Option<&'a str>,
    pub name: Option<&'a str>,
    /// The current mode in device pixels.
    pub mode: (u32, u32),
    pub physical_size_mm: Option<(u32, u32)>,
    /// Device pixels per logical point.
    pub scale: f64,
}

/// "<make model> <diagonal>-inch W × H @ Sx": make and model, else the description, else the connector name; the
/// diagonal only with a plausible physical size; "W × H" joined by narrow no-break spaces. `None` without a mode or
/// anything to name the display by.
pub fn display_value(display: &Display<'_>) -> Option<String> {
    let (w, h) = display.mode;
    if w == 0 || h == 0 {
        return None;
    }
    let title = display_title(display)?;
    let size = display
        .physical_size_mm
        .map(|(w, h)| f64::from(w).hypot(f64::from(h)) / MM_PER_INCH)
        .map(|inches| (inches * 10.0).round() / 10.0)
        .filter(|inches| PLAUSIBLE_DIAGONAL.contains(inches))
        .map(|inches| format!(" {}-inch", decimals(inches, 1)))
        .unwrap_or_default();
    let nb = NARROW_NO_BREAK_SPACE;
    Some(format!(
        "{title}{size} {} @{nb}{}",
        mode_text(display.mode),
        scale_text(display.scale)
    ))
}

/// The display's make and model, else its description, else its connector name; `None` without any.
pub fn display_title(display: &Display<'_>) -> Option<String> {
    let identification: Vec<String> = [display.make, display.model]
        .into_iter()
        .filter_map(present)
        .map(|s| capped(s, MAX_IDENTIFICATION_CHARS))
        .collect();
    if identification.is_empty() {
        present(display.description)
            .or(present(display.name))
            .map(str::to_owned)
    } else {
        Some(identification.join(" "))
    }
}

/// "W × H", joined by narrow no-break spaces.
pub fn mode_text((w, h): (u32, u32)) -> String {
    let nb = NARROW_NO_BREAK_SPACE;
    format!("{w}{nb}×{nb}{h}")
}

/// "2x", "1.25x": on the fractional-scale grid, at most two decimals.
pub fn scale_text(scale: f64) -> String {
    format!(
        "{}x",
        decimals((scale * SCALE_STEPS).round() / SCALE_STEPS, 2)
    )
}

/// "60 Hz", "59.95 Hz" from millihertz.
pub fn refresh_text(millihertz: u32) -> String {
    format!(
        "{}{NO_BREAK_SPACE}Hz",
        decimals(f64::from(millihertz) / 1000.0, 2)
    )
}

/// `text` trimmed, `None` when nothing is left.
fn present(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|s| !s.is_empty())
}

/// `text` cut to `max` characters, the last one an ellipsis.
fn capped(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// `value` with at most `places` decimals and no trailing zeros.
fn decimals(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: &'static str,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct About {
    pub version: String,
    pub os: Option<String>,
    pub rows: Vec<Row>,
}

impl About {
    /// `root` is `/` in production; tests supply a fixture tree. No commands or privileged APIs are used.
    pub fn read(root: &Path) -> Self {
        let read = |path: &str| read_text(&root.join(path));
        let computer = read("etc/machine-info")
            .and_then(|s| assignment(&s, "PRETTY_HOSTNAME"))
            .or_else(|| read("etc/hostname").and_then(|s| single_line(&s)));
        let processor = read("proc/cpuinfo").and_then(|s| processor(&s));
        let processor = processor.or_else(|| {
            read("proc/device-tree/model").and_then(|s| single_line(s.trim_end_matches('\0')))
        });
        let memory = read("proc/meminfo").and_then(|s| memory(&s));
        let kernel = read("proc/sys/kernel/osrelease").and_then(|s| single_line(&s));
        let os = read("etc/os-release").and_then(|s| assignment(&s, "PRETTY_NAME"));
        let mut rows = Vec::new();
        for (label, value) in [
            ("Computer", computer),
            ("Processor", processor),
            ("Memory", memory),
        ] {
            if let Some(value) = value {
                rows.push(Row { label, value });
            }
        }
        let ids = read("usr/share/hwdata/pci.ids").or_else(|| read("usr/share/misc/pci.ids"));
        let drm = root.join("sys/class/drm");
        match std::fs::read_dir(&drm) {
            Ok(entries) => {
                let mut cards = Vec::new();
                for entry in entries {
                    match entry {
                        Ok(entry) if card_name(&entry.file_name().to_string_lossy()) => {
                            cards.push(entry.path())
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("cupertino-settings: {}: {e}", drm.display()),
                    }
                }
                cards.sort();
                for card in cards {
                    let device = card.join("device");
                    let vendor = read_text(&device.join("vendor")).and_then(|s| pci_id(&s));
                    let product = read_text(&device.join("device")).and_then(|s| pci_id(&s));
                    if let Some((vendor, product)) = vendor.zip(product) {
                        let mut value = ids
                            .as_deref()
                            .and_then(|s| pci_name(s, vendor, product))
                            .unwrap_or_else(|| format!("PCI {vendor:04x}:{product:04x}"));
                        match std::fs::read_link(device.join("driver")) {
                            Ok(path) => {
                                if let Some(driver) = path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .and_then(single_line)
                                {
                                    value.push_str(&format!(" ({driver})"));
                                }
                            }
                            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                            Err(e) => eprintln!(
                                "cupertino-settings: {}: {e}",
                                device.join("driver").display()
                            ),
                        }
                        rows.push(Row {
                            label: "Graphics",
                            value,
                        });
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("cupertino-settings: {}: {e}", drm.display()),
        }
        if let Some(value) = kernel {
            rows.push(Row {
                label: "Kernel",
                value,
            });
        }
        Self {
            version: version(
                env!("CARGO_PKG_VERSION"),
                option_env!("CUPERTINO_BUILD_COMMIT"),
            ),
            os,
            rows,
        }
    }

    /// A Displays row before Kernel for each connected display with a current mode: one short connection asks the
    /// server.
    #[cfg(feature = "app")]
    pub fn read_displays(&mut self) -> Result<(), cupertino_wayland_client::Error> {
        let mut conn = cupertino_wayland_client::Connection::connect()?;
        conn.roundtrip()?;
        for output in conn.outputs() {
            let Some(mode) = output.mode else { continue };
            let display = Display {
                make: output.make.as_deref(),
                model: output.model.as_deref(),
                description: output.description.as_deref(),
                name: output.name.as_deref(),
                mode,
                physical_size_mm: output.physical_size_mm,
                scale: output
                    .effective_scale()
                    .unwrap_or_else(|| output.scale.factor()),
            };
            if let Some(value) = display_value(&display) {
                let before_kernel = self
                    .rows
                    .iter()
                    .position(|r| r.label == "Kernel")
                    .unwrap_or(self.rows.len());
                self.rows.insert(
                    before_kernel,
                    Row {
                        label: "Displays",
                        value,
                    },
                );
            }
        }
        Ok(())
    }
}

fn version(package: &str, commit: Option<&str>) -> String {
    match commit.filter(|s| !s.is_empty()) {
        Some(commit) => format!("Version {package} (build {commit})"),
        None => format!("Version {package}"),
    }
}

fn read_text(path: &Path) -> Option<String> {
    let result = (|| {
        let mut text = String::new();
        File::open(path)?
            .take(MAX_FILE_BYTES + 1)
            .read_to_string(&mut text)?;
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "system description exceeds size limit",
            ));
        }
        Ok(text)
    })();
    match result {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!("cupertino-settings: {}: {e}", path.display());
            None
        }
    }
}

fn single_line(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

/// Parse a literal shell-style assignment without evaluating expansions or commands.
fn assignment(text: &str, key: &str) -> Option<String> {
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))?
        .trim();
    let value = if let Some(value) = value.strip_prefix('"') {
        value.strip_suffix('"')?
    } else if let Some(value) = value.strip_prefix('\'') {
        value.strip_suffix('\'')?
    } else {
        if value
            .chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '\'')
        {
            return None;
        }
        value
    };
    single_line(value)
}

fn processor(text: &str) -> Option<String> {
    let fields: Vec<_> = text
        .lines()
        .filter_map(|s| s.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    let name = fields
        .iter()
        .find(|(k, _)| *k == "model name")
        .or_else(|| fields.iter().find(|(k, _)| *k == "Model"))?
        .1;
    let name = single_line(name)?;
    let threads = fields
        .iter()
        .filter(|(k, v)| *k == "processor" && v.parse::<u32>().is_ok())
        .count();
    Some(if threads == 0 {
        name
    } else if threads == 1 {
        format!("{name} · 1 thread")
    } else {
        format!("{name} · {threads} threads")
    })
}

fn memory(text: &str) -> Option<String> {
    let value = text.lines().find_map(|s| s.strip_prefix("MemTotal:"))?;
    let mut fields = value.split_whitespace();
    let kb = fields.next()?.parse::<u64>().ok()?;
    if fields.next()? != "kB" || fields.next().is_some() || kb == 0 {
        return None;
    }
    let gb = kb.checked_add(512 * 1024)? / (1024 * 1024);
    Some(format!("{gb} GB"))
}

fn card_name(name: &str) -> bool {
    name.strip_prefix("card")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

fn pci_id(text: &str) -> Option<u16> {
    let text = text.trim().strip_prefix("0x")?;
    (text.len() == 4)
        .then(|| u16::from_str_radix(text, 16).ok())
        .flatten()
}

fn pci_name(text: &str, vendor: u16, product: u16) -> Option<String> {
    let mut in_vendor = false;
    for line in text
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        if !line.starts_with('\t') {
            in_vendor = line.get(..4).and_then(|s| u16::from_str_radix(s, 16).ok()) == Some(vendor);
        } else if in_vendor && !line.starts_with("\t\t") {
            let (id, name) = line[1..].split_once(char::is_whitespace)?;
            if u16::from_str_radix(id, 16).ok() == Some(product) {
                return single_line(name);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_display_shows_make_model_diagonal_mode_and_snapped_scale() {
        let display = Display {
            make: Some("ACME"),
            model: Some("Panel 27"),
            description: Some("ACME Panel 27 (DP-1)"),
            name: Some("DP-1"),
            mode: (2560, 1600),
            physical_size_mm: Some((286, 179)),
            scale: 2560.0 / 1707.0,
        };
        assert_eq!(
            display_value(&display).as_deref(),
            Some("ACME Panel 27 13.3-inch 2560\u{202f}×\u{202f}1600 @\u{202f}1.5x")
        );
        let integer = Display {
            physical_size_mm: Some((597, 336)),
            mode: (3840, 2160),
            scale: 2.0,
            ..display.clone()
        };
        assert_eq!(
            display_value(&integer).as_deref(),
            Some("ACME Panel 27 27-inch 3840\u{202f}×\u{202f}2160 @\u{202f}2x")
        );
        let quarter = Display {
            scale: 1.25,
            physical_size_mm: None,
            ..display.clone()
        };
        assert_eq!(
            display_value(&quarter).as_deref(),
            Some("ACME Panel 27 2560\u{202f}×\u{202f}1600 @\u{202f}1.25x")
        );
    }

    #[test]
    fn a_size_that_is_only_an_aspect_ratio_shows_no_diagonal() {
        let display = Display {
            name: Some("HDMI-A-1"),
            mode: (1920, 1080),
            scale: 1.0,
            ..Display::default()
        };
        for size in [(16, 9), (160, 90), (1, 1), (5000, 3000)] {
            let value = display_value(&Display {
                physical_size_mm: Some(size),
                ..display.clone()
            })
            .unwrap();
            assert!(!value.contains("-inch"), "{size:?}: {value}");
        }
        let laptop = display_value(&Display {
            physical_size_mm: Some((221, 125)),
            ..display.clone()
        })
        .unwrap();
        assert!(laptop.contains(" 10-inch "), "{laptop}");
    }

    #[test]
    fn a_wrapped_display_value_never_splits_its_mode() {
        let value = display_value(&Display {
            name: Some("HDMI-A-1"),
            mode: (2560, 1600),
            scale: 1.5,
            ..Display::default()
        })
        .unwrap();
        let mut measure = |s: &str| s.chars().count() as f64;
        let lines = wrap_value(&value, 12.0, &mut measure);
        assert!(
            lines.contains(&"2560\u{202f}×\u{202f}1600".to_owned()),
            "{lines:?}"
        );
        assert!(lines.contains(&"@\u{202f}1.5x".to_owned()), "{lines:?}");
        assert_eq!(
            wrap_value("a\u{a0}b c", 3.0, &mut measure),
            ["a\u{a0}b", "c"]
        );
    }

    #[test]
    fn a_display_without_identification_falls_back_and_long_names_are_capped() {
        let unnamed = Display {
            description: Some("  "),
            name: Some("HDMI-A-1"),
            mode: (1920, 1080),
            scale: 1.0,
            ..Display::default()
        };
        assert_eq!(
            display_value(&unnamed).as_deref(),
            Some("HDMI-A-1 1920\u{202f}×\u{202f}1080 @\u{202f}1x")
        );
        assert_eq!(
            display_value(&Display {
                name: None,
                description: None,
                ..unnamed.clone()
            }),
            None
        );
        assert_eq!(
            display_value(&Display {
                mode: (0, 1080),
                ..unnamed.clone()
            }),
            None
        );
        let long = "M".repeat(200);
        let value = display_value(&Display {
            model: Some(&long),
            ..unnamed.clone()
        })
        .unwrap();
        assert!(value.starts_with(&format!("{}…", "M".repeat(MAX_IDENTIFICATION_CHARS - 1))));
        assert_eq!(
            value.chars().count(),
            MAX_IDENTIFICATION_CHARS + " 1920\u{202f}×\u{202f}1080 @\u{202f}1x".chars().count()
        );
    }

    #[test]
    fn absent_files_hide_all_rows() {
        let root = tempfile::tempdir().unwrap();
        let about = About::read(root.path());
        assert!(about.rows.is_empty());
        assert_eq!(about.os, None);
    }

    #[test]
    fn malformed_files_hide_their_rows_and_optional_pci_names_do_not_hide_valid_devices() {
        let root = tempfile::tempdir().unwrap();
        for (path, value) in [
            ("etc/machine-info", "PRETTY_HOSTNAME=\"unfinished"),
            ("etc/hostname", "bad\nhost"),
            ("etc/os-release", "PRETTY_NAME=\"unfinished"),
            ("proc/cpuinfo", "processor: 0\nmodel name:"),
            ("proc/device-tree/model", "bad\0model"),
            ("proc/meminfo", "MemTotal: invalid kB"),
            ("proc/sys/kernel/osrelease", "bad\nkernel"),
            ("sys/class/drm/card0/device/vendor", "invalid"),
            ("sys/class/drm/card0/device/device", "0x5678"),
        ] {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, value).unwrap();
        }
        let about = About::read(root.path());
        assert!(about.rows.is_empty());
        assert_eq!(about.os, None);
        std::fs::write(
            root.path().join("sys/class/drm/card0/device/vendor"),
            "0x1234",
        )
        .unwrap();
        assert_eq!(
            About::read(root.path()).rows,
            vec![Row {
                label: "Graphics",
                value: "PCI 1234:5678".into()
            }]
        );
    }

    #[test]
    fn system_files_produce_the_expected_visible_rows() {
        let root = tempfile::tempdir().unwrap();
        for (path, value) in [
            (
                "etc/machine-info",
                "PRETTY_HOSTNAME=\"Example workstation\"",
            ),
            ("etc/os-release", "PRETTY_NAME=\"Example Linux 1\""),
            ("proc/cpuinfo", "processor: 0\nmodel name: Example CPU"),
            ("proc/meminfo", "MemTotal: 33554432 kB"),
            ("proc/sys/kernel/osrelease", "6.1-example\n"),
        ] {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, value).unwrap();
        }
        let about = About::read(root.path());
        assert_eq!(
            about
                .rows
                .iter()
                .map(|r| (r.label, r.value.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("Computer", "Example workstation"),
                ("Processor", "Example CPU · 1 thread"),
                ("Memory", "32 GB"),
                ("Kernel", "6.1-example"),
            ]
        );
        assert_eq!(about.os.as_deref(), Some("Example Linux 1"));
        let layout = about.layout(|text| text.chars().count() as f64);
        assert_eq!(layout.size().0, 280);
        assert_eq!(
            layout.size().1 as f64,
            layout.button.bottom() + metrics::BOTTOM
        );
    }

    #[test]
    fn long_values_remain_complete_and_push_later_rows_and_the_button_down() {
        let values = [
            "AMD Ryzen 7 5700X · 16 threads",
            "GA106 [GeForce RTX 3060] (driver)",
            "Built-in 2560 × 1600 @ 2x",
        ];
        let about = About {
            version: "Version 1".into(),
            os: None,
            rows: values
                .iter()
                .map(|value| Row {
                    label: "Hardware",
                    value: (*value).into(),
                })
                .collect(),
        };
        let layout = about.layout(|text| text.chars().count() as f64 * 8.0);
        let mut bottom = metrics::ROWS_Y - metrics::ROW_GAP;
        for (row, value) in layout.rows.iter().zip(values) {
            assert_eq!(row.lines.join(" "), value);
            assert!(row.lines.len() > 1);
            assert!(row
                .lines
                .iter()
                .all(|s| s.chars().count() as f64 * 8.0 <= 110.0));
            assert_eq!(row.y, bottom + metrics::ROW_GAP);
            bottom = row.y + row.lines.len() as f64 * metrics::ROW_HEIGHT;
        }
        assert_eq!(layout.button.y, bottom + metrics::BUTTON_GAP);
        assert_eq!(
            layout.size().1 as f64,
            layout.button.bottom() + metrics::BOTTOM
        );
        assert_eq!(layout.rows[1].label, "");
    }

    #[test]
    fn unbroken_unicode_values_wrap_without_losing_characters() {
        let value = "Display-長い表示名-123456789";
        let lines = wrap_value(value, 5.0, &mut |text| text.chars().count() as f64);
        assert_eq!(lines.concat(), value);
        assert!(lines.iter().all(|s| s.chars().count() <= 5));
    }

    #[test]
    fn assignment_parser_rejects_missing_malformed_and_empty_values() {
        for key in ["PRETTY_HOSTNAME", "PRETTY_NAME"] {
            for text in [
                "",
                "garbage",
                &format!("{key}=\"unfinished"),
                &format!("{key}=\"\""),
            ] {
                assert_eq!(assignment(text, key), None);
            }
            assert_eq!(
                assignment(&format!("{key}=\"Example Linux 1\""), key),
                Some("Example Linux 1".into())
            );
        }
    }

    #[test]
    fn processor_parser_counts_threads_and_hides_malformed_or_missing_models() {
        assert_eq!(
            processor("processor: 0\nmodel name: Example CPU\nprocessor: 1"),
            Some("Example CPU · 2 threads".into())
        );
        for text in ["", "garbage", "model name:", "processor: 0"] {
            assert_eq!(processor(text), None);
        }
        assert_eq!(
            processor("Model: Example Board"),
            Some("Example Board".into())
        );
    }

    #[test]
    fn memory_parser_requires_positive_kibibytes() {
        assert_eq!(memory("MemTotal: 33554432 kB"), Some("32 GB".into()));
        for text in [
            "",
            "MemTotal: no kB",
            "MemTotal: 0 kB",
            "MemTotal: 1 GB",
            "MemTotal: 18446744073709551615 kB",
        ] {
            assert_eq!(memory(text), None);
        }
    }

    #[test]
    fn line_parser_hides_missing_or_multiline_hostname_kernel_and_device_tree_values() {
        for text in ["", " \n", "bad\nvalue", "bad\0value"] {
            assert_eq!(single_line(text), None);
        }
        assert_eq!(single_line("6.1-test\n"), Some("6.1-test".into()));
    }

    #[test]
    fn graphics_ids_and_optional_database_are_validated() {
        for text in ["", "0xzzzz", "garbage", "0x12345"] {
            assert_eq!(pci_id(text), None);
        }
        assert_eq!(pci_id("0x1234\n"), Some(0x1234));
        assert_eq!(pci_name("", 0x1234, 0x5678), None);
        assert_eq!(pci_name("garbage", 0x1234, 0x5678), None);
        assert_eq!(
            pci_name(
                "1234  Vendor\n\t5678  Example GPU\n\t\t0000 subsystem",
                0x1234,
                0x5678
            ),
            Some("Example GPU".into())
        );
        assert!(card_name("card10"));
        assert!(!card_name("card0-DP-1"));
    }

    #[test]
    fn build_commit_is_optional() {
        assert_eq!(version("1.0", None), "Version 1.0");
        assert_eq!(version("1.0", Some("abc123")), "Version 1.0 (build abc123)");
    }
}
