//! Pictures from files and into files: PNG and JPEG decoded within the limits the caller gives, scaled, cut, turned
//! upright, and encoded again. Every function is plain work on [`ImageResource`]s that a program runs as a
//! [`task::cpu`](crate::task::cpu), never on its UI thread. A picture made here is premultiplied RGBA with rows of
//! four bytes a pixel; one given may be premultiplied or straight.

use cupertino_geometry::OutputTransform;
use cupertino_ui_render::{ImageResource, PixelFormat};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, ImageFormat, ImageReader, Limits, RgbaImage};
use std::fmt;
use std::io::Cursor;
use std::sync::Arc;

/// The most pixels a picture made here has: those of a window drawn on the CPU (`app::run`'s renderer).
const MAX_PIXELS: u64 = 1 << 26;

/// The most bytes a decoder holds for one picture: [`MAX_PIXELS`] at eight bytes a pixel, the widest pixel a PNG has.
const MAX_DECODE_BYTES: u64 = MAX_PIXELS * 8;

/// Why a picture was not made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// Wider or taller than the caller's limit, or more pixels than a picture made here has.
    TooLarge,
    /// Neither PNG nor JPEG.
    Unsupported,
    /// Data that does not decode or encode, a picture whose pixels do not fill its size, or a size of no pixels.
    Invalid(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::TooLarge => f.write_str("the picture is too large"),
            ImageError::Unsupported => f.write_str("only PNG and JPEG pictures are read"),
            ImageError::Invalid(why) => write!(f, "not a picture: {why}"),
        }
    }
}

impl std::error::Error for ImageError {}

/// The PNG or JPEG picture in `data`, refused when it is wider or taller than `max_side` pixels or has more pixels
/// than a picture made here has.
pub fn decode(data: &[u8], max_side: u32) -> Result<ImageResource, ImageError> {
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| ImageError::Invalid(e.to_string()))?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err(ImageError::Unsupported);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => ImageError::TooLarge,
        e => ImageError::Invalid(e.to_string()),
    })?;
    let (width, height) = (decoded.width(), decoded.height());
    checked_size(width, height)?;
    let mut pixels = decoded.into_rgba8().into_raw();
    premultiply(&mut pixels);
    Ok(made(width, height, pixels))
}

/// `image` scaled to `width` x `height` pixels, smoothly in both directions.
pub fn resize(image: &ImageResource, width: u32, height: u32) -> Result<ImageResource, ImageError> {
    checked_size(width, height)?;
    let source = RgbaImage::from_raw(image.width, image.height, premultiplied(image)?)
        .ok_or_else(|| ImageError::Invalid("pixels short of the size".into()))?;
    let mut pixels =
        image::imageops::resize(&source, width, height, FilterType::CatmullRom).into_raw();
    // the filter's overshoot can lift a colour above its alpha
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let a = pixel[3];
        for c in &mut pixel[..3] {
            *c = (*c).min(a);
        }
    }
    Ok(made(width, height, pixels))
}

/// The `width` x `height` pixels of `image` from pixel (`x`, `y`), refused when they are not all within it.
pub fn crop(
    image: &ImageResource,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<ImageResource, ImageError> {
    checked_size(width, height)?;
    if x.checked_add(width).is_none_or(|right| right > image.width)
        || y.checked_add(height)
            .is_none_or(|bottom| bottom > image.height)
    {
        return Err(ImageError::Invalid(
            "the area is not within the picture".into(),
        ));
    }
    let source = premultiplied(image)?;
    let row = image.width as usize * 4;
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for line in source
        .chunks_exact(row)
        .skip(y as usize)
        .take(height as usize)
    {
        pixels.extend_from_slice(&line[x as usize * 4..(x + width) as usize * 4]);
    }
    Ok(made(width, height, pixels))
}

/// The upright picture of `image`, which holds it turned by `transform` (as an output's buffer does).
pub fn orient(
    image: &ImageResource,
    transform: OutputTransform,
) -> Result<ImageResource, ImageError> {
    let source = premultiplied(image)?;
    let (bw, bh) = (image.width, image.height);
    let (uw, uh) = transform.turn_pixels((bw, bh));
    let mut pixels = vec![0; source.len()];
    for y in 0..bh {
        for x in 0..bw {
            let (ux, uy) = transform.upright_pixel((x, y), (bw, bh));
            let i = ((y * bw + x) * 4) as usize;
            let o = ((uy * uw + ux) * 4) as usize;
            pixels[o..o + 4].copy_from_slice(&source[i..i + 4]);
        }
    }
    Ok(made(uw, uh, pixels))
}

/// The PNG file of `image`, its alpha kept.
pub fn encode_png(image: &ImageResource) -> Result<Vec<u8>, ImageError> {
    let mut pixels = premultiplied(image)?;
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let a = u32::from(pixel[3]);
        if a != 0 && a != 255 {
            for c in &mut pixel[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(&pixels, image.width, image.height, ExtendedColorType::Rgba8)
        .map_err(|e| ImageError::Invalid(e.to_string()))?;
    Ok(bytes)
}

/// The JPEG file of `image` at `quality` (1 to 100), drawn over black where it is not opaque.
pub fn encode_jpeg(image: &ImageResource, quality: u8) -> Result<Vec<u8>, ImageError> {
    let pixels: Vec<u8> = premultiplied(image)?
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .write_image(&pixels, image.width, image.height, ExtendedColorType::Rgb8)
        .map_err(|e| ImageError::Invalid(e.to_string()))?;
    Ok(bytes)
}

/// Refuses a size of no pixels, or of more than [`MAX_PIXELS`].
fn checked_size(width: u32, height: u32) -> Result<(), ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::Invalid("a size of no pixels".into()));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageError::TooLarge);
    }
    Ok(())
}

/// The pixels of `image` as premultiplied rows of four bytes a pixel, with no gap between rows.
fn premultiplied(image: &ImageResource) -> Result<Vec<u8>, ImageError> {
    checked_size(image.width, image.height)?;
    let row = image.width as usize * 4;
    let stride = image.stride as usize;
    let needed = stride
        .checked_mul(image.height as usize - 1)
        .and_then(|rows| rows.checked_add(row));
    if stride < row || needed.is_none_or(|needed| image.pixels.len() < needed) {
        return Err(ImageError::Invalid("pixels short of the size".into()));
    }
    let mut pixels = Vec::with_capacity(row * image.height as usize);
    for y in 0..image.height as usize {
        pixels.extend_from_slice(&image.pixels[y * stride..y * stride + row]);
    }
    if image.format == PixelFormat::Rgba8 {
        premultiply(&mut pixels);
    }
    Ok(pixels)
}

/// Straight RGBA `pixels` premultiplied by their alpha.
fn premultiply(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let a = u32::from(pixel[3]);
        for c in &mut pixel[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

/// A picture made here of `pixels`, premultiplied rows of `width` x `height` pixels.
fn made(width: u32, height: u32, pixels: Vec<u8>) -> ImageResource {
    ImageResource {
        revision: 1,
        width,
        height,
        stride: width * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: Arc::from(pixels),
    }
}
