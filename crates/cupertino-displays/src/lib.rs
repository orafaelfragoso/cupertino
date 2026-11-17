//! Display mode model: the scaled "looks like" choices a display offers and the scale a new
//! display starts at. Pure functions, no I/O.
//!
//! [`looks_like`] is the design display settings' trimmed mode list
//! (§1a): buckets of mode heights around the height that gives about
//! 110 points per inch, one representative mode per bucket, at most five choices. [`candidates`] is
//! the mode list this compositor feeds it (it renders any fractional scale, so it has no installed
//! scaled-mode table), and [`default_choice`] picks the scale a display starts at.
//!
//! A built-in panel offers a fixed ladder of sizes instead ([`catalogue`]): its default size, three sizes with larger
//! text and one with more space, each drawn from a buffer of twice its points. [`geometry`] keeps the quantities of an
//! output apart: the mode in panel pixels, the logical extent in points, the effective scale and buffer rasters.

pub mod arrangement;
pub mod geometry;

use std::ops::RangeInclusive;

pub use geometry::{Extent, GeometryError, OutputGeometry};

/// No mode is listed below this height in points; for a display rotated by 90 or 270 degrees the
/// floor is this times the aspect ratio..
pub const MIN_HEIGHT_POINTS: f32 = 640.0;
/// The list is centred on the point height that gives this many points per inch..
pub const TARGET_POINTS_PER_INCH: f32 = 110.0;
/// Bucket half-width: the target height divided by this..
pub const BUCKET_DIVISOR: f32 = 12.0;
/// Step between bucket centres: the anchor mode's height divided by this..
pub const COARSE_DIVISOR: f32 = 6.0;
/// When two coarse steps below the native height reach the floor, the half-width becomes
/// (native height − floor) / this and the step twice that. Value, role.
pub const NEAR_FLOOR_DIVISOR: f32 = 5.0;
/// Bucket slots: the anchor, then larger sizes in odd slots and smaller sizes in even slots..
pub const BUCKET_SLOTS: usize = 11;
/// Choices the list stops at (the pane's five chiclets)..
pub const MAX_CHOICES: usize = 5;
/// With fewer HiDPI choices than this, 1x modes are added..
pub const HIDPI_CHOICES_WANTED: usize = 4;
/// A mode is offered only below this many points per inch (pixel density / backing scale)..
pub const POINT_DENSITY_LIMIT: f32 = 180.0;
/// Modes whose aspect ratio differs from the display's by more than this are not bucketed..
pub const ASPECT_TOLERANCE: f64 = 0.01;
/// Distance a bucket's choice starts from..
const UNSET_DISTANCE: f32 = 100_000_000.0;

/// A display the list is built for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    pub pixels_wide: u32,
    pub pixels_high: u32,
    /// Vertical pixel density in pixels per inch.
    pub vertical_dpi: f32,
    /// Rotated by 90 or 270 degrees.
    pub rotated: bool,
    /// The display asks for precise refresh rates (override policy bit 0x01000000): modes
    /// are compared on the millihertz rate instead of whole hertz.
    pub precise_rate: bool,
    /// The machine's own panel (a laptop's or a tablet's): it offers the built-in ladder of sizes ([`catalogue`])
    /// instead of the scaled-mode buckets of an external display.
    pub built_in: bool,
}

impl Panel {
    /// A panel of `pixels` whose image is `millimetres` large (unrotated). `None` when the physical
    /// height is unknown (zero), as for virtual outputs and displays without a size in their EDID.
    pub fn from_physical(
        pixels: (u32, u32),
        millimetres: (u32, u32),
        rotated: bool,
    ) -> Option<Panel> {
        if millimetres.1 == 0 || pixels.1 == 0 {
            return None;
        }
        let vertical_dpi = pixels.1 as f32 / (millimetres.1 as f32 / 25.4);
        Some(Panel {
            pixels_wide: pixels.0,
            pixels_high: pixels.1,
            vertical_dpi,
            rotated,
            precise_rate: false,
            built_in: false,
        })
    }

    /// The same panel, marked as the machine's own.
    pub fn built_in(self, built_in: bool) -> Panel {
        Panel { built_in, ..self }
    }

    fn aspect_ratio(&self) -> f32 {
        self.pixels_wide as f32 / self.pixels_high as f32
    }
}

/// A mode the display can show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mode {
    /// Size in points: what the desktop "looks like".
    pub width: u32,
    pub height: u32,
    /// Backing scale: drawing-buffer pixels per point (1 for a plain mode, 2 for a HiDPI mode).
    pub scale: f32,
    /// Height of the drawing buffer in pixels.
    pub pixels_high: u32,
    pub refresh_mhz: u32,
    /// The panel's own timing shown 1:1.
    pub native: bool,
}

impl Mode {
    fn aspect_ratio(&self) -> f32 {
        self.width as f32 / self.height as f32
    }

    /// The rate ties are broken on: whole hertz, or millihertz for a precise-rate display. The
    /// reference reads or; that the first is rounded hertz is.
    fn rate(&self, precise: bool) -> u32 {
        if precise {
            self.refresh_mhz
        } else {
            (self.refresh_mhz + 500) / 1000
        }
    }
}

/// The list for one display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LooksLike {
    /// Indices into the modes, in the order they were picked: the anchor's bucket first, then
    /// larger and smaller sizes alternately, HiDPI modes before 1x modes. At most [`MAX_CHOICES`].
    pub choices: Vec<usize>,
    /// The representative of the bucket around the 110 points-per-inch height; the other buckets
    /// are spaced from it.
    pub anchor: Option<usize>,
}

impl LooksLike {
    /// The choices from the largest text (smallest point height) to the most space.
    pub fn by_height(&self, modes: &[Mode]) -> Vec<usize> {
        let mut sorted = self.choices.clone();
        sorted.sort_by_key(|&i| (modes[i].height, modes[i].width));
        sorted
    }
}

/// The modes whose height is in a range, and the height the choice among them should be near.
struct Bucket {
    target: f32,
    members: Vec<usize>,
}

impl Bucket {
    /// The bucket with every mode that matches the display's aspect ratio, fits its pixel size and
    /// whose height is in range.
    fn fill(panel: &Panel, modes: &[Mode], min: f32, max: f32, target: f32) -> Bucket {
        let aspect = panel.aspect_ratio();
        let members = modes
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                let diff = f64::from(aspect - m.aspect_ratio());
                (-ASPECT_TOLERANCE..=ASPECT_TOLERANCE).contains(&diff)
                    && m.width <= panel.pixels_wide
                    && m.height <= panel.pixels_high
                    && min <= m.height as f32
                    && (m.height as f32) < max
            })
            .map(|(i, _)| i)
            .collect();
        Bucket { target, members }
    }

    /// The bucket's representative. A mode whose buffer is exactly the native height wins over
    /// the others (then the native timing over scaled ones); otherwise the higher backing scale,
    /// then the higher rate, then the height nearest the target; a full tie goes to the later mode
    /// (the design compares a field of the mode description there, unnamed: approximate).
    fn best(&self, modes: &[Mode], native_height: u32, precise: bool) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_distance = UNSET_DISTANCE;
        let mut exact_seen = false;
        for &i in &self.members {
            let m = &modes[i];
            let distance = (self.target - m.height as f32).abs();
            let exact = m.pixels_high == native_height;
            let takes = if !exact_seen {
                if exact {
                    exact_seen = true;
                    true
                } else {
                    Self::beats(
                        m,
                        distance,
                        best.map(|b| (&modes[b], best_distance)),
                        precise,
                    )
                }
            } else if !exact {
                false
            } else {
                let b = &modes[best.expect("an exact mode was taken")];
                if m.native == b.native {
                    Self::beats(m, distance, Some((b, best_distance)), precise)
                } else {
                    m.native
                }
            };
            if takes {
                best = Some(i);
                best_distance = distance;
            }
        }
        best
    }

    /// Whether `m` at `distance` replaces the current choice.
    fn beats(m: &Mode, distance: f32, current: Option<(&Mode, f32)>, precise: bool) -> bool {
        let Some((b, best_distance)) = current else {
            return true;
        };
        if m.scale != b.scale {
            return m.scale > b.scale;
        }
        let (rate, best_rate) = (m.rate(precise), b.rate(precise));
        if rate != best_rate {
            return rate > best_rate;
        }
        distance <= best_distance
    }
}

/// The "looks like" list of `panel` among `modes`.
pub fn looks_like(panel: &Panel, modes: &[Mode]) -> LooksLike {
    let empty = LooksLike {
        choices: Vec::new(),
        anchor: None,
    };
    let floor = if panel.rotated {
        MIN_HEIGHT_POINTS * panel.aspect_ratio()
    } else {
        MIN_HEIGHT_POINTS
    };
    let native = panel.pixels_high as f32;
    if floor >= native {
        return empty;
    }
    let best = |b: &Bucket| b.best(modes, panel.pixels_high, panel.precise_rate);
    let target = native / panel.vertical_dpi * TARGET_POINTS_PER_INCH;
    let half = target / BUCKET_DIVISOR;

    // The anchor: walk down from the target height to the floor, else up to the native height.
    let mut anchor: Option<(Bucket, usize)> = None;
    let mut centre = target;
    if floor <= centre {
        loop {
            let bucket = Bucket::fill(panel, modes, centre - half, centre + half, centre);
            let this = centre;
            centre -= half;
            if let Some(i) = best(&bucket) {
                anchor = Some((bucket, i));
                centre = this;
                break;
            }
            if centre < floor {
                break;
            }
        }
    }
    if anchor.is_none() {
        while centre <= native {
            let bucket = Bucket::fill(panel, modes, centre - half, centre + half, centre);
            if let Some(i) = best(&bucket) {
                anchor = Some((bucket, i));
                break;
            }
            centre += half;
            if native < centre {
                return empty;
            }
        }
    }
    let Some((anchor_bucket, anchor_mode)) = anchor else {
        return empty;
    };
    let anchor_height = modes[anchor_mode].height as f32;

    let mut step = anchor_height / COARSE_DIVISOR;
    let half = if floor <= native - 2.0 * step {
        anchor_bucket.target / BUCKET_DIVISOR
    } else {
        let h = (native - floor) / NEAR_FLOOR_DIVISOR;
        step = 2.0 * h;
        h
    };
    let mut slots: Vec<Option<Bucket>> = (0..BUCKET_SLOTS).map(|_| None).collect();
    slots[0] = Some(anchor_bucket);

    // Larger sizes (more space) in the odd slots.
    let mut slot = 1;
    let mut height = anchor_height + step;
    while slot < BUCKET_SLOTS && height <= native {
        let bucket = Bucket::fill(panel, modes, height - half, height + half, height);
        let found = best(&bucket);
        slots[slot] = Some(bucket);
        match found {
            Some(i) => {
                height = modes[i].height as f32 + step;
                slot += 2;
            }
            None => height += half,
        }
    }

    // Smaller sizes (larger text) in the even slots, never below the floor.
    let mut slot = 2;
    let mut height = anchor_height - step;
    while floor <= height + half {
        let target = height.max(floor);
        let min = (height - half).max(floor);
        let bucket = Bucket::fill(panel, modes, min, height + half, target);
        let found = best(&bucket);
        slots[slot] = Some(bucket);
        match found {
            Some(i) => {
                height = modes[i].height as f32 - step;
                slot += 2;
            }
            None => {
                if min == target {
                    break;
                }
                height = target - half;
            }
        }
        if slot >= BUCKET_SLOTS {
            break;
        }
    }

    // One choice per bucket, HiDPI modes first; 1x modes only when too few HiDPI ones were found.
    let mut choices: Vec<usize> = Vec::new();
    let pick = |hidpi: bool, choices: &mut Vec<usize>| {
        for bucket in slots.iter().flatten() {
            if choices.len() >= MAX_CHOICES {
                break;
            }
            let Some(i) = best(bucket) else { continue };
            let m = &modes[i];
            let kind = if hidpi { m.scale > 1.0 } else { m.scale == 1.0 };
            if !choices.contains(&i) && kind && panel.vertical_dpi / m.scale < POINT_DENSITY_LIMIT {
                choices.push(i);
            }
        }
    };
    pick(true, &mut choices);
    if choices.len() < HIDPI_CHOICES_WANTED {
        pick(false, &mut choices);
    }
    LooksLike {
        choices,
        anchor: Some(anchor_mode),
    }
}

/// Output scales the compositor offers, in eighths: 1 to 3 (approximate: the design picks among
/// installed scaled resolutions, a compositor that renders fractional scales has none,
/// and eighths are exact in the 1/120 units of the fractional-scale protocol).
pub const SCALE_EIGHTHS: RangeInclusive<u32> = 8..=24;

/// One "looks like" size of a display and the output scale that gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub mode: Mode,
    /// Physical pixels per logical point on the output: the effective scale the output is set to.
    pub scale: f64,
}

impl Candidate {
    /// The size the desktop looks like, in points.
    pub fn logical(&self) -> Extent {
        Extent::new(self.mode.width, self.mode.height)
    }

    /// Buffer pixels per point the size is drawn from: 2 for a scaled size, 1 for an unscaled one.
    pub fn backing(&self) -> u32 {
        self.mode.scale as u32
    }

    /// The buffer the size is drawn from: its points times its backing scale.
    pub fn raster(&self) -> Extent {
        let backing = self.backing();
        Extent::new(self.mode.width * backing, self.mode.height * backing)
    }
}

/// The sizes a display of `pixels` at `refresh_mhz` can look like, one per scale of
/// [`SCALE_EIGHTHS`]. A fractional scale is drawn from a buffer of the next whole scale, as the
/// reference draws its scaled modes from a 2x buffer, so that is the mode's backing scale.
pub fn candidates(pixels: (u32, u32), refresh_mhz: u32) -> Vec<Candidate> {
    SCALE_EIGHTHS
        .map(|eighths| {
            let scale = f64::from(eighths) / 8.0;
            let width = (f64::from(pixels.0) / scale).round() as u32;
            let height = (f64::from(pixels.1) / scale).round() as u32;
            let backing = eighths.div_ceil(8);
            let mode = Mode {
                width,
                height,
                scale: backing as f32,
                pixels_high: height * backing,
                refresh_mhz,
                native: eighths == 8,
            };
            Candidate { mode, scale }
        })
        .collect()
}

/// The sizes a built-in panel offers, as widths in [`BUILT_IN_DEFAULT_STEP`]ths of its default width: three with
/// larger text, the default and one with more space.
pub const BUILT_IN_STEPS: [u32; 5] = [1024, 1147, 1352, 1512, 1800];
/// The step of the default size in [`BUILT_IN_STEPS`].
pub const BUILT_IN_DEFAULT_STEP: u32 = 1512;
/// Points per inch a built-in panel's default size aims at.
pub const BUILT_IN_POINTS_PER_INCH: f32 = 127.0;
/// A whole scale whose density is within this fraction of the aimed one is the default scale: the panel is then
/// drawn without resampling.
pub const WHOLE_SCALE_SNAP: f32 = 0.05;
/// Buffer pixels per point of a scaled built-in size: each is drawn from a raster twice its points.
pub const BUILT_IN_BACKING: u32 = 2;

/// Connector kinds of the machine's own panel: embedded DisplayPort, LVDS and DSI.
pub const BUILT_IN_CONNECTORS: [&str; 3] = ["eDP", "LVDS", "DSI"];

/// Whether the connector named `connector` (`eDP-1`, `DP-2`) drives the machine's own panel.
pub fn is_built_in(connector: &str) -> bool {
    BUILT_IN_CONNECTORS.iter().any(|kind| {
        connector
            .strip_prefix(kind)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    })
}

/// The "looks like" choices of a display and the one it starts at.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalogue {
    /// From the largest text (fewest points) to the most space.
    pub choices: Vec<Candidate>,
    /// Index of the default size in `choices`.
    pub default: Option<usize>,
}

impl Catalogue {
    pub fn default_choice(&self) -> Option<Candidate> {
        self.default.map(|i| self.choices[i])
    }

    /// The index of the choice an output of `logical` points shows, if it shows one.
    pub fn position(&self, logical: Extent) -> Option<usize> {
        self.choices.iter().position(|c| c.logical() == logical)
    }
}

/// The lowest height a list may offer: the floor in points, times the aspect ratio for a turned panel.
fn floor_height(panel: &Panel) -> f32 {
    if panel.rotated {
        MIN_HEIGHT_POINTS * panel.aspect_ratio()
    } else {
        MIN_HEIGHT_POINTS
    }
}

/// The width of a built-in panel's default size: the one at [`BUILT_IN_POINTS_PER_INCH`], at a whole scale when one
/// is within [`WHOLE_SCALE_SNAP`] of it, never wider than the panel and never below the floor.
fn built_in_default_width(panel: &Panel, floor: f32) -> u32 {
    let (wide, high) = (panel.pixels_wide as f32, panel.pixels_high as f32);
    let aimed = panel.vertical_dpi / BUILT_IN_POINTS_PER_INCH;
    let whole = aimed.round().max(1.0);
    let scale = if aimed <= 1.0 {
        1.0
    } else if (aimed - whole).abs() <= WHOLE_SCALE_SNAP * whole {
        whole
    } else {
        aimed
    };
    let width = (wide / scale).round();
    let narrowest = (floor * wide / high).ceil();
    width.max(narrowest).min(wide) as u32
}

/// A built-in panel's ladder: [`BUILT_IN_STEPS`] of its default width, each height following the panel's aspect
/// ratio. Sizes below the floor are left out; a size with more space than the panel has is its own size instead,
/// unscaled, once.
fn built_in_catalogue(panel: &Panel, refresh_mhz: u32) -> Catalogue {
    let empty = Catalogue {
        choices: Vec::new(),
        default: None,
    };
    let floor = floor_height(panel);
    if panel.vertical_dpi.is_nan() || floor >= panel.pixels_high as f32 {
        return empty;
    }
    let default_width = built_in_default_width(panel, floor);
    let (wide, high) = (u64::from(panel.pixels_wide), u64::from(panel.pixels_high));
    let mut choices = Vec::new();
    let mut default = None;
    for step in BUILT_IN_STEPS {
        let width = (u64::from(default_width) * u64::from(step)
            + u64::from(BUILT_IN_DEFAULT_STEP) / 2)
            / u64::from(BUILT_IN_DEFAULT_STEP);
        let width = width.min(wide);
        let height = (width * high + wide / 2) / wide;
        let listed = choices
            .iter()
            .any(|c: &Candidate| u64::from(c.mode.width) == width);
        if width == 0 || listed || (height as f32) < floor {
            continue;
        }
        let (width, height) = (width as u32, height as u32);
        let native = width == panel.pixels_wide;
        let backing = if native { 1 } else { BUILT_IN_BACKING };
        if step == BUILT_IN_DEFAULT_STEP {
            default = Some(choices.len());
        }
        choices.push(Candidate {
            mode: Mode {
                width,
                height,
                scale: backing as f32,
                pixels_high: height * backing,
                refresh_mhz,
                native,
            },
            scale: f64::from(panel.pixels_wide) / f64::from(width),
        });
    }
    Catalogue { choices, default }
}

/// The "looks like" choices of a display and its default size. A built-in panel offers its ladder
/// ([`BUILT_IN_STEPS`]) around the size at [`BUILT_IN_POINTS_PER_INCH`]; an external display the buckets of
/// [`looks_like`] among the compositor's [`candidates`], starting at the anchor.
pub fn catalogue(panel: &Panel, refresh_mhz: u32) -> Catalogue {
    if panel.built_in {
        return built_in_catalogue(panel, refresh_mhz);
    }
    let all = candidates((panel.pixels_wide, panel.pixels_high), refresh_mhz);
    let modes: Vec<Mode> = all.iter().map(|c| c.mode).collect();
    let list = looks_like(panel, &modes);
    let order = list.by_height(&modes);
    let default = list.anchor.and_then(|a| order.iter().position(|&i| i == a));
    Catalogue {
        choices: order.into_iter().map(|i| all[i]).collect(),
        default,
    }
}

/// The candidate a new display starts at. A built-in panel starts at its default size; an external display at the
/// anchor of its list (the size nearest 110 points per inch), even when that is not one of the listed choices.
/// `None` when the display has no list, because its native height is at or below the 640-point floor: it is shown
/// unscaled.
pub fn default_choice(panel: &Panel, refresh_mhz: u32) -> Option<Candidate> {
    if panel.built_in {
        return built_in_catalogue(panel, refresh_mhz).default_choice();
    }
    let all = candidates((panel.pixels_wide, panel.pixels_high), refresh_mhz);
    let modes: Vec<Mode> = all.iter().map(|c| c.mode).collect();
    looks_like(panel, &modes).anchor.map(|i| all[i])
}

/// The "looks like" choices of a display, from the largest text to the most space ([`catalogue`]).
pub fn scaled_choices(panel: &Panel, refresh_mhz: u32) -> Vec<Candidate> {
    catalogue(panel, refresh_mhz).choices
}
