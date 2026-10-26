//! Display mode model: the scaled "looks like" choices a display offers and the scale a new
//! display starts at. Pure functions, no I/O.
//!
//! [`looks_like`] is the design display settings' trimmed mode list
//! (§1a): buckets of mode heights around the height that gives about
//! 110 points per inch, one representative mode per bucket, at most five choices. [`candidates`] is
//! the mode list this compositor feeds it (it renders any fractional scale, so it has no installed
//! scaled-mode table), and [`default_choice`] picks the scale a display starts at.

use std::ops::RangeInclusive;

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
        })
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
    /// Physical pixels per logical point on the output.
    pub scale: f64,
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

/// The candidate a new display starts at: the anchor of its list (the size nearest 110 points per
/// inch; that the default follows the anchor is, the design's default mode comes from
/// the display's own data). `None` when the display has no list, because its native height is
/// at or below the 640-point floor: it is shown unscaled.
pub fn default_choice(panel: &Panel, refresh_mhz: u32) -> Option<Candidate> {
    let all = candidates((panel.pixels_wide, panel.pixels_high), refresh_mhz);
    let modes: Vec<Mode> = all.iter().map(|c| c.mode).collect();
    looks_like(panel, &modes).anchor.map(|i| all[i])
}

/// The "looks like" choices of a display among the compositor's candidates, from the largest text
/// to the most space.
pub fn scaled_choices(panel: &Panel, refresh_mhz: u32) -> Vec<Candidate> {
    let all = candidates((panel.pixels_wide, panel.pixels_high), refresh_mhz);
    let modes: Vec<Mode> = all.iter().map(|c| c.mode).collect();
    looks_like(panel, &modes)
        .by_height(&modes)
        .into_iter()
        .map(|i| all[i])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(pixels: (u32, u32), dpi: f32) -> Panel {
        Panel {
            pixels_wide: pixels.0,
            pixels_high: pixels.1,
            vertical_dpi: dpi,
            rotated: false,
            precise_rate: false,
        }
    }

    fn heights(choices: &[Candidate]) -> Vec<u32> {
        choices.iter().map(|c| c.mode.height).collect()
    }

    #[test]
    fn a_dense_laptop_panel_defaults_to_twice_the_pixels_per_point() {
        // 2560x1600 at 227 ppi: the 110 ppi height is 775 points; the 800-point mode fills the
        // panel's 1600 pixels exactly from a 2x buffer and wins its bucket.
        let p = panel((2560, 1600), 227.0);
        let choice = default_choice(&p, 60_000).unwrap();
        assert_eq!(
            (choice.mode.width, choice.mode.height, choice.scale),
            (1280, 800, 2.0)
        );
        assert_eq!(
            heights(&scaled_choices(&p, 60_000)),
            vec![674, 800, 914, 1067, 1164]
        );
    }

    #[test]
    fn a_4k_desktop_monitor_looks_like_1440_points() {
        // 3840x2160 on a 27-inch panel (336 mm high, 163 ppi): the 110 ppi height is 1457 points.
        let p = Panel::from_physical((3840, 2160), (597, 336), false).unwrap();
        let choice = default_choice(&p, 60_000).unwrap();
        assert_eq!(
            (choice.mode.width, choice.mode.height, choice.scale),
            (2560, 1440, 1.5)
        );
        let choices = scaled_choices(&p, 60_000);
        assert_eq!(choices.len(), MAX_CHOICES);
        assert!(
            choices
                .windows(2)
                .all(|w| w[0].mode.height < w[1].mode.height),
            "ordered from larger text to more space"
        );
        assert!(
            choices.iter().all(|c| c.scale > 1.0),
            "five HiDPI choices leave no room for 1x modes"
        );
    }

    #[test]
    fn a_low_density_monitor_stays_unscaled_and_offers_larger_text() {
        // 1920x1080 on a 24-inch panel (296 mm high, 93 ppi): the target lies above the native height,
        // the anchor is the native mode, and the two HiDPI sizes below it are too few, so the 1x
        // native mode joins the list.
        let p = Panel::from_physical((1920, 1080), (527, 296), false).unwrap();
        let choice = default_choice(&p, 60_000).unwrap();
        assert_eq!(
            (choice.mode.height, choice.scale, choice.mode.native),
            (1080, 1.0, true)
        );
        let all = candidates((1920, 1080), 60_000);
        let modes: Vec<Mode> = all.iter().map(|c| c.mode).collect();
        let list = looks_like(&p, &modes);
        let picked: Vec<u32> = list.choices.iter().map(|&i| modes[i].height).collect();
        assert_eq!(
            picked,
            vec![864, 665, 1080],
            "HiDPI choices first, then the 1x one"
        );
    }

    #[test]
    fn a_panel_at_the_floor_has_no_list() {
        let p = panel((1024, 640), 100.0);
        assert_eq!(default_choice(&p, 60_000), None);
        assert!(scaled_choices(&p, 60_000).is_empty());
    }

    #[test]
    fn a_rotated_panel_raises_the_floor_by_its_aspect_ratio() {
        // A portrait 360x640 panel: unrotated its height is the floor and it has no list; rotated
        // the floor is 640 x 0.5625 = 360 points and sizes below the native height appear.
        let mut p = panel((360, 640), 100.0);
        assert!(scaled_choices(&p, 60_000).is_empty());
        p.rotated = true;
        let choices = scaled_choices(&p, 60_000);
        assert!(
            choices.iter().any(|c| c.mode.height < 640),
            "{:?}",
            heights(&choices)
        );
        assert!(
            choices.iter().all(|c| c.mode.height >= 360),
            "{:?}",
            heights(&choices)
        );
    }

    #[test]
    fn a_mode_filling_the_native_buffer_wins_over_a_nearer_one() {
        let near = Mode {
            width: 1100,
            height: 550,
            scale: 2.0,
            pixels_high: 1100,
            refresh_mhz: 60_000,
            native: false,
        };
        let exact = Mode {
            width: 1000,
            height: 500,
            scale: 2.0,
            pixels_high: 1000,
            refresh_mhz: 60_000,
            native: false,
        };
        let bucket = Bucket {
            target: 550.0,
            members: vec![0, 1],
        };
        assert_eq!(bucket.best(&[near, exact], 1000, false), Some(1));
    }

    #[test]
    fn ties_go_to_the_higher_scale_then_rate_then_the_nearer_height() {
        let m = |height: u32, scale: f32, refresh_mhz: u32| Mode {
            width: height * 2,
            height,
            scale,
            pixels_high: 1,
            refresh_mhz,
            native: false,
        };
        let bucket = |n: usize| Bucket {
            target: 500.0,
            members: (0..n).collect(),
        };
        assert_eq!(
            bucket(2).best(&[m(500, 1.0, 60_000), m(450, 2.0, 60_000)], 1000, false),
            Some(1),
            "scale"
        );
        assert_eq!(
            bucket(2).best(&[m(500, 2.0, 59_940), m(450, 2.0, 60_000)], 1000, true),
            Some(1),
            "precise rate"
        );
        assert_eq!(
            bucket(2).best(&[m(450, 2.0, 59_940), m(500, 2.0, 60_000)], 1000, false),
            Some(1),
            "60 Hz either way, nearer wins"
        );
        assert_eq!(
            bucket(2).best(&[m(500, 2.0, 60_000), m(450, 2.0, 60_000)], 1000, false),
            Some(0),
            "nearer wins"
        );
    }

    #[test]
    fn modes_of_another_aspect_ratio_are_not_bucketed() {
        let p = panel((1920, 1200), 220.0);
        let wide = Mode {
            width: 1280,
            height: 720,
            scale: 2.0,
            pixels_high: 1440,
            refresh_mhz: 60_000,
            native: false,
        };
        let bucket = Bucket::fill(&p, &[wide], 0.0, 2000.0, 720.0);
        assert!(bucket.members.is_empty());
    }

    #[test]
    fn candidates_cover_one_to_three_in_eighths() {
        let all = candidates((3840, 2160), 60_000);
        assert_eq!(all.len(), 17);
        assert_eq!(
            (all[0].mode.height, all[0].mode.scale, all[0].mode.native),
            (2160, 1.0, true)
        );
        let one_and_a_half = all.iter().find(|c| c.scale == 1.5).unwrap();
        assert_eq!(
            (
                one_and_a_half.mode.width,
                one_and_a_half.mode.height,
                one_and_a_half.mode.scale,
                one_and_a_half.mode.pixels_high
            ),
            (2560, 1440, 2.0, 2880)
        );
        assert_eq!(all.last().unwrap().mode.scale, 3.0);
    }
}
