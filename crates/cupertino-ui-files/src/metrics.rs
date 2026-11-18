//! Measured metrics of the design file manager's views.
//!
//! All lengths are in points (logical pixels); times in seconds.

/// Icon view.
pub mod icon_view {
    pub const ICON_SIZE_DEFAULT: u32 = 64;
    pub const ICON_SIZE_MIN: u32 = 16;
    pub const ICON_SIZE_MAX: u32 = 512;
    /// Tick marks of the view-options slider: 16..=128 in the design, the code accepts up to 512..
    pub const ICON_SLIDER_NIB_RANGE: (u32, u32) = (16, 128);
    pub const GRID_SPACING_DEFAULT: u32 = 54;
    pub const GRID_SPACING_MIN: u32 = 1;
    pub const GRID_SPACING_MAX: u32 = 100;
    pub const TEXT_SIZE_DEFAULT: u16 = 12;
    pub const TEXT_SIZE_MIN: u16 = 10;
    pub const TEXT_SIZE_MAX: u16 = 16;
    /// Subtitle font = max(9, title - 2)..
    pub const SUBTITLE_MIN_SIZE: f64 = 9.0;
    pub const SUBTITLE_SIZE_DELTA: f64 = -2.0;
    /// Padding added around the icon: `PAD_FRACTION * max(w, h)..
    pub const PAD_FRACTION: f64 = 0.125;
    /// Hidden preference defaults of the label-size computation: maximum label width in
    /// characters (`[5, 100]`), minimum in characters (`>= 4`), extra width, vertical gap, and the
    /// bounds of the line count.
    pub const LABEL_MAX_CHARS: u32 = 20;
    pub const LABEL_MIN_CHARS: u32 = 4;
    pub const LABEL_EXTRA_WIDTH: u32 = 1;
    pub const LABEL_GAP: u32 = 2;
    pub const LABEL_LINES_MAX: f64 = 2.0;
    pub const LABEL_LINES_MIN: f64 = 1.0;
    /// Fixed additions of the cell size..
    pub const LABEL_WIDTH_PAD: f64 = 6.0;
    pub const BOTTOM_LABEL_PAD: (f64, f64) = (2.0, 4.0);
    pub const RIGHT_LABEL_PAD: f64 = 2.0;
    /// Header of a group section: height 32 (width 100 is a value)..
    pub const GROUP_HEADER_HEIGHT: f64 = 32.0;
    /// Space between the arranged icon view's edges and its cells: at the leading and trailing sides, and above the first
    /// row and below the last.
    pub const GRID_INSET: (f64, f64) = (10.0, 8.0);
    /// Margins of the free-form grid and its default cell..
    pub const FREE_GRID_MARGIN: f64 = 4.0;
    pub const FREE_GRID_DEFAULT_CELL: f64 = 32.0;
    /// Smallest icon box for which the label-right layout uses the compact vertical rule..
    pub const MIN_ICON: f64 = 16.0;
}

/// List view.
pub mod list_view {
    pub const ICON_SIZE_DEFAULT: u32 = 16;
    pub const ICON_SIZE_MIN: u32 = 16;
    pub const ICON_SIZE_MAX: u32 = 32;
    pub const TEXT_SIZE_DEFAULT: u16 = 13;
    pub const TEXT_SIZE_MIN: u16 = 10;
    pub const TEXT_SIZE_MAX: u16 = 16;
    /// Extra height added to `max(icon, text line) when both are above zero. (which supersedes 's
    /// pitch).
    pub const ROW_PADDING: f64 = 4.0;
    /// Intercell spacing of the table (columns, rows): the file manager's list view resets it to zero, so the row pitch
    /// is the row height. (`TListViewController::configureView`).
    pub const INTERCELL: (f64, f64) = (0.0, 0.0);
    /// The table template's intercell spacing, which the list view does not use.
    pub const INTERCELL_TEMPLATE: (f64, f64) = (5.0, 2.0);
    /// Indentation per level (the code uses 15 pt, the template 16 pt; 1 pt when flat)..
    pub const INDENT_PER_LEVEL: f64 = 15.0;
    pub const INDENT_PER_LEVEL_TEMPLATE: f64 = 16.0;
    pub const INDENT_FLAT: f64 = 1.0;
    /// Header height: 23 pt in the template, 28 pt from the helper `ListHeaderRowHeight`..
    pub const HEADER_HEIGHT_TEMPLATE: f64 = 23.0;
    pub const HEADER_HEIGHT: f64 = 28.0;
}

/// Column (browser) view.
pub mod column_view {
    /// Initial preferred column width of the settings object..
    pub const PREFERRED_WIDTH: f64 = 245.0;
    /// Template minimum and preferred width, row height, visible columns.
    pub const MIN_WIDTH: f64 = 100.0;
    pub const TEMPLATE_ROW_HEIGHT: f64 = 24.0;
    pub const TEMPLATE_VISIBLE_COLUMNS: u32 = 6;
    /// Row height = icon size + this..
    pub const ROW_EXTRA: f64 = 6.0;
    /// Preview column of the browser: width 312, preview square 292 with 10 pt side insets, icon 48.
    pub const PREVIEW_COLUMN_WIDTH: f64 = 312.0;
    pub const PREVIEW_SIDE_INSET: f64 = 10.0;
    pub const PREVIEW_ICON: f64 = 48.0;
    /// Default width of the preview pane (list / icon views). (registry).
    pub const PREVIEW_PANE_WIDTH: f64 = 250.0;
}

/// Timings.
pub mod timing {
    /// Sidebar toggle and splitter animation..
    pub const SIDEBAR_TOGGLE: f64 = 0.3;
    /// Slow-motion multipliers when only Shift is held..
    pub const SLOW_MO_SIDEBAR: f64 = 10.0;
    pub const SLOW_MO_SPLITTER: f64 = 6.0;
    /// Window frame animation: cap and constants of the system helper. structure,
    /// approximate function and velocity.
    pub const WINDOW_RESIZE_CAP: f64 = 0.35;
    /// Minimum character delay after which the type-select buffer clears..
    pub const TYPE_SELECT_MIN_TIMEOUT: f64 = 2.0;
    /// Default double-click interval used before a rename starts..
    pub const DOUBLE_CLICK_INTERVAL: f64 = 0.5;
    /// Number of items a confirmation is asked for when opening many..
    pub const OPEN_ITEMS_WARNING_LIMIT: u32 = 25;
    /// Search typing grace period in milliseconds..
    pub const SEARCH_GRACE_MS: u32 = 500;
}

/// Selection look.
pub mod selection {
    /// The icon bubble's padding is the longest side of the icon over this: `max(w, h) * 2 * 0.03125`..
    pub const BUBBLE_DIVISOR: f64 = 16.0;
    /// The bubble's corner radius over the longest side of the icon: 7 pt around a 64 pt icon.
    pub const BUBBLE_RADIUS_RATIO: f64 = 7.0 / 64.0;
    /// Upper bound of the bubble's border width, which is `min(2, trunc(padding)); the border is white at this alpha..
    pub const BUBBLE_BORDER_MAX: f64 = 2.0;
    pub const BUBBLE_BORDER_ALPHA: f32 = 0.5;
    /// The width of the accent ring round the item a context menu is open on.
    pub const CONTEXT_RING: f64 = 2.0;
}

/// The rubber band drawn while dragging on empty space.
pub mod marquee {
    /// Border width of the collection view's band: 1 pt, in the tertiary label colour, over a quaternary label fill.
    /// (the file manager's icon view uses the default band).
    pub const BORDER_WIDTH: f64 = 1.0;
    /// Seconds between the auto-scroll steps while the pointer is held near an edge..
    pub const AUTOSCROLL_INTERVAL: f64 = 0.05;
    /// Distance from the view's edge inside which auto-scroll runs, and the largest step per tick, the step ramping
    /// linearly with the depth. for the generic view auto-scroll; that the band uses it is.
    pub const EDGE_ZONE: f64 = 50.0;
    pub const MAX_STEP: f64 = 20.0;
}

/// Dragging items.
pub mod drag {
    /// The pointer must move this far from the press before a drag starts. as the toolkit's hysteresis;
    /// that the file manager's views use it is.
    pub const START_DISTANCE: f64 = 5.0;
}
