//! The picture of a minimized window that the dock shows on its tile: the window drawn at its output's scale, scaled
//! down to fit within [`MAX_SIDE`] pixels on its longer side by averaging the pixels each one covers.

/// The longest side of a picture, in pixels: a tile shows it at most a little under the magnified tile's width at
/// twice the scale.
pub const MAX_SIDE: u32 = 256;

/// A window's picture: `width` × `height` pixels of `wl_shm` ARGB8888 (premultiplied, little-endian), rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowThumbnail {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u8>,
}

/// The size of a `width` × `height` picture brought within `max` pixels on its longer side, its shape kept and never
/// enlarged; at least one pixel each way.
pub fn fitted_size(width: u32, height: u32, max: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max || long == 0 {
        return (width.max(1), height.max(1));
    }
    let k = f64::from(max) / f64::from(long);
    let side = |v: u32| ((f64::from(v) * k).round() as u32).clamp(1, max);
    (side(width), side(height))
}

/// The picture of an image of premultiplied RGBA bytes (rows top to bottom, packed), fitted within [`MAX_SIDE`]: each
/// pixel is the mean of the image's pixels under it, partly covered ones by the share they cover. `None` for an empty
/// image or one whose bytes do not match its size.
pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<WindowThumbnail> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let (tw, th) = fitted_size(width, height, MAX_SIDE);
    let (sx, sy) = (
        f64::from(width) / f64::from(tw),
        f64::from(height) / f64::from(th),
    );
    // the share of each source column (row) that output column (row) `i` covers
    let spans = |i: u32, step: f64, limit: u32| -> Vec<(usize, f64)> {
        let (a, b) = (f64::from(i) * step, f64::from(i + 1) * step);
        let first = a.floor() as u32;
        let last = (b.ceil() as u32).min(limit);
        (first..last)
            .map(|s| {
                let lo = f64::from(s).max(a);
                let hi = f64::from(s + 1).min(b);
                (s as usize, (hi - lo).max(0.0))
            })
            .filter(|&(_, w)| w > 0.0)
            .collect()
    };
    let columns: Vec<_> = (0..tw).map(|x| spans(x, sx, width)).collect();
    let mut argb = Vec::with_capacity(tw as usize * th as usize * 4);
    for y in 0..th {
        let rows = spans(y, sy, height);
        for column in &columns {
            let mut sum = [0.0f64; 4];
            let mut total = 0.0;
            for &(ry, wy) in &rows {
                let row = ry * width as usize;
                for &(cx, wx) in column {
                    let w = wx * wy;
                    let i = (row + cx) * 4;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += f64::from(rgba[i + c]) * w;
                    }
                    total += w;
                }
            }
            let [r, g, b, a] = sum.map(|s| (s / total).round().clamp(0.0, 255.0) as u8);
            // memory order of little-endian ARGB8888: blue, green, red, alpha
            argb.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(WindowThumbnail {
        width: tw,
        height: th,
        argb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_fits_within_256_pixels_on_its_longer_side_keeping_its_shape() {
        assert_eq!(fitted_size(1600, 1000, MAX_SIDE), (256, 160));
        assert_eq!(fitted_size(800, 1200, MAX_SIDE), (171, 256));
        assert_eq!(
            fitted_size(200, 100, MAX_SIDE),
            (200, 100),
            "never enlarged"
        );
        assert_eq!(fitted_size(5000, 3, MAX_SIDE), (256, 1), "at least a pixel");
    }

    #[test]
    fn each_pixel_is_the_mean_of_what_it_covers_in_little_endian_argb() {
        // 512 × 2: the left half red, the right half half-transparent blue (premultiplied)
        let (w, h) = (512u32, 2u32);
        let mut rgba = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(if x < 256 {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 128, 128]
                });
            }
        }
        let t = from_rgba(w, h, &rgba).unwrap();
        assert_eq!((t.width, t.height), (256, 1));
        assert_eq!(
            &t.argb[..4],
            &[0, 0, 255, 255],
            "red as blue, green, red, alpha"
        );
        assert_eq!(&t.argb[t.argb.len() - 4..], &[128, 0, 0, 128]);
        let half = from_rgba(
            512,
            1,
            &[[255u8, 255, 255, 255], [0, 0, 0, 0]].concat().repeat(256),
        )
        .unwrap();
        assert_eq!(&half.argb[..4], &[128, 128, 128, 128], "two pixels in one");
        assert_eq!(from_rgba(2, 2, &[0; 15]), None);
        assert_eq!(from_rgba(0, 2, &[]), None);
    }

    #[test]
    fn a_pixel_covering_part_of_a_source_pixel_weighs_it_by_that_share() {
        // 900 × 1 into 256 × 1, white, black, white over and over: a pixel covers about three and a half
        let big = [[255u8, 255, 255, 255], [0, 0, 0, 255], [255, 255, 255, 255]]
            .concat()
            .repeat(300);
        // 900 × 1 into 256 × 1, a repeating white, black, white
        let t = from_rgba(900, 1, &big).unwrap();
        assert_eq!(t.width, 256);
        let mean: f64 = t.argb.chunks(4).map(|p| f64::from(p[0])).sum::<f64>() / 256.0;
        assert!(
            (mean - 170.0).abs() < 1.5,
            "two thirds white on average: {mean}"
        );
    }
}
