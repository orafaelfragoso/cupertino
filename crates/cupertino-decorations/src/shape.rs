//! Conversion of `cupertino-geometry` shapes to `tiny-skia` paths.

use cupertino_geometry::{CornerStyle, Path, PathSeg, Radii, Rect, RoundedRect};

pub fn to_skia_path(path: &Path, scale: f64) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    let f = |v: f64| (v * scale) as f32;
    for seg in &path.segments {
        match *seg {
            PathSeg::MoveTo(p) => pb.move_to(f(p.x), f(p.y)),
            PathSeg::LineTo(p) => pb.line_to(f(p.x), f(p.y)),
            PathSeg::CubicTo(a, b, c) => {
                pb.cubic_to(f(a.x), f(a.y), f(b.x), f(b.y), f(c.x), f(c.y))
            }
            PathSeg::Close => pb.close(),
        }
    }
    pb.finish()
}

/// Closed outline of a rounded rectangle (rect and radii in logical units) at `scale` device
/// pixels per unit.
pub fn rounded_path(
    rect: Rect,
    radii: Radii,
    style: CornerStyle,
    scale: f64,
) -> Option<tiny_skia::Path> {
    to_skia_path(&RoundedRect::with_radii(rect, radii, style).path(), scale)
}
