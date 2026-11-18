//! Pictures a program draws from paint operations, drawn on the CPU pool.

use crate::task;
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{Command, Environment};
use cupertino_ui_render::{
    CpuConfig, CpuRenderer, DamageRegion, DisplayList, ImageResource, PaintNode, PaintNodeId,
    PaintOp, PaintResources, PaintRevision, RenderRequest, Transform, UiRenderer,
};
use cupertino_ui_text::GlyphRaster;
use std::sync::Arc;

/// The most device pixels a picture has: those of a window drawn on the CPU (`app::run`'s renderer).
const MAX_PICTURE_PIXELS: u64 = 1 << 26;

/// The glyph cache of one picture's renderer: enough for the glyphs of a few lines of text, kept only while it draws.
const PICTURE_GLYPH_CACHE_BYTES: usize = 1 << 20;

/// Draws `ops`, shapes and text in `env`'s text system, into a picture of `size` points at `scale` device pixels a
/// point, on the CPU pool, and gives it to `done` on the UI thread, whose message the application then hears. The
/// picture is premultiplied RGBA of `size` times `scale` pixels a side, rounded up. `done` is given `None` for a picture
/// of no pixels or more than a window drawn on the CPU has, an operation that draws an image, or one the renderer
/// refuses.
pub fn picture<M: Send + 'static>(
    env: &Environment,
    ops: Vec<PaintOp>,
    size: Size,
    scale: f64,
    done: impl FnOnce(Option<ImageResource>) -> M + Send + 'static,
) -> Command<M> {
    let text = Arc::clone(&env.text);
    task::cpu(move || draw(text, ops, size, scale), done)
}

fn draw(
    text: Arc<cupertino_ui_text::TextSystem>,
    ops: Vec<PaintOp>,
    size: Size,
    scale: f64,
) -> Option<ImageResource> {
    let side = |points: f64| (points * scale).ceil();
    let (width, height) = (side(size.width), side(size.height));
    if !(scale.is_finite() && scale > 0.0 && width >= 1.0 && height >= 1.0)
        || width * height > MAX_PICTURE_PIXELS as f64
    {
        return None;
    }
    let (width, height) = (width as u32, height as u32);
    let node = PaintNode {
        layer: None,
        id: PaintNodeId(1),
        parent: None,
        revision: PaintRevision(1),
        transform: Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        clip: None,
        group: None,
        bounds: Rect::new(
            0.0,
            0.0,
            f64::from(width) / scale,
            f64::from(height) / scale,
        ),
        hit: Rect::new(0.0, 0.0, 0.0, 0.0),
        ops,
    };
    let list = DisplayList { nodes: vec![node] };
    let mut renderer = CpuRenderer::new(
        CpuConfig {
            max_pixels: MAX_PICTURE_PIXELS,
            glyph_cache_bytes: PICTURE_GLYPH_CACHE_BYTES,
            image_cache_bytes: 0,
            font_smoothing: true,
        },
        GlyphRaster::new(text),
    );
    let frame = renderer
        .render(&RenderRequest {
            list: &list,
            resources: &PaintResources::default(),
            scale,
            width,
            height,
            damage: &DamageRegion::Full,
        })
        .ok()?;
    let frame = renderer.frame(frame).ok()?;
    Some(ImageResource {
        revision: 1,
        width,
        height,
        stride: frame.stride,
        format: frame.format,
        pixels: Arc::from(frame.pixels.as_slice()),
    })
}
