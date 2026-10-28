//! What changed since the last frame drawn, in points.

use cupertino_geometry::Rect;

/// Rectangles past which one bounding rectangle is cheaper than keeping them apart.
const MAX_RECTS: usize = 8;

/// The area of a surface to redraw: collected from the changes between frames, taken by the next frame. A new `Damage`
/// is the whole surface, as is the one after [`Damage::add_whole`].
#[derive(Clone, Debug, PartialEq)]
pub struct Damage {
    whole: bool,
    rects: Vec<Rect>,
}

impl Default for Damage {
    fn default() -> Self {
        Damage {
            whole: true,
            rects: Vec::new(),
        }
    }
}

impl Damage {
    pub fn new() -> Damage {
        Damage::default()
    }

    /// Redraw all of the surface.
    pub fn add_whole(&mut self) {
        self.whole = true;
        self.rects.clear();
    }

    /// Redraw `rect` too. Overlapping rectangles merge into their bounds.
    pub fn add(&mut self, rect: Rect) {
        if self.whole || rect.is_empty() {
            return;
        }
        let mut merged = rect;
        while let Some(i) = self.rects.iter().position(|r| r.intersects(&merged)) {
            merged = merged.union(&self.rects.swap_remove(i));
        }
        self.rects.push(merged);
        if self.rects.len() > MAX_RECTS {
            let bounds = self.rects.drain(..).reduce(|a, b| a.union(&b));
            self.rects.extend(bounds);
        }
    }

    /// Whether a frame has anything to draw.
    pub fn is_pending(&self) -> bool {
        self.whole || !self.rects.is_empty()
    }

    /// The rectangles the next frame redraws, `None` for all of the surface, and start collecting for the frame after.
    /// Rectangles come ordered by position so that equal damage compares equal.
    pub fn take(&mut self) -> Option<Vec<Rect>> {
        let whole = std::mem::replace(&mut self.whole, false);
        let mut rects = std::mem::take(&mut self.rects);
        if whole {
            return None;
        }
        rects.sort_by(|a, b| {
            (a.min_y(), a.min_x())
                .partial_cmp(&(b.min_y(), b.min_x()))
                .unwrap()
        });
        Some(rects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn a_new_damage_is_the_whole_surface_once() {
        let mut d = Damage::new();
        d.add(r(0.0, 0.0, 10.0, 10.0));
        assert_eq!(d.take(), None);
        assert!(!d.is_pending());
        assert_eq!(d.take(), Some(vec![]));
    }

    #[test]
    fn separate_rectangles_stay_apart_and_overlapping_ones_merge() {
        let mut d = Damage::new();
        d.take();
        d.add(r(100.0, 0.0, 10.0, 10.0));
        d.add(r(0.0, 0.0, 10.0, 10.0));
        d.add(r(5.0, 5.0, 10.0, 10.0));
        assert!(d.is_pending());
        assert_eq!(
            d.take(),
            Some(vec![r(0.0, 0.0, 15.0, 15.0), r(100.0, 0.0, 10.0, 10.0)])
        );
    }

    #[test]
    fn a_bridging_rectangle_merges_all_it_touches() {
        let mut d = Damage::new();
        d.take();
        d.add(r(0.0, 0.0, 10.0, 10.0));
        d.add(r(20.0, 0.0, 10.0, 10.0));
        d.add(r(5.0, 0.0, 20.0, 10.0));
        assert_eq!(d.take(), Some(vec![r(0.0, 0.0, 30.0, 10.0)]));
    }

    #[test]
    fn more_than_eight_rectangles_become_their_bounds() {
        let mut d = Damage::new();
        d.take();
        for i in 0..9 {
            d.add(r(f64::from(i) * 20.0, 0.0, 10.0, 10.0));
        }
        assert_eq!(d.take(), Some(vec![r(0.0, 0.0, 170.0, 10.0)]));
    }

    #[test]
    fn empty_rectangles_add_nothing_and_the_whole_surface_absorbs_the_rest() {
        let mut d = Damage::new();
        d.take();
        d.add(r(0.0, 0.0, 0.0, 10.0));
        assert!(!d.is_pending());
        d.add(r(0.0, 0.0, 10.0, 10.0));
        d.add_whole();
        d.add(r(0.0, 0.0, 10.0, 10.0));
        assert_eq!(d.take(), None);
    }
}
