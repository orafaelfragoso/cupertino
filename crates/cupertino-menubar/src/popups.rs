//! Placement of the popups that show the open menu levels.
//!
//! The controller lays every open level out in the coordinates of the bar surface, which sits at
//! the top-left corner of its output. Each level is one popup surface; a level's popup hangs off
//! the previous level's (the first off the bar) and is positioned relative to its parent.

use crate::geom::{Point, Rect};

/// The smallest rectangle on whole points that contains `rect`: popup surfaces cover whole points.
pub fn snap(rect: Rect) -> Rect {
    let (x, y) = (rect.x.floor(), rect.y.floor());
    Rect::new(x, y, rect.right().ceil() - x, rect.bottom().ceil() - y)
}

/// Position of each level's popup relative to its parent: the first against the bar surface's
/// origin, the others against the level before. `levels` are bar-surface rectangles.
pub fn relative_to_parents(levels: &[Rect]) -> Vec<Rect> {
    let mut parent = Point::new(0.0, 0.0);
    levels
        .iter()
        .map(|r| {
            let relative = r.translate(-parent.x, -parent.y);
            parent = Point::new(r.x, r.y);
            relative
        })
        .collect()
}
