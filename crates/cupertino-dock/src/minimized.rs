//! How a minimized window's tile looks: the window's picture, as the server drew it when the window minimized, fitted
//! across the tile a little above its middle, and the application's icon small over the picture's lower trailing
//! corner. Every length is at a 64 point tile and scales with it. A tile whose window has no picture shows the
//! application's icon at full size.

use std::sync::Arc;

use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::next_resource_id;
use cupertino_kit::geometry::Rect;
use cupertino_kit::render::{ImageResource, PixelFormat};

/// The box the picture is fitted into, its shape kept: as wide as the tile less 2 points each side, and at most as
/// tall as an icon's artwork.
pub const PICTURE_WIDTH: f64 = 60.0;
pub const PICTURE_HEIGHT: f64 = 52.0;
/// How far the picture's middle stands above the tile's middle.
pub const PICTURE_RISE: f64 = 1.25;
/// The application icon's artwork side, and where its trailing and bottom edges stand inside the tile's.
pub const ICON_ARTWORK: f64 = 19.5;
pub const ICON_TRAILING_INSET: f64 = 3.25;
pub const ICON_BOTTOM_INSET: f64 = 4.5;

/// The rectangle the picture of a `width` × `height` window is drawn at in a tile at `tile` (the tile's rectangle):
/// fitted into [`PICTURE_WIDTH`] × [`PICTURE_HEIGHT`], centred across the tile, its middle [`PICTURE_RISE`] above the
/// tile's.
pub fn picture_rect(tile: Rect, width: f64, height: f64) -> Rect {
    let k = tile.width() / 64.0;
    let fit = if width > 0.0 && height > 0.0 {
        (PICTURE_WIDTH / width).min(PICTURE_HEIGHT / height)
    } else {
        0.0
    };
    let (w, h) = (width * fit * k, height * fit * k);
    let middle = (
        tile.x() + tile.width() / 2.0,
        tile.y() + tile.height() / 2.0 - PICTURE_RISE * k,
    );
    Rect::new(middle.0 - w / 2.0, middle.1 - h / 2.0, w, h)
}

/// The rectangle of the application icon's artwork over the picture, in a tile at `tile`.
pub fn icon_rect(tile: Rect) -> Rect {
    let k = tile.width() / 64.0;
    let side = ICON_ARTWORK * k;
    Rect::new(
        tile.x() + tile.width() - ICON_TRAILING_INSET * k - side,
        tile.y() + tile.height() - ICON_BOTTOM_INSET * k - side,
        side,
        side,
    )
}

/// A picture of `width` × `height` pixels of little-endian ARGB8888 (premultiplied) as an image to draw; a colour
/// channel over its alpha is cut to it. `None` for an empty picture or one whose bytes do not match its size.
pub fn image(width: u32, height: u32, argb: &[u8]) -> Option<ImageSource> {
    if width == 0 || height == 0 || argb.len() != width as usize * height as usize * 4 {
        return None;
    }
    let rgba: Vec<u8> = argb
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, a]| [r.min(a), g.min(a), b.min(a), a])
        .collect();
    Some(ImageSource {
        id: next_resource_id(),
        image: ImageResource {
            revision: 0,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(rgba),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rect, b: [f64; 4]) -> bool {
        [a.x(), a.y(), a.width(), a.height()]
            .iter()
            .zip(b)
            .all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn a_wide_window_spans_60_points_a_little_above_the_tiles_middle() {
        let tile = Rect::new(100.0, 10.0, 64.0, 64.0);
        // a 1600 × 986 window: 60 × 37 points
        let r = picture_rect(tile, 1600.0, 986.0);
        assert!(
            close(r, [102.0, 10.0 + 32.0 - 1.25 - 18.4875, 60.0, 36.975]),
            "{r:?}"
        );
        // a tall one is as tall as an icon's artwork and keeps its shape
        let tall = picture_rect(tile, 400.0, 800.0);
        assert!(
            close(tall, [119.0, 10.0 + 30.75 - 26.0, 26.0, 52.0]),
            "{tall:?}"
        );
        // all of it scales with the tile
        let big = picture_rect(Rect::new(0.0, 0.0, 128.0, 128.0), 1600.0, 986.0);
        assert!((big.width() - 120.0).abs() < 1e-9 && (big.height() - 73.95).abs() < 1e-9);
    }

    #[test]
    fn the_icon_is_19_and_a_half_points_over_the_pictures_lower_trailing_corner() {
        let tile = Rect::new(100.0, 10.0, 64.0, 64.0);
        let icon = icon_rect(tile);
        assert!(close(icon, [141.25, 50.0, 19.5, 19.5]), "{icon:?}");
        let picture = picture_rect(tile, 1600.0, 986.0);
        // it stands 1.25 points inside the picture's trailing edge and hangs 10 points below its bottom
        assert!((picture.x() + picture.width() - (icon.x() + icon.width()) - 1.25).abs() < 1e-9);
        assert!((icon.y() + icon.height() - (picture.y() + picture.height()) - 10.25).abs() < 0.02);
        let half = icon_rect(Rect::new(0.0, 0.0, 32.0, 32.0));
        assert!(close(half, [20.625, 20.0, 9.75, 9.75]), "{half:?}");
    }

    #[test]
    fn a_picture_turns_from_argb_into_premultiplied_rgba() {
        let source = image(2, 1, &[10, 20, 30, 255, 200, 0, 0, 100]).unwrap();
        assert_eq!(&source.image.pixels[..], &[30, 20, 10, 255, 0, 0, 100, 100]);
        assert_eq!(
            (source.image.width, source.image.height, source.image.stride),
            (2, 1, 8)
        );
        assert!(image(2, 2, &[0; 8]).is_none());
        assert!(image(0, 0, &[]).is_none());
    }
}
