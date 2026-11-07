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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 x 2 picture whose pixels are numbered row by row, as the tests turn it.
    fn picture() -> Vec<Vec<u32>> {
        vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7]]
    }

    /// `upright` (rows of pixels) turned by `t`, built from [`OutputTransform::upright_pixel`] alone.
    fn turned(upright: &[Vec<u32>], t: OutputTransform) -> Vec<Vec<u32>> {
        let (w, h) = (upright[0].len() as u32, upright.len() as u32);
        let (bw, bh) = t.turn_pixels((w, h));
        (0..bh)
            .map(|y| {
                (0..bw)
                    .map(|x| {
                        let (ux, uy) = t.upright_pixel((x, y), (bw, bh));
                        upright[uy as usize][ux as usize]
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn each_transform_turns_and_mirrors_the_picture_as_wl_output_names_it() {
        let p = picture();
        let expect = |t, rows: &[&[u32]]| {
            let rows: Vec<Vec<u32>> = rows.iter().map(|r| r.to_vec()).collect();
            assert_eq!(turned(&p, t), rows, "{t:?}");
        };
        expect(OutputTransform::Normal, &[&[0, 1, 2, 3], &[4, 5, 6, 7]]);
        // a quarter counter-clockwise: the right column becomes the top row
        expect(
            OutputTransform::Rotated90,
            &[&[3, 7], &[2, 6], &[1, 5], &[0, 4]],
        );
        expect(OutputTransform::Rotated180, &[&[7, 6, 5, 4], &[3, 2, 1, 0]]);
        expect(
            OutputTransform::Rotated270,
            &[&[4, 0], &[5, 1], &[6, 2], &[7, 3]],
        );
        expect(OutputTransform::Flipped, &[&[3, 2, 1, 0], &[7, 6, 5, 4]]);
        // mirrored first, then turned: the left column becomes the top row
        expect(
            OutputTransform::Flipped90,
            &[&[0, 4], &[1, 5], &[2, 6], &[3, 7]],
        );
        expect(OutputTransform::Flipped180, &[&[4, 5, 6, 7], &[0, 1, 2, 3]]);
        expect(
            OutputTransform::Flipped270,
            &[&[7, 3], &[6, 2], &[5, 1], &[4, 0]],
        );
    }

    #[test]
    fn the_inverse_turns_a_buffer_back_upright() {
        let size = Size::new(40.0, 30.0);
        let p = Point::new(7.0, 3.0);
        for t in OutputTransform::ALL {
            let b = t.buffer_point(p, size);
            assert_eq!(t.upright_point(b, t.turn_size(size)), p, "{t:?}");
            assert_eq!(t.inverse().inverse(), t);
            assert_eq!(
                turned(&turned(&picture(), t), t.inverse()),
                picture(),
                "{t:?}"
            );
        }
        assert_eq!(
            OutputTransform::from_wayland(1),
            Some(OutputTransform::Rotated90)
        );
        assert_eq!(
            OutputTransform::from_wayland(7),
            Some(OutputTransform::Flipped270)
        );
        assert_eq!(OutputTransform::from_wayland(8), None);
    }

    /// Cut `area` of a 4 x 2 output out of its buffer turned by `t` at `scale` pixels per point, and turn the cut
    /// upright again: the upright pixels of the area.
    fn cut(area: Rect, t: OutputTransform, scale: u32) -> Vec<Vec<u32>> {
        let upright: Vec<Vec<u32>> = (0..2 * scale)
            .map(|y| {
                (0..4 * scale)
                    .map(|x| (y / scale) * 4 + x / scale)
                    .collect()
            })
            .collect();
        let buffer = turned(&upright, t);
        let size = (buffer[0].len() as u32, buffer.len() as u32);
        let output = Rect::new(100.0, 50.0, 4.0, 2.0);
        let a = area_in_buffer(&area, &output, size, t).expect("inside the output");
        let piece: Vec<Vec<u32>> = (a.y..a.y + a.height)
            .map(|y| {
                (a.x..a.x + a.width)
                    .map(|x| buffer[y as usize][x as usize])
                    .collect()
            })
            .collect();
        turned(&piece, t.inverse())
    }

    #[test]
    fn an_area_cut_from_a_turned_buffer_and_turned_back_is_the_upright_area_for_every_transform() {
        // the middle two pixels of the top row and the one under the second
        let area = Rect::new(101.0, 50.0, 2.0, 1.0);
        let column = Rect::new(101.0, 50.0, 1.0, 2.0);
        for t in OutputTransform::ALL {
            assert_eq!(cut(area, t, 1), vec![vec![1, 2]], "{t:?}");
            assert_eq!(cut(column, t, 1), vec![vec![1], vec![5]], "{t:?}");
            // at two pixels per point each point is a 2 x 2 block
            assert_eq!(
                cut(area, t, 2),
                vec![vec![1, 1, 2, 2], vec![1, 1, 2, 2]],
                "{t:?}"
            );
        }
    }

    #[test]
    fn a_quarter_turned_buffer_has_the_output_on_its_side() {
        // a 1920 x 1080 mode turned a quarter: the output is 1080 x 1920 points at scale 1, 540 x 960 at scale 2
        let output = Rect::new(1000.0, 0.0, 540.0, 960.0);
        let area = Rect::new(1000.0, 0.0, 100.0, 10.0);
        let a = area_in_buffer(&area, &output, (1920, 1080), OutputTransform::Rotated90).unwrap();
        // the top-left corner of the picture is the bottom-left of the buffer; 200 x 20 pixels on their side
        assert_eq!(
            a,
            PixelArea {
                x: 0,
                y: 880,
                width: 20,
                height: 200
            }
        );
        let a = area_in_buffer(&area, &output, (1920, 1080), OutputTransform::Rotated270).unwrap();
        assert_eq!(
            a,
            PixelArea {
                x: 1900,
                y: 0,
                width: 20,
                height: 200
            }
        );
    }

    #[test]
    fn fractional_and_mixed_scales_round_each_edge_and_stay_inside_the_buffer() {
        // 1.5 pixels per point: 2560 x 1440 for 1706.67 x 960 points is not whole; a 1 x 1 pixel output beside it
        let output = Rect::new(0.0, 0.0, 2560.0 / 1.5, 960.0);
        let area = Rect::new(10.0, 10.0, 101.0, 33.0);
        let a = area_in_buffer(&area, &output, (2560, 1440), OutputTransform::Normal).unwrap();
        assert_eq!(
            (a.x, a.y, a.width, a.height),
            (15, 15, 152, 50),
            "edges at 15, 166.5 → 167, 15 and 64.5 → 65"
        );
        // past the output's edge the area is cut to the buffer, on a quarter-turned output too
        let past = Rect::new(1600.0, 900.0, 500.0, 500.0);
        for t in OutputTransform::ALL {
            let size = t.turn_pixels((2560, 1440));
            let a = area_in_buffer(&past, &output, size, t).unwrap();
            assert!(a.x + a.width <= size.0 && a.y + a.height <= size.1, "{t:?}");
            assert_eq!(
                (a.width * a.height),
                160 * 90,
                "{t:?}: 2560 − 2400 by 1440 − 1350"
            );
        }
        assert_eq!(
            area_in_buffer(
                &Rect::new(5000.0, 0.0, 10.0, 10.0),
                &output,
                (2560, 1440),
                OutputTransform::Normal
            ),
            None,
            "an area beside the output covers none of its pixels"
        );
    }
}
