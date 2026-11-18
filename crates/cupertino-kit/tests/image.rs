//! `kit::image`: a picture larger than the caller's limit, or than a picture Kit makes, is refused before its pixels
//! are made.

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
