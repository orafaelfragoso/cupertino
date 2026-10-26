//! Conversion of `cupertino-geometry` shapes to `tiny-skia` paths.

use cupertino_geometry::{CornerStyle, Path, PathSeg, Radii, Rect, RoundedRect};

pub fn to_skia_path(path: &Path, scale: f64) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    let f = |v: f64| (v * scale) as f32;
    for seg in &path.segments {
        match *seg {
            PathSeg::MoveTo(p) => pb.move_to(f(p.x), f(p.y)),
            PathSeg::LineTo(p) => pb.line_to(f(p.x), f(p.y)),
            PathSeg::CubicTo(a, b, c) => pb.cubic_to(f(a.x), f(a.y), f(b.x), f(b.y), f(c.x), f(c.y)),
            PathSeg::Close => pb.close(),
        }
    }
    pb.finish()
}

/// Closed outline of a rounded rectangle (rect and radii in logical units) at `scale` device
/// pixels per unit.
pub fn rounded_path(rect: Rect, radii: Radii, style: CornerStyle, scale: f64) -> Option<tiny_skia::Path> {
    to_skia_path(&RoundedRect::with_radii(rect, radii, style).path(), scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_bounds_follow_the_scale() {
        let p = rounded_path(Rect::new(0.0, 0.0, 100.0, 50.0), Radii::uniform(10.0), CornerStyle::Circular, 2.0).unwrap();
        let b = p.bounds();
        assert!((b.width() - 200.0).abs() < 1e-3 && (b.height() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn square_radii_give_a_plain_rectangle() {
        let p = rounded_path(Rect::new(5.0, 5.0, 10.0, 10.0), Radii::uniform(0.0), CornerStyle::continuous(), 1.0).unwrap();
        assert_eq!(p.bounds().left(), 5.0);
        assert_eq!(p.bounds().right(), 15.0);
    }
}
