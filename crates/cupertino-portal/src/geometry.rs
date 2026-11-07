//! Where a display's pixels are, for screenshots, screen sharing and the colour picker: a copied buffer turned back
//! upright by the transform it carries ([`OutputTransform`], shared with the rest of the desktop), a display's place in
//! the compositor's space when the server reports no logical geometry, and the pixel of a display's upright image
//! under a point of that space.

use cupertino_geometry::{OutputTransform, Point, Size};
use tiny_skia::Pixmap;

/// Where pixel `(x, y)` of an upright image of `size` pixels lands in the buffer that holds it turned by `transform`:
/// the buffer pixel under the upright pixel's centre.
pub fn buffer_pixel(
    transform: OutputTransform,
    (x, y): (u32, u32),
    (width, height): (u32, u32),
) -> (u32, u32) {
    let upright = Size::new(f64::from(width), f64::from(height));
    let centre = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
    let p = transform.buffer_point(centre, upright);
    (p.x.floor() as u32, p.y.floor() as u32)
}

/// The image of `buffer` turned back upright, for a buffer whose content was turned by `transform`.
pub fn upright(buffer: &Pixmap, transform: OutputTransform) -> Option<Pixmap> {
    if transform == OutputTransform::Normal {
        return Some(buffer.clone());
    }
    let size = transform.turn_pixels((buffer.width(), buffer.height()));
    remap(buffer, size, |p| buffer_pixel(transform, p, size))
}

/// `image` turned by `transform`, as a server would hand out a buffer of it: the inverse of [`upright`].
pub fn turned(image: &Pixmap, transform: OutputTransform) -> Option<Pixmap> {
    let size = transform.turn_pixels((image.width(), image.height()));
    let mut out = Pixmap::new(size.0, size.1)?;
    let upright_size = (image.width(), image.height());
    let (src, dst) = (image.data(), out.data_mut());
    for y in 0..upright_size.1 {
        for x in 0..upright_size.0 {
            let (tx, ty) = buffer_pixel(transform, (x, y), upright_size);
            let from = (y * upright_size.0 + x) as usize * 4;
            let to = (ty * size.0 + tx) as usize * 4;
            dst[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
    Some(out)
}

/// An image of `size` whose pixel `(x, y)` is the pixel of `source` at `from((x, y))`.
fn remap(
    source: &Pixmap,
    size: (u32, u32),
    from: impl Fn((u32, u32)) -> (u32, u32),
) -> Option<Pixmap> {
    let mut out = Pixmap::new(size.0, size.1)?;
    let stride = source.width();
    let (src, dst) = (source.data(), out.data_mut());
    for y in 0..size.1 {
        for x in 0..size.0 {
            let (sx, sy) = from((x, y));
            let at = (sy * stride + sx) as usize * 4;
            let to = (y * size.0 + x) as usize * 4;
            dst[to..to + 4].copy_from_slice(&src[at..at + 4]);
        }
    }
    Some(out)
}

/// A display's place in the compositor's logical space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub position: (i32, i32),
    /// Its logical size, upright: a quarter-turned display is taller than wide when its mode is wider than tall.
    pub size: (i32, i32),
}

/// The logical size of a display from its mode in device pixels, its transform and its scale, for a server that
/// reports no logical geometry: the mode upright, divided by the scale.
pub fn logical_size(mode: (u32, u32), transform: OutputTransform, scale: f64) -> (i32, i32) {
    let (width, height) = transform.turn_pixels(mode);
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let side = |px: u32| (f64::from(px) / scale).round() as i32;
    (side(width), side(height))
}

/// The pixel of a display's upright image of `image` pixels under `point` of the compositor's logical space, for a
/// display at `placement`; `None` when the point is outside the display. A display at a scale above 1 has several
/// pixels for each point: the one whose area holds the point.
pub fn pixel_under(
    placement: Placement,
    image: (u32, u32),
    point: (f64, f64),
) -> Option<(u32, u32)> {
    let (left, top) = (
        f64::from(placement.position.0),
        f64::from(placement.position.1),
    );
    let (width, height) = (f64::from(placement.size.0), f64::from(placement.size.1));
    if width <= 0.0 || height <= 0.0 || image.0 == 0 || image.1 == 0 {
        return None;
    }
    let (u, v) = ((point.0 - left) / width, (point.1 - top) / height);
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    let px = |t: f64, n: u32| ((t * f64::from(n)).floor() as u32).min(n - 1);
    Some((px(u, image.0), px(v, image.1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image whose every pixel differs: its red channel is ten times the column and its green channel the row.
    fn marked(width: u32, height: u32) -> Pixmap {
        let mut image = Pixmap::new(width, height).unwrap();
        for (i, px) in image.pixels_mut().iter_mut().enumerate() {
            let (x, y) = (i as u32 % width, i as u32 / width);
            *px = tiny_skia::ColorU8::from_rgba(x as u8 * 10, y as u8 * 10, 0, 255).premultiply();
        }
        image
    }

    fn at(image: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = image.pixel(x, y).unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    #[test]
    fn each_transform_puts_the_top_left_pixel_where_its_wl_output_value_says() {
        // a 3 x 2 image: a quarter turn is counter-clockwise, and a flip mirrors left to right before the turn
        let top_left = [
            (OutputTransform::Normal, (0, 0)),
            (OutputTransform::Rotated90, (0, 2)),
            (OutputTransform::Rotated180, (2, 1)),
            (OutputTransform::Rotated270, (1, 0)),
            (OutputTransform::Flipped, (2, 0)),
            (OutputTransform::Flipped90, (0, 0)),
            (OutputTransform::Flipped180, (0, 1)),
            (OutputTransform::Flipped270, (1, 2)),
        ];
        for (value, (transform, at)) in top_left.into_iter().enumerate() {
            assert_eq!(OutputTransform::from_wayland(value as u32), Some(transform));
            assert_eq!(buffer_pixel(transform, (0, 0), (3, 2)), at, "{transform:?}");
        }
        assert_eq!(OutputTransform::from_wayland(8), None);
    }

    #[test]
    fn a_quarter_turn_moves_the_top_left_corner_to_the_bottom_left() {
        let image = marked(3, 2);
        let turned = turned(&image, OutputTransform::Rotated90).unwrap();
        assert_eq!((turned.width(), turned.height()), (2, 3));
        assert_eq!(at(&turned, 0, 2), at(&image, 0, 0), "counter-clockwise");
        assert_eq!(at(&turned, 1, 0), at(&image, 2, 1));
        let flipped = super::turned(&image, OutputTransform::Flipped).unwrap();
        assert_eq!(
            at(&flipped, 2, 0),
            at(&image, 0, 0),
            "about the vertical axis"
        );
        let half = super::turned(&image, OutputTransform::Rotated180).unwrap();
        assert_eq!(at(&half, 2, 1), at(&image, 0, 0));
    }

    #[test]
    fn every_turned_buffer_comes_back_upright_with_its_sides_in_place() {
        let image = marked(5, 3);
        for turn in OutputTransform::ALL {
            let buffer = turned(&image, turn).unwrap();
            let expected = if turn.swaps_sides() { (3, 5) } else { (5, 3) };
            assert_eq!((buffer.width(), buffer.height()), expected, "{turn:?}");
            let back = upright(&buffer, turn).unwrap();
            assert_eq!((back.width(), back.height()), (5, 3), "{turn:?}");
            assert_eq!(back.data(), image.data(), "{turn:?}");
        }
    }

    #[test]
    fn the_logical_size_of_a_turned_display_is_its_mode_upright_over_its_scale() {
        assert_eq!(
            logical_size((2560, 1440), OutputTransform::Normal, 2.0),
            (1280, 720)
        );
        assert_eq!(
            logical_size((2560, 1440), OutputTransform::Rotated90, 2.0),
            (720, 1280)
        );
        assert_eq!(
            logical_size((2560, 1440), OutputTransform::Flipped270, 1.0),
            (1440, 2560)
        );
        assert_eq!(
            logical_size((1920, 1080), OutputTransform::Rotated180, 1.5),
            (1280, 720)
        );
        assert_eq!(
            logical_size((800, 600), OutputTransform::Normal, 0.0),
            (800, 600),
            "no scale is 1"
        );
    }

    #[test]
    fn the_pixel_under_a_point_follows_the_display_and_its_scale() {
        // a quarter-turned display at scale 2, to the left of the origin: 600 x 800 points, 1200 x 1600 pixels
        let placement = Placement {
            position: (-600, 0),
            size: (600, 800),
        };
        let image = (1200, 1600);
        assert_eq!(pixel_under(placement, image, (-600.0, 0.0)), Some((0, 0)));
        assert_eq!(pixel_under(placement, image, (-599.75, 0.6)), Some((0, 1)));
        assert_eq!(
            pixel_under(placement, image, (-0.01, 799.99)),
            Some((1199, 1599))
        );
        assert_eq!(
            pixel_under(placement, image, (0.0, 10.0)),
            None,
            "the next display"
        );
        assert_eq!(
            pixel_under(placement, image, (-300.0, 800.0)),
            None,
            "below it"
        );
        assert_eq!(pixel_under(placement, image, (-601.0, 10.0)), None);
    }
}
