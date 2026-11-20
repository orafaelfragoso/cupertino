//! The measured metrics of the desktop's items. All lengths are in points; the rubber band's fades are the
//! catalogue's (`Timing::DESKTOP_BAND_FADE_IN`, `Timing::DESKTOP_BAND_FADE_OUT`).

/// Side of an item's icon.
pub const ICON_SIZE: f64 = 64.0;
/// The grid the icons sit in: from one column to the next, and from one row to the next.
pub const COLUMN_PITCH: f64 = 96.0;
pub const ROW_PITCH: f64 = 106.0;
/// From the desktop's right edge to the first column, and from its top edge to the first row.
pub const RIGHT_MARGIN: f64 = 6.0;
pub const TOP_MARGIN: f64 = 8.0;
/// From the top of a grid cell to the selection backdrop around the icon.
pub const ICON_TOP: f64 = 2.0;
/// Room between the icon and the edge of the backdrop a selected icon sits on.
pub const BACKDROP_PAD: f64 = 4.0;
/// The backdrop's corner radius over the icon's side, and the most it reaches.
pub const BACKDROP_RADIUS_RATIO: f64 = 0.25;
pub const BACKDROP_RADIUS_MAX: f64 = 7.0;
/// The opacity of the grey backdrop behind a selected icon.
pub const BACKDROP_ALPHA: f32 = 0.3;
/// Between the backdrop and the name under it.
pub const LABEL_GAP: f64 = 2.0;
/// The widest a name runs, and the most lines it takes.
pub const LABEL_WIDTH: f64 = 80.0;
pub const LABEL_LINES: u32 = 2;
/// Text size of a name.
pub const TEXT_SIZE: f64 = 12.0;
/// Room between a selected name and the ends of the accent capsule behind it, sideways and up and down.
pub const LABEL_PAD: (f64, f64) = (4.0, 1.0);
/// The most a selected name's capsule is rounded.
pub const LABEL_RADIUS_MAX: f64 = 8.0;
/// The soft shadow under a name that is not selected: its downward offset, its blur and its opacity.
pub const LABEL_SHADOW_OFFSET: f64 = 1.0;
pub const LABEL_SHADOW_BLUR: f64 = 2.5;
pub const LABEL_SHADOW_ALPHA: f32 = 0.6;
/// The opacity of the icons a drag moves, at the pointer.
pub const DRAGGED_ALPHA: f32 = 0.6;
