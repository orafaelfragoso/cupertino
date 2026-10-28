//! Offscreen rendering of an output to an image (`--screenshot PATH`, headless mode, tests).

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use image::RgbaImage;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Offscreen};
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};

use super::CupertinoElement;

/// Render `elements` (front to back) into a texture of `size` and read it back as RGBA.
///
/// A fresh damage tracker is used, so the whole output is painted regardless of earlier frames.
pub fn render_to_image<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    elements: &[E],
    size: Size<i32, Physical>,
    scale: f64,
    clear: Color32F,
) -> Result<RgbaImage> {
    render_to_image_transformed(renderer, elements, size, scale, Transform::Normal, clear)
}

/// [`render_to_image`] for an output of mode `size` that is rotated or flipped by `transform`: the elements are in
/// the logical space of the output as it is shown, and the image is the framebuffer of the mode.
pub fn render_to_image_transformed<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    elements: &[E],
    size: Size<i32, Physical>,
    scale: f64,
    transform: Transform,
    clear: Color32F,
) -> Result<RgbaImage> {
    let buf_size: Size<i32, Buffer> = (size.w, size.h).into();
    let mut texture: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, buf_size)
        .map_err(|e| anyhow!("create offscreen buffer: {e}"))?;
    let mut tracker = OutputDamageTracker::new(size, scale, transform);
    let mapping = {
        let mut target = renderer
            .bind(&mut texture)
            .map_err(|e| anyhow!("bind offscreen buffer: {e}"))?;
        tracker
            .render_output(renderer, &mut target, 0, elements, clear)
            .map_err(|e| anyhow!("render offscreen: {e:?}"))?;
        renderer
            .copy_framebuffer(&target, Rectangle::from_size(buf_size), Fourcc::Abgr8888)
            .map_err(|e| anyhow!("read back framebuffer: {e}"))?
    };
    let bytes = renderer
        .map_texture(&mapping)
        .map_err(|e| anyhow!("map framebuffer: {e}"))?;
    RgbaImage::from_raw(size.w as u32, size.h as u32, bytes.to_vec())
        .ok_or_else(|| anyhow!("unexpected pixel buffer size"))
}

pub fn save_png(image: &RgbaImage, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    image
        .save(path)
        .with_context(|| format!("writing {}", path.display()))
}

/// Scene elements helper alias for callers that only render scenes.
pub fn render_scene_to_png(
    renderer: &mut GlesRenderer,
    elements: &[CupertinoElement],
    size: Size<i32, Physical>,
    scale: f64,
    path: &Path,
) -> Result<RgbaImage> {
    let img = render_to_image(
        renderer,
        elements,
        size,
        scale,
        Color32F::new(0.0, 0.0, 0.0, 1.0),
    )?;
    save_png(&img, path)?;
    Ok(img)
}

/// Fraction of pixels that differ noticeably from the first pixel (cheap "is anything drawn" test).
pub fn nonuniform_fraction(img: &RgbaImage) -> f64 {
    let first = *img.get_pixel(0, 0);
    let diff = img
        .pixels()
        .filter(|p| {
            p.0.iter()
                .zip(first.0.iter())
                .any(|(a, b)| (*a as i32 - *b as i32).abs() > 6)
        })
        .count();
    diff as f64 / (img.width() as f64 * img.height() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonuniform_fraction_counts_differences() {
        let mut img = RgbaImage::from_pixel(10, 10, image::Rgba([10, 10, 10, 255]));
        assert_eq!(nonuniform_fraction(&img), 0.0);
        for x in 1..6 {
            img.put_pixel(x, 0, image::Rgba([200, 10, 10, 255]));
        }
        assert!((nonuniform_fraction(&img) - 0.05).abs() < 1e-9);
    }
}
