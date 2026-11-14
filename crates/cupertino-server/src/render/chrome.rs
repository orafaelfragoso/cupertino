//! The chrome (title bars, their window buttons, rims) drawn on the GPU from its list of draws
//! ([`cupertino_decorations::DrawList`]): the borrowed device lays the list's draws one after the other into a
//! texture of the renderer with the compositor's chrome program ([`super::passes::CHROME`]), which computes the shapes
//! from the same distance as the CPU painter and lays each over what the draws before it made with the painter's own
//! arithmetic (it reads the image below from a second texture, blending off), so both give the same bytes. The
//! buttons' glyph paths and the title's glyphs are coverage images kept in an atlas. A list is drawn again only when
//! the chrome's state changed it; drawing it costs no CPU raster and no upload, but for a path or glyph the atlas does
//! not hold yet.
//!
//! [`ChromeImage`] is one list, the texture it was drawn into on each renderer, and a CPU painting of it for a
//! renderer whose device cannot draw it. The keyboard HUDs (the input-source HUD, the shortcut notice, the accent
//! popup) are lists of rounded fills and glyph runs drawn the same way ([`super::image::ImageCache`]).

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use cupertino_decorations::coverage::{Outline, Shade};
use cupertino_decorations::palette::Rgba;
use cupertino_decorations::{Draw, DrawList};
use cupertino_gpu::atlas::{Atlas, AtlasConfig};
use cupertino_gpu::shaders::NamedBinding;
use cupertino_gpu::{
    BindGroup, BindGroupEntry, BindGroupLayout, BindingKind, BindingLayout, BindingResource, Blend,
    Buffer, BufferDesc, BufferUsage, Device, LoadOp, Pipeline, PipelineDesc, StepMode, Texture,
    TextureDesc, TextureFormat, TextureUsage, Topology, VertexAttribute, VertexFormat,
    VertexLayout,
};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::element::texture::TextureRenderBuffer;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{ContextId, Offscreen, Renderer, Texture as _};
use smithay::utils::{Rectangle, Transform};

use super::uploads::Drawer;

/// Uniform block of one draw of the chrome program (`shaders/passes/chrome.wgsl`, `Chrome`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChromeBlock {
    /// The target's size in pixels; the mode (0 a draw, 1 a copy of the texture below).
    pub frame: [f32; 4],
    /// The shape's centre and half size, in target pixels.
    pub outline: [f32; 4],
    /// The shape's corner extents: top left, top right, bottom right, bottom left.
    pub corners: [f32; 4],
    /// The corners' exponent, the width of the covered band inside the outline (0: all of it), what covers (0 the
    /// shape, 1 the coverage image), whether the paint is dithered.
    pub shape: [f32; 4],
    /// A solid paint's colour, premultiplied, 0 to 1.
    pub color: [f32; 4],
    /// Paint kind (0 solid, 1 linear) and the gradient's stop count.
    pub paint: [f32; 4],
    /// A linear gradient's start and end, in target pixels.
    pub geometry: [f32; 4],
    pub offsets: [[f32; 4]; 2],
    pub colors: [[f32; 4]; 8],
}

/// Bytes between two draws' blocks in the uniform buffer: a [`ChromeBlock`] rounded up to 256, the most any driver
/// asks a uniform range's offset to be a multiple of.
const BLOCK_STRIDE: u64 = (std::mem::size_of::<ChromeBlock>() as u64).div_ceil(256) * 256;
/// Bytes of one quad: its rectangle in target pixels and the atlas texel of its first pixel.
const INSTANCE_BYTES: u64 = 32;

/// The atlas of the chrome's coverage images (button glyph paths, title glyphs): 8-bit pages of 1024 x 512, two.
const MASKS: AtlasConfig = AtlasConfig {
    format: TextureFormat::R8,
    page_size: (1024, 512),
    max_pages: 2,
};

/// One draw of the chrome program: its block, the atlas page its coverage images lie on, its quads (which do not
/// overlap, so each pixel is laid once).
struct Item {
    block: ChromeBlock,
    page: Option<usize>,
    quads: Vec<[f32; 8]>,
}

/// What the device keeps to draw chrome: the chrome program and its layout, an empty coverage image for the draws
/// that use none, the atlas, the texture a draw writes before its quads are brought back, and the uniform and
/// instance buffers with how many blocks and quads they hold.
pub struct ChromePainter {
    layout: BindGroupLayout,
    pipeline: Pipeline,
    empty: Texture,
    atlas: Atlas,
    scratch: Option<Texture>,
    uniforms: Option<(Buffer, u64)>,
    quads: Option<(Buffer, u64)>,
    /// Lists drawn, the atlas's notion of a frame.
    lists: u64,
}

fn layout_of(device: &Device) -> cupertino_gpu::Result<BindGroupLayout> {
    let mut entries: Vec<BindingLayout> = super::passes::CHROME
        .bindings()
        .into_iter()
        .map(|b| match b {
            NamedBinding::Uniform { binding, .. } => BindingLayout {
                binding,
                kind: BindingKind::UniformBuffer,
            },
            NamedBinding::Texture { binding, .. } => BindingLayout {
                binding,
                kind: BindingKind::Texture,
            },
        })
        .collect();
    entries.sort_by_key(|e| e.binding);
    entries.dedup();
    device.create_bind_group_layout(&entries)
}

/// A straight colour premultiplied, 0 to 1.
fn premultiplied(c: Rgba) -> [f32; 4] {
    let c = c.map(|v| v.clamp(0.0, 1.0));
    let a = c[3];
    [c[0] * a, c[1] * a, c[2] * a, a]
}

/// The block of a draw over a target `frame` pixels large in a solid colour, covering by its shape.
fn solid_block(frame: (u32, u32), color: Rgba) -> ChromeBlock {
    ChromeBlock {
        frame: [frame.0 as f32, frame.1 as f32, 0.0, 0.0],
        shape: [2.0, 0.0, 0.0, 0.0],
        color: premultiplied(color),
        ..ChromeBlock::default()
    }
}

/// The block of a shape: its outline (centre, half size, the four corners and their exponent) in target pixels,
/// filled or a band inside it, in `shade`.
fn shape_block(
    frame: (u32, u32),
    outline: &Outline,
    shade: &Shade,
    stroke: Option<f32>,
) -> ChromeBlock {
    let (c, h) = (outline.centre(), outline.half());
    let mut b = solid_block(frame, [0.0; 4]);
    b.outline = [c[0], c[1], h[0], h[1]];
    b.corners = outline.extents();
    b.shape = [outline.exponent(), stroke.unwrap_or(0.0), 0.0, 0.0];
    match shade {
        Shade::Solid(color) => b.color = premultiplied(*color),
        Shade::Linear { start, end, stops } => {
            let stops = stops.as_slice();
            b.shape[3] = f32::from(u8::from(cupertino_raster_math::dither::BY_DEFAULT));
            b.paint = [1.0, stops.len() as f32, 0.0, 0.0];
            b.geometry = [start[0], start[1], end[0], end[1]];
            for (i, s) in stops.iter().enumerate().take(8) {
                b.offsets[i / 4][i % 4] = s.offset;
                b.colors[i] = s.color;
            }
        }
    }
    b
}

/// Whether two quads share a pixel.
fn overlap(a: &[f32; 8], b: &[f32; 8]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// A quad of target pixels `x0 y0 x1 y1` held inside a target of `frame`, its coverage image's first texel at
/// `atlas` moved with its first pixel; `None` when nothing of it is inside.
fn quad(frame: (u32, u32), rect: [i32; 4], atlas: (u32, u32)) -> Option<[f32; 8]> {
    let (w, h) = (frame.0 as i32, frame.1 as i32);
    let (x0, y0) = (rect[0].max(0), rect[1].max(0));
    let (x1, y1) = (rect[2].min(w), rect[3].min(h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some([
        x0 as f32,
        y0 as f32,
        x1 as f32,
        y1 as f32,
        (atlas.0 as i32 + x0 - rect[0]) as f32,
        (atlas.1 as i32 + y0 - rect[1]) as f32,
        0.0,
        0.0,
    ])
}

/// A path's coverage in a mask of whole pixels around it.
struct PathMask {
    /// The mask's first pixel, in the list's pixels.
    at: (i32, i32),
    size: (u32, u32),
    bytes: Vec<u8>,
    /// The same for the same coverage wherever it lies: the path moved by whole pixels covers the same.
    key: u64,
}

/// A path's coverage on the CPU, filled or stroked with round caps as the CPU painter lays it into an image of
/// `frame` pixels.
fn path_mask(path: &tiny_skia::Path, stroke: Option<f32>, frame: (u32, u32)) -> Option<PathMask> {
    let b = path.bounds();
    let pad = stroke.map_or(0.0, |w| w / 2.0) + 2.0;
    // held to the list's image, whose edges cut the path as they cut it on the CPU
    let (fw, fh) = (frame.0 as f32, frame.1 as f32);
    let (ox, oy) = (
        (b.left() - pad).floor().max(0.0),
        (b.top() - pad).floor().max(0.0),
    );
    let (x1, y1) = (
        (b.right() + pad).ceil().min(fw),
        (b.bottom() + pad).ceil().min(fh),
    );
    if x1 <= ox || y1 <= oy {
        return None;
    }
    let (w, h) = ((x1 - ox) as u32, (y1 - oy) as u32);
    let mut pm = tiny_skia::Pixmap::new(w, h)?;
    let moved = path
        .clone()
        .transform(tiny_skia::Transform::from_translate(-ox, -oy))?;
    let paint = cupertino_decorations::draws::solid([1.0; 4]);
    match stroke {
        Some(width) => {
            let s = tiny_skia::Stroke {
                width,
                line_cap: tiny_skia::LineCap::Round,
                ..tiny_skia::Stroke::default()
            };
            pm.stroke_path(&moved, &paint, &s, tiny_skia::Transform::identity(), None);
        }
        None => pm.fill_path(
            &moved,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        ),
    }
    let bytes: Vec<u8> = pm.data().as_chunks::<4>().0.iter().map(|p| p[3]).collect();
    let mut hasher = DefaultHasher::new();
    "path".hash(&mut hasher);
    stroke.map(f32::to_bits).hash(&mut hasher);
    (w, h).hash(&mut hasher);
    for v in moved.verbs() {
        (*v as u8).hash(&mut hasher);
    }
    for p in moved.points() {
        (p.x.to_bits(), p.y.to_bits()).hash(&mut hasher);
    }
    Some(PathMask {
        at: (ox as i32, oy as i32),
        size: (w, h),
        bytes,
        key: hasher.finish(),
    })
}

fn glyph_key(key: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    ("glyph", key).hash(&mut hasher);
    hasher.finish()
}

impl ChromePainter {
    /// The painter's program, layout and empty image on `device`, entered.
    pub fn new(device: &Device) -> cupertino_gpu::Result<ChromePainter> {
        let layout = layout_of(device)?;
        let pipeline = device.create_pipeline(&PipelineDesc {
            shader: &super::passes::CHROME,
            layouts: &[&layout],
            vertex_buffers: &[VertexLayout {
                stride: INSTANCE_BYTES as u32,
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
            }],
            blend: Blend::Replace,
            topology: Topology::TriangleStrip,
        })?;
        let empty = match device.create_texture(&TextureDesc {
            size: (1, 1),
            format: TextureFormat::R8,
            usage: TextureUsage::SAMPLED,
        }) {
            Ok(t) => t,
            Err(e) => {
                device.destroy_pipeline(pipeline);
                return Err(e);
            }
        };
        Ok(ChromePainter {
            layout,
            pipeline,
            empty,
            atlas: Atlas::with_config(MASKS),
            scratch: None,
            uniforms: None,
            quads: None,
            lists: 0,
        })
    }

    /// Release what the painter made on `device`, entered.
    pub fn destroy(mut self, device: &Device) {
        self.atlas.clear(device);
        device.destroy_pipeline(self.pipeline);
        device.destroy_texture(self.empty);
        if let Some(t) = self.scratch.take() {
            device.destroy_texture(t);
        }
        for (b, _) in [self.uniforms.take(), self.quads.take()]
            .into_iter()
            .flatten()
        {
            device.destroy_buffer(b);
        }
    }

    /// The coverage image `key` on the atlas: its page and first texel, uploaded first when the atlas lacks it.
    fn image(
        &mut self,
        device: &Device,
        key: u64,
        size: (u32, u32),
        bytes: impl FnOnce() -> Vec<u8>,
    ) -> cupertino_gpu::Result<(usize, (u32, u32))> {
        let slot = match self.atlas.get(key, self.lists) {
            Some(slot) => slot,
            None => self.atlas.insert(device, key, size, &bytes(), self.lists)?,
        };
        Ok((slot.page, (slot.region.x, slot.region.y)))
    }

    /// The list's draws as the program's: blocks, pages and quads, a run of glyphs split into draws whose quads do
    /// not overlap (each glyph is laid over the ones before it, as the CPU painter lays them).
    fn items(&mut self, device: &Device, list: &DrawList) -> cupertino_gpu::Result<Vec<Item>> {
        let frame = (list.width, list.height);
        let mut items = Vec::with_capacity(list.draws.len());
        for d in &list.draws {
            match d {
                Draw::Shape {
                    outline,
                    shade,
                    stroke,
                } => {
                    if stroke.is_some_and(|w| w <= 0.0) {
                        continue;
                    }
                    let (x0, y0, x1, y1) = outline.span(frame.0, frame.1);
                    let Some(q) = quad(frame, [x0 as i32, y0 as i32, x1 as i32, y1 as i32], (0, 0))
                    else {
                        continue;
                    };
                    items.push(Item {
                        block: shape_block(frame, outline, shade, *stroke),
                        page: None,
                        quads: vec![q],
                    });
                }
                Draw::Rect {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => {
                    let r = [
                        x.round() as i32,
                        y.round() as i32,
                        (x + width).round() as i32,
                        (y + height).round() as i32,
                    ];
                    let Some(q) = quad(frame, r, (0, 0)) else {
                        continue;
                    };
                    let mut block = solid_block(frame, *color);
                    let (hw, hh) = ((r[2] - r[0]) as f32 / 2.0, (r[3] - r[1]) as f32 / 2.0);
                    block.outline = [r[0] as f32 + hw, r[1] as f32 + hh, hw, hh];
                    items.push(Item {
                        block,
                        page: None,
                        quads: vec![q],
                    });
                }
                Draw::Path {
                    path,
                    stroke,
                    color,
                } => {
                    let Some(PathMask {
                        at,
                        size,
                        bytes,
                        key,
                    }) = path_mask(path, *stroke, frame)
                    else {
                        continue;
                    };
                    let (page, texel) = self.image(device, key, size, || bytes)?;
                    let rect = [at.0, at.1, at.0 + size.0 as i32, at.1 + size.1 as i32];
                    let Some(q) = quad(frame, rect, texel) else {
                        continue;
                    };
                    let mut block = solid_block(frame, *color);
                    block.shape[2] = 1.0;
                    items.push(Item {
                        block,
                        page: Some(page),
                        quads: vec![q],
                    });
                }
                Draw::Glyphs { glyphs, color } => {
                    let mut block = solid_block(frame, *color);
                    block.shape[2] = 1.0;
                    let first = items.len();
                    for g in glyphs {
                        let (w, h) = (g.mask.width, g.mask.height);
                        if w == 0 || h == 0 {
                            continue;
                        }
                        let (page, texel) =
                            self.image(device, glyph_key(g.key), (w, h), || g.mask.data.clone())?;
                        let rect = [g.x, g.y, g.x + w as i32, g.y + h as i32];
                        let Some(q) = quad(frame, rect, texel) else {
                            continue;
                        };
                        // the first draw of the run after every one it overlaps, on the same page
                        let last_overlapping = items[first..]
                            .iter()
                            .rposition(|i| i.quads.iter().any(|o| overlap(o, &q)));
                        let from = first + last_overlapping.map_or(0, |i| i + 1);
                        match items[from..].iter_mut().find(|i| i.page == Some(page)) {
                            Some(item) => item.quads.push(q),
                            None => items.push(Item {
                                block,
                                page: Some(page),
                                quads: vec![q],
                            }),
                        }
                    }
                }
            }
        }
        Ok(items)
    }

    /// A buffer of at least `need` elements of `each` bytes in `slot`, grown to a power of two when smaller.
    fn ensure(
        device: &Device,
        slot: &mut Option<(Buffer, u64)>,
        need: u64,
        each: u64,
        usage: BufferUsage,
    ) -> cupertino_gpu::Result<()> {
        if slot.as_ref().is_none_or(|(_, n)| *n < need) {
            if let Some((old, _)) = slot.take() {
                device.destroy_buffer(old);
            }
            let count = need.max(8).next_power_of_two();
            *slot = Some((
                device.create_buffer(&BufferDesc {
                    size: count * each,
                    usage,
                })?,
                count,
            ));
        }
        Ok(())
    }

    /// The texture a draw writes before its quads are brought back: `size`, kept between lists of that size.
    fn scratch(&mut self, device: &Device, size: (u32, u32)) -> cupertino_gpu::Result<()> {
        if self.scratch.as_ref().is_some_and(|t| t.size() == size) {
            return Ok(());
        }
        if let Some(old) = self.scratch.take() {
            device.destroy_texture(old);
        }
        self.scratch = Some(device.create_texture(&TextureDesc {
            size,
            format: TextureFormat::Rgba8,
            usage: TextureUsage {
                sampled: true,
                render_target: true,
            },
        })?);
        Ok(())
    }

    fn group(
        &self,
        device: &Device,
        uniforms: &Buffer,
        block: u64,
        image: &Texture,
        below: &Texture,
    ) -> cupertino_gpu::Result<BindGroup> {
        device.create_bind_group(
            &self.layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer {
                        buffer: uniforms,
                        offset: block * BLOCK_STRIDE,
                        size: std::mem::size_of::<ChromeBlock>() as u64,
                    },
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(image),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Texture(below),
                },
            ],
        )
    }

    /// Draw `list` into `target` (its size the list's, sampled and drawn into), cleared first, the device entered:
    /// each draw writes its quads into the scratch texture from the target below them, and the quads are copied back.
    pub fn draw(
        &mut self,
        device: &Device,
        target: &Texture,
        list: &DrawList,
    ) -> cupertino_gpu::Result<()> {
        self.lists += 1;
        let frame = (list.width, list.height);
        let items = self.items(device, list)?;
        let quads: u64 = items.iter().map(|i| i.quads.len() as u64).sum();
        // the draws' blocks, then the copy's
        let blocks = items.len() as u64 + 1;
        Self::ensure(
            device,
            &mut self.uniforms,
            blocks,
            BLOCK_STRIDE,
            BufferUsage::Uniform,
        )?;
        Self::ensure(
            device,
            &mut self.quads,
            quads.max(1),
            INSTANCE_BYTES,
            BufferUsage::Vertex,
        )?;
        self.scratch(device, frame)?;
        let mut bytes = vec![0u8; blocks as usize * BLOCK_STRIDE as usize];
        let mut instances = Vec::with_capacity(quads as usize * 8);
        let copy = ChromeBlock {
            frame: [frame.0 as f32, frame.1 as f32, 1.0, 0.0],
            ..ChromeBlock::default()
        };
        for (i, block) in items.iter().map(|i| &i.block).chain([&copy]).enumerate() {
            bytes[i * BLOCK_STRIDE as usize..][..std::mem::size_of::<ChromeBlock>()]
                .copy_from_slice(bytemuck::bytes_of(block));
        }
        for item in &items {
            for q in &item.quads {
                instances.extend_from_slice(q);
            }
        }
        let (uniforms, _) = self.uniforms.as_ref().expect("made above");
        let (quad_buffer, _) = self.quads.as_ref().expect("made above");
        let scratch = self.scratch.as_ref().expect("made above");
        device.write_buffer(uniforms, 0, &bytes)?;
        if !instances.is_empty() {
            device.write_buffer(quad_buffer, 0, bytemuck::cast_slice(&instances))?;
        }
        let copy_group = self.group(device, uniforms, items.len() as u64, &self.empty, scratch)?;
        device.begin_pass(target, LoadOp::Clear([0.0; 4]))?.end();
        let mut first = 0u64;
        for (i, item) in items.iter().enumerate() {
            let image = match item.page {
                Some(page) => self.atlas.page(page).unwrap_or(&self.empty),
                None => &self.empty,
            };
            let group = self.group(device, uniforms, i as u64, image, target)?;
            let n = item.quads.len() as u64;
            for (into, group) in [(scratch, &group), (target, &copy_group)] {
                let mut pass = device.begin_pass(into, LoadOp::Load)?;
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, group)?;
                pass.set_vertex_buffer(0, quad_buffer, first * INSTANCE_BYTES)?;
                pass.draw(0..4, n as u32)?;
                pass.end();
            }
            first += n;
        }
        Ok(())
    }
}

/// Lists painted on the CPU and drawn on the GPU so far, on this thread, by what they show (the frame statistics).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChromeCounts {
    /// Title bars and rims.
    pub painted: u64,
    pub drawn: u64,
    /// The keyboard HUDs.
    pub hud_painted: u64,
    pub hud_drawn: u64,
}

thread_local! {
    static COUNTS: Cell<ChromeCounts> = Cell::new(ChromeCounts::default());
}

/// The lists painted and drawn on this thread so far.
pub fn counts() -> ChromeCounts {
    COUNTS.with(Cell::get)
}

fn count(f: impl FnOnce(&mut ChromeCounts)) {
    COUNTS.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}

/// A list drawn into a texture of one renderer, and which state of the list it holds.
struct Drawn {
    context: ContextId<GlesTexture>,
    generation: u64,
    texture: GlesTexture,
    buffer: TextureRenderBuffer<GlesTexture>,
}

/// One title bar or rim: its list of draws, which changes with the chrome's state, the texture each renderer drew it
/// into, and a CPU painting of it for a renderer that cannot draw it.
pub struct ChromeImage {
    list: RefCell<DrawList>,
    generation: Cell<u64>,
    drawn: RefCell<Vec<Drawn>>,
    painted: RefCell<Option<(u64, MemoryRenderBuffer)>>,
    /// A GPU draw failed once already (logged once).
    failed: Cell<bool>,
    /// What it shows, for the counts.
    drawer: Drawer,
}

impl std::fmt::Debug for ChromeImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = self.list.borrow();
        f.debug_struct("ChromeImage")
            .field("size", &(list.width, list.height))
            .field("draws", &list.draws.len())
            .field("generation", &self.generation.get())
            .finish()
    }
}

/// What a renderer draws a chrome image from.
pub enum Resolved {
    Texture(TextureRenderBuffer<GlesTexture>),
    Memory(MemoryRenderBuffer),
}

impl ChromeImage {
    /// An image of `list`, which shows what `drawer` names (a title bar or rim, or a keyboard HUD).
    pub fn new(list: DrawList, drawer: Drawer) -> Rc<ChromeImage> {
        Rc::new(ChromeImage {
            list: RefCell::new(list),
            generation: Cell::new(0),
            drawn: RefCell::new(Vec::new()),
            painted: RefCell::new(None),
            failed: Cell::new(false),
            drawer,
        })
    }

    /// Replace the list: every renderer draws it again when it next shows the image.
    pub fn set(&self, list: DrawList) {
        *self.list.borrow_mut() = list;
        self.generation.set(self.generation.get() + 1);
    }

    /// The image's size in device pixels.
    pub fn size(&self) -> (i32, i32) {
        let list = self.list.borrow();
        (list.width as i32, list.height as i32)
    }

    /// The list painted on the CPU (counted), or the painting of this state already made.
    pub fn painted(&self) -> MemoryRenderBuffer {
        let generation = self.generation.get();
        let mut painted = self.painted.borrow_mut();
        if let Some((g, buffer)) = painted.as_ref() {
            if *g == generation {
                return buffer.clone();
            }
        }
        let hud = self.drawer == Drawer::Hud;
        count(|c| {
            if hud {
                c.hud_painted += 1
            } else {
                c.painted += 1
            }
        });
        let pm = self.list.borrow().paint();
        let buffer = MemoryRenderBuffer::from_slice(
            pm.data(),
            Fourcc::Abgr8888,
            (pm.width() as i32, pm.height() as i32),
            1,
            Transform::Normal,
            None,
        );
        *painted = Some((generation, buffer.clone()));
        buffer
    }

    /// What `renderer` draws the image from: the texture its device drew the list into (drawn again when the list
    /// changed since), or, when its device cannot draw it, the CPU painting.
    pub fn resolve(&self, renderer: &mut GlesRenderer, passes: &super::passes::Passes) -> Resolved {
        match self.drawn_on(renderer, passes) {
            Ok(buffer) => Resolved::Texture(buffer),
            Err(e) => {
                if !self.failed.replace(true) {
                    tracing::warn!(error = %e, "the chrome could not be drawn on the GPU; it is painted on the CPU");
                }
                Resolved::Memory(self.painted())
            }
        }
    }

    fn drawn_on(
        &self,
        renderer: &mut GlesRenderer,
        passes: &super::passes::Passes,
    ) -> Result<TextureRenderBuffer<GlesTexture>, String> {
        let context = renderer.context_id();
        let generation = self.generation.get();
        let list = self.list.borrow();
        let size = (list.width as i32, list.height as i32);
        let mut drawn = self.drawn.borrow_mut();
        let at = drawn.iter().position(|d| d.context == context);
        if let Some(i) = at {
            if drawn[i].generation == generation {
                return Ok(drawn[i].buffer.clone());
            }
        }
        // the texture this renderer drew into before, when the size is the same; otherwise a new one
        let reused = at
            .map(|i| &drawn[i].texture)
            .filter(|t| (t.size().w, t.size().h) == size)
            .cloned();
        let texture = match &reused {
            Some(t) => t.clone(),
            None => renderer
                .create_buffer(Fourcc::Abgr8888, size.into())
                .map_err(|e| format!("a texture of {} x {}: {e}", size.0, size.1))?,
        };
        let name = texture.tex_id();
        renderer
            .with_context(|_| passes.draw_list(name, (list.width, list.height), &list))
            .map_err(|e| format!("making the context current failed: {e}"))?
            .map_err(|e| e.to_string())?;
        let hud = self.drawer == Drawer::Hud;
        count(|c| if hud { c.hud_drawn += 1 } else { c.drawn += 1 });
        match (at, reused) {
            (Some(i), Some(_)) => {
                let d = &mut drawn[i];
                let whole = Rectangle::from_size(size.into());
                let _ = d.buffer.render().draw(|_| Ok::<_, ()>(vec![whole]));
                d.generation = generation;
                Ok(d.buffer.clone())
            }
            _ => {
                let buffer = TextureRenderBuffer::from_texture(
                    renderer,
                    texture.clone(),
                    1,
                    Transform::Normal,
                    None,
                );
                let d = Drawn {
                    context,
                    generation,
                    texture,
                    buffer: buffer.clone(),
                };
                match at {
                    Some(i) => drawn[i] = d,
                    None => drawn.push(d),
                }
                Ok(buffer)
            }
        }
    }
}
