//! Geometry of the panel in points, top-left origin, origin at the top-left corner of the card (the part inside the
//! shadow), and the hit test over it.
//!
//! The grid is two columns of 134 points, with 10 points between rows and columns, inside 10 points of padding:
//! `298 - 2 * 10 = 278 = 2 * 134 + 10`. The connectivity block (134 high) is the left column; the right column
//! holds the focus mode tile (62 high) over the stage and screen mirroring tiles (62 square: `(134 - 10) / 2 = 62`);
//! three rows of 62 follow. The integer fit is why the column gap is taken as 10.

use cupertino_geometry::{Insets, Point, Rect, Size};

use crate::model::{Module, Slider};

/// Width of the card (`_idealWidth` 298; the per-language extra width is not applied, English is +0).
pub const PANEL_WIDTH: f64 = 298.0;
/// Padding of the card and spacing of the grid (10; the horizontal spacing is ).
pub const PADDING: f64 = 10.0;
pub const SPACING: f64 = 10.0;
/// Height of a compact tile.
pub const COMPACT_HEIGHT: f64 = 62.0;
/// Height of the connectivity tile: `2 * 62 + 10` (derivation).
pub const CONNECTIVITY_HEIGHT: f64 = 134.0;
/// Height of a connectivity row and spacing between rows: `3 * 40 + 2 * 7 = 134`.
pub const ROW_HEIGHT: f64 = 40.0;
pub const ROW_SPACING: f64 = 7.0;
/// Height of the optional first row, used for the battery (role).
pub const FIRST_ROW_HEIGHT: f64 = 28.0;
/// Corner radius of the card (16, continuous).
pub const CARD_RADIUS: f64 = 16.0;
/// Corner radius of a tile (approximate).
pub const TILE_RADIUS: f64 = 12.0;

/// Slider track height and the width below which the design does not shrink it.
pub const TRACK_HEIGHT: f64 = 22.0;
pub const TRACK_MIN_WIDTH: f64 = 120.0;
/// The knob's hit frame is 40 points square with alignment insets of 9: it reaches 9 points past the track on every
/// side.
pub const KNOB_HIT_OUTSET: f64 = 9.0;
/// Half the track height: the knob's travel starts this far inside the track (caps of 11).
pub const KNOB_INSET: f64 = TRACK_HEIGHT / 2.0;
/// Distance of the slider track from the left and right edges of its tile, and from the bottom edge (approximate).
pub const SLIDER_SIDE_INSET: f64 = 12.0;
pub const SLIDER_BOTTOM_INSET: f64 = 10.0;

/// The room the surface keeps around the card for its shadow: top, left, bottom, right (shadow
/// insets (6, 16, 16, 16)).
pub const SHADOW_INSETS: Insets = Insets::new(6.0, 16.0, 16.0, 16.0);
/// Distance of the card from the right edge of the output and below the menu bar (derived from the clamp margin of
/// 6).
pub const EDGE_MARGIN: i32 = 6;

/// What lies under a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// Not on the card.
    Outside,
    /// On the card but on no control.
    Panel,
    /// A tile or connectivity row that toggles.
    Toggle(Module),
    /// A slider and the position its track gives the point, 0 to 1.
    Slider(Slider, f64),
}

/// The frames of the panel for a machine with or without a battery row.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    size: Size,
    battery: Option<Rect>,
    frames: Vec<(Module, Rect)>,
}

impl Layout {
    /// The layout; the battery row is the optional first row.
    pub fn new(has_battery: bool) -> Self {
        let column = (PANEL_WIDTH - 2.0 * PADDING - SPACING) / 2.0;
        let right_x = PADDING + column + SPACING;
        let mut y = PADDING;
        let battery = has_battery.then(|| {
            let row = Rect::new(PADDING, y, PANEL_WIDTH - 2.0 * PADDING, FIRST_ROW_HEIGHT);
            y += FIRST_ROW_HEIGHT + SPACING;
            row
        });
        let top = y;
        let small = (column - SPACING) / 2.0;
        let mut frames = Vec::with_capacity(Module::ALL.len());
        for (index, module) in [Module::WiFi, Module::Bluetooth, Module::FileSharing].into_iter().enumerate() {
            frames.push((module, Rect::new(PADDING, top + index as f64 * (ROW_HEIGHT + ROW_SPACING), column, ROW_HEIGHT)));
        }
        frames.push((Module::FocusMode, Rect::new(right_x, top, column, COMPACT_HEIGHT)));
        let second = top + COMPACT_HEIGHT + SPACING;
        frames.push((Module::Stage, Rect::new(right_x, second, small, COMPACT_HEIGHT)));
        frames.push((Module::ScreenMirroring, Rect::new(right_x + small + SPACING, second, small, COMPACT_HEIGHT)));
        let mut row_y = top + CONNECTIVITY_HEIGHT + SPACING;
        for module in [Module::Display, Module::Sound, Module::NowPlaying] {
            frames.push((module, Rect::new(PADDING, row_y, PANEL_WIDTH - 2.0 * PADDING, COMPACT_HEIGHT)));
            row_y += COMPACT_HEIGHT + SPACING;
        }
        Layout { size: Size::new(PANEL_WIDTH, row_y - SPACING + PADDING), battery, frames }
    }

    /// Size of the card.
    pub fn size(&self) -> Size {
        self.size
    }

    /// The card.
    pub fn card(&self) -> Rect {
        Rect::from_origin_size(Point::ZERO, self.size)
    }

    /// The optional first row, when there is a battery.
    pub fn battery(&self) -> Option<Rect> {
        self.battery
    }

    /// The frame of a module: a connectivity row, or the tile itself.
    pub fn frame(&self, module: Module) -> Rect {
        self.frames.iter().find(|(m, _)| *m == module).map(|(_, frame)| *frame).expect("every module has a frame")
    }

    /// The compact tiles, which carry a background. The connectivity rows sit on the card itself: three rows of 40 and
    /// two gaps of 7 fill the 134 points exactly, leaving no room for a padded background.
    pub fn tiles(&self) -> Vec<Rect> {
        self.frames.iter().filter(|(m, _)| !matches!(m, Module::WiFi | Module::Bluetooth | Module::FileSharing)).map(|(_, frame)| *frame).collect()
    }

    /// The track of a slider, inside its tile.
    pub fn track(&self, slider: Slider) -> Rect {
        let tile = self.frame(slider.module());
        Rect::new(tile.x() + SLIDER_SIDE_INSET, tile.max_y() - SLIDER_BOTTOM_INSET - TRACK_HEIGHT, tile.width() - 2.0 * SLIDER_SIDE_INSET, TRACK_HEIGHT)
    }

    /// What lies under `point` (card coordinates).
    pub fn hit(&self, point: Point) -> Hit {
        if !self.card().contains(point) {
            return Hit::Outside;
        }
        for slider in [Slider::Display, Slider::Sound] {
            let track = self.track(slider);
            if track.outset(KNOB_HIT_OUTSET).contains(point) {
                return Hit::Slider(slider, fraction_at(&track, point.x));
            }
        }
        match self.frames.iter().find(|(_, frame)| frame.contains(point)) {
            Some((module, _)) => Hit::Toggle(*module),
            None => Hit::Panel,
        }
    }
}

/// The position along `track` for a pointer at `x`, 0 to 1: `(x - minX - 11) / (w - 22) clamped
/// (the mirroring for right-to-left layouts is not applied).
pub fn fraction_at(track: &Rect, x: f64) -> f64 {
    ((x - track.min_x() - KNOB_INSET) / (track.width() - 2.0 * KNOB_INSET)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(layout: &Layout, module: Module) -> (f64, f64, f64, f64) {
        let r = layout.frame(module);
        (r.x(), r.y(), r.width(), r.height())
    }

    #[test]
    fn the_grid_has_literal_frames_without_a_battery() {
        let layout = Layout::new(false);
        assert_eq!(rect(&layout, Module::WiFi), (10.0, 10.0, 134.0, 40.0));
        assert_eq!(rect(&layout, Module::Bluetooth), (10.0, 57.0, 134.0, 40.0));
        assert_eq!(rect(&layout, Module::FileSharing), (10.0, 104.0, 134.0, 40.0));
        assert_eq!(rect(&layout, Module::FocusMode), (154.0, 10.0, 134.0, 62.0));
        assert_eq!(rect(&layout, Module::Stage), (154.0, 82.0, 62.0, 62.0));
        assert_eq!(rect(&layout, Module::ScreenMirroring), (226.0, 82.0, 62.0, 62.0));
        assert_eq!(rect(&layout, Module::Display), (10.0, 154.0, 278.0, 62.0));
        assert_eq!(rect(&layout, Module::Sound), (10.0, 226.0, 278.0, 62.0));
        assert_eq!(rect(&layout, Module::NowPlaying), (10.0, 298.0, 278.0, 62.0));
        assert_eq!(layout.size(), Size::new(298.0, 370.0));
    }

    #[test]
    fn a_battery_adds_a_28_point_first_row() {
        let layout = Layout::new(true);
        let battery = layout.battery().expect("a battery row");
        assert_eq!((battery.x(), battery.y(), battery.width(), battery.height()), (10.0, 10.0, 278.0, 28.0));
        assert_eq!(rect(&layout, Module::WiFi), (10.0, 48.0, 134.0, 40.0));
        assert_eq!(layout.size(), Size::new(298.0, 408.0));
        assert_eq!(Layout::new(false).battery(), None);
    }

    #[test]
    fn the_slider_track_is_22_high_at_the_bottom_of_its_tile() {
        let layout = Layout::new(false);
        let track = layout.track(Slider::Display);
        assert_eq!((track.x(), track.y(), track.width(), track.height()), (22.0, 184.0, 254.0, 22.0));
    }

    #[test]
    fn the_slider_position_runs_between_the_knob_travel_ends() {
        let track = Rect::new(22.0, 184.0, 254.0, 22.0);
        assert_eq!(fraction_at(&track, 0.0), 0.0);
        assert_eq!(fraction_at(&track, 33.0), 0.0);
        assert_eq!(fraction_at(&track, 149.0), 0.5);
        assert_eq!(fraction_at(&track, 265.0), 1.0);
        assert_eq!(fraction_at(&track, 400.0), 1.0);
    }

    #[test]
    fn hit_testing_finds_rows_tiles_sliders_and_the_outside() {
        let layout = Layout::new(false);
        assert_eq!(layout.hit(Point::new(50.0, 30.0)), Hit::Toggle(Module::WiFi));
        assert_eq!(layout.hit(Point::new(50.0, 77.0)), Hit::Toggle(Module::Bluetooth));
        assert_eq!(layout.hit(Point::new(200.0, 40.0)), Hit::Toggle(Module::FocusMode));
        assert_eq!(layout.hit(Point::new(240.0, 100.0)), Hit::Toggle(Module::ScreenMirroring));
        assert_eq!(layout.hit(Point::new(149.0, 193.0)), Hit::Slider(Slider::Display, 0.5));
        assert_eq!(layout.hit(Point::new(149.0, 263.0)), Hit::Slider(Slider::Sound, 0.5));
        // the gap between the connectivity rows, and the title area of the display tile
        assert_eq!(layout.hit(Point::new(50.0, 53.0)), Hit::Panel);
        assert_eq!(layout.hit(Point::new(50.0, 160.0)), Hit::Toggle(Module::Display));
        assert_eq!(layout.hit(Point::new(-1.0, 30.0)), Hit::Outside);
        assert_eq!(layout.hit(Point::new(150.0, 371.0)), Hit::Outside);
    }

    #[test]
    fn the_knob_hit_frame_reaches_nine_points_past_the_track() {
        let layout = Layout::new(false);
        // the track spans x = 22..276, y = 184..206, its hit frame x = 13..285, y = 175..215
        assert_eq!(layout.hit(Point::new(14.0, 193.0)), Hit::Slider(Slider::Display, 0.0));
        assert_eq!(layout.hit(Point::new(100.0, 176.0)), Hit::Slider(Slider::Display, (100.0 - 22.0 - 11.0) / 232.0));
        assert_eq!(layout.hit(Point::new(100.0, 174.0)), Hit::Toggle(Module::Display));
    }
}
