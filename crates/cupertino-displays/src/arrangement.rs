//! The arrangement of the displays that are on: where the logical extent of each one lies in the global space.
//!
//! A configuration is checked as a whole before any of it is applied ([`check`]): every display that stays on has an
//! extent, lies in the range the output protocols can carry, and covers no point another display covers (there is no
//! mirroring, so two displays showing the same area would be two views of one place). A display that comes back to a
//! place another display has taken meanwhile goes beside the others instead ([`free_place`]). What was on a display
//! that went away keeps its place relative to the display's top-left corner ([`relocate`]), and a point that no display
//! covers any more is brought onto the nearest one ([`reachable_point`]).

use std::fmt;

/// The largest distance from the origin an edge of a display may lie at, in points: positions travel as 32-bit
/// integers in the output protocols.
pub const COORDINATE_LIMIT: f64 = i32::MAX as f64;

/// An area of the global logical space in points: its top-left corner and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Area {
        Area { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// Whether the two areas share any point that is not on an edge: displays may touch, not overlap.
    pub fn overlaps(&self, other: &Area) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    pub fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The point of the area nearest to `p`, at least one point inside its right and bottom edges, as the pointer
    /// stays.
    fn nearest(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (
            x.clamp(self.x, (self.right() - 1.0).max(self.x)),
            y.clamp(self.y, (self.bottom() - 1.0).max(self.y)),
        )
    }
}

/// Why an arrangement cannot be applied. Indices are positions in the list that was checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangementError {
    /// No display would be on.
    Nothing,
    /// A display would have no area, or one that is not a finite size.
    Empty(usize),
    /// A display would lie beyond [`COORDINATE_LIMIT`].
    OutOfRange(usize),
    /// Two displays would cover the same points.
    Overlap(usize, usize),
}

impl fmt::Display for ArrangementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArrangementError::Nothing => write!(f, "at least one display must stay on"),
            ArrangementError::Empty(i) => write!(f, "display {i} would have no area"),
            ArrangementError::OutOfRange(i) => {
                write!(f, "display {i} would lie beyond the coordinate range")
            }
            ArrangementError::Overlap(a, b) => write!(f, "displays {a} and {b} would overlap"),
        }
    }
}

impl std::error::Error for ArrangementError {}

/// Check the areas of every display that would be on, as a whole: at least one, each with a finite area inside the
/// coordinate range, and no two overlapping. The first problem found is returned.
pub fn check(areas: &[Area]) -> Result<(), ArrangementError> {
    if areas.is_empty() {
        return Err(ArrangementError::Nothing);
    }
    for (i, a) in areas.iter().enumerate() {
        let finite = [a.x, a.y, a.w, a.h].iter().all(|v| v.is_finite());
        if !finite || a.w <= 0.0 || a.h <= 0.0 {
            return Err(ArrangementError::Empty(i));
        }
        let in_range = |v: f64| v.abs() <= COORDINATE_LIMIT;
        if ![a.x, a.y, a.right(), a.bottom()].into_iter().all(in_range) {
            return Err(ArrangementError::OutOfRange(i));
        }
    }
    for (i, a) in areas.iter().enumerate() {
        if let Some(j) = areas[i + 1..].iter().position(|b| a.overlaps(b)) {
            return Err(ArrangementError::Overlap(i, i + 1 + j));
        }
    }
    Ok(())
}

/// Where a display that wants `wanted` goes among `others`: its own place when that overlaps none of them, else right
/// of all of them with its top level with the highest of them. The result never overlaps any of `others`.
pub fn free_place(others: &[Area], wanted: Area) -> (f64, f64) {
    if !others.iter().any(|o| o.overlaps(&wanted)) {
        return (wanted.x, wanted.y);
    }
    let right = others.iter().map(Area::right).fold(f64::MIN, f64::max);
    let top = others.iter().map(|o| o.y).fold(f64::MAX, f64::min);
    (right, top)
}

/// `frame` moved from the display at `from` to the display at `to`, keeping its distance from the top-left corner.
/// Fitting it into the new display's usable area is the caller's.
pub fn relocate(frame: Area, from: Area, to: Area) -> Area {
    Area::new(
        to.x + (frame.x - from.x),
        to.y + (frame.y - from.y),
        frame.w,
        frame.h,
    )
}

/// `p` when a display covers it, else the nearest point of the nearest display; `None` without displays.
pub fn reachable_point(areas: &[Area], p: (f64, f64)) -> Option<(f64, f64)> {
    if areas.iter().any(|a| a.contains(p)) {
        return Some(p);
    }
    let distance = |a: &Area| {
        let (x, y) = a.nearest(p);
        (x - p.0).hypot(y - p.1)
    };
    areas
        .iter()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .map(|a| a.nearest(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Area = Area::new(0.0, 0.0, 1512.0, 982.0);
    const RIGHT: Area = Area::new(1512.0, 0.0, 1920.0, 1080.0);

    #[test]
    fn displays_side_by_side_or_stacked_are_valid_and_overlapping_ones_are_not() {
        assert_eq!(check(&[LEFT, RIGHT]), Ok(()));
        assert_eq!(
            check(&[LEFT, Area::new(0.0, 982.0, 800.0, 600.0)]),
            Ok(()),
            "stacked, touching along an edge"
        );
        assert_eq!(
            check(&[LEFT, RIGHT, Area::new(1400.0, 900.0, 640.0, 480.0)]),
            Err(ArrangementError::Overlap(0, 2))
        );
        assert_eq!(
            check(&[LEFT, LEFT]),
            Err(ArrangementError::Overlap(0, 1)),
            "the same place twice is mirroring, which is not offered"
        );
    }

    #[test]
    fn an_empty_list_an_empty_area_and_an_area_beyond_the_range_are_refused() {
        assert_eq!(check(&[]), Err(ArrangementError::Nothing));
        assert_eq!(
            check(&[LEFT, Area::new(1512.0, 0.0, 0.0, 600.0)]),
            Err(ArrangementError::Empty(1))
        );
        assert_eq!(
            check(&[Area::new(0.0, 0.0, f64::NAN, 600.0)]),
            Err(ArrangementError::Empty(0))
        );
        assert_eq!(
            check(&[Area::new(COORDINATE_LIMIT - 10.0, 0.0, 800.0, 600.0)]),
            Err(ArrangementError::OutOfRange(0))
        );
        assert_eq!(
            check(&[Area::new(-COORDINATE_LIMIT, 0.0, 800.0, 600.0)]),
            Ok(()),
            "the left edge on the limit"
        );
    }

    #[test]
    fn a_display_coming_back_keeps_a_free_place_and_otherwise_goes_right_of_the_others() {
        let back = Area::new(-1280.0, 0.0, 1280.0, 800.0);
        assert_eq!(free_place(&[LEFT, RIGHT], back), (-1280.0, 0.0));
        // its place was taken by a display that moved there meanwhile
        let taken = Area::new(0.0, 0.0, 1280.0, 800.0);
        let (x, y) = free_place(&[LEFT, RIGHT], taken);
        assert_eq!((x, y), (RIGHT.right(), 0.0));
        let placed = Area::new(x, y, taken.w, taken.h);
        assert_eq!(check(&[LEFT, RIGHT, placed]), Ok(()));
        assert_eq!(free_place(&[], taken), (0.0, 0.0));
    }

    #[test]
    fn a_frame_keeps_its_distance_from_the_top_left_corner_of_its_display() {
        let frame = Area::new(1612.0, 100.0, 600.0, 400.0);
        assert_eq!(
            relocate(frame, RIGHT, LEFT),
            Area::new(100.0, 100.0, 600.0, 400.0)
        );
        assert_eq!(relocate(relocate(frame, RIGHT, LEFT), LEFT, RIGHT), frame);
    }

    #[test]
    fn a_point_no_display_covers_comes_onto_the_nearest_display() {
        assert_eq!(reachable_point(&[LEFT], (10.0, 20.0)), Some((10.0, 20.0)));
        // the pointer was on the right display, which went
        assert_eq!(
            reachable_point(&[LEFT], (2000.0, 500.0)),
            Some((1511.0, 500.0))
        );
        assert_eq!(
            reachable_point(&[LEFT], (2000.0, 5000.0)),
            Some((1511.0, 981.0))
        );
        // below the left display, the nearer one, rather than across the gap
        let low = Area::new(3000.0, 2000.0, 800.0, 600.0);
        assert_eq!(
            reachable_point(&[LEFT, low], (100.0, 1100.0)),
            Some((100.0, 981.0))
        );
        assert_eq!(reachable_point(&[], (0.0, 0.0)), None);
    }
}
