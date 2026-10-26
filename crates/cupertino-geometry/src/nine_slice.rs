//! Nine-slice (nine-patch) scaling math.
//!
//! A source image of size `source` is cut by `insets` (in source texels) into nine regions. When
//! drawn into a destination rectangle the four corners keep their size (multiplied by `scale`),
//! the edges stretch along one axis and the centre stretches along both. Three-part and six-part
//! assets are the special cases where two insets are zero.

use crate::types::{Insets, Rect, Size};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slice {
    /// Region in source texels.
    pub src: Rect,
    /// Region in destination units.
    pub dst: Rect,
}

impl Slice {
    pub fn is_empty(&self) -> bool {
        self.src.is_empty() || self.dst.is_empty()
    }

    /// Normalised texture coordinates `[u0, v0, u1, v1]` for a texture of size `texture`.
    pub fn uv(&self, texture: Size) -> [f64; 4] {
        [
            self.src.min_x() / texture.width,
            self.src.min_y() / texture.height,
            self.src.max_x() / texture.width,
            self.src.max_y() / texture.height,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NineSlice {
    pub source: Size,
    pub insets: Insets,
}

impl NineSlice {
    pub fn new(source: Size, insets: Insets) -> Self {
        Self { source, insets }
    }

    /// Insets clamped so opposite insets never overlap in the source image.
    fn source_insets(&self) -> Insets {
        let (mut l, mut r) = (self.insets.left.max(0.0), self.insets.right.max(0.0));
        let (mut t, mut b) = (self.insets.top.max(0.0), self.insets.bottom.max(0.0));
        if l + r > self.source.width && l + r > 0.0 {
            let f = self.source.width / (l + r);
            l *= f;
            r *= f;
        }
        if t + b > self.source.height && t + b > 0.0 {
            let f = self.source.height / (t + b);
            t *= f;
            b *= f;
        }
        Insets::new(t, l, b, r)
    }

    /// Smallest destination size for which the centre does not collapse.
    pub fn min_size(&self, scale: f64) -> Size {
        let i = self.source_insets();
        Size::new(i.horizontal() * scale, i.vertical() * scale)
    }

    /// The nine slices in row-major order (top-left, top, top-right, left, centre, right,
    /// bottom-left, bottom, bottom-right). `scale` is destination units per source texel. When the
    /// destination is smaller than the scaled corners, opposite insets shrink proportionally.
    pub fn slices(&self, dest: Rect, scale: f64) -> [Slice; 9] {
        let si = self.source_insets();
        let mut d = si.scale(scale);
        if d.horizontal() > dest.width() && d.horizontal() > 0.0 {
            let f = dest.width().max(0.0) / d.horizontal();
            d.left *= f;
            d.right *= f;
        }
        if d.vertical() > dest.height() && d.vertical() > 0.0 {
            let f = dest.height().max(0.0) / d.vertical();
            d.top *= f;
            d.bottom *= f;
        }
        let sx = [0.0, si.left, self.source.width - si.right, self.source.width];
        let sy = [0.0, si.top, self.source.height - si.bottom, self.source.height];
        let dx = [dest.min_x(), dest.min_x() + d.left, dest.max_x() - d.right, dest.max_x()];
        let dy = [dest.min_y(), dest.min_y() + d.top, dest.max_y() - d.bottom, dest.max_y()];
        std::array::from_fn(|i| {
            let (row, col) = (i / 3, i % 3);
            Slice {
                src: Rect::from_ltrb(sx[col], sy[row], sx[col + 1], sy[row + 1]),
                dst: Rect::from_ltrb(dx[col], dy[row], dx[col + 1], dy[row + 1]),
            }
        })
    }

    /// Non-empty slices only.
    pub fn visible_slices(&self, dest: Rect, scale: f64) -> Vec<Slice> {
        self.slices(dest, scale).into_iter().filter(|s| !s.is_empty()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns() -> NineSlice {
        NineSlice::new(Size::new(27.0, 27.0), Insets::uniform(12.0))
    }

    #[test]
    fn sources_tile_the_image() {
        let s = ns().slices(Rect::new(0.0, 0.0, 200.0, 100.0), 1.0);
        let area: f64 = s.iter().map(|x| x.src.area()).sum();
        assert!((area - 27.0 * 27.0).abs() < 1e-9);
        assert_eq!(s[4].src, Rect::new(12.0, 12.0, 3.0, 3.0));
    }

    #[test]
    fn destinations_tile_the_rect() {
        let dest = Rect::new(5.0, 7.0, 200.0, 100.0);
        let s = ns().slices(dest, 1.0);
        let area: f64 = s.iter().map(|x| x.dst.area()).sum();
        assert!((area - dest.area()).abs() < 1e-9);
        assert_eq!(s[0].dst, Rect::new(5.0, 7.0, 12.0, 12.0));
        assert_eq!(s[8].dst, Rect::new(193.0, 95.0, 12.0, 12.0));
        assert_eq!(s[4].dst, Rect::new(17.0, 19.0, 176.0, 76.0));
    }

    #[test]
    fn corners_keep_scaled_size() {
        let s = ns().slices(Rect::new(0.0, 0.0, 300.0, 300.0), 2.0);
        assert_eq!(s[0].dst.size, Size::new(24.0, 24.0));
        assert_eq!(s[2].dst.size, Size::new(24.0, 24.0));
    }

    #[test]
    fn small_destination_shrinks_corners_proportionally() {
        let s = ns().slices(Rect::new(0.0, 0.0, 12.0, 100.0), 1.0);
        assert!((s[0].dst.width() - 6.0).abs() < 1e-9 && (s[2].dst.width() - 6.0).abs() < 1e-9);
        assert_eq!(s[1].dst.width(), 0.0);
        let area: f64 = s.iter().map(|x| x.dst.area()).sum();
        assert!((area - 1200.0).abs() < 1e-9);
    }

    #[test]
    fn three_part_has_six_empty_slices() {
        let n = NineSlice::new(Size::new(38.0, 32.0), Insets::symmetric(16.0, 0.0));
        let v = n.visible_slices(Rect::new(0.0, 0.0, 100.0, 32.0), 1.0);
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].dst, Rect::new(16.0, 0.0, 68.0, 32.0));
    }

    #[test]
    fn uv_is_normalised() {
        let s = ns().slices(Rect::new(0.0, 0.0, 100.0, 100.0), 1.0);
        let uv = s[4].uv(Size::new(27.0, 27.0));
        assert!((uv[0] - 12.0 / 27.0).abs() < 1e-12 && (uv[2] - 15.0 / 27.0).abs() < 1e-12);
    }

    #[test]
    fn oversized_insets_are_clamped() {
        let n = NineSlice::new(Size::new(10.0, 10.0), Insets::uniform(20.0));
        let s = n.slices(Rect::new(0.0, 0.0, 100.0, 100.0), 1.0);
        let area: f64 = s.iter().map(|x| x.src.area()).sum();
        assert!((area - 100.0).abs() < 1e-9);
        assert_eq!(n.min_size(1.0), Size::new(10.0, 10.0));
    }
}
