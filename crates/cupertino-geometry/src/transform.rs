//! Turned and flipped outputs: where an upright picture of an output lands in a buffer that holds it turned, and which
//! buffer pixels a logical rectangle of the output covers.
//!
//! [`OutputTransform`] has the meaning of `wl_output.transform`: it is what was applied to the upright picture to make
//! the buffer. [`OutputTransform::Rotated90`] holds the picture turned a quarter counter-clockwise; the flipped
//! transforms mirror the picture left to right first and then turn it. Turning a buffer of transform `t` upright is
//! applying [`OutputTransform::inverse`].
//!
//! Points are continuous coordinates, `(0, 0)` the top-left corner of the top-left pixel; a pixel `(x, y)` covers
//! `x..x + 1` by `y..y + 1`.

use crate::{Point, Rect, Size};

/// How an output's picture is turned and flipped in a buffer (`wl_output.transform`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutputTransform {
    #[default]
    Normal,
    /// A quarter turn counter-clockwise.
    Rotated90,
    Rotated180,
    /// Three quarters counter-clockwise, a quarter clockwise.
    Rotated270,
    /// Mirrored left to right.
    Flipped,
    /// Mirrored, then a quarter turn counter-clockwise.
    Flipped90,
    Flipped180,
    Flipped270,
}

/// A rectangle of whole pixels of a buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PixelArea {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl OutputTransform {
    /// Every transform, in `wl_output.transform` order.
    pub const ALL: [OutputTransform; 8] = [
        OutputTransform::Normal,
        OutputTransform::Rotated90,
        OutputTransform::Rotated180,
        OutputTransform::Rotated270,
        OutputTransform::Flipped,
        OutputTransform::Flipped90,
        OutputTransform::Flipped180,
        OutputTransform::Flipped270,
    ];

    /// The transform of `wl_output.transform` value `value` (0 to 7).
    pub fn from_wayland(value: u32) -> Option<OutputTransform> {
        Self::ALL.get(value as usize).copied()
    }

    /// Whether the picture's width runs along the buffer's height.
    pub fn swaps_sides(self) -> bool {
        matches!(
            self,
            Self::Rotated90 | Self::Rotated270 | Self::Flipped90 | Self::Flipped270
        )
    }

    /// The transform that undoes this one: a quarter turn the other way; a flip, or a half turn, undoes itself.
    pub fn inverse(self) -> OutputTransform {
        match self {
            Self::Rotated90 => Self::Rotated270,
            Self::Rotated270 => Self::Rotated90,
            other => other,
        }
    }

    /// The size of the buffer that holds a picture of `size` turned by this transform; also the upright size of a
    /// buffer of `size`, since a transform and its inverse swap the same sides.
    pub fn turn_size(self, size: Size) -> Size {
        if self.swaps_sides() {
            Size::new(size.height, size.width)
        } else {
            size
        }
    }

    /// [`OutputTransform::turn_size`] in whole pixels.
    pub fn turn_pixels(self, (width, height): (u32, u32)) -> (u32, u32) {
        if self.swaps_sides() {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// Where the point `p` of an upright picture of `upright` size lands in the buffer that holds it turned.
    pub fn buffer_point(self, p: Point, upright: Size) -> Point {
        let (w, h) = (upright.width, upright.height);
        let (x, y) = (p.x, p.y);
        let (bx, by) = match self {
            Self::Normal => (x, y),
            Self::Rotated90 => (y, w - x),
            Self::Rotated180 => (w - x, h - y),
            Self::Rotated270 => (h - y, x),
            Self::Flipped => (w - x, y),
            Self::Flipped90 => (y, x),
            Self::Flipped180 => (x, h - y),
            Self::Flipped270 => (h - y, w - x),
        };
        Point::new(bx, by)
    }

    /// Where the point `p` of a buffer of `buffer` size, holding a picture turned by this transform, is in the
    /// upright picture.
    pub fn upright_point(self, p: Point, buffer: Size) -> Point {
        self.inverse().buffer_point(p, buffer)
    }

    /// The rectangle `r` of an upright picture of `upright` size in the buffer that holds it turned.
    pub fn buffer_rect(self, r: &Rect, upright: Size) -> Rect {
        corners(r, |p| self.buffer_point(p, upright))
    }

    /// The rectangle `r` of a buffer of `buffer` size in the upright picture.
    pub fn upright_rect(self, r: &Rect, buffer: Size) -> Rect {
        corners(r, |p| self.upright_point(p, buffer))
    }

    /// The pixel of the upright picture that pixel `(x, y)` of a `buffer`-sized buffer shows; for turning a buffer
    /// upright one pixel at a time.
    pub fn upright_pixel(self, (x, y): (u32, u32), buffer: (u32, u32)) -> (u32, u32) {
        let size = Size::new(f64::from(buffer.0), f64::from(buffer.1));
        let centre = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
        let p = self.upright_point(centre, size);
        (p.x.floor() as u32, p.y.floor() as u32)
    }
}

/// The rectangle spanned by `r`'s corners mapped by `map`.
fn corners(r: &Rect, map: impl Fn(Point) -> Point) -> Rect {
    let a = map(Point::new(r.min_x(), r.min_y()));
    let b = map(Point::new(r.max_x(), r.max_y()));
    Rect::from_ltrb(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

/// The pixels of a buffer of `buffer` pixels that a rectangle `area` of an output covers. `area` and `output` are in
/// the same logical space (an output's place in the desktop, say); the buffer holds the output's picture turned by
/// `transform`, at as many pixels per point as its upright size has over `output`'s size, which may differ between
/// the axes and need not be whole. The upright pixel edges are rounded to the nearest whole pixel, so neighbouring
/// areas share their edge, and the result is cut to the buffer. `None` when nothing of the buffer is left.
///
/// Turning the cut-out pixels by `transform.inverse()` gives the upright picture of `area`.
pub fn area_in_buffer(
    area: &Rect,
    output: &Rect,
    buffer: (u32, u32),
    transform: OutputTransform,
) -> Option<PixelArea> {
    if output.width() <= 0.0 || output.height() <= 0.0 {
        return None;
    }
    let (uw, uh) = transform.turn_pixels(buffer);
    let (uw, uh) = (f64::from(uw), f64::from(uh));
    let (sx, sy) = (uw / output.width(), uh / output.height());
    let px = |v: f64, s: f64, max: f64| (v * s).round().clamp(0.0, max);
    let upright = Rect::from_ltrb(
        px(area.min_x() - output.min_x(), sx, uw),
        px(area.min_y() - output.min_y(), sy, uh),
        px(area.max_x() - output.min_x(), sx, uw),
        px(area.max_y() - output.min_y(), sy, uh),
    );
    if upright.width() < 1.0 || upright.height() < 1.0 {
        return None;
    }
    let r = transform.buffer_rect(&upright, Size::new(uw, uh));
    Some(PixelArea {
        x: r.min_x() as u32,
        y: r.min_y() as u32,
        width: r.width() as u32,
        height: r.height() as u32,
    })
}
