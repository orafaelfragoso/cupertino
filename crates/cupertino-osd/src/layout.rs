//! Geometry of the overlay in points, top-left origin.
//!
//! The design lists its frames with a bottom-left origin inside the 200 x 200 window; they are converted here.

use cupertino_geometry::Rect;

use crate::model::{SEGMENTS, SEGMENT_WIDTH};

/// Side of the square window (200 x 200).
pub const WINDOW_SIZE: u32 = 200;

/// Corner radius of the window (a 37 x 37 mask with 18 pt cap insets).
pub const CORNER_RADIUS: f64 = 18.0;

/// Distance from the bottom of the output to the window (y = 140, bottom-left origin). The window is
/// centred horizontally.
pub const BOTTOM_MARGIN: i32 = 140;

/// Meter container in the design's bottom-left coordinates: (20, 20), 161 x 8.
const METER_LEFT: f64 = 20.0;
const METER_BOTTOM: f64 = 20.0;
const METER_WIDTH: f64 = 161.0;
const METER_HEIGHT: f64 = 8.0;

/// Segment pitch and size (x = i * 10 + 1, y = 1, 9 x 6 inside the container).
const SEGMENT_PITCH: f64 = 10.0;
const SEGMENT_INSET: f64 = 1.0;
const SEGMENT_HEIGHT: f64 = 6.0;

/// Side of the glyph (approximate: the glyph artwork and size is not known).
pub const GLYPH_SIZE: f64 = 96.0;

/// Top of the glyph (approximate).
pub const GLYPH_TOP: f64 = 28.0;

/// The meter container.
pub fn meter() -> Rect {
    Rect::new(
        METER_LEFT,
        f64::from(WINDOW_SIZE) - METER_BOTTOM - METER_HEIGHT,
        METER_WIDTH,
        METER_HEIGHT,
    )
}

/// Segment `index` (0 is leftmost).
pub fn segment(index: usize) -> Rect {
    assert!(index < SEGMENTS, "the meter has {SEGMENTS} segments");
    let container = meter();
    Rect::new(
        container.x() + SEGMENT_PITCH * index as f64 + SEGMENT_INSET,
        container.y() + SEGMENT_INSET,
        f64::from(SEGMENT_WIDTH),
        SEGMENT_HEIGHT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_container_sits_20_points_above_the_bottom_edge() {
        assert_eq!(meter(), Rect::new(20.0, 172.0, 161.0, 8.0));
    }

    #[test]
    fn segments_have_a_pitch_of_ten_inside_the_container() {
        assert_eq!(segment(0), Rect::new(21.0, 173.0, 9.0, 6.0));
        assert_eq!(segment(1), Rect::new(31.0, 173.0, 9.0, 6.0));
        assert_eq!(segment(15), Rect::new(171.0, 173.0, 9.0, 6.0));
    }

    #[test]
    fn the_last_segment_ends_inside_the_container() {
        let last = segment(15);
        assert!(
            last.max_x() <= meter().max_x(),
            "{} > {}",
            last.max_x(),
            meter().max_x()
        );
    }
}
