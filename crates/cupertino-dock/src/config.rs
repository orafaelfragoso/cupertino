//! Dock configuration. Setting names are re-expressed as neutral field names; the existence of the
//! corresponding settings in the design is as designed.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Bottom,
    Left,
    Right,
}

impl Orientation {
    /// True when tiles flow horizontally.
    pub fn is_horizontal(self) -> bool {
        matches!(self, Orientation::Bottom)
    }

    /// Unit vector pointing away from the screen edge the dock sits on.
    pub fn outward(self) -> (f64, f64) {
        match self {
            Orientation::Bottom => (0.0, -1.0),
            Orientation::Left => (1.0, 0.0),
            Orientation::Right => (-1.0, 0.0),
        }
    }
}

/// Visual effect used when a window is minimized into the dock. The three effects exist in the
/// reference (names, durations unknown); the compositor implements the meshes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MinimizeEffect {
    WarpMinimize,
    Scale,
    FunnelMinimize,
}

impl MinimizeEffect {
    pub const ALL: [MinimizeEffect; 3] = [
        MinimizeEffect::WarpMinimize,
        MinimizeEffect::Scale,
        MinimizeEffect::FunnelMinimize,
    ];

    /// Effect duration in milliseconds, into the slot: warp 450, scale 200, funnel 400.
    pub fn duration_ms(self) -> f64 {
        match self {
            MinimizeEffect::WarpMinimize => 450.0,
            MinimizeEffect::Scale => 200.0,
            MinimizeEffect::FunnelMinimize => 400.0,
        }
    }

    /// Duration in seconds; the design doubles it while its slow-motion modifier is held.
    pub fn duration_s(self, slow_motion: bool) -> f64 {
        self.duration_ms() / 1000.0 * if slow_motion { 2.0 } else { 1.0 }
    }
}

pub use cupertino_defaults::dock::{
    default_tile_size, sanitize_large_size, sanitize_tile_size, DEFAULT_LARGE_SIZE,
    DEFAULT_TILE_SIZE, DEFAULT_TILE_SIZE_SMALL_DISPLAY, SMALL_DISPLAY_HEIGHT,
};
/// Auto-hide speed per unit of the time modifier, in milliseconds.
pub const AUTOHIDE_SPEED_MS: f64 = 60.0;

/// Gap between neighbouring tiles, as a fraction of the tile size: 4 pt between 64 pt tiles.
pub const TILE_GAP_RATIO: f64 = 4.0 / 64.0;
/// Padding between the bar's edge and its tiles, as a fraction of the tile size: 10 pt around 64 pt tiles, an
/// 84 pt deep bar.
pub const BAR_PADDING_RATIO: f64 = 10.0 / 64.0;
/// Extent of a separator's slot along the bar, as a fraction of the tile size: 26 pt at 64, so with the gaps either
/// side the tiles around it stand 34 pt apart.
pub const SEPARATOR_RATIO: f64 = 26.0 / 64.0;
/// Padding between the tiles and the bar's edge on the screen-edge side, as a fraction of the tile size: 10 pt at
/// 64, the same as on the other side.
pub const BAR_EDGE_PADDING_RATIO: f64 = 10.0 / 64.0;
/// Gap between the bar and the screen edge, in points.
pub const EDGE_MARGIN: f64 = 5.0;
/// How many recent applications the bar's recents section holds at most.
pub const RECENTS_MAX: usize = 3;

#[derive(Clone, Debug)]
pub struct DockConfig {
    /// default 64 (52 on displays at most 800 pt tall, see [`DockConfig::for_display_height`]).
    pub tile_size: f64,
    /// default 128.
    pub large_size: f64,
    /// default off.
    pub magnification: bool,
    pub orientation: Orientation,
    /// Gap between neighbouring tiles as a fraction of the tile size (4 pt at 64).
    pub spacing: f64,
    /// Half-width of the magnification bump in unmagnified tile units: the fisheye size is
    /// `fish_span · tile_size` (3.0).
    pub fish_span: f64,
    /// constant 42.0
    pub fish_speed: f64,
    /// constant 270.0, unit milliseconds
    pub move_speed_ms: f64,
    /// Extent of a separator's slot along the dock axis as a fraction of the tile size (26 pt at 64): with the gaps
    /// on both sides the tiles either side of it stand 34 pt apart, the line in the middle.
    pub separator_ratio: f64,
    /// Padding between the bar's edge and the tiles as a fraction of the tile size (10 pt at 64, so the bar is
    /// 84 pt deep).
    pub padding_ratio: f64,
    /// Padding between the tiles and the bar's edge on the screen-edge side as a fraction of the tile size (10 pt at
    /// 64); the rest of the bar's depth is on the other side.
    pub edge_padding_ratio: f64,
    /// Gap between the bar and the screen edge, in points.
    pub edge_margin: f64,
    /// Show the recent applications' section, between the applications and the windows and the trash.
    pub show_recents: bool,
    /// How many recent applications that section holds at most ([`RECENTS_MAX`]).
    pub recents_max: usize,
    /// Draw a running indicator under running apps (setting existence, default unknown).
    pub show_running_indicators: bool,
    /// Send minimized windows to the app tile instead of creating a tile per window (setting
    /// existence, default unknown).
    pub minimize_to_app: bool,
    pub minimize_effect: MinimizeEffect,
    /// Bounce a tile while its app launches (default on).
    pub launch_animation: bool,
    /// Hide the dock when the pointer is away (default off).
    pub autohide: bool,
    /// Seconds before a hidden dock is revealed (default 0.22 s).
    pub autohide_delay: f64,
    /// Show/hide duration in seconds. The default is 0.22 s.
    pub autohide_duration: f64,
    /// Multiplier of the auto-hide speed (default 1.0).
    pub autohide_time_modifier: f64,
    /// Space above the tiles inside the bar (literal 6). Used by the
    /// auto-hide duration formula; the layout padding is `bar_padding`.
    pub distance_top: f64,
    /// Space below the tiles (literal 10). Used by the auto-hide formula.
    pub distance_bottom: f64,
    /// Space at the sides of a vertical dock (literal 8).
    pub distance_sides: f64,
    /// Thickness of the reveal strip at the screen edge. approximate (1–4 px).
    pub reveal_thickness: f64,
    /// Duration of a tile appearing or disappearing (width growing from / shrinking to zero).
    /// approximate.
    pub tile_presence_s: f64,
}

impl Default for DockConfig {
    fn default() -> Self {
        Self {
            tile_size: DEFAULT_TILE_SIZE,
            large_size: DEFAULT_LARGE_SIZE,
            magnification: false,
            orientation: Orientation::Bottom,
            spacing: TILE_GAP_RATIO,
            fish_span: 3.0,
            fish_speed: 42.0,
            move_speed_ms: 270.0,
            separator_ratio: SEPARATOR_RATIO,
            padding_ratio: BAR_PADDING_RATIO,
            edge_padding_ratio: BAR_EDGE_PADDING_RATIO,
            edge_margin: EDGE_MARGIN,
            show_recents: false,
            recents_max: RECENTS_MAX,
            show_running_indicators: true,
            minimize_to_app: false,
            minimize_effect: MinimizeEffect::WarpMinimize,
            launch_animation: true,
            autohide: false,
            autohide_delay: 0.22,
            autohide_duration: 0.22,
            autohide_time_modifier: 1.0,
            distance_top: 6.0,
            distance_bottom: 10.0,
            distance_sides: 8.0,
            reveal_thickness: 2.0,
            tile_presence_s: 0.25,
        }
    }
}

impl DockConfig {
    /// Defaults for a display of the given height in points (tile size 52 when it is at most
    /// 800 pt tall, else 64).
    pub fn for_display_height(display_height: f64) -> Self {
        Self {
            tile_size: default_tile_size(display_height),
            ..Self::default()
        }
    }

    /// Configuration from optional stored preferences with the design validation rules.
    pub fn from_preferences(
        tile_size: Option<f64>,
        large_size: Option<f64>,
        magnification: Option<bool>,
        display_height: f64,
    ) -> Self {
        Self {
            tile_size: sanitize_tile_size(tile_size, display_height),
            large_size: sanitize_large_size(large_size),
            magnification: magnification.unwrap_or(false),
            ..Self::default()
        }
    }

    /// Gap between neighbouring tiles in points.
    pub fn gap(&self) -> f64 {
        self.tile_size * self.spacing
    }

    /// Padding between the bar's edge and its tiles in points.
    pub fn bar_padding(&self) -> f64 {
        self.tile_size * self.padding_ratio
    }

    /// Extent of a separator's slot along the bar in points.
    pub fn separator_extent(&self) -> f64 {
        self.tile_size * self.separator_ratio
    }

    /// Padding between the tiles and the bar's edge on the screen-edge side, in points: the rest of the bar's depth
    /// is on the other side.
    pub fn edge_padding(&self) -> f64 {
        self.tile_size * self.edge_padding_ratio
    }

    /// Depth of the bar across its edge: a tile and the padding either side.
    pub fn bar_thickness(&self) -> f64 {
        self.tile_size + 2.0 * self.bar_padding()
    }

    /// Auto-hide speed in milliseconds: `time_modifier · 60`.
    pub fn autohide_speed_ms(&self) -> f64 {
        self.autohide_time_modifier * AUTOHIDE_SPEED_MS
    }

    /// Height of the bar used by the auto-hide formula: `top + tile + bottom`.
    pub fn bar_extent(&self) -> f64 {
        self.distance_top + self.tile_size + self.distance_bottom
    }

    /// Duration of the configured minimize effect in seconds.
    pub fn minimize_duration_s(&self, slow_motion: bool) -> f64 {
        self.minimize_effect.duration_s(slow_motion)
    }
}

/// Which bounce description drives [`crate::Bounce`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BounceModel {
    /// The curves: launch parabola with a 700 ms cycle capped at 120 s, attention as
    /// three hops of 500/326/212 ms with amplitudes 1/0.5/0.25 repeating every 2000 ms.
    Verified,
    /// Generic parametric arcs driven by `height`, `decay`, `informational_arcs` and the
    /// `secondary_* fields (approximate, kept as an alternative).
    Parametric,
}

/// Lift in points as a function of the tile length `len`: `k · (len − m) + c`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiftFormula {
    pub k: f64,
    pub m: f64,
    pub c: f64,
}

impl LiftFormula {
    pub fn pt(&self, len: f64) -> f64 {
        self.k * (len - self.m) + self.c
    }
}

/// One hop of the attention sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hop {
    pub length_ms: f64,
    pub amplitude: f64,
}

/// Launch and attention bounce parameters.
///
/// With [`BounceModel::Verified`] (default) the timings, amplitudes and lift formulas are the
/// values: launch lift `0.2 · (len − 8) + 6` pt, attention
/// lift `len + 1` pt, informational attention plays the hop sequence once (about 1.04 s), critical
/// attention repeats it every 2000 ms until cleared. The fields `height`, `decay`,
/// `informational_arcs`, `secondary_duration` and `secondary_interval` only drive
/// [`BounceModel::Parametric`] (approximate). The preference defaults of the design
/// (`bounce-duration` 60 s, `bounce-secondary-duration` 3 s, `bounce-secondary-interval` 60 s,
/// ) govern when attention is re-armed and are exposed as
/// `attention_* fields.
#[derive(Clone, Debug)]
pub struct BounceConfig {
    pub model: BounceModel,
    /// Peak lift of an arc as a fraction of the tile size (parametric model).
    pub height: f64,
    /// Seconds per launch arc (0.7 = 2 · 350 ms).
    pub period: f64,
    /// Lift multiplier applied per successive informational arc (parametric model).
    pub decay: f64,
    /// A launch bounce gives up after this many seconds (120).
    pub launch_timeout: f64,
    /// Arcs of an informational attention request (parametric model).
    pub informational_arcs: u32,
    /// Length in seconds of one burst of a critical attention request (parametric model).
    pub secondary_duration: f64,
    /// Pause in seconds between critical bursts (parametric model).
    pub secondary_interval: f64,
    /// Tile length the lift formulas are evaluated for by [`crate::Bounce::offset`]; the dock
    /// layout passes its own tile size instead.
    pub tile_len: f64,
    pub launch_lift: LiftFormula,
    pub attention_lift: LiftFormula,
    pub attention_hops: [Hop; 3],
    /// Repeat period of the attention sequence in seconds (2.0).
    pub attention_cycle: f64,
    /// Seconds a critical attention request bounces in total (`bounce-duration`, 60).
    pub attention_initial_duration: f64,
    /// Length of a repeated attention burst in seconds (`bounce-secondary-duration`, 3).
    pub attention_secondary_duration: f64,
    /// Seconds between attention re-arms (`bounce-secondary-interval`, 60).
    pub attention_secondary_interval: f64,
}

impl Default for BounceConfig {
    fn default() -> Self {
        Self {
            model: BounceModel::Verified,
            height: 0.5,
            period: 0.7,
            decay: 1.0,
            launch_timeout: 120.0,
            informational_arcs: 1,
            secondary_duration: 1.4,
            secondary_interval: 1.0,
            tile_len: DEFAULT_TILE_SIZE,
            launch_lift: LiftFormula {
                k: 0.2,
                m: 8.0,
                c: 6.0,
            },
            attention_lift: LiftFormula {
                k: 1.0,
                m: 0.0,
                c: 1.0,
            },
            attention_hops: [
                Hop {
                    length_ms: 500.0,
                    amplitude: 1.0,
                },
                Hop {
                    length_ms: 326.0,
                    amplitude: 0.5,
                },
                Hop {
                    length_ms: 212.0,
                    amplitude: 0.25,
                },
            ],
            attention_cycle: 2.0,
            attention_initial_duration: 60.0,
            attention_secondary_duration: 3.0,
            attention_secondary_interval: 60.0,
        }
    }
}

impl BounceConfig {
    /// The generic parametric arcs (approximate), as used before the curves were
    /// available: lift 0.5 tiles, 0.7 s arcs, 20 s launch timeout.
    pub fn parametric() -> Self {
        Self {
            model: BounceModel::Parametric,
            launch_timeout: 20.0,
            ..Self::default()
        }
    }

    /// Total length of the hop sequence in seconds (about 1.038 s).
    pub fn attention_sequence_s(&self) -> f64 {
        self.attention_hops.iter().map(|h| h.length_ms).sum::<f64>() / 1000.0
    }
}
