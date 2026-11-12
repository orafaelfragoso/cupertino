//! Measured metrics of the design file manager.
//!
//! All lengths are in points (logical pixels); times in seconds.

/// Window chrome.
pub mod window {
    /// Default content size of a new browser window. (window template rect).
    pub const DEFAULT_SIZE: (f64, f64) = (920.0, 436.0);
    /// Template origin of the window (bottom-left, screen of the authoring machine).,
    /// but only the cascade offset below is meaningful on another screen.
    pub const TEMPLATE_ORIGIN: (f64, f64) = (75.0, 740.0);
    /// Offset used when a new window is cascaded from the front window..
    pub const CASCADE_OFFSET: f64 = 29.0;
    /// Height of the unified toolbar + titlebar. data table; that this window uses
    /// the unified style is.
    pub const UNIFIED_TOOLBAR_HEIGHT: f64 = 52.0;
    /// Height of the compact variant. data table.
    pub const COMPACT_TOOLBAR_HEIGHT: f64 = 40.0;
    /// Path bar height..
    pub const PATH_BAR_HEIGHT: f64 = 28.0;
    /// Status bar height (default mode).
    pub const STATUS_BAR_HEIGHT: f64 = 20.0;
    /// Status bar height of the second mode. meaning of the mode unknown.
    pub const STATUS_BAR_HEIGHT_ALT: f64 = 28.0;
    /// Horizontal spacing of the status bar items and its leading inset.
    pub const STATUS_BAR_ITEM_SPACING: f64 = 8.0;
    pub const STATUS_BAR_LEADING_INSET: f64 = 4.0;
    /// Zoom slider of the status bar.
    pub const STATUS_BAR_SLIDER: (f64, f64) = (137.0, 20.0);
    /// Toolbar controls of the browser window, `(width, height)`: the back and forward group, and the search field at
    /// rest, a round button.
    pub const TOOLBAR_BACK_FORWARD: (f64, f64) = (73.0, 36.0);
    pub const TOOLBAR_SEARCH_FIELD: (f64, f64) = (36.0, 36.0);
    /// The search field's width while it has the keyboard or holds text, and the time it takes to change (seconds).
    pub const SEARCH_OPEN: f64 = 249.0;
    pub const SEARCH_GROW_SECONDS: f64 = 0.2;
    /// The most of the toolbar the open search field takes.
    pub const SEARCH_MAX_SHARE: f64 = 0.5;
    /// Horizontal inset of the toolbar's items from its edges, and the space between two items.
    pub const TOOLBAR_INSET: f64 = 8.0;
    pub const TOOLBAR_ITEM_GAP: f64 = 8.0;
    /// Room the folder's title keeps when the search field grows (approximate).
    pub const TITLE_MIN: f64 = 60.0;
    /// The toolbar's controls between the title and the search field, `(width, height)`: the view-mode choice, the
    /// View menu button and the share, Tags and Action group.
    pub const TOOLBAR_VIEW_MODES: (f64, f64) = (160.0, 36.0);
    pub const TOOLBAR_VIEW_MENU: (f64, f64) = (47.0, 36.0);
    pub const TOOLBAR_ACTIONS: (f64, f64) = (109.5, 36.0);
}

/// Sidebar.
pub mod sidebar {
    /// Default width of the sidebar pane, from the window's leading edge to the divider; the content starts 1 pt
    /// further on, past the divider.
    pub const DEFAULT_WIDTH: f64 = 223.0;
    /// Minimum width is this constant plus the width of a legacy (non-overlay) scroller..
    pub const MIN_WIDTH_BASE: f64 = 128.0;
    /// Width of a legacy scroller (the overlay style adds nothing)..
    pub const LEGACY_SCROLLER_WIDTH: f64 = 15.0;
    /// No maximum is set by the application. (absence of a
    /// call in the sidebar setup).
    pub const MAX_WIDTH: Option<f64> = None;
    /// Row size styles of the outline view: small / medium / large. data table.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RowSize {
        Small,
        Medium,
        Large,
    }
    impl RowSize {
        /// Default style when the user preference is unset..
        pub const DEFAULT: RowSize = RowSize::Medium;
        pub fn row_height(self) -> f64 {
            match self {
                RowSize::Small => 24.0,
                RowSize::Medium => 28.0,
                RowSize::Large => 32.0,
            }
        }
        pub fn font_size(self) -> f64 {
            match self {
                RowSize::Small => 11.0,
                RowSize::Medium => 13.0,
                RowSize::Large => 15.0,
            }
        }
        /// Icon size (field 1 of the metrics record, 16/20/24); the field meaning is.
        pub fn icon_size(self) -> f64 {
            match self {
                RowSize::Small => 16.0,
                RowSize::Medium => 20.0,
                RowSize::Large => 24.0,
            }
        }
    }
    /// Group (section header) row: font 11, height 19..
    pub const GROUP_ROW_HEIGHT: f64 = 19.0;
    pub const GROUP_FONT_SIZE: f64 = 11.0;
    /// Leading indent of rows, space between groups, indent of the group row..
    pub const INDENT: f64 = 6.0;
    pub const INTERGROUP_SPACING: f64 = 13.0;
    pub const GROUP_ROW_INDENT: f64 = 6.0;
    /// Indentation per level of the outline.
    pub const INDENTATION_PER_LEVEL: f64 = 14.0;
    /// Cell layout: 17 pt icon at x = 0, text field from x = 22, 17 pt text height.
    pub const CELL_ICON: f64 = 17.0;
    pub const CELL_TEXT_X: f64 = 22.0;
    pub const CELL_TEXT_HEIGHT: f64 = 17.0;
    /// Eject / action button of a row.
    pub const CELL_BUTTON: (f64, f64) = (19.0, 17.0);
}

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

/// Context menus. The context menu of the design uses
/// the same menu window as the menu bar's menus.
pub mod menu {
    /// Item height, regular control size..
    pub const ITEM_HEIGHT: f64 = 22.0;
    /// Separator height..
    pub const SEPARATOR_HEIGHT: f64 = 11.0;
    /// Padding above the first and below the last item. (`_paddingOnEdge).
    pub const EDGE_PADDING: f64 = 5.0;
    /// Corner radius of the menu, continuous..
    pub const RADIUS: f64 = 6.0;
    /// Corner radius of the highlighted row and its inset from both sides, continuous..
    pub const SELECTION_RADIUS: f64 = 4.0;
    pub const SELECTION_INSET: f64 = 5.0;
    /// Text size of regular menus. (sizes 13 to 14 pt map to the regular control size; 13 is the lower).
    pub const TEXT_SIZE: f64 = 13.0;
    /// Leading space before the title of an item without a state column and indent: 9 + 8 - 2..
    pub const TITLE_OFFSET: f64 = 15.0;
    /// Trailing padding, regular control size..
    pub const TRAILING_PADDING: f64 = 14.0;
    pub const SUBMENU_ARROW: f64 = 16.0;
    /// Minimum menu width. approximate: 150 pt applies only to menus with type-select behaviour.
    pub const MIN_WIDTH: f64 = 150.0;
    /// Submenu overlap with the row of its parent, and its vertical offset from that row..
    pub const SUBMENU_OVERLAP: f64 = 5.0;
    pub const SUBMENU_TOP_OFFSET: f64 = 5.0;
    /// The menu keeps this far from the output's edges. approximate.
    pub const SCREEN_MARGIN: f64 = 4.0;
    /// The tag row's view: 192 wide for the colour dots plus 24, 49 tall (`TLabelViewForMenu`).
    pub const TAG_ROW_WIDTH: f64 = 216.0;
    pub const TAG_ROW_HEIGHT: f64 = 49.0;
    pub const TAG_DOT_DIAMETER: f64 = 16.0;
    pub const TAG_DOT_TOP: f64 = 8.0;
}

/// The strip under the toolbar of the trash's view, which holds its Empty button.
pub mod trash {
    /// Height of the strip, its separator line included.
    pub const STRIP_HEIGHT: f64 = 38.0;
    /// From the window's leading edge to the strip's label.
    pub const STRIP_INSET: f64 = 20.0;
    /// From the Empty button to the window's trailing edge.
    pub const BUTTON_INSET: f64 = 18.0;
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

/// The icons on the desktop.
pub mod desktop {
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
    /// How long the rubber band takes to fade in when a drag on the desktop starts, and to fade out once it ends.
    pub const BAND_FADE_IN: f64 = 0.15;
    pub const BAND_FADE_OUT: f64 = 0.2;
    /// The opacity of the icons a drag moves, at the pointer.
    pub const DRAGGED_ALPHA: f32 = 0.6;
}
