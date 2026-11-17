//! Typography: substitute family, size presets, tracking table and line metrics.
//!
//! font metrics and the tracking table are as designed from the system UI font
//! (–2`); point sizes of the control size classes are as designed. Rendering uses an open font.

use crate::tokens::{RawFont, RawTypography};
use std::collections::BTreeMap;

/// Open substitute for the system UI font.
pub const SUBSTITUTE_FAMILY: &str = "Inter";

/// Size multiplier suggested for the substitute because of its larger x-height
/// (0.546 vs 0.508 em). approximate; configurable through
/// [`Typography::substitute_size_scale`].
pub const DEFAULT_SUBSTITUTE_SIZE_SCALE: f64 = 0.96;

/// Tracking table of the system UI font, track 0,
/// (size in points → value in font units)..
const BUILTIN_TRACK_0: [(f64, f64); 21] = [
    (6.0, 82.0),
    (9.0, 38.0),
    (10.0, 24.0),
    (11.0, 12.0),
    (12.0, 0.0),
    (13.0, -12.0),
    (14.0, -22.0),
    (15.0, -32.0),
    (16.0, -40.0),
    (17.0, -52.0),
    (20.0, -46.0),
    (22.0, -24.0),
    (24.0, 6.0),
    (28.0, 28.0),
    (32.0, 26.0),
    (36.0, 21.0),
    (50.0, 14.0),
    (64.0, 7.0),
    (80.0, 0.0),
    (100.0, 0.0),
    (138.0, 0.0),
];

/// Tracks −1 and +1 shift track 0 by ∓28 units at 13 pt in the real table; the built-in table only
/// carries track 0, so other tracks are available only when loaded from the specification.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackingTable {
    pub units_per_em: f64,
    /// `(track, [(size_pt, units)])` sorted by track, each row sorted by size.
    tracks: Vec<(f64, Vec<(f64, f64)>)>,
}

impl TrackingTable {
    pub fn builtin() -> Self {
        Self {
            units_per_em: 2048.0,
            tracks: vec![(0.0, BUILTIN_TRACK_0.to_vec())],
        }
    }

    pub fn from_tracks(
        units_per_em: f64,
        tracks: impl IntoIterator<Item = (f64, Vec<(f64, f64)>)>,
    ) -> Self {
        let mut tracks: Vec<_> = tracks.into_iter().collect();
        for (_, row) in tracks.iter_mut() {
            row.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        tracks.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self {
            units_per_em,
            tracks,
        }
    }

    pub fn track_values(&self) -> Vec<f64> {
        self.tracks.iter().map(|t| t.0).collect()
    }

    pub fn row(&self, track: f64) -> Option<&[(f64, f64)]> {
        self.tracks
            .iter()
            .find(|t| t.0 == track)
            .map(|t| t.1.as_slice())
    }

    fn interp_row(row: &[(f64, f64)], size: f64) -> f64 {
        let (Some(first), Some(last)) = (row.first(), row.last()) else {
            return 0.0;
        };
        if size <= first.0 {
            return first.1;
        }
        if size >= last.0 {
            return last.1;
        }
        let i = row.partition_point(|p| p.0 <= size);
        let (a, b) = (row[i - 1], row[i]);
        a.1 + (b.1 - a.1) * (size - a.0) / (b.0 - a.0)
    }

    /// Tracking in font units for `size` points, linearly interpolated between table sizes (clamped
    /// outside the table) and, for fractional `track` values, between tracks.
    pub fn units(&self, size: f64, track: f64) -> f64 {
        let Some(first) = self.tracks.first() else {
            return 0.0;
        };
        let last = self.tracks.last().expect("non-empty");
        if track <= first.0 || self.tracks.len() == 1 {
            return Self::interp_row(&first.1, size);
        }
        if track >= last.0 {
            return Self::interp_row(&last.1, size);
        }
        let i = self.tracks.partition_point(|t| t.0 <= track);
        let (a, b) = (&self.tracks[i - 1], &self.tracks[i]);
        let (ua, ub) = (Self::interp_row(&a.1, size), Self::interp_row(&b.1, size));
        ua + (ub - ua) * (track - a.0) / (b.0 - a.0)
    }

    /// Additional advance per glyph in points: `units / units_per_em · size`.
    pub fn letter_spacing(&self, size: f64, track: f64) -> f64 {
        self.units(size, track) / self.units_per_em * size
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub units_per_em: f64,
    pub ascender: f64,
    pub descender: f64,
    pub line_gap: f64,
    pub cap_height: f64,
    pub x_height: f64,
}

impl FontMetrics {
    /// System UI font metrics: UPM 2048, ascender 1980, descender −432.
    pub const BUILTIN: FontMetrics = FontMetrics {
        units_per_em: 2048.0,
        ascender: 1980.0,
        descender: -432.0,
        line_gap: 0.0,
        cap_height: 1443.0,
        x_height: 1040.0,
    };

    /// Default line height in em: `(ascender − descender + line_gap) / units_per_em`.
    pub fn line_height_em(&self) -> f64 {
        (self.ascender - self.descender + self.line_gap) / self.units_per_em
    }

    pub(crate) fn from_font(f: &RawFont) -> Self {
        Self {
            units_per_em: f.units_per_em,
            ascender: f.ascender,
            descender: f.descender,
            line_gap: f.line_gap,
            cap_height: f.cap_height,
            x_height: f.x_height,
        }
    }
}

/// Point sizes of the control size classes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizePresets {
    pub regular: f64,
    pub small: f64,
    pub mini: f64,
    pub label: f64,
}

impl Default for SizePresets {
    fn default() -> Self {
        Self {
            regular: 13.0,
            small: 11.0,
            mini: 9.0,
            label: 10.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f64,
    /// CSS-style weight (400 regular, 600 semibold, 700 bold).
    pub weight: u16,
}

#[derive(Clone, Debug)]
pub struct Typography {
    pub family: &'static str,
    pub sizes: SizePresets,
    pub tracking: TrackingTable,
    pub metrics: FontMetrics,
    pub substitute_size_scale: f64,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            family: SUBSTITUTE_FAMILY,
            sizes: SizePresets::default(),
            tracking: TrackingTable::builtin(),
            metrics: FontMetrics::BUILTIN,
            substitute_size_scale: DEFAULT_SUBSTITUTE_SIZE_SCALE,
        }
    }
}

impl Typography {
    pub(crate) fn from_raw(raw: &RawTypography) -> Self {
        let mut t = Typography::default();
        let get = |k: &str| raw.sizes.get(k).copied();
        t.sizes = SizePresets {
            regular: get("regular").unwrap_or(t.sizes.regular),
            small: get("small").unwrap_or(t.sizes.small),
            mini: get("mini").unwrap_or(t.sizes.mini),
            label: get("label").unwrap_or(t.sizes.label),
        };
        let font = &raw.system_font;
        t.metrics = FontMetrics::from_font(font);
        let tracks = font.tracking.iter().filter_map(|(track, row)| {
            let track: f64 = track.parse().ok()?;
            let row: Vec<(f64, f64)> = row
                .iter()
                .filter_map(|(s, v)| Some((s.parse().ok()?, *v)))
                .collect();
            Some((track, row))
        });
        let table = TrackingTable::from_tracks(font.units_per_em, tracks);
        if !table.tracks.is_empty() {
            t.tracking = table;
        }
        t
    }

    /// Letter spacing in points for a run of `size` points (track 0, the normal setting).
    pub fn letter_spacing(&self, size: f64) -> f64 {
        self.tracking.letter_spacing(size, 0.0)
    }

    /// Default line height in points.
    pub fn line_height(&self, size: f64) -> f64 {
        self.metrics.line_height_em() * size
    }

    /// Size to request from the substitute family for a nominal `size`.
    pub fn substitute_size(&self, size: f64) -> f64 {
        size * self.substitute_size_scale
    }

    /// Window title: 13 pt semibold. approximate (size, weight needs runtime
    /// verification).
    pub fn window_title(&self) -> TextStyle {
        TextStyle {
            size: self.sizes.regular,
            weight: 600,
        }
    }

    /// Menu bar item: 13 pt regular, the application name bold. approximate.
    pub fn menu_bar_item(&self) -> TextStyle {
        TextStyle {
            size: self.sizes.regular,
            weight: 400,
        }
    }

    pub fn menu_bar_app_name(&self) -> TextStyle {
        TextStyle {
            size: self.sizes.regular,
            weight: 700,
        }
    }

    /// Table of the loaded track-0 row as an ordered map, for inspection.
    pub fn track_zero(&self) -> BTreeMap<u32, f64> {
        self.tracking
            .row(0.0)
            .into_iter()
            .flatten()
            .map(|&(s, u)| (s as u32, u))
            .collect()
    }
}
