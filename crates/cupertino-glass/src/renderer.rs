//! `MaterialRenderer`: renders N material regions per frame with an external wgpu device.
//!
//! Pipeline (all full-screen fragment passes; the backdrop passes are the generated programs of [`crate::shaders`], the
//! region and vibrant programs are mirrored by the GLSL ES sources in `shaders/gles/`):
//!
//! ```text
//! source ─(2× box)→ capture @ backdrop scale ─(2× box pyramid)→ separable Gaussian
//!        ─→ colour filter chain ─→ cached processed backdrop
//! region quad: SDF rounded mask ─ base = backdrop (or source) ─ fill layers ─ alpha blend
//! ```
//!
//! The processed backdrop depends only on the recipe's backdrop settings, the display scale and the
//! source size, so it is shared between regions and cached across frames keyed by those settings and
//! by a caller-provided *damage generation counter*: bump the counter whenever the source content
//! (or the source texture object) changes; while it is unchanged the blur is not recomputed.

use crate::ops::{color_ops, fill_ops, Op, PassParams, RegionParams, VibrantUniform, MAX_FILLS};
use crate::vibrant::VibrantDraw;
use cupertino_geometry::{CornerStyle, Rect, RoundedRect};
use cupertino_raster_math::blur::{plan_blur, BlurPlan};
use cupertino_theme::recipe::Recipe;
use std::collections::HashMap;
use wgpu::util::DeviceExt;

const BACKDROP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MATERIAL_SHADER: &str = include_str!("shaders/material.wgsl");
const VIBRANT_SHADER: &str = include_str!("shaders/vibrant.wgsl");
const GLASS_SHADER: &str = include_str!("shaders/glass.wgsl");

pub const MATERIAL_WGSL: &str = MATERIAL_SHADER;
pub const VIBRANT_WGSL: &str = VIBRANT_SHADER;
pub const GLASS_WGSL: &str = GLASS_SHADER;

/// One material to draw: a rectangle in target pixels (y down) plus its recipe.
#[derive(Clone, Copy, Debug)]
pub struct MaterialRegion<'a> {
    /// x, y, width, height in target pixels.
    pub rect: [f32; 4],
    pub recipe: &'a Recipe,
    /// Corner radius in pixels (nominal radius; the continuous style expands it).
    pub corner_radius: f32,
    pub corner_style: CornerStyle,
    /// Multiplies the mask coverage (0..=1).
    pub opacity: f32,
}

impl<'a> MaterialRegion<'a> {
    /// Region without rounded corners and full opacity.
    pub fn new(rect: [f32; 4], recipe: &'a Recipe) -> Self {
        Self {
            rect,
            recipe,
            corner_radius: 0.0,
            corner_style: CornerStyle::Circular,
            opacity: 1.0,
        }
    }

    /// Region that takes its corner radius (points → pixels with `display_scale`) and curve from the
    /// recipe's archive, if it specifies one.
    pub fn from_recipe(rect: [f32; 4], recipe: &'a Recipe, display_scale: f32) -> Self {
        let mut r = Self::new(rect, recipe);
        if let Some(rad) = recipe.corner_radius {
            r.corner_radius = rad * display_scale;
        }
        if recipe.corner_curve == cupertino_theme::CornerCurve::Continuous {
            r.corner_style = CornerStyle::continuous();
        }
        r
    }

    pub fn with_corner(mut self, radius_px: f32, style: CornerStyle) -> Self {
        self.corner_radius = radius_px;
        self.corner_style = style;
        self
    }

    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// Resolved corner extent in pixels (after fitting to the rectangle) and superellipse exponent;
    /// shared by the GPU uniform and the CPU reference.
    pub fn shape(&self) -> (f32, f32) {
        let rr = self.rounded_rect();
        (rr.extents()[0] as f32, rr.style.exponent() as f32)
    }

    pub fn rounded_rect(&self) -> RoundedRect {
        RoundedRect::new(
            Rect::new(
                self.rect[0] as f64,
                self.rect[1] as f64,
                self.rect[2] as f64,
                self.rect[3] as f64,
            ),
            self.corner_radius.max(0.0) as f64,
            self.corner_style,
        )
    }
}

/// What the target contains before the regions are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetInit {
    /// Draw over the existing target content.
    Keep,
    /// Overwrite the target with the source first (headless rendering and tests).
    CopySource,
}

pub struct FrameDesc<'a> {
    /// Content below the materials. Must be a different texture than `target`, sampled as
    /// non-sRGB values.
    pub source: &'a wgpu::TextureView,
    pub source_size: (u32, u32),
    /// Render attachment of the renderer's target format.
    pub target: &'a wgpu::TextureView,
    pub target_size: (u32, u32),
    /// Device pixels per layout point; scales σ and the recipe corner radii.
    pub display_scale: f32,
    /// Increment when the source content or texture object changes (see module docs).
    pub damage_generation: u64,
    /// Colour of the tint layer (approximate: wallpaper average).
    pub tint_color: [f32; 3],
    pub init: TargetInit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub backdrop_hits: usize,
    pub backdrop_misses: usize,
    pub passes_encoded: usize,
    pub regions_drawn: usize,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct BackdropKey {
    src_size: (u32, u32),
    scale: u32,
    sigma: u32,
    display_scale: u32,
    ops: Vec<u32>,
}

struct PassDesc {
    pipeline: PassKind,
    bind_group: wgpu::BindGroup,
    target: wgpu::TextureView,
    _uniform: wgpu::Buffer,
}

#[derive(Clone, Copy)]
enum PassKind {
    Downsample,
    Blur,
    Color,
}

struct BackdropEntry {
    generation: u64,
    last_used: u64,
    passes: Vec<PassDesc>,
    final_view: Option<wgpu::TextureView>,
    _textures: Vec<wgpu::Texture>,
}

pub struct MaterialRenderer {
    device: wgpu::Device,
    #[allow(dead_code)]
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,
    layout_pass: wgpu::BindGroupLayout,
    layout_region: wgpu::BindGroupLayout,
    layout_vibrant: wgpu::BindGroupLayout,
    pipe_downsample: wgpu::RenderPipeline,
    pipe_blur: wgpu::RenderPipeline,
    pipe_color: wgpu::RenderPipeline,
    pipe_blit: wgpu::RenderPipeline,
    pipe_material: wgpu::RenderPipeline,
    pipe_vibrant: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    dummy_backdrop: wgpu::TextureView,
    cache: HashMap<BackdropKey, BackdropEntry>,
    frame: u64,
}

fn bgl_texture(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn bgl_sampler(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn bgl_uniform(
    binding: u32,
    vis: wgpu::ShaderStages,
    dynamic: bool,
    size: usize,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: wgpu::BufferSize::new(size as u64),
        },
        count: None,
    }
}

fn align_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

impl MaterialRenderer {
    /// Create pipelines on an external device. `target_format` is the colour format of the
    /// textures later passed as [`FrameDesc::target`] (use a non-sRGB format: blending operates on
    /// gamma-encoded values).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        // each backdrop pass is a program of its own, generated with its includes and the shared module
        let pass_module = |label: &str, source: &crate::shaders::Program| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.wgsl.into()),
            })
        };
        let downsample = pass_module("glass-downsample", &crate::shaders::DOWNSAMPLE);
        let blur = pass_module("glass-blur", &crate::shaders::BLUR);
        let color = pass_module("glass-color", &crate::shaders::COLOR);
        let blit = pass_module("glass-blit", &crate::shaders::BLIT);
        let material = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glass-material"),
            source: wgpu::ShaderSource::Wgsl(MATERIAL_SHADER.into()),
        });
        let vibrant = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glass-vibrant"),
            source: wgpu::ShaderSource::Wgsl(VIBRANT_SHADER.into()),
        });

        let both = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let layout_pass = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass-pass-layout"),
            entries: &[
                bgl_uniform(
                    0,
                    wgpu::ShaderStages::FRAGMENT,
                    false,
                    std::mem::size_of::<PassParams>(),
                ),
                bgl_texture(1),
                bgl_sampler(2),
            ],
        });
        let layout_region = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass-region-layout"),
            entries: &[
                bgl_uniform(0, both, true, std::mem::size_of::<RegionParams>()),
                bgl_texture(1),
                bgl_texture(2),
                bgl_sampler(3),
            ],
        });
        let layout_vibrant = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass-vibrant-layout"),
            entries: &[
                bgl_uniform(0, both, true, std::mem::size_of::<VibrantUniform>()),
                bgl_texture(1),
                bgl_texture(2),
                bgl_sampler(3),
            ],
        });
        let pl = |label: &str, l: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[l],
                push_constant_ranges: &[],
            })
        };
        let pl_pass = pl("glass-pass-pl", &layout_pass);
        let pl_region = pl("glass-region-pl", &layout_region);
        let pl_vibrant = pl("glass-vibrant-pl", &layout_vibrant);

        let make = |label: &str,
                    layout: &wgpu::PipelineLayout,
                    module: &wgpu::ShaderModule,
                    vs: &str,
                    fs: &str,
                    format: wgpu::TextureFormat,
                    blend: Option<wgpu::BlendState>,
                    strip: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: if strip {
                        wgpu::PrimitiveTopology::TriangleStrip
                    } else {
                        wgpu::PrimitiveTopology::TriangleList
                    },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let over = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipe_downsample = make(
            "glass-downsample",
            &pl_pass,
            &downsample,
            "vs_main",
            "fs_main",
            BACKDROP_FORMAT,
            None,
            false,
        );
        let pipe_blur = make(
            "glass-blur",
            &pl_pass,
            &blur,
            "vs_main",
            "fs_main",
            BACKDROP_FORMAT,
            None,
            false,
        );
        let pipe_color = make(
            "glass-color",
            &pl_pass,
            &color,
            "vs_main",
            "fs_main",
            BACKDROP_FORMAT,
            None,
            false,
        );
        let pipe_blit = make(
            "glass-blit",
            &pl_pass,
            &blit,
            "vs_main",
            "fs_main",
            target_format,
            None,
            false,
        );
        let pipe_material = make(
            "glass-material",
            &pl_region,
            &material,
            "vs_region",
            "fs_material",
            target_format,
            Some(over),
            true,
        );
        let pipe_vibrant = make(
            "glass-vibrant",
            &pl_vibrant,
            &vibrant,
            "vs_vibrant",
            "fs_vibrant",
            target_format,
            None,
            true,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glass-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glass-dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: BACKDROP_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &dummy,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8; 8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let dummy_backdrop = dummy.create_view(&Default::default());

        Self {
            device: device.clone(),
            queue: queue.clone(),
            target_format,
            layout_pass,
            layout_region,
            layout_vibrant,
            pipe_downsample,
            pipe_blur,
            pipe_color,
            pipe_blit,
            pipe_material,
            pipe_vibrant,
            sampler,
            dummy_backdrop,
            cache: HashMap::new(),
            frame: 0,
        }
    }

    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    /// Number of cached processed backdrops.
    pub fn cached_backdrops(&self) -> usize {
        self.cache.len()
    }

    /// Drop every cached backdrop (e.g. after a device-wide resource reset).
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    fn key_for(frame: &FrameDesc, b: &cupertino_theme::Backdrop) -> BackdropKey {
        let mut ops = Vec::new();
        for o in color_ops(&b.ops) {
            ops.extend(o.a.iter().chain(o.b.iter()).map(|f| f.to_bits()));
        }
        BackdropKey {
            src_size: frame.source_size,
            scale: b.scale.to_bits(),
            sigma: b.sigma_pt.to_bits(),
            display_scale: frame.display_scale.to_bits(),
            ops,
        }
    }

    fn texture(&self, w: u32, h: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glass-backdrop"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: BACKDROP_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    fn pass_bind_group(
        &self,
        params: &PassParams,
        src: &wgpu::TextureView,
    ) -> (wgpu::BindGroup, wgpu::Buffer) {
        let ub = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("glass-pass-params"),
                contents: bytemuck::bytes_of(params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glass-pass-bg"),
            layout: &self.layout_pass,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        (bg, ub)
    }

    /// Build the pass list (textures, uniforms, bind groups) of one backdrop for the current source.
    fn build_entry(&self, frame: &FrameDesc, b: &cupertino_theme::Backdrop) -> BackdropEntry {
        let (w, h) = frame.source_size;
        let plan: BlurPlan = plan_blur(w, h, b.scale, b.sigma_pt, frame.display_scale);
        let mut passes = Vec::new();
        let mut textures = Vec::new();
        let mut cur_view: Option<wgpu::TextureView> = None;
        let mut cur_size = (w, h);
        let push = |this: &Self,
                    kind: PassKind,
                    params: PassParams,
                    src: &wgpu::TextureView,
                    size: (u32, u32),
                    passes: &mut Vec<PassDesc>,
                    textures: &mut Vec<wgpu::Texture>|
         -> wgpu::TextureView {
            let tex = this.texture(size.0, size.1);
            let view = tex.create_view(&Default::default());
            let (bind_group, uniform) = this.pass_bind_group(&params, src);
            passes.push(PassDesc {
                pipeline: kind,
                bind_group,
                target: view.clone(),
                _uniform: uniform,
            });
            textures.push(tex);
            view
        };
        for &(lw, lh) in &plan.levels {
            let params = PassParams::new(cur_size, (lw, lh));
            let src = cur_view.as_ref().unwrap_or(frame.source);
            let v = push(
                self,
                PassKind::Downsample,
                params,
                src,
                (lw, lh),
                &mut passes,
                &mut textures,
            );
            cur_view = Some(v);
            cur_size = (lw, lh);
        }
        if plan.weights.len() > 1 {
            for dir in [[1, 0], [0, 1]] {
                let mut params = PassParams::new(cur_size, cur_size);
                params.dir_radius = [dir[0], dir[1], (plan.weights.len() - 1) as i32, 0];
                params.set_weights(&plan.weights);
                let src = cur_view.as_ref().unwrap_or(frame.source);
                let v = push(
                    self,
                    PassKind::Blur,
                    params,
                    src,
                    cur_size,
                    &mut passes,
                    &mut textures,
                );
                cur_view = Some(v);
            }
        }
        let cops = color_ops(&b.ops);
        if !cops.is_empty() {
            let mut params = PassParams::new(cur_size, cur_size);
            params.set_ops(&cops);
            let src = cur_view.as_ref().unwrap_or(frame.source);
            let v = push(
                self,
                PassKind::Color,
                params,
                src,
                cur_size,
                &mut passes,
                &mut textures,
            );
            cur_view = Some(v);
        }
        BackdropEntry {
            generation: frame.damage_generation,
            last_used: self.frame,
            passes,
            final_view: cur_view,
            _textures: textures,
        }
    }

    fn encode_entry(&self, enc: &mut wgpu::CommandEncoder, entry: &BackdropEntry) {
        for p in &entry.passes {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glass-backdrop-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &p.target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(match p.pipeline {
                PassKind::Downsample => &self.pipe_downsample,
                PassKind::Blur => &self.pipe_blur,
                PassKind::Color => &self.pipe_color,
            });
            rp.set_bind_group(0, &p.bind_group, &[]);
            rp.draw(0..3, 0..1);
        }
    }

    /// Encode the work for one frame into `enc` (the caller submits). Backdrop passes are only
    /// encoded for cache misses.
    pub fn render(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        frame: &FrameDesc,
        regions: &[MaterialRegion],
    ) -> RenderStats {
        self.frame += 1;
        let mut stats = RenderStats::default();

        let mut backdrop_views: Vec<Option<wgpu::TextureView>> = Vec::with_capacity(regions.len());
        for r in regions {
            let Some(b) = &r.recipe.backdrop else {
                backdrop_views.push(None);
                continue;
            };
            let key = Self::key_for(frame, b);
            let stale = self
                .cache
                .get(&key)
                .is_none_or(|e| e.generation != frame.damage_generation);
            if stale {
                let entry = self.build_entry(frame, b);
                self.encode_entry(enc, &entry);
                stats.passes_encoded += entry.passes.len();
                stats.backdrop_misses += 1;
                self.cache.insert(key.clone(), entry);
            } else {
                stats.backdrop_hits += 1;
            }
            let e = self.cache.get_mut(&key).expect("entry exists");
            e.last_used = self.frame;
            backdrop_views.push(Some(
                e.final_view.clone().unwrap_or_else(|| frame.source.clone()),
            ));
        }
        let frame_no = self.frame;
        self.cache.retain(|_, e| frame_no - e.last_used <= 120);

        if regions.is_empty() && frame.init == TargetInit::Keep {
            return stats;
        }

        let align = self.device.limits().min_uniform_buffer_offset_alignment as u64;
        let stride = align_up(std::mem::size_of::<RegionParams>() as u64, align);
        let mut data = vec![0u8; (stride as usize) * regions.len().max(1)];
        for (i, r) in regions.iter().enumerate() {
            let (extent, exponent) = r.shape();
            let mut p: RegionParams = bytemuck::Zeroable::zeroed();
            p.rect = r.rect;
            p.shape = [extent, exponent, r.opacity.clamp(0.0, 1.0), 1.0];
            p.info = [
                0.0,
                if r.recipe.backdrop.is_some() {
                    1.0
                } else {
                    0.0
                },
                0.0,
                0.0,
            ];
            p.sizes = [
                frame.target_size.0 as f32,
                frame.target_size.1 as f32,
                frame.source_size.0 as f32,
                frame.source_size.1 as f32,
            ];
            p.tint = [
                frame.tint_color[0],
                frame.tint_color[1],
                frame.tint_color[2],
                1.0,
            ];
            let fills: Vec<Op> = fill_ops(r.recipe);
            debug_assert!(fills.len() <= MAX_FILLS);
            p.set_fills(&fills);
            let off = i * stride as usize;
            data[off..off + std::mem::size_of::<RegionParams>()]
                .copy_from_slice(bytemuck::bytes_of(&p));
        }
        let region_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("glass-regions"),
                contents: &data,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_groups: Vec<wgpu::BindGroup> = backdrop_views
            .iter()
            .map(|bv| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("glass-region-bg"),
                    layout: &self.layout_region,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &region_buf,
                                offset: 0,
                                size: wgpu::BufferSize::new(
                                    std::mem::size_of::<RegionParams>() as u64
                                ),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(frame.source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(
                                bv.as_ref().unwrap_or(&self.dummy_backdrop),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                })
            })
            .collect();

        let blit = if frame.init == TargetInit::CopySource {
            let mut params = PassParams::new(frame.source_size, frame.target_size);
            params.sizes = [
                frame.source_size.0 as i32,
                frame.source_size.1 as i32,
                frame.target_size.0 as i32,
                frame.target_size.1 as i32,
            ];
            let (bg, ub) = self.pass_bind_group(&params, frame.source);
            Some((bg, ub))
        } else {
            None
        };

        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glass-regions-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame.target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if blit.is_some() {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some((bg, _)) = &blit {
                rp.set_pipeline(&self.pipe_blit);
                rp.set_bind_group(0, bg, &[]);
                rp.draw(0..3, 0..1);
            }
            rp.set_pipeline(&self.pipe_material);
            for (i, bg) in bind_groups.iter().enumerate() {
                rp.set_bind_group(0, bg, &[(i as u64 * stride) as u32]);
                rp.draw(0..4, 0..1);
                stats.regions_drawn += 1;
            }
        }
        stats
    }

    /// Draw template glyphs / text with the vibrant blend functions (`vibrant.wgsl`).
    ///
    /// `backdrop` must hold a copy of the current target content (the shader reads the destination
    /// colour from it and overwrites the covered pixels of `target`).
    pub fn draw_vibrant(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        backdrop: &wgpu::TextureView,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
        draws: &[VibrantDraw],
    ) {
        if draws.is_empty() {
            return;
        }
        let align = self.device.limits().min_uniform_buffer_offset_alignment as u64;
        let stride = align_up(std::mem::size_of::<VibrantUniform>() as u64, align);
        let mut data = vec![0u8; stride as usize * draws.len()];
        for (i, d) in draws.iter().enumerate() {
            let u = d.uniform(target_size);
            let off = i * stride as usize;
            data[off..off + std::mem::size_of::<VibrantUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
        }
        let buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("glass-vibrant-uniforms"),
                contents: &data,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bgs: Vec<wgpu::BindGroup> = draws
            .iter()
            .map(|d| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("glass-vibrant-bg"),
                    layout: &self.layout_vibrant,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &buf,
                                offset: 0,
                                size: wgpu::BufferSize::new(
                                    std::mem::size_of::<VibrantUniform>() as u64
                                ),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(d.glyphs),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(backdrop),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                })
            })
            .collect();
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("glass-vibrant-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_pipeline(&self.pipe_vibrant);
        for (i, bg) in bgs.iter().enumerate() {
            rp.set_bind_group(0, bg, &[(i as u64 * stride) as u32]);
            rp.draw(0..4, 0..1);
        }
    }
}
