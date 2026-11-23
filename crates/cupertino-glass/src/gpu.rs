//! Headless helpers for the binary, tests and benchmarks: a `cupertino-gpu` device of its own, texture upload and
//! read-back, the glass composite drawn on its own, a procedural wallpaper.

use crate::glass::GlassUniform;
use crate::ops::Placement;
use crate::renderer::{FrameDesc, MaterialRegion, MaterialRenderer, TargetInit};
use crate::shaders;
use cupertino_gpu::{
    AddressMode, BindGroupEntry, BindingKind, BindingLayout, BindingResource, Blend, BufferDesc,
    BufferUsage, Device, DeviceDesc, Filter, LoadOp, PipelineDesc, Platform, Region, Result,
    SamplerDesc, Texture, TextureDesc, TextureFormat, TextureUsage, Topology,
};

pub struct HeadlessGpu {
    pub device: Device,
    pub adapter_name: String,
}

impl HeadlessGpu {
    /// A device on the surfaceless platform; `None` when there is none (no EGL or GLES 3.0).
    pub fn new() -> Option<Self> {
        let device = Device::new(&DeviceDesc {
            platform: Platform::Surfaceless,
        })
        .ok()?;
        let adapter_name = device.info().renderer.clone();
        Some(Self {
            device,
            adapter_name,
        })
    }

    /// Upload an RGBA8 image as a sampled, non-sRGB texture.
    pub fn upload(&self, img: &image::RgbaImage) -> Result<Texture> {
        let (w, h) = img.dimensions();
        let texture = self.device.create_texture(&TextureDesc {
            size: (w, h),
            format: TextureFormat::Rgba8,
            usage: TextureUsage::SAMPLED,
        })?;
        self.device
            .write_texture(&texture, Region::new(0, 0, w, h), img.as_raw())?;
        Ok(texture)
    }

    /// RGBA8 render target that can be sampled and read back.
    pub fn target(&self, w: u32, h: u32) -> Result<Texture> {
        self.device.create_texture(&TextureDesc {
            size: (w, h),
            format: TextureFormat::Rgba8,
            usage: TextureUsage::TARGET,
        })
    }

    /// The pixels of a render target, first row first.
    pub fn read(&self, tex: &Texture) -> Result<image::RgbaImage> {
        let (w, h) = tex.size();
        let bytes = self.device.read_bytes(tex, Region::new(0, 0, w, h))?;
        Ok(image::RgbaImage::from_raw(w, h, bytes).expect("a whole image"))
    }

    /// One-shot helper: render `regions` over `wall` and read the result back.
    pub fn render_once(
        &self,
        wall: &image::RgbaImage,
        regions: &[MaterialRegion],
        display_scale: f32,
        tint: [f32; 3],
    ) -> Result<image::RgbaImage> {
        let (w, h) = wall.dimensions();
        let src = self.upload(wall)?;
        let dst = self.target(w, h)?;
        let mut r = MaterialRenderer::new(&self.device)?;
        let drawn = r.render(
            &self.device,
            &FrameDesc {
                source: &src,
                source_size: (w, h),
                target: &dst,
                target_size: (w, h),
                display_scale,
                damage_generation: 1,
                tint_color: tint,
                init: TargetInit::CopySource,
            },
            regions,
        );
        let img = drawn.and_then(|_| self.read(&dst));
        r.destroy(&self.device);
        self.device.destroy_texture(src);
        self.device.destroy_texture(dst);
        img
    }
}

impl HeadlessGpu {
    /// The glass composite of one container ([`shaders::GLASS`]) drawn once over an opaque black target `size` pixels:
    /// `textures` are what the program reads at its texture bindings, in order (from binding 1), and
    /// `uniform` maps target pixels to their coordinates. The target's colour is the glass's straight colour times its
    /// coverage, as the compositor's blend lays it over black.
    pub fn render_glass(
        &self,
        uniform: &GlassUniform,
        textures: &[image::RgbaImage],
        size: (u32, u32),
    ) -> Result<image::RgbaImage> {
        let device = &self.device;
        let uploaded = textures
            .iter()
            .map(|t| self.upload(t))
            .collect::<Result<Vec<_>>>()?;
        let mut kinds = vec![BindingKind::UniformBuffer];
        kinds.extend(uploaded.iter().map(|_| BindingKind::Texture));
        kinds.push(BindingKind::Sampler);
        let entries: Vec<BindingLayout> = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| BindingLayout {
                binding: i as u32,
                kind,
            })
            .collect();
        let layout_glass = device.create_bind_group_layout(&entries)?;
        let layout_place = device.create_bind_group_layout(&[BindingLayout {
            binding: 0,
            kind: BindingKind::UniformBuffer,
        }])?;
        let pipeline = device.create_pipeline(&PipelineDesc {
            shader: &shaders::GLASS.source,
            layouts: &[&layout_glass, &layout_place],
            vertex_buffers: &[],
            blend: Blend::StraightOver,
            topology: Topology::TriangleStrip,
        })?;
        let sampler = device.create_sampler(&SamplerDesc {
            filter: Filter::Linear,
            address: AddressMode::ClampToEdge,
        })?;
        let (w, h) = (size.0 as f32, size.1 as f32);
        let projection = [2.0 / w, 0.0, 0.0, 0.0, 2.0 / h, 0.0, -1.0, -1.0, 1.0];
        let place = Placement::new(&projection, [0.0, 0.0, w, h], false);
        let mut buffers = Vec::new();
        for bytes in [bytemuck::bytes_of(uniform), bytemuck::bytes_of(&place)] {
            let buffer = device.create_buffer(&BufferDesc {
                size: bytes.len() as u64,
                usage: BufferUsage::Uniform,
            })?;
            device.write_buffer(&buffer, 0, bytes)?;
            buffers.push(buffer);
        }
        fn whole(b: &cupertino_gpu::Buffer) -> BindingResource<'_> {
            BindingResource::Buffer {
                buffer: b,
                offset: 0,
                size: b.size(),
            }
        }
        let mut group = vec![BindGroupEntry {
            binding: 0,
            resource: whole(&buffers[0]),
        }];
        for (i, t) in uploaded.iter().enumerate() {
            group.push(BindGroupEntry {
                binding: i as u32 + 1,
                resource: BindingResource::Texture(t),
            });
        }
        group.push(BindGroupEntry {
            binding: uploaded.len() as u32 + 1,
            resource: BindingResource::Sampler(&sampler),
        });
        let g0 = device.create_bind_group(&layout_glass, &group)?;
        let g1 = device.create_bind_group(
            &layout_place,
            &[BindGroupEntry {
                binding: 0,
                resource: whole(&buffers[1]),
            }],
        )?;
        let target = self.target(size.0, size.1)?;
        let drawn = (|| {
            let mut rp = device.begin_pass(&target, LoadOp::Clear([0.0, 0.0, 0.0, 1.0]))?;
            rp.set_pipeline(&pipeline);
            rp.set_bind_group(0, &g0)?;
            rp.set_bind_group(1, &g1)?;
            rp.draw(0..4, 1)?;
            rp.end();
            Ok(())
        })();
        let img = drawn.and_then(|()| self.read(&target));
        device.destroy_texture(target);
        for t in uploaded {
            device.destroy_texture(t);
        }
        for b in buffers {
            device.destroy_buffer(b);
        }
        device.destroy_pipeline(pipeline);
        device.destroy_sampler(sampler);
        img
    }
}

/// Procedural test wallpaper with saturated colour fields and fine detail.
pub fn procedural_wallpaper(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let u = x as f32 / w as f32;
        let v = y as f32 / h as f32;
        let mut r = 0.15 + 0.7 * u;
        let mut g = 0.25 + 0.5 * (6.0 * v + 2.0 * u).sin().abs();
        let mut b = 0.9 - 0.6 * v;
        let d = ((u - 0.7).powi(2) + (v - 0.35).powi(2)).sqrt();
        if d < 0.15 {
            r = 1.0;
            g = 0.55;
            b = 0.1;
        }
        if ((x / 12) + (y / 12)) % 2 == 0 && v > 0.75 {
            r *= 0.3;
            g *= 0.3;
            b *= 0.3;
        }
        image::Rgba([
            (r.clamp(0.0, 1.0) * 255.0) as u8,
            (g.clamp(0.0, 1.0) * 255.0) as u8,
            (b.clamp(0.0, 1.0) * 255.0) as u8,
            255,
        ])
    })
}

/// Mean colour of an image (the tint layer approximation).
pub fn average_color(img: &image::RgbaImage) -> [f32; 3] {
    let mut s = [0f64; 3];
    for p in img.pixels() {
        for i in 0..3 {
            s[i] += p[i] as f64;
        }
    }
    let n = (img.width() as f64) * (img.height() as f64) * 255.0;
    [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
}

/// PSNR in dB over all RGBA bytes (infinite for identical images).
pub fn psnr(a: &image::RgbaImage, b: &image::RgbaImage) -> f64 {
    let mse: f64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| (*x as f64 - *y as f64).powi(2))
        .sum::<f64>()
        / a.as_raw().len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}
