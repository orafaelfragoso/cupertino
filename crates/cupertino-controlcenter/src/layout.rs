//! Geometry of the panel in points.
//!
//! The grid is two columns of 134 points, with 10 points between rows and columns, inside 10 points of padding:
//! `298 - 2 * 10 = 278 = 2 * 134 + 10`. The connectivity block (134 high) is the left column; the right column
//! holds the focus mode tile (62 high) over the stage and screen mirroring tiles (62 square: `(134 - 10) / 2 = 62`);
//! three rows of 62 follow. The integer fit is why the column gap is taken as 10.

use cupertino_geometry::Size;

/// Width of the card (`_idealWidth` 298; the per-language extra width is not applied, English is +0).
pub const PANEL_WIDTH: f64 = 298.0;
/// Padding of the card and spacing of the grid (10; the horizontal spacing is ).
pub const PADDING: f64 = 10.0;
pub const SPACING: f64 = 10.0;
/// Width of a grid column: `(298 - 2 * 10 - 10) / 2`.
pub const COLUMN: f64 = (PANEL_WIDTH - 2.0 * PADDING - SPACING) / 2.0;
/// Height of a compact tile.
pub const COMPACT_HEIGHT: f64 = 62.0;
/// Height of a switch row and spacing of the connectivity rows: `3 * 40 + 2 * 7 = 134`.
pub const ROW_HEIGHT: f64 = 40.0;
pub const ROW_SPACING: f64 = 7.0;
/// Height of the optional first row, used for the battery (role).
pub const FIRST_ROW_HEIGHT: f64 = 28.0;
/// Corner radius of the card (16, continuous).
pub const CARD_RADIUS: f64 = 16.0;
/// Distance of a slider tile's title from its top edge, of its track from the left and right edges, and from the
/// bottom edge (approximate).
pub const SLIDER_TOP_INSET: f64 = 8.0;
pub const SLIDER_SIDE_INSET: f64 = 12.0;
pub const SLIDER_BOTTOM_INSET: f64 = 10.0;
/// Space between the battery glyph and its percentage (HStack spacing 4).
pub const BATTERY_GAP: f64 = 4.0;
/// Distance of the card from the right edge of the output and below the menu bar (derived from the clamp margin of
/// 6).
pub const EDGE_MARGIN: i32 = 6;
/// The grid rows below the optional battery row: connectivity (two rows), display, sound and now playing.
pub const GRID_ROWS: usize = 5;

/// Size of the card; the battery row is the optional first row.
pub fn card_size(has_battery: bool) -> Size {
    let battery = if has_battery {
        FIRST_ROW_HEIGHT + SPACING
    } else {
        0.0
    };
    let rows = GRID_ROWS as f64;
    Size::new(
        PANEL_WIDTH,
        2.0 * PADDING + battery + rows * COMPACT_HEIGHT + (rows - 1.0) * SPACING,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_is_298_wide_and_a_battery_adds_a_28_point_row() {
        assert_eq!(card_size(false), Size::new(298.0, 370.0));
        assert_eq!(card_size(true), Size::new(298.0, 408.0));
        assert_eq!(COLUMN, 134.0);
    }
}
