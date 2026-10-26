//! Scale minimize: the window shrinks straight into its slot.
//!
//! The duration (200 ms) is as designed. The easing and the straight-line path
//! are approximate: the cosine ease is used because every other minimize phase uses it.

use crate::ease::Ease;
use crate::mesh::Mesh;
use cupertino_geometry::{Point, Rect, Size};

pub fn lerp_rect(a: Rect, b: Rect, t: f64) -> Rect {
    Rect::from_origin_size(
        Point::new(
            a.origin.x + (b.origin.x - a.origin.x) * t,
            a.origin.y + (b.origin.y - a.origin.y) * t,
        ),
        Size::new(
            a.size.width + (b.size.width - a.size.width) * t,
            a.size.height + (b.size.height - a.size.height) * t,
        ),
    )
}

/// Rectangle of the scaled window at progress `p` (eased with `ease`).
pub fn scale_rect(window: Rect, slot: Rect, p: f64, ease: Ease) -> Rect {
    lerp_rect(window, slot, ease.apply(p))
}

pub fn scale_mesh(window: Rect, slot: Rect, p: f64, ease: Ease) -> Mesh {
    Mesh::quad(scale_rect(window, slot, p, ease))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_and_monotonic_progress() {
        let (w, s) = (
            Rect::new(100.0, 50.0, 800.0, 600.0),
            Rect::new(900.0, 700.0, 64.0, 64.0),
        );
        assert_eq!(scale_rect(w, s, 0.0, Ease::Cosine), w);
        assert_eq!(scale_rect(w, s, 1.0, Ease::Cosine), s);
        let mut prev = f64::INFINITY;
        for k in 0..=100 {
            let r = scale_rect(w, s, k as f64 / 100.0, Ease::Cosine);
            assert!(r.width() <= prev + 1e-9);
            prev = r.width();
        }
        assert_eq!(scale_mesh(w, s, 0.5, Ease::Linear).vertex_count(), 4);
    }
}
