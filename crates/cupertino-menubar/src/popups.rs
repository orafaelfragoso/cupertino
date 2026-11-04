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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_covers_the_rect_on_whole_points() {
        assert_eq!(
            snap(Rect::new(10.4, 24.0, 153.2, 88.5)),
            Rect::new(10.0, 24.0, 154.0, 89.0)
        );
        assert_eq!(
            snap(Rect::new(10.0, 24.0, 120.0, 80.0)),
            Rect::new(10.0, 24.0, 120.0, 80.0)
        );
    }

    #[test]
    fn levels_are_relative_to_their_parent() {
        let levels = [
            Rect::new(100.0, 24.0, 150.0, 200.0),
            Rect::new(248.0, 60.0, 120.0, 80.0),
        ];
        assert_eq!(
            relative_to_parents(&levels),
            vec![
                Rect::new(100.0, 24.0, 150.0, 200.0),
                Rect::new(148.0, 36.0, 120.0, 80.0)
            ]
        );
    }
}
