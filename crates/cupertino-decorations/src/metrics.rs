//! Titlebar and control metrics, window classes and interaction settings.
//!
//! the window control button diameter (12 pt) and art box (16 × 16 pt) are as designed
//! (`cupertino_theme::metrics`); corner radii are as designed;
//! the rim width of one device pixel is as designed; the live-resize zones are as designed; the button cell, spacing and leading insets are as designed. Every other number below is approximate, labelled, and configurable.

use cupertino_theme::metrics::{alignment_height, Control, ControlSize};

/// Titlebar flavour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TitlebarStyle {
    /// Toolbar-less window: a plain titlebar with a centred title.
    Standard,
    /// Titlebar merged with the client's toolbar: the bar is the 52 pt toolbar strip, the client's
    /// content runs under it (full-size content), and the decorations draw only the window control
    /// buttons, vertically centred in the strip with the unified leading inset.
    /// The client draws the toolbar, its fill ([`crate::palette::Palette::titlebar`]) and the title.
    Unified,
}

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
    /// approximate: height of a toolbar-less titlebar.
    pub titlebar_height_standard: f64,
    /// approximate: height of a titlebar with a unified toolbar.
    pub titlebar_height_unified: f64,
    /// diameter of a window control button, regular size class.
    pub button_diameter: f64,
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
    /// rim width in device pixels.
    pub rim_width_px: f64,
}

impl Default for DecorationMetrics {
    fn default() -> Self {
        Self::for_button_size(ControlSize::Regular)
    }
}

impl DecorationMetrics {
    /// Metrics with the button diameter of a size class (12 regular, 9 small).
    pub fn for_button_size(size: ControlSize) -> Self {
        let d = alignment_height(Control::WindowControlButton, size).unwrap_or(12.0);
        Self {
            titlebar_height_standard: 28.0,
            titlebar_height_unified: 52.0,
            button_diameter: d,
            button_art: 16.0,
            button_cell: (14.0, 16.0),
            button_spacing: 6.0,
            button_leading_standard: 7.0,
            button_leading_unified: 19.0,
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
            rim_width_px: 1.0,
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
        }
    }

    /// Height of the titlebar strip of a style.
    pub fn titlebar_height(&self, style: TitlebarStyle) -> f64 {
        match style {
            TitlebarStyle::Standard => self.titlebar_height_standard,
            TitlebarStyle::Unified => self.titlebar_height_unified,
        }
    }

    /// Height the strip takes from the frame: the client's surface starts below it. Zero for a
    /// unified bar, which lies over the client's content.
    pub fn titlebar_reserved(&self, style: TitlebarStyle) -> f64 {
        match style {
            TitlebarStyle::Standard => self.titlebar_height_standard,
            TitlebarStyle::Unified => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_size_classes_follow_the_control_table() {
        assert_eq!(DecorationMetrics::default().button_diameter, 12.0);
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
        assert_eq!(m.titlebar_reserved(TitlebarStyle::Standard), 28.0);
        assert_eq!(m.titlebar_reserved(TitlebarStyle::Unified), 0.0);
    }
}
