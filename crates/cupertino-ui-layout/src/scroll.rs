//! Scroll arithmetic. The offset lives in the application's model; velocity and deceleration live with the node.

use crate::{Axis, ScrollState};
use cupertino_geometry::Point;

/// A page scroll moves this share of the viewport, so the last row of the old page stays in view.
const PAGE_FRACTION: f64 = 0.9;

impl ScrollState {
    /// The largest offset along each axis; zero when the content fits.
    pub fn max_offset(&self) -> Point {
        Point::new(
            (self.content.width - self.viewport.width).max(0.0),
            (self.content.height - self.viewport.height).max(0.0),
        )
    }

    /// The same state with the offset moved inside `0..=max_offset`. A non-finite offset becomes zero.
    pub fn clamped(self) -> ScrollState {
        let max = self.max_offset();
        let fit = |value: f64, max: f64| {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                0.0
            }
        };
        ScrollState {
            offset: Point::new(fit(self.offset.x, max.x), fit(self.offset.y, max.y)),
            ..self
        }
    }

    /// How far a Page Up or Page Down key moves along `axis`.
    pub fn page_step(&self, axis: Axis) -> f64 {
        match axis {
            Axis::Horizontal => self.viewport.width * PAGE_FRACTION,
            Axis::Vertical => self.viewport.height * PAGE_FRACTION,
        }
    }

    /// Moves by `delta` and returns the part that did not fit, which the parent scroll view takes (scroll chaining).
    pub fn scrolled_by(self, delta: Point) -> (ScrollState, Point) {
        let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
        let delta = Point::new(finite(delta.x), finite(delta.y));
        let start = self.clamped();
        let moved = ScrollState {
            offset: Point::new(start.offset.x + delta.x, start.offset.y + delta.y),
            ..start
        }
        .clamped();
        let applied = Point::new(
            moved.offset.x - start.offset.x,
            moved.offset.y - start.offset.y,
        );
        (moved, Point::new(delta.x - applied.x, delta.y - applied.y))
    }
}
