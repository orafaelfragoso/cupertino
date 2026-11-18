//! `kit::image`: a picture larger than the caller's limit, or than a picture Kit makes, is refused before its pixels
//! are made; a crop takes its area, a picture held turned by an output transform comes out upright, and PNG and JPEG
//! keep what their formats can.

use cupertino_kit::image::{self, ImageError};
use cupertino_kit::render::{ImageResource, PixelFormat};
use std::sync::Arc;

/// An opaque grey picture of `width` x `height` pixels.
fn grey(width: u32, height: u32) -> ImageResource {
    ImageResource {
        revision: 1,
        width,
        height,
        stride: width * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: Arc::from(vec![128; (width * height * 4) as usize]),
    }
}

#[test]
fn a_picture_past_the_limits_is_refused_and_one_within_them_is_made() {
    let png = image::encode_png(&grey(8, 4)).unwrap();
    let jpeg = image::encode_jpeg(&grey(8, 4), 90).unwrap();
    let picture = grey(8, 4);
    let cases = [
        ("PNG within its limit", Ok((8, 4)), image::decode(&png, 8)),
        (
            "PNG wider than its limit",
            Err(ImageError::TooLarge),
            image::decode(&png, 7),
        ),
        ("JPEG within its limit", Ok((8, 4)), image::decode(&jpeg, 8)),
        (
            "JPEG wider than its limit",
            Err(ImageError::TooLarge),
            image::decode(&jpeg, 4),
        ),
        (
            "not a picture",
            Err(ImageError::Unsupported),
            image::decode(b"GIF89a", 8),
        ),
        (
            "resized within the most pixels",
            Ok((16, 8)),
            image::resize(&picture, 16, 8),
        ),
        (
            "resized past the most pixels",
            Err(ImageError::TooLarge),
            image::resize(&picture, 1 << 14, 1 << 14),
        ),
    ];
    for (case, expected, made) in cases {
        assert_eq!(
            made.map(|made| (made.width, made.height)),
            expected,
            "{case}"
        );
    }
}

/// A picture of the pixels `rows`, each pixel four bytes, with `pad` bytes after each row.
fn pixels(rows: &[&[[u8; 4]]], format: PixelFormat, pad: u32) -> ImageResource {
    let width = rows[0].len() as u32;
    let stride = width * 4 + pad;
    let mut bytes = Vec::new();
    for row in rows {
        bytes.extend(row.iter().flatten());
        bytes.extend(std::iter::repeat_n(0xEE, pad as usize));
    }
    ImageResource {
        revision: 1,
        width,
        height: rows.len() as u32,
        stride,
        format,
        pixels: Arc::from(bytes),
    }
}

/// An 8 x 8 picture of one premultiplied pixel.
fn solid(pixel: [u8; 4]) -> ImageResource {
    let row = [pixel; 8];
    pixels(&[row.as_slice(); 8], PixelFormat::Rgba8Premultiplied, 0)
}

fn rows_of(image: &ImageResource) -> Vec<Vec<[u8; 4]>> {
    assert_eq!(image.format, PixelFormat::Rgba8Premultiplied);
    assert_eq!(image.stride, image.width * 4);
    image
        .pixels
        .chunks(image.stride as usize)
        .map(|row| row.chunks(4).map(|p| [p[0], p[1], p[2], p[3]]).collect())
        .collect()
}

const P: [u8; 4] = [10, 0, 0, 255];
const Q: [u8; 4] = [0, 20, 0, 255];
const R: [u8; 4] = [0, 0, 30, 255];
const S: [u8; 4] = [40, 40, 0, 255];

#[test]
fn a_crop_takes_the_pixels_of_its_area_whatever_the_row_padding_and_refuses_one_outside_the_picture(
) {
    let grid = [[P, Q, R], [S, P, Q], [R, S, P]];
    let rows: Vec<&[[u8; 4]]> = grid.iter().map(|r| r.as_slice()).collect();
    for pad in [0, 8] {
        let picture = pixels(&rows, PixelFormat::Rgba8Premultiplied, pad);
        let got = image::crop(&picture, 1, 1, 2, 2).unwrap();
        assert_eq!(rows_of(&got), [[P, Q], [S, P]], "padding {pad}");
    }
    let picture = pixels(&rows, PixelFormat::Rgba8Premultiplied, 0);
    for (what, area) in [
        ("past the right edge", (2, 0, 2, 1)),
        ("past the bottom edge", (0, 2, 1, 2)),
        ("no width", (0, 0, 0, 1)),
        ("an origin that overflows", (u32::MAX, 0, 2, 1)),
    ] {
        let (x, y, w, h) = area;
        assert!(
            matches!(
                image::crop(&picture, x, y, w, h),
                Err(ImageError::Invalid(_))
            ),
            "{what}"
        );
    }
    let short = ImageResource {
        pixels: Arc::from(vec![0; 10]),
        ..picture.clone()
    };
    assert!(matches!(
        image::crop(&short, 0, 0, 1, 1),
        Err(ImageError::Invalid(_))
    ));

    let straight = pixels(&[&[[200, 100, 50, 128]]], PixelFormat::Rgba8, 0);
    let got = image::crop(&straight, 0, 0, 1, 1).unwrap();
    assert_eq!(
        rows_of(&got),
        [[[100, 50, 25, 128]]],
        "a straight picture comes out premultiplied"
    );
}

#[test]
fn a_picture_held_turned_by_an_output_transform_comes_out_upright() {
    use cupertino_kit::image::OutputTransform as T;
    // the upright picture is [[P, Q], [R, S]]; each transform's buffer holds it turned that way
    let upright = [[P, Q], [R, S]];
    let buffers: [(T, [[[u8; 4]; 2]; 2]); 8] = [
        (T::Normal, [[P, Q], [R, S]]),
        (T::Rotated90, [[Q, S], [P, R]]),
        (T::Rotated180, [[S, R], [Q, P]]),
        (T::Rotated270, [[R, P], [S, Q]]),
        (T::Flipped, [[Q, P], [S, R]]),
        (T::Flipped90, [[P, R], [Q, S]]),
        (T::Flipped180, [[R, S], [P, Q]]),
        (T::Flipped270, [[S, Q], [R, P]]),
    ];
    for (transform, buffer) in buffers {
        let rows: Vec<&[[u8; 4]]> = buffer.iter().map(|r| r.as_slice()).collect();
        let held = pixels(&rows, PixelFormat::Rgba8Premultiplied, 0);
        let got = image::orient(&held, transform).unwrap();
        assert_eq!(rows_of(&got), upright, "{transform:?}");
    }

    let wide = pixels(&[&[P, Q, R]], PixelFormat::Rgba8Premultiplied, 0);
    let got = image::orient(&wide, T::Rotated90).unwrap();
    assert_eq!(
        (got.width, got.height),
        (1, 3),
        "a quarter turn swaps the sides"
    );
}

#[test]
fn a_png_keeps_the_alpha_of_its_picture_and_a_jpeg_keeps_its_colour_on_black() {
    let clear = [0, 0, 0, 0];
    let half = [60, 30, 0, 128];
    let picture = pixels(
        &[&[P, half], &[clear, S]],
        PixelFormat::Rgba8Premultiplied,
        0,
    );
    let png = image::decode(&image::encode_png(&picture).unwrap(), 16).unwrap();
    let close = |a: u8, b: u8| a.abs_diff(b) <= 1;
    for (got, want) in rows_of(&png)
        .concat()
        .iter()
        .zip(rows_of(&picture).concat())
    {
        assert!(
            got.iter().zip(want).all(|(g, w)| close(*g, w)),
            "{got:?} against {want:?}"
        );
    }

    let opaque = solid([200, 200, 200, 255]);
    let jpeg = image::decode(&image::encode_jpeg(&opaque, 95).unwrap(), 16).unwrap();
    assert!(rows_of(&jpeg)
        .concat()
        .iter()
        .all(|p| p[0].abs_diff(200) <= 4 && p[3] == 255));
    let faint = solid([100, 100, 100, 100]);
    let on_black = image::decode(&image::encode_jpeg(&faint, 95).unwrap(), 16).unwrap();
    assert!(
        rows_of(&on_black)
            .concat()
            .iter()
            .all(|p| p[0].abs_diff(100) <= 4 && p[3] == 255),
        "its premultiplied colour is what shows"
    );
}
