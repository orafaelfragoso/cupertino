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
    pub const NAME_Y: f64 = TOP + MARK + 8.0;
    pub const NAME_HEIGHT: f64 = 28.0;
    pub const VERSION_Y: f64 = NAME_Y + NAME_HEIGHT + 4.0;
    pub const VERSION_HEIGHT: f64 = 16.0;
    pub const ROWS_Y: f64 = VERSION_Y + VERSION_HEIGHT + 12.0;
    pub const ROW_HEIGHT: f64 = 16.0;
    pub const ROW_GAP: f64 = 0.0;
    /// How far before the window's centre the right-aligned labels end, and how far after it the values start.
    pub const LABEL_END: f64 = 6.5;
    pub const VALUE_START: f64 = 5.0;
    /// The width of the labels' column, from the leading padding to [`LABEL_END`].
    pub const LABEL_COLUMN: f64 = WIDTH as f64 / 2.0 - LABEL_END - PADDING;
    /// The width the values wrap at, from [`VALUE_START`] to the trailing padding.
    pub const VALUE_COLUMN: f64 = WIDTH as f64 / 2.0 - VALUE_START - PADDING;
    pub const BUTTON_GAP: f64 = 16.0;
    pub const BUTTON_WIDTH: f64 = 92.0;
    pub const BUTTON_HEIGHT: f64 = 24.0;
    pub const BOTTOM: f64 = 16.0;
    /// Of a screen's height, what the window leaves free: the menu bar above it, the Dock below it and a margin.
    pub const SCREEN_RESERVED: f64 = 120.0;
    /// The rows always show at least this many lines, however small the screen.
    pub const MIN_ROW_LINES: f64 = 3.0;
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowLayout {
    pub label: &'static str,
    pub lines: Vec<String>,
    pub y: f64,
    /// The row's first line takes [`metrics::ROW_HEIGHT`], each wrapped line after it one text line.
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub rows: Vec<RowLayout>,
    pub button: crate::Rect,
    /// The height the rows show when the window is capped to a screen too short for them all; they scroll in it.
    /// `None` when every row shows.
    pub rows_view: Option<f64>,
    /// The height of one line of the rows' text: the pitch of a value's wrapped lines, which start at the top of
    /// their row.
    pub line_height: f64,
}

impl Layout {
    /// The rows' whole height, from the first row's top to the last row's bottom.
    pub fn rows_height(&self) -> f64 {
        self.rows
            .last()
            .map_or(0.0, |row| row.y + row.height - metrics::ROWS_Y)
    }

    /// Where, from the first row's top, the rows may be cut without cutting through a line of text: under each
    /// wrapped line, and at each row's end.
    pub fn line_ends(&self) -> Vec<f64> {
        let mut ends = Vec::new();
        for row in &self.rows {
            let top = row.y - metrics::ROWS_Y;
            for i in 1..row.lines.len() {
                ends.push(top + i as f64 * self.line_height);
            }
            // as [`Layout::rows_height`] sums it
            ends.push(row.y + row.height - metrics::ROWS_Y);
        }
        ends
    }

    /// Keep the window within a screen `screen_height` points tall: when it would be taller than the screen leaves
    /// ([`metrics::SCREEN_RESERVED`]), the rows show only the whole lines that fit, at least
    /// [`metrics::MIN_ROW_LINES`], and scroll, the button moving up with them.
    pub fn fit(&mut self, screen_height: f64) {
        let available = screen_height - metrics::SCREEN_RESERVED;
        let excess = self.button.bottom() + metrics::BOTTOM - available;
        if excess <= 0.0 || self.rows.is_empty() {
            return;
        }
        let whole = self.rows_height();
        let ends = self.line_ends();
        let least = ends
            .get(metrics::MIN_ROW_LINES as usize - 1)
            .copied()
            .unwrap_or(whole);
        // whole lines, so the last one shown is never cut through
        let shown = ends
            .iter()
            .copied()
            .filter(|&end| end <= whole - excess)
            .fold(least, f64::max);
        self.button.y -= whole - shown;
        self.rows_view = Some(shown);
    }

    pub fn size(&self) -> (u32, u32) {
        (
            metrics::WIDTH,
            (self.button.bottom() + metrics::BOTTOM).ceil() as u32,
        )
    }
}

impl About {
    /// Wrap values at the painter's measured width, `line_height` the height of one line of their text; painting,
    /// hit testing and the window share this layout.
    pub fn layout(&self, mut measure: impl FnMut(&str) -> f64, line_height: f64) -> Layout {
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
            let height = ROW_HEIGHT + (lines.len() - 1) as f64 * line_height;
            rows.push(RowLayout {
                label,
                lines,
                y,
                height,
            });
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
        Layout {
            rows,
            button,
            rows_view: None,
            line_height,
        }
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
    /// `root` is `/` in production; tests supply a fixture tree. No commands or privileged APIs are used, so a
    /// graphics device whose memory only its driver's tool reports shows without it ([`About::read_with`]).
    pub fn read(root: &Path) -> Self {
        Self::read_with(root, &[])
    }

    /// As [`About::read`], with the memory of graphics devices that sysfs does not report, by PCI address, as
    /// [`nvidia_video_memory`] reads it.
    pub fn read_with(root: &Path, video_memory: &[(PciAddress, u64)]) -> Self {
        let read = |path: &str| read_text(&root.join(path));
        let computer = read("etc/machine-info")
            .and_then(|s| assignment(&s, "PRETTY_HOSTNAME"))
            .or_else(|| read("etc/hostname").and_then(|s| single_line(&s)));
        let processor = read("proc/cpuinfo").and_then(|s| processor(&s));
        let processor = processor.or_else(|| {
            read("proc/device-tree/model").and_then(|s| single_line(s.trim_end_matches('\0')))
        });
        let memory = installed_memory(&root.join("sys/devices/system/memory"))
            .or_else(|| read("proc/meminfo").and_then(|s| memory(&s)))
            .map(memory_text);
        let kernel = read("proc/sys/kernel/osrelease").and_then(|s| single_line(&s));
        let os = read("etc/os-release").and_then(|s| assignment(&s, "PRETTY_NAME"));
        let disk = read("proc/self/mountinfo")
            .and_then(|s| startup_disk(root, &s))
            .and_then(|disk| disk.value());
        let mut rows = Vec::new();
        for (label, value) in [
            ("Computer", computer),
            ("Processor", processor),
            ("Memory", memory),
            ("Startup disk", disk),
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
                        let name = ids
                            .as_deref()
                            .and_then(|s| pci_name(s, vendor, product))
                            .map(|name| product_name(&name))
                            .unwrap_or_else(|| format!("PCI {vendor:04x}:{product:04x}"));
                        let memory = read_text(&device.join("mem_info_vram_total"))
                            .and_then(|s| s.trim().parse::<u64>().ok())
                            .or_else(|| {
                                let address = std::fs::canonicalize(&device).ok()?;
                                let address = pci_address(address.file_name()?.to_str()?)?;
                                video_memory
                                    .iter()
                                    .find(|(a, _)| *a == address)
                                    .map(|(_, bytes)| *bytes)
                            });
                        rows.push(Row {
                            label: "Graphics",
                            value: graphics_value(&name, memory),
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
    /// Add a Displays row for each output, and give the height in points of the shortest one, which the window keeps
    /// within ([`Layout::fit`]).
    pub fn read_displays(&mut self) -> Result<Option<f64>, cupertino_wayland_client::Error> {
        let mut shortest: Option<f64> = None;
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
            let height = f64::from(mode.1) / display.scale.max(1.0);
            shortest = Some(shortest.map_or(height, |h: f64| h.min(height)));
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
        Ok(shortest)
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

/// Bytes in a binary gigabyte, as memory sizes are written.
const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// The memory the kernel may use, from `MemTotal`, in bytes rounded up to the common module total
/// ([`common_memory_total`]): firmware and reserved regions keep it a little below what is installed.
fn memory(text: &str) -> Option<u64> {
    let value = text.lines().find_map(|s| s.strip_prefix("MemTotal:"))?;
    let mut fields = value.split_whitespace();
    let kb = fields.next()?.parse::<u64>().ok()?;
    if fields.next()? != "kB" || fields.next().is_some() || kb == 0 {
        return None;
    }
    let bytes = kb.checked_mul(1024)?;
    common_memory_total(bytes, true)
}

/// The installed memory in bytes: the memory blocks the kernel has online, each `block_size_bytes` (hexadecimal)
/// long, under `memory` (`/sys/devices/system/memory`). A whole number of gigabytes is shown as it is; else it is
/// taken to the nearest common module total. `None` without blocks.
fn installed_memory(memory: &Path) -> Option<u64> {
    let block = read_text(&memory.join("block_size_bytes"))
        .and_then(|s| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .filter(|&b| b > 0)?;
    let entries = std::fs::read_dir(memory).ok()?;
    let online = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("memory")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
        })
        .filter(|e| {
            read_text(&e.path().join("online")).is_some_and(|s| s.trim() == "1")
                || read_text(&e.path().join("state")).is_some_and(|s| s.trim() == "online")
        })
        .count() as u64;
    let bytes = block.checked_mul(online).filter(|&b| b > 0)?;
    if bytes % GIB == 0 {
        Some(bytes)
    } else {
        common_memory_total(bytes, false)
    }
}

/// The memory totals modules are sold in: 1, 2, 3, 4, 6, 8, 12, 16, 24 … gigabytes, a power of two or three times
/// one. `bytes` taken up to the next of them when `up`, else to the nearest by ratio.
fn common_memory_total(bytes: u64, up: bool) -> Option<u64> {
    let totals = (0..40u32).flat_map(|n| {
        let power = 1u64.checked_shl(n).unwrap_or(0);
        [power, power / 2 * 3].into_iter().filter(|&t| t > 0)
    });
    let gib = bytes as f64 / GIB as f64;
    let mut below = 0;
    for total in totals {
        if total as f64 >= gib {
            let nearest = if up || below == 0 || total as f64 / gib < gib / below as f64 {
                total
            } else {
                below
            };
            return nearest.checked_mul(GIB);
        }
        below = total;
    }
    None
}

/// "32 GB", "512 MB": memory in binary units, whole gigabytes from one up.
fn memory_text(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{} GB", (bytes + GIB / 2) / GIB)
    } else {
        format!("{} MB", (bytes + MIB / 2) / MIB)
    }
}

/// A graphics device's product name and, when known, its memory: "GeForce RTX 3060 Lite 12 GB", the size joined
/// by a no-break space.
fn graphics_value(name: &str, memory: Option<u64>) -> String {
    match memory.filter(|&b| b > 0) {
        Some(bytes) => format!("{name} {}", memory_text(bytes).replace(' ', "\u{a0}")),
        None => name.to_owned(),
    }
}

/// The product name of a PCI database entry: the bracketed marketing name when it has one ("GA106 [GeForce RTX
/// 3060]" is "GeForce RTX 3060"), "Lite Hash Rate" shortened to "Lite".
fn product_name(entry: &str) -> String {
    let name = entry
        .find('[')
        .zip(entry.rfind(']'))
        .filter(|(open, close)| open < close)
        .map(|(open, close)| entry[open + 1..close].trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(entry.trim());
    match name.strip_suffix("Lite Hash Rate") {
        Some(start) => format!("{start}Lite"),
        None => name.to_owned(),
    }
}

/// A PCI address, domain and "bus:device.function" in lower case; the kernel writes the domain in four digits,
/// the driver's tool in eight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciAddress {
    domain: u32,
    function: String,
}

impl PciAddress {
    /// "0000:05:00.0" or "00000000:05:00.0".
    pub fn parse(text: &str) -> Option<Self> {
        pci_address(text)
    }
}

fn pci_address(text: &str) -> Option<PciAddress> {
    let (domain, function) = text.trim().split_once(':')?;
    let domain = u32::from_str_radix(domain, 16).ok()?;
    let function = function.to_ascii_lowercase();
    let valid = function.len() == 7
        && function.bytes().enumerate().all(|(i, c)| match i {
            2 => c == b':',
            5 => c == b'.',
            _ => c.is_ascii_hexdigit(),
        });
    valid.then_some(PciAddress { domain, function })
}

/// The longest the NVIDIA driver's tool may take to answer before its graphics memory is left off.
pub const VIDEO_MEMORY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// The memory of each NVIDIA device, which its driver reports only through its own tool: run once, given
/// `timeout` to answer, killed past it. Empty when the tool is missing, fails or is late.
pub fn nvidia_video_memory(timeout: std::time::Duration) -> Vec<(PciAddress, u64)> {
    use std::process::{Command, Stdio};
    let child = Command::new("nvidia-smi")
        .args([
            "--query-gpu=pci.bus_id,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("cupertino-settings: nvidia-smi: {e}");
            return Vec::new();
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        return Vec::new();
    };
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = (&mut stdout).take(MAX_FILE_BYTES).read_to_string(&mut text);
        let _ = send.send(read.map(|_| text));
    });
    let text = receive.recv_timeout(timeout);
    if text.is_err() {
        eprintln!("cupertino-settings: nvidia-smi did not answer in {timeout:?}");
        let _ = child.kill();
    }
    let status = child.wait();
    match (text, status) {
        (Ok(Ok(text)), Ok(status)) if status.success() => nvidia_memory_list(&text),
        _ => Vec::new(),
    }
}

/// "00000000:05:00.0, 12288" lines, the size in mebibytes, as bytes by address; malformed lines are skipped.
fn nvidia_memory_list(text: &str) -> Vec<(PciAddress, u64)> {
    text.lines()
        .filter_map(|line| {
            let (address, mib) = line.split_once(',')?;
            let mib = mib.trim().parse::<u64>().ok().filter(|&m| m > 0)?;
            Some((pci_address(address)?, mib.checked_mul(MIB)?))
        })
        .collect()
}

/// The disk holding the root file system, as sysfs describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StartupDisk {
    /// The file system's label, when it has one.
    label: Option<String>,
    /// The disk's model.
    model: Option<String>,
    bytes: u64,
}

impl StartupDisk {
    /// "<label or model> 1 TB", the size joined by a no-break space; `None` without a size.
    fn value(&self) -> Option<String> {
        let size = disk_size_text(self.bytes)?;
        Some(match self.label.as_ref().or(self.model.as_ref()) {
            Some(name) => format!("{name} {size}"),
            None => size,
        })
    }
}

/// Decimal units, as disks are sold: "1 TB", "1.92 TB", "512 GB", at most two decimals from a terabyte up and
/// whole units below. `None` for an empty disk.
fn disk_size_text(bytes: u64) -> Option<String> {
    const UNITS: [(f64, &str); 4] = [(1e15, "PB"), (1e12, "TB"), (1e9, "GB"), (1e6, "MB")];
    if bytes == 0 {
        return None;
    }
    let bytes = bytes as f64;
    let (scale, unit) = UNITS
        .iter()
        .find(|(scale, _)| bytes >= scale * 0.9995)
        .copied()
        .unwrap_or((1e6, "MB"));
    let places = if scale >= 1e12 { 2 } else { 0 };
    // a disk under a megabyte still shows as one
    let value = decimals((bytes / scale).max(1.0), places);
    Some(format!("{value}{NO_BREAK_SPACE}{unit}"))
}

/// The deepest a mapped device may stack (encryption on logical volumes on RAID …) before the walk gives up.
const MAX_STACK: usize = 16;

/// The disk under the file system mounted at `/` in `mountinfo`: its device by number, else by the name its source
/// gives, then through mapped devices (`slaves`) and partitions to the whole disk, in `root`'s sysfs.
fn startup_disk(root: &Path, mountinfo: &str) -> Option<StartupDisk> {
    let (number, source) = mountinfo.lines().rev().find_map(|line| {
        let fields: Vec<&str> = line.split(' ').collect();
        let dash = fields.iter().position(|f| *f == "-")?;
        (fields.get(4) == Some(&"/")).then_some((*fields.get(2)?, *fields.get(dash + 2)?))
    })?;
    let sys = root.join("sys");
    let block = sys.join("class/block");
    let device = std::fs::canonicalize(sys.join("dev/block").join(number))
        .ok()
        .or_else(|| {
            let name = source_device_name(root, &block, source)?;
            std::fs::canonicalize(block.join(name)).ok()
        })?;
    let label = btrfs_label(&sys, &device).or_else(|| by_label(root, &device));
    let mut device = device;
    for _ in 0..MAX_STACK {
        let mut slaves: Vec<_> = std::fs::read_dir(device.join("slaves"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name())
                    .collect()
            })
            .unwrap_or_default();
        slaves.sort();
        if let Some(slave) = slaves.first() {
            device = std::fs::canonicalize(block.join(slave)).ok()?;
        } else if device.join("partition").exists() {
            device = device.parent()?.to_path_buf();
        } else {
            let bytes = read_text(&device.join("size"))?
                .trim()
                .parse::<u64>()
                .ok()?
                .checked_mul(512)?;
            let model = ["device/model", "device/name"]
                .iter()
                .find_map(|f| read_text(&device.join(f)).and_then(|s| single_line(&s)));
            return Some(StartupDisk {
                label,
                model,
                bytes,
            });
        }
    }
    None
}

/// The block device a mount source names: "/dev/mapper/NAME" by the mapped device's name, else the device file
/// (through its link, as "/dev/disk/by-uuid/…" are).
fn source_device_name(root: &Path, block: &Path, source: &str) -> Option<String> {
    let path = source.strip_prefix('/')?;
    if let Some(mapped) = path.strip_prefix("dev/mapper/") {
        return std::fs::read_dir(block)
            .ok()?
            .filter_map(Result::ok)
            .find_map(|e| {
                let name = read_text(&e.path().join("dm/name"))?;
                (name.trim() == mapped).then(|| e.file_name().to_string_lossy().into_owned())
            });
    }
    let file = root.join(path);
    let target = std::fs::read_link(&file).unwrap_or(file);
    Some(target.file_name()?.to_str()?.to_owned())
}

/// The label of the btrfs file system `device` belongs to.
fn btrfs_label(sys: &Path, device: &Path) -> Option<String> {
    let name = device.file_name()?;
    std::fs::read_dir(sys.join("fs/btrfs"))
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.path().join("devices").join(name).exists())
        .find_map(|e| read_text(&e.path().join("label")).and_then(|s| single_line(&s)))
}

/// The label udev links to `device` under `/dev/disk/by-label`, its escapes ("\x20") undone.
fn by_label(root: &Path, device: &Path) -> Option<String> {
    let name = device.file_name()?;
    std::fs::read_dir(root.join("dev/disk/by-label"))
        .ok()?
        .filter_map(Result::ok)
        .find(|e| std::fs::read_link(e.path()).is_ok_and(|target| target.file_name() == Some(name)))
        .and_then(|e| single_line(&unescape_label(&e.file_name().to_string_lossy())))
}

fn unescape_label(text: &str) -> String {
    let mut bytes = Vec::new();
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        if first == b'\\' && tail.first() == Some(&b'x') {
            if let Some(byte) = tail
                .get(1..3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                bytes.push(byte);
                rest = &tail[3..];
                continue;
            }
        }
        bytes.push(first);
        rest = tail;
    }
    String::from_utf8_lossy(&bytes).into_owned()
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
