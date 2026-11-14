//! CupertinoKit's GPU renderer. [`GpuRenderer`] draws a [`DisplayList`](cupertino_ui_render::DisplayList) with
//! `cupertino-gpu` into a window-sized texture it keeps between frames, and computes what the CPU renderer computes:
//!
//! * rectangles and rounded rectangles, filled and stroked, by the shared distance, their coverage stored as a byte
//!   and multiplied by the clips' in the CPU renderer's 8-bit steps;
//! * gradients by the shared rule at each pixel's centre, dithered by the window pixel;
//! * glyphs rasterized by the same glyph rasterizer into the coverage and colour atlases, placed at quarter-pixel x
//!   phase and whole-pixel y and tinted on the GPU;
//! * paths, and the edges of images that do not lie on whole pixels, rasterized on the CPU into the path mask atlas;
//! * images sampled nearest or linearly as the CPU renderer's image pipeline samples them;
//! * clips: rectangles and rounded rectangles analytically in the programs, nested by multiplied coverage; path clips
//!   through the path mask atlas; a clip that is a rectangle on whole pixels, whose coverage is all or nothing, is
//!   the scissor rectangle alone (there is no stencil);
//! * groups and layers with opacity as offscreen targets from the pool (or of their own when the pool cannot hold
//!   them), laid over through the clips around them;
//! * blur, drop shadow and inner shadow: the shared pyramid of 2x halvings and the Gaussian in float levels, read
//!   back at every pixel and stored as the CPU renderer stores them.
//!
//! Both renderers agree within 2/255 per channel at every pixel; the tests compare them on the goldens and on
//! random scenes at scales 1, 1.5 and 2.
//!
//! A frame repaints the rectangle around the request's damage (or everything), so pixels outside it keep the last
//! frame's. A scroll layer keeps its content in a texture between frames: a scroll moves what stays in view and paints
//! only what comes into view (`record::ScrollPlan`). The frame itself stays on the GPU: [`GpuRenderer::texture`] is what a presenter draws from, and
//! [`GpuRenderer::read_pixels`] reads it back for tests and for a surface that falls back to shared memory.

mod exec;
mod geometry;
mod record;

use std::collections::HashMap;
use std::rc::Rc;

use cupertino_gpu::atlas::{Atlas, AtlasKind};
use cupertino_gpu::pool::{Pooled, TargetPool, TARGET_POOL_CAP};
use cupertino_gpu::{
    shaders, BindGroupLayout, BindingKind, BindingLayout, Blend, Buffer, BufferDesc, BufferUsage,
    Device, Error as GpuError, Pipeline, PipelineDesc, Region, StepMode, Texture, TextureDesc,
    TextureFormat, TextureUsage, Topology, VertexAttribute, VertexFormat, VertexLayout,
};
use cupertino_ui_render::{
    damage_rects, CpuFrame, DamageRegion, DeviceFrame, DeviceFrameRef, Layer, PaintNodeId,
    PixelFormat, PixelRect, RenderError, RenderFrameId, RenderRequest, ResourceId, UiRenderer,
};
use cupertino_ui_text::{GlyphFormat, GlyphKey, GlyphRasterizer};

use crate::geometry::Bounds;

/// What a GPU renderer may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuConfig {
    /// The most pixels the target or one layer may have.
    pub max_pixels: u64,
    /// Darkens glyph stems by the fill colour, as the CPU renderer's option of the same name.
    pub font_smoothing: bool,
    /// Bytes of image textures kept between frames.
    pub image_bytes: u64,
}

impl Default for GpuConfig {
    fn default() -> Self {
        GpuConfig {
            max_pixels: 1 << 26,
            font_smoothing: true,
            image_bytes: 32 << 20,
        }
    }
}

/// A frame of the GPU renderer: its size and the pixels it repainted. The pixels are in [`GpuRenderer::texture`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuFrame {
    pub id: RenderFrameId,
    pub width: u32,
    pub height: u32,
    pub damage: Vec<PixelRect>,
}

/// What the last frame did, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuStats {
    pub draws: u32,
    pub passes: u32,
    /// Offscreen targets used for groups, layers and effects.
    pub layers: u32,
    /// Of them, those whose texture was made for the frame, not taken from the pool or from a target the frame had
    /// finished with.
    pub targets_made: u32,
    pub glyph_uploads: u32,
    pub mask_uploads: u32,
    pub image_uploads: u32,
    /// Bytes of uniforms and instances written.
    pub uniform_bytes: u64,
    pub instance_bytes: u64,
    /// Scroll layers whose content is kept, and the pixels of them painted again this frame.
    pub kept_layers: u32,
    pub scroll_pixels: u64,
}

/// Bytes every uniform block starts at a multiple of: the largest offset alignment drivers ask for.
const UNIFORM_ALIGN: usize = 256;

/// Bytes of one quad instance: its rectangle and its atlas texel, two `vec4`.
const INSTANCE_BYTES: u32 = 32;

/// The programs and fixed resources of a renderer.
pub(crate) struct Programs {
    pub fill_layout: BindGroupLayout,
    pub fill: Pipeline,
    /// The fill program replacing what is under it: clears a repainted rectangle.
    pub clear: Pipeline,
    pub texture_layout: BindGroupLayout,
    pub texture: Pipeline,
    pub level_layout: BindGroupLayout,
    pub halve: Pipeline,
    pub gauss: Pipeline,
    pub resolve_layout: BindGroupLayout,
    pub resolve: Pipeline,
    /// Bound where a program reads no texture.
    pub empty: Texture,
}

impl Programs {
    fn new(device: &Device) -> Result<Programs, GpuError> {
        let texture = |binding| BindingLayout {
            binding,
            kind: BindingKind::Texture,
        };
        let uniform = BindingLayout {
            binding: 0,
            kind: BindingKind::UniformBuffer,
        };
        let instances = VertexLayout {
            stride: INSTANCE_BYTES,
            step: StepMode::Instance,
            attributes: vec![
                VertexAttribute {
                    location: 0,
                    format: VertexFormat::Float32x4,
                    offset: 0,
                },
                VertexAttribute {
                    location: 1,
                    format: VertexFormat::Float32x4,
                    offset: 16,
                },
            ],
        };
        let fill_layout = device.create_bind_group_layout(&[uniform, texture(1), texture(2)])?;
        let texture_layout =
            device.create_bind_group_layout(&[uniform, texture(1), texture(2), texture(3)])?;
        let level_layout = device.create_bind_group_layout(&[uniform, texture(1)])?;
        let resolve_layout = device.create_bind_group_layout(&[uniform, texture(1), texture(2)])?;
        let quads = |shader, layout: &BindGroupLayout, blend| {
            device.create_pipeline(&PipelineDesc {
                shader,
                layouts: &[layout],
                vertex_buffers: std::slice::from_ref(&instances),
                blend,
                topology: Topology::TriangleStrip,
            })
        };
        let whole = |shader, layout: &BindGroupLayout| {
            device.create_pipeline(&PipelineDesc {
                shader,
                layouts: &[layout],
                vertex_buffers: &[],
                blend: Blend::Replace,
                topology: Topology::TriangleList,
            })
        };
        let empty = device.create_texture(&TextureDesc {
            size: (1, 1),
            format: TextureFormat::Rgba8,
            usage: TextureUsage::SAMPLED,
        })?;
        device.write_texture(&empty, Region::new(0, 0, 1, 1), &[0; 4])?;
        Ok(Programs {
            fill: quads(&shaders::UI_FILL, &fill_layout, Blend::PremultipliedOver)?,
            clear: quads(&shaders::UI_FILL, &fill_layout, Blend::Replace)?,
            texture: quads(
                &shaders::UI_TEXTURE,
                &texture_layout,
                Blend::PremultipliedOver,
            )?,
            halve: whole(&shaders::BLUR_HALVE, &level_layout)?,
            gauss: whole(&shaders::BLUR_GAUSS, &level_layout)?,
            resolve: whole(&shaders::UI_EFFECT, &resolve_layout)?,
            fill_layout,
            texture_layout,
            level_layout,
            resolve_layout,
            empty,
        })
    }

    fn destroy(self, device: &Device) {
        for p in [
            self.fill,
            self.clear,
            self.texture,
            self.halve,
            self.gauss,
            self.resolve,
        ] {
            device.destroy_pipeline(p);
        }
        device.destroy_texture(self.empty);
    }
}

/// A glyph raster's place: its format, size and bearing (its pixels are in an atlas, keyed by the glyph key's hash).
#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphMeta {
    pub format: GlyphFormat,
    pub width: u32,
    pub height: u32,
    pub bearing_x: i32,
    pub bearing_y: i32,
}

/// An image uploaded premultiplied.
pub(crate) struct ImageTexture {
    pub revision: u64,
    pub texture: Texture,
    pub used: u64,
}

/// A target drawn into during a frame.
pub(crate) enum TargetMem {
    /// The window's own texture.
    Window,
    Pooled(Pooled<Texture>),
    /// Larger than the pool holds: made for the frame and destroyed after it.
    Own(Texture),
    /// A kept scroll layer's texture, taken for the frame from the layer's `slot` and given back after it.
    Kept {
        node: PaintNodeId,
        slot: usize,
        texture: Texture,
    },
}

/// The extra bytes the kept scroll layers of one renderer hold at most beyond two textures the size of its window
/// (each layer keeps two, the content moved from one into the other as it scrolls): a window keeps one large layer
/// and a few small ones. A layer past the cap is not kept: what it shows is painted in place every frame it moves.
const SCROLL_SLACK_BYTES: u64 = 16 << 20;

/// A kept scroll layer: its content as the window last showed it, under none of its clips, in one of two textures
/// the size of its viewport (the other takes it moved as the layer scrolls).
pub(crate) struct KeptScroll {
    pub textures: [Option<Texture>; 2],
    /// The texture holding the content.
    pub current: usize,
    pub scale: f64,
    /// Where its node's clip showed it, in window pixels.
    pub viewport: Bounds,
    /// The offset it was drawn at, in device pixels.
    pub shift: (f64, f64),
}

impl KeptScroll {
    fn destroy(self, device: &Device) {
        for t in self.textures.into_iter().flatten() {
            device.destroy_texture(t);
        }
    }
}

/// Paints display lists into a window-sized texture on the GPU, repainting the damage.
pub struct GpuRenderer<R> {
    device: Rc<Device>,
    rasterizer: R,
    config: GpuConfig,
    programs: Option<Rc<GpuPrograms>>,
    window: Option<Rc<WindowTexture>>,
    size: (u32, u32),
    scale: f64,
    /// The window texture holds the last frame's complete contents.
    valid: bool,
    coverage: Atlas,
    color: Atlas,
    masks: Atlas,
    glyphs: HashMap<GlyphKey, Option<GlyphMeta>>,
    images: HashMap<ResourceId, ImageTexture>,
    pool: TargetPool<Texture>,
    uniforms: Option<Buffer>,
    instances: Option<Buffer>,
    /// Frames drawn, for the atlases' and the pool's least-recently-used order.
    frames: u64,
    next_id: u64,
    frame: GpuFrame,
    stats: GpuStats,
    /// Each scroll layer of the last frame ([`cupertino_ui_render::scroll_layers`]): the window pixels its clip
    /// showed and its drawn offset in device pixels.
    scrolls: HashMap<PaintNodeId, (Bounds, (i32, i32))>,
    /// The scroll layers whose content is kept.
    kept: HashMap<PaintNodeId, KeptScroll>,
}

/// A GPU failure as a render error: the frame is not drawn and the next one is drawn whole. A lost device is also
/// seen by its owner's status checks, which replace the renderer.
fn gpu(e: GpuError) -> RenderError {
    match e {
        GpuError::OverBudget { .. } => RenderError::ResourceLimit,
        _ => RenderError::ResourceLimit,
    }
}

/// The programs a renderer draws with, which every renderer on one device can share: made once (on any thread the
/// device is current on), then handed to [`GpuRenderer::with_programs`].
pub struct GpuPrograms {
    inner: Programs,
}

impl GpuPrograms {
    /// The programs on `device`, compiled or loaded from its saved programs.
    pub fn new(device: &Device) -> Result<GpuPrograms, GpuError> {
        Programs::new(device).map(|inner| GpuPrograms { inner })
    }

    /// Frees the programs on `device`, which they were made on.
    pub fn destroy(self, device: &Device) {
        self.inner.destroy(device);
    }
}

/// The window-sized texture a renderer draws its frames into, shared with the frames it hands out
/// ([`WindowRenderer`]): a presenter on the same device draws from it, anything else reads it back. It goes with the
/// last of them; the renderer draws its next frame into it again, so a frame kept after the next one is drawn shows
/// that one.
pub struct WindowTexture {
    device: Rc<Device>,
    texture: Option<Texture>,
    size: (u32, u32),
}

impl WindowTexture {
    /// The texture: premultiplied RGBA, first row first.
    pub fn texture(&self) -> &Texture {
        self.texture.as_ref().expect("held until dropped")
    }

    /// The device the texture is on.
    pub fn device(&self) -> &Rc<Device> {
        &self.device
    }

    /// Its size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

impl DeviceFrame for WindowTexture {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn read_pixels(&self) -> Result<Vec<u8>, RenderError> {
        let (w, h) = self.size;
        self.device
            .read_bytes(self.texture(), Region::new(0, 0, w, h))
            .map_err(gpu)
    }
}

impl Drop for WindowTexture {
    fn drop(&mut self) {
        if let Some(t) = self.texture.take() {
            self.device.destroy_texture(t);
        }
    }
}

impl<R: GlyphRasterizer> GpuRenderer<R> {
    /// A renderer on `device` (current on this thread), its programs made now.
    pub fn new(
        device: Rc<Device>,
        config: GpuConfig,
        rasterizer: R,
    ) -> Result<GpuRenderer<R>, GpuError> {
        let programs = Rc::new(GpuPrograms::new(&device)?);
        Ok(Self::with_programs(device, programs, config, rasterizer))
    }

    /// A renderer on `device` (current on this thread) drawing with `programs`, made on the same device.
    pub fn with_programs(
        device: Rc<Device>,
        programs: Rc<GpuPrograms>,
        config: GpuConfig,
        rasterizer: R,
    ) -> GpuRenderer<R> {
        GpuRenderer {
            device,
            rasterizer,
            config,
            programs: Some(programs),
            window: None,
            size: (0, 0),
            scale: 1.0,
            valid: false,
            coverage: Atlas::new(AtlasKind::Coverage),
            color: Atlas::new(AtlasKind::Color),
            masks: Atlas::new(AtlasKind::PathMask),
            glyphs: HashMap::new(),
            images: HashMap::new(),
            pool: TargetPool::new(TARGET_POOL_CAP),
            uniforms: None,
            instances: None,
            frames: 0,
            next_id: 1,
            frame: GpuFrame {
                id: RenderFrameId(0),
                width: 0,
                height: 0,
                damage: Vec::new(),
            },
            stats: GpuStats::default(),
            scrolls: HashMap::new(),
            kept: HashMap::new(),
        }
    }

    /// The device the renderer draws on.
    pub fn device(&self) -> &Rc<Device> {
        &self.device
    }

    /// What the last frame did.
    pub fn stats(&self) -> &GpuStats {
        &self.stats
    }

    /// The texture holding the latest frame (premultiplied RGBA, the frame's size), once a frame was drawn.
    pub fn texture(&self) -> Option<&Texture> {
        self.window
            .as_deref()
            .filter(|_| self.valid)
            .map(WindowTexture::texture)
    }

    /// The texture holding the latest frame, to share with its frames, once a frame was drawn.
    pub fn window_texture(&self) -> Option<Rc<WindowTexture>> {
        self.window.clone().filter(|_| self.valid)
    }

    /// The latest frame's pixels, premultiplied RGBA rows first to last.
    pub fn read_pixels(&self) -> Result<Vec<u8>, RenderError> {
        let window = self
            .texture()
            .ok_or(RenderError::UnknownFrame(self.frame.id))?;
        let (w, h) = self.size;
        self.device
            .read_bytes(window, Region::new(0, 0, w, h))
            .map_err(gpu)
    }

    /// Frees the atlases, pooled targets, image textures and kept scroll layers (a hidden window drops them),
    /// keeping the frame.
    pub fn trim(&mut self) {
        self.drop_kept();
        let device = Rc::clone(&self.device);
        self.coverage.clear(&device);
        self.color.clear(&device);
        self.masks.clear(&device);
        self.glyphs.clear();
        for (_, image) in self.images.drain() {
            device.destroy_texture(image.texture);
        }
        self.pool.trim(&mut &*device);
    }

    /// Sizes the window texture for `request`: whether the frame is drawn whole.
    fn prepare(&mut self, request: &RenderRequest<'_>) -> Result<bool, RenderError> {
        cupertino_ui_render::check_request(request)?;
        if u64::from(request.width) * u64::from(request.height) > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let size = (request.width, request.height);
        let max = self.device.info().max_texture_size;
        if size.0 > max || size.1 > max {
            return Err(RenderError::ResourceLimit);
        }
        let resized = size != self.size || request.scale != self.scale || !self.valid;
        self.valid = false;
        if size != self.size || self.window.is_none() {
            // the old texture goes with the last frame that shows it
            self.window = None;
            let texture = self
                .device
                .create_texture(&TextureDesc {
                    size,
                    format: TextureFormat::Rgba8,
                    usage: TextureUsage::TARGET,
                })
                .map_err(gpu)?;
            self.window = Some(Rc::new(WindowTexture {
                device: Rc::clone(&self.device),
                texture: Some(texture),
                size,
            }));
            self.size = size;
        }
        self.scale = request.scale;
        Ok(resized || matches!(request.damage, DamageRegion::Full))
    }
}

/// Writes `bytes` into a uniform or instance buffer, growing it as needed.
pub(crate) fn upload(
    device: &Device,
    slot: &mut Option<Buffer>,
    usage: BufferUsage,
    bytes: &[u8],
) -> Result<(), GpuError> {
    if bytes.is_empty() {
        return Ok(());
    }
    let need = bytes.len() as u64;
    if slot.as_ref().is_none_or(|b| b.size() < need) {
        if let Some(old) = slot.take() {
            device.destroy_buffer(old);
        }
        let size = need.next_power_of_two().max(64 << 10);
        *slot = Some(device.create_buffer(&BufferDesc { size, usage })?);
    }
    device.write_buffer(slot.as_ref().expect("made above"), 0, bytes)
}

impl<R> Drop for GpuRenderer<R> {
    fn drop(&mut self) {
        let device = Rc::clone(&self.device);
        // the last renderer of shared programs frees them
        if let Some(p) = self.programs.take().and_then(Rc::into_inner) {
            p.destroy(&device);
        }
        self.window = None;
        self.drop_kept();
        self.coverage.clear(&device);
        self.color.clear(&device);
        self.masks.clear(&device);
        for (_, image) in self.images.drain() {
            device.destroy_texture(image.texture);
        }
        self.pool.trim(&mut &*device);
        for b in [self.uniforms.take(), self.instances.take()]
            .into_iter()
            .flatten()
        {
            device.destroy_buffer(b);
        }
    }
}

impl<R: GlyphRasterizer> UiRenderer for GpuRenderer<R> {
    type Frame = GpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        self.stats = GpuStats::default();
        let full = self.prepare(request)?;
        let whole = PixelRect {
            x: 0,
            y: 0,
            width: request.width,
            height: request.height,
        };
        let bounds = |d: &PixelRect| Bounds {
            x0: d.x as i32,
            y0: d.y as i32,
            x1: (d.x + d.width) as i32,
            y1: (d.y + d.height) as i32,
        };
        let damage: Vec<PixelRect> = match (full, request.damage) {
            (false, DamageRegion::Rects(rects)) => {
                damage_rects(rects, request.scale, request.width, request.height)
            }
            _ => vec![whole],
        };
        let damaged: Vec<Bounds> = damage.iter().map(bounds).collect();
        let (plans, moved) = self.plan_scrolls(request, full, &damaged);
        let mut dirty = damage;
        if !full {
            // what the damage already repaints is not repainted twice
            let held = |b: &Bounds| damaged.iter().any(|d| d.intersect(b) == *b);
            dirty.extend(
                moved
                    .iter()
                    .filter(|b| !b.is_empty() && !held(b))
                    .map(|b| PixelRect {
                        x: b.x0 as u32,
                        y: b.y0 as u32,
                        width: b.width(),
                        height: b.height(),
                    }),
            );
        }
        self.frames += 1;
        if !dirty.is_empty() || plans.iter().any(|p| !p.paint.is_empty()) {
            let rects: Vec<Bounds> = dirty.iter().map(bounds).collect();
            let mut rec = record::Recording::new(full);
            let painted = record::paint(self, &mut rec, request, &rects, &plans)
                .and_then(|()| self.execute(&mut rec).map_err(gpu));
            rec.release(self);
            if painted.is_err() {
                // a kept layer may be half painted: the next frame paints each one whole
                self.drop_kept();
            }
            painted?;
        }
        self.stats.kept_layers = self.kept.len() as u32;
        self.trim_images();
        self.frame = GpuFrame {
            id: RenderFrameId(self.next_id),
            width: request.width,
            height: request.height,
            damage: dirty,
        };
        self.next_id += 1;
        self.valid = true;
        Ok(self.frame.id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&GpuFrame, RenderError> {
        if self.valid && id == self.frame.id {
            Ok(&self.frame)
        } else {
            Err(RenderError::UnknownFrame(id))
        }
    }
}

impl<R> GpuRenderer<R> {
    /// Lets every kept scroll layer go; the layers stay seen, so the next frame lays each one over whole.
    fn drop_kept(&mut self) {
        for (_, k) in self.kept.drain() {
            k.destroy(&self.device);
        }
    }

    /// What this frame does to its scroll layers ([`cupertino_ui_render::scroll_layers`]), and the window pixels it
    /// paints again for them besides the damage `damage`. A frame's damage leaves out what only moved with a scroll
    /// layer's offset: a kept layer's texture takes what stays in view moved by as many device pixels, and what comes
    /// into view and what the damage names is painted into it, so only those pixels of the content are painted again
    /// and the viewport is laid over. A layer moved by a part of a pixel, or past what its texture holds, is painted
    /// whole; one not kept paints what it showed and shows in place. A layer new to the frame has only nodes new to
    /// it, which the damage covers.
    fn plan_scrolls(
        &mut self,
        request: &RenderRequest<'_>,
        full: bool,
        damage: &[Bounds],
    ) -> (Vec<record::ScrollPlan>, Vec<Bounds>) {
        let scale = request.scale;
        let whole = Bounds::of_size(request.width, request.height);
        let mut layers: Vec<usize> = cupertino_ui_render::scroll_layers(request.list)
            .into_iter()
            .flatten()
            .collect();
        layers.sort_unstable();
        layers.dedup();
        let before = std::mem::take(&mut self.scrolls);
        let mut old = std::mem::take(&mut self.kept);
        let mut budget =
            u64::from(request.width) * u64::from(request.height) * 8 + SCROLL_SLACK_BYTES;
        let mut plans = Vec::new();
        let mut repaint = Vec::new();
        let far = f64::from(1 << 28);
        for index in layers {
            let node = &request.list.nodes[index];
            let (Some(Layer::Scroll { offset }), Some(clip)) = (node.layer, &node.clip) else {
                continue;
            };
            let viewport = geometry::clip_bounds(clip, &node.transform, scale).intersect(&whole);
            let exact = (
                (offset.x * scale).clamp(-far, far),
                (offset.y * scale).clamp(-far, far),
            );
            let shift = (exact.0.round() as i32, exact.1.round() as i32);
            let previous = before.get(&node.id).copied();
            self.scrolls.insert(node.id, (viewport, shift));
            if let Some((was, _)) = previous.filter(|(was, _)| *was != viewport) {
                repaint.push(was);
            }
            let moved = previous.is_some_and(|(was, s)| was != viewport || s != shift);
            let bytes = u64::from(viewport.width()) * u64::from(viewport.height()) * 8;
            if viewport.is_empty() || bytes > budget {
                if moved {
                    repaint.push(viewport);
                }
                continue;
            }
            let in_view = damage
                .iter()
                .map(|d| d.intersect(&viewport))
                .filter(|d| !d.is_empty());
            // a texture of the viewport's size serves again, with its content when nothing else changed
            let size = (viewport.width(), viewport.height());
            let reused = old.remove(&node.id).and_then(|k| {
                let fits = k.textures[k.current]
                    .as_ref()
                    .is_some_and(|t| t.size() == size);
                if fits {
                    Some(k)
                } else {
                    k.destroy(&self.device);
                    None
                }
            });
            let mut plan = record::ScrollPlan {
                index,
                id: node.id,
                viewport,
                moved: None,
                paint: Vec::new(),
                shift,
            };
            let kept = match reused {
                Some(mut k) => {
                    let same = !full && k.scale == scale && k.viewport == viewport;
                    let d = (exact.0 - k.shift.0, exact.1 - k.shift.1);
                    let whole_px = |v: f64| (v - v.round()).abs() < 1e-6;
                    let step = (d.0.round() as i64, d.1.round() as i64);
                    let within = step.0.unsigned_abs() < u64::from(viewport.width())
                        && step.1.unsigned_abs() < u64::from(viewport.height());
                    if !(same && whole_px(d.0) && whole_px(d.1) && within) {
                        plan.paint.push(viewport);
                        repaint.push(viewport);
                    } else if step != (0, 0) {
                        let step = (step.0 as i32, step.1 as i32);
                        let stays = viewport.intersect(&viewport.offset(-step.0, -step.1));
                        plan.moved = Some(step);
                        plan.paint = geometry::subtract(viewport, &[stays]);
                        plan.paint.extend(in_view);
                        repaint.push(viewport);
                    } else {
                        // it stays where it was: only the damage is painted again and laid over
                        plan.paint.extend(in_view);
                    }
                    k.shift = exact;
                    k.scale = scale;
                    k.viewport = viewport;
                    k
                }
                None => {
                    let Ok(texture) = self.device.create_texture(&TextureDesc {
                        size,
                        format: TextureFormat::Rgba8,
                        usage: TextureUsage::TARGET,
                    }) else {
                        if moved {
                            repaint.push(viewport);
                        }
                        continue;
                    };
                    self.stats.targets_made += 1;
                    plan.paint.push(viewport);
                    if previous.is_some() {
                        repaint.push(viewport);
                    }
                    KeptScroll {
                        textures: [Some(texture), None],
                        current: 0,
                        scale,
                        viewport,
                        shift: exact,
                    }
                }
            };
            budget -= bytes;
            self.kept.insert(node.id, kept);
            plans.push(plan);
        }
        for (_, k) in old.drain() {
            k.destroy(&self.device);
        }
        (plans, repaint)
    }

    /// Drops image textures past the budget, those used least recently first, never one of this frame.
    fn trim_images(&mut self) {
        let bytes = |t: &ImageTexture| {
            let (w, h) = t.texture.size();
            u64::from(w) * u64::from(h) * 4
        };
        let mut total: u64 = self.images.values().map(bytes).sum();
        while total > self.config.image_bytes {
            let Some((&id, _)) = self
                .images
                .iter()
                .filter(|(_, t)| t.used < self.frames)
                .min_by_key(|(_, t)| t.used)
            else {
                break;
            };
            let image = self.images.remove(&id).expect("found above");
            total -= bytes(&image);
            self.device.destroy_texture(image.texture);
        }
    }
}

/// A [`GpuRenderer`] as a window's renderer: its frames are [`CpuFrame`]s whose image stays on the device
/// ([`CpuFrame::device`] holds the [`WindowTexture`]) and whose `pixels` are empty. A presenter on the same device
/// draws from the texture; anything else reads it back ([`CpuFrame::in_memory`]).
pub struct WindowRenderer<R> {
    inner: GpuRenderer<R>,
    frame: Option<CpuFrame>,
}

impl<R> WindowRenderer<R> {
    pub fn new(inner: GpuRenderer<R>) -> WindowRenderer<R> {
        WindowRenderer { inner, frame: None }
    }

    /// The renderer that draws the frames.
    pub fn renderer(&self) -> &GpuRenderer<R> {
        &self.inner
    }
}

impl<R: GlyphRasterizer> UiRenderer for WindowRenderer<R> {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        // the last frame lets go of its texture first, so a resize frees it before the next is made
        self.frame = None;
        let id = self.inner.render(request)?;
        let damage = self.inner.frame(id)?.damage.clone();
        let window = self
            .inner
            .window_texture()
            .ok_or(RenderError::UnknownFrame(id))?;
        let (width, height) = window.size();
        self.frame = Some(CpuFrame {
            id,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Vec::new(),
            damage,
            device: Some(DeviceFrameRef(window)),
        });
        Ok(id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        self.frame
            .as_ref()
            .filter(|f| f.id == id)
            .ok_or(RenderError::UnknownFrame(id))
    }

    fn drop_layers(&mut self) {
        self.inner.trim();
    }
}
