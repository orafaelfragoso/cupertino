//! Where a display's pixels are, for screenshots and screen sharing: a copied buffer turned back upright by the
//! transform it carries ([`OutputTransform`], shared with the rest of the desktop), and a display's place in the
//! compositor's space when the server reports no logical geometry.

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
