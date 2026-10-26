//! Placement of the popup surfaces that show the open menu levels.
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

/// A change of the popup surfaces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PopupOp {
    /// Create the popup of `level` at `rect` (relative to its parent).
    Create { level: usize, rect: Rect },
    /// Move or resize the popup of `level` to `rect` (relative to its parent).
    Reposition { level: usize, rect: Rect },
    Destroy { level: usize },
}

/// The changes that turn the popups at `current` into popups at `desired` (both bar-surface
/// rectangles by level). Popups are destroyed deepest first, then repositioned and created
/// from the first level down, so a popup never outlives or precedes its parent.
pub fn plan(current: &[Rect], desired: &[Rect]) -> Vec<PopupOp> {
    let (have, want) = (relative_to_parents(current), relative_to_parents(desired));
    let mut ops: Vec<PopupOp> = (want.len()..have.len()).rev().map(|level| PopupOp::Destroy { level }).collect();
    for (level, rect) in want.iter().enumerate() {
        match have.get(level) {
            Some(existing) if existing == rect => {}
            Some(_) => ops.push(PopupOp::Reposition { level, rect: *rect }),
            None => ops.push(PopupOp::Create { level, rect: *rect }),
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_covers_the_rect_on_whole_points() {
        assert_eq!(snap(Rect::new(10.4, 24.0, 153.2, 88.5)), Rect::new(10.0, 24.0, 154.0, 89.0));
        assert_eq!(snap(Rect::new(10.0, 24.0, 120.0, 80.0)), Rect::new(10.0, 24.0, 120.0, 80.0));
    }

    #[test]
    fn levels_are_relative_to_their_parent() {
        let levels = [Rect::new(100.0, 24.0, 150.0, 200.0), Rect::new(248.0, 60.0, 120.0, 80.0)];
        assert_eq!(relative_to_parents(&levels), vec![Rect::new(100.0, 24.0, 150.0, 200.0), Rect::new(148.0, 36.0, 120.0, 80.0)]);
    }

    #[test]
    fn opening_menus_creates_popups_from_the_first_level_down() {
        let desired = [Rect::new(100.0, 24.0, 150.0, 200.0), Rect::new(248.0, 60.0, 120.0, 80.0)];
        assert_eq!(
            plan(&[], &desired),
            vec![PopupOp::Create { level: 0, rect: Rect::new(100.0, 24.0, 150.0, 200.0) }, PopupOp::Create { level: 1, rect: Rect::new(148.0, 36.0, 120.0, 80.0) }]
        );
    }

    #[test]
    fn closing_levels_destroys_the_deepest_first() {
        let current = [Rect::new(100.0, 24.0, 150.0, 200.0), Rect::new(248.0, 60.0, 120.0, 80.0), Rect::new(366.0, 70.0, 90.0, 40.0)];
        assert_eq!(plan(&current, &current[..1]), vec![PopupOp::Destroy { level: 2 }, PopupOp::Destroy { level: 1 }]);
        assert_eq!(plan(&current, &[]), vec![PopupOp::Destroy { level: 2 }, PopupOp::Destroy { level: 1 }, PopupOp::Destroy { level: 0 }]);
    }

    #[test]
    fn unchanged_levels_stay_and_changed_ones_are_repositioned() {
        let current = [Rect::new(100.0, 24.0, 150.0, 200.0), Rect::new(248.0, 60.0, 120.0, 80.0)];
        assert_eq!(plan(&current, &current), vec![]);
        // a reloaded menu got taller; the submenu keeps its place relative to it
        let taller = [Rect::new(100.0, 24.0, 150.0, 260.0), Rect::new(248.0, 60.0, 120.0, 80.0)];
        assert_eq!(plan(&current, &taller), vec![PopupOp::Reposition { level: 0, rect: Rect::new(100.0, 24.0, 150.0, 260.0) }]);
    }

    #[test]
    fn a_moved_parent_with_a_fixed_child_repositions_both() {
        let current = [Rect::new(100.0, 24.0, 150.0, 200.0), Rect::new(248.0, 60.0, 120.0, 80.0)];
        let desired = [Rect::new(80.0, 24.0, 150.0, 200.0), Rect::new(248.0, 60.0, 120.0, 80.0)];
        assert_eq!(
            plan(&current, &desired),
            vec![PopupOp::Reposition { level: 0, rect: Rect::new(80.0, 24.0, 150.0, 200.0) }, PopupOp::Reposition { level: 1, rect: Rect::new(168.0, 36.0, 120.0, 80.0) }]
        );
    }
}
