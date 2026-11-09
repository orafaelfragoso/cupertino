//! Titlebar and control metrics, window styles and interaction settings.
//!
//! Four window styles share the window control buttons: 14 pt discs whose centres are 23 pt apart, the first one as
//! far from the window's leading edge as from its top edge, so the group sits in the middle of the row it occupies
//! (the 32 pt title bar of a titled window, the 52 pt toolbar row of a unified window, the 52 pt top of the sidebar
//! panel of a full-height-sidebar window). The compact 40 pt row is the exception: its group starts 12 pt from the
//! leading edge and 13 pt below the top, centred vertically only. A right-to-left window mirrors the group to the
//! trailing edge ([`DecorationMetrics::direction`]). Every style has the 16 pt window corner radius. The rim width of
//! one device pixel and the live-resize zones are as designed. The buttons of the smaller size classes are 9 pt discs;
//! the title metrics and the glyph proportions are approximate, labelled, and configurable.

use cupertino_theme::metrics::ControlSize;

/// The window style: where the title bar is and who draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TitlebarStyle {
    /// A titled window (plain windows, dialogs): a 32 pt title bar above the client's surface with
    /// the buttons and a centred title, drawn by the decorations.
    Standard,
    /// Titlebar merged with the client's toolbar: the bar is the 52 pt toolbar row, the client's
    /// content runs under it (full-size content), and the decorations draw only the window control
    /// buttons, vertically centred in the row with the unified leading inset.
    /// The client draws the toolbar, its fill ([`crate::palette::Palette::titlebar`]) and the title.
    Unified,
    /// [`TitlebarStyle::Unified`] with the compact 40 pt toolbar row.
    UnifiedCompact,
    /// A full-height sidebar: the client's surface fills the window, its sidebar panel runs from the
    /// window's top edge to its bottom edge, and the decorations draw only the window control
    /// buttons, in the 52 pt top of that panel. The client's toolbar floats over its content beside
    /// the panel. As with the unified styles the client moves the window from its toolbar and the
    /// top of its sidebar.
    Sidebar,
}

impl TitlebarStyle {
    pub const ALL: [TitlebarStyle; 4] = [
        TitlebarStyle::Standard,
        TitlebarStyle::Unified,
        TitlebarStyle::UnifiedCompact,
        TitlebarStyle::Sidebar,
    ];

    /// The client draws the row the buttons sit in and moves the window from it: the decorations
    /// draw the buttons alone, over the client's surface, and take no room from the frame.
    pub fn client_drawn(self) -> bool {
        !matches!(self, TitlebarStyle::Standard)
    }

    /// The index of the style in [`TitlebarStyle::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Diameter of a window control button, regular size.
pub const WINDOW_BUTTON_DIAMETER: f64 = 14.0;
/// Diameter of a window control button at the small and smaller size classes, and at any other than regular.
pub const WINDOW_BUTTON_DIAMETER_SMALL: f64 = 9.0;
/// Distance between the centres of neighbouring window control buttons.
pub const WINDOW_BUTTON_PITCH: f64 = 23.0;
/// Height of a titled window's title bar.
pub const TITLED_BAR_HEIGHT: f64 = 32.0;
/// Height of a toolbar row: the unified bar, and the top of a full-height sidebar.
pub const TOOLBAR_ROW_HEIGHT: f64 = 52.0;
/// Height of the compact unified toolbar row.
pub const COMPACT_TOOLBAR_ROW_HEIGHT: f64 = 40.0;
/// Distance from the leading edge to the first button of the compact unified row (its top is 13 pt down, the middle
/// of the row).
pub const COMPACT_BUTTON_LEADING: f64 = 12.0;
/// The window's corner radius.
pub const WINDOW_CORNER_RADIUS: f64 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ButtonKind {
    Close,
    Minimize,
    Zoom,
}

impl ButtonKind {
    pub const ALL: [ButtonKind; 3] = [ButtonKind::Close, ButtonKind::Minimize, ButtonKind::Zoom];

    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleAlignment {
    Center,
    Leading,
}

/// What a double click on the titlebar does. Which action is the default is approximate
/// (the setting exists in the design, its default is not known).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoubleClickAction {
    Zoom,
    Minimize,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecorationMetrics {
    /// height of the title bar of a titled window.
    pub titlebar_height_standard: f64,
    /// height of the toolbar row of a unified window, regular size.
    pub titlebar_height_unified: f64,
    /// height of the toolbar row of a unified window, compact size.
    pub titlebar_height_compact: f64,
    /// height of the top of the sidebar panel that holds the buttons, and of the toolbar row
    /// floating over the content, in a full-height-sidebar window.
    pub titlebar_height_sidebar: f64,
    /// the window's corner radius, in every style.
    pub corner_radius: f64,
    /// diameter of a window control button, regular size class.
    pub button_diameter: f64,
    /// Which way the title bar lays out: right to left puts the button group at the right edge, close outermost.
    pub direction: crate::chrome::Direction,
    /// edge of the square art box of a button, used as its hit area.
    pub button_art: f64,
    /// size of the frame of one button (the cell the frame view lays out);
    /// the disc and its art box are centred in it.
    pub button_cell: (f64, f64),
    /// gap between neighbouring button cells.
    pub button_spacing: f64,
    /// distance from the left edge to the first cell of a titlebar without toolbar.
    pub button_leading_standard: f64,
    /// the same for a unified toolbar window whose buttons are centred in the bar.
    pub button_leading_unified: f64,
    /// the same for the compact unified toolbar.
    pub button_leading_compact: f64,
    /// the same for the top of the sidebar panel of a full-height-sidebar window.
    pub button_leading_sidebar: f64,
    /// Glyph half-extent as a fraction of the button diameter (authored, approximate).
    pub glyph_half_extent: f64,
    /// Glyph stroke width in points (authored, approximate).
    pub glyph_stroke: f64,
    /// approximate: ring around a button as a fraction of its diameter drawn in a darker tone.
    pub button_ring: f64,
    /// -ish: title size in points.
    pub title_size: f64,
    /// -ish: title weight (semibold).
    pub title_weight: u16,
    /// approximate: minimum gap between the button group and the title.
    pub title_gap: f64,
    /// approximate: space kept free at the trailing edge for a leading-aligned title.
    pub title_trailing: f64,
    /// resize zone outside the frame geometry of titled windows.
    pub resize_outset: f64,
    /// resize zone inside the frame geometry.
    pub resize_inset: f64,
    /// corner resize zone of the top-left, top-right and bottom-left corners.
    /// Measured along each edge from the corner (a length, not a square side).
    pub resize_corner: f64,
    /// corner resize zone of the bottom-right corner.
    pub resize_corner_bottom_right: f64,
}

impl Default for DecorationMetrics {
    fn default() -> Self {
        Self::for_button_size(ControlSize::Regular)
    }
}

impl DecorationMetrics {
    /// Metrics with the button diameter of a size class (14 regular, 9 small).
    pub fn for_button_size(size: ControlSize) -> Self {
        let d = match size {
            ControlSize::Regular => WINDOW_BUTTON_DIAMETER,
            _ => WINDOW_BUTTON_DIAMETER_SMALL,
        };
        // the first button's centre is as far from the leading edge as from the top: half the row
        let leading = |row: f64| row / 2.0 - WINDOW_BUTTON_DIAMETER / 2.0;
        Self {
            titlebar_height_standard: TITLED_BAR_HEIGHT,
            titlebar_height_unified: TOOLBAR_ROW_HEIGHT,
            titlebar_height_compact: COMPACT_TOOLBAR_ROW_HEIGHT,
            titlebar_height_sidebar: TOOLBAR_ROW_HEIGHT,
            corner_radius: WINDOW_CORNER_RADIUS,
            button_diameter: d,
            direction: crate::chrome::Direction::LeftToRight,
            button_art: 16.0,
            button_cell: (WINDOW_BUTTON_DIAMETER, 16.0),
            button_spacing: WINDOW_BUTTON_PITCH - WINDOW_BUTTON_DIAMETER,
            button_leading_standard: leading(TITLED_BAR_HEIGHT),
            button_leading_unified: leading(TOOLBAR_ROW_HEIGHT),
            button_leading_compact: COMPACT_BUTTON_LEADING,
            button_leading_sidebar: leading(TOOLBAR_ROW_HEIGHT),
            glyph_half_extent: 0.2,
            glyph_stroke: 1.1,
            button_ring: 0.04,
            title_size: 13.0,
            title_weight: 600,
            title_gap: 8.0,
            title_trailing: 12.0,
            resize_outset: 4.0,
            resize_inset: 3.0,
            resize_corner: 12.0,
            resize_corner_bottom_right: 15.0,
        }
    }

    /// Distance between the centres of neighbouring buttons: the cell width plus the spacing.
    pub fn button_pitch(&self) -> f64 {
        self.button_cell.0 + self.button_spacing
    }

    /// Distance from the left edge to the first button cell of a style.
    pub fn button_leading(&self, style: TitlebarStyle) -> f64 {
        match style {
            TitlebarStyle::Standard => self.button_leading_standard,
            TitlebarStyle::Unified => self.button_leading_unified,
            TitlebarStyle::UnifiedCompact => self.button_leading_compact,
            TitlebarStyle::Sidebar => self.button_leading_sidebar,
        }
    }

    /// Height of the titlebar strip of a style: the row the buttons are centred in.
    pub fn titlebar_height(&self, style: TitlebarStyle) -> f64 {
        match style {
            TitlebarStyle::Standard => self.titlebar_height_standard,
            TitlebarStyle::Unified => self.titlebar_height_unified,
            TitlebarStyle::UnifiedCompact => self.titlebar_height_compact,
            TitlebarStyle::Sidebar => self.titlebar_height_sidebar,
        }
    }

    /// Height the strip takes from the frame: the client's surface starts below it. Zero for the
    /// styles whose row the client draws, which lies over the client's content.
    pub fn titlebar_reserved(&self, style: TitlebarStyle) -> f64 {
        if style.client_drawn() {
            0.0
        } else {
            self.titlebar_height(style)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_size_classes_follow_the_control_table() {
        assert_eq!(DecorationMetrics::default().button_diameter, 14.0);
        assert_eq!(
            DecorationMetrics::for_button_size(ControlSize::Small).button_diameter,
            9.0
        );
        assert_eq!(DecorationMetrics::default().button_art, 16.0);
    }

    #[test]
    fn a_unified_titlebar_is_taller_and_takes_no_frame_height() {
        let m = DecorationMetrics::default();
        assert!(
            m.titlebar_height(TitlebarStyle::Unified) > m.titlebar_height(TitlebarStyle::Standard)
        );
        assert_eq!(m.titlebar_reserved(TitlebarStyle::Standard), 32.0);
        for style in [
            TitlebarStyle::Unified,
            TitlebarStyle::UnifiedCompact,
            TitlebarStyle::Sidebar,
        ] {
            assert_eq!(m.titlebar_reserved(style), 0.0, "{style:?}");
            assert!(style.client_drawn());
        }
        assert!(!TitlebarStyle::Standard.client_drawn());
    }

    #[test]
    fn each_style_has_its_row_and_the_first_button_in_the_middle_of_it() {
        let m = DecorationMetrics::default();
        let rows = [
            (TitlebarStyle::Standard, 32.0),
            (TitlebarStyle::Unified, 52.0),
            (TitlebarStyle::UnifiedCompact, 40.0),
            (TitlebarStyle::Sidebar, 52.0),
        ];
        for (style, row) in rows {
            assert_eq!(m.titlebar_height(style), row, "{style:?}");
            // the first cell is half a button from the centre, which is half the row from the edge; the compact row's
            // group starts a point nearer the edge
            let centre = if style == TitlebarStyle::UnifiedCompact {
                19.0
            } else {
                row / 2.0
            };
            assert_eq!(
                m.button_leading(style) + m.button_cell.0 / 2.0,
                centre,
                "{style:?}"
            );
        }
        assert_eq!(m.button_pitch(), 23.0);
        assert_eq!(m.corner_radius, 16.0);
    }
}
