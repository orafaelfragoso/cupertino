//! Headless wgpu helpers for the binary, tests and benchmarks: adapter selection, texture upload
//! and read-back, a procedural wallpaper.

use crate::renderer::{FrameDesc, MaterialRegion, MaterialRenderer, TargetInit};
use wgpu::util::DeviceExt;

pub struct HeadlessGpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
}

impl HeadlessGpu {
    /// `None` when no adapter or device is available.
    pub fn new() -> Option<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
                .ok()?;
        Some(Self {
            device,
            queue,
            adapter_name,
        })
    }

    /// Upload an RGBA8 image as a sampled, non-sRGB texture.
    pub fn upload(&self, img: &image::RgbaImage) -> wgpu::Texture {
        let (w, h) = img.dimensions();
        self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("image"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            img.as_raw(),
        )
    }

    /// RGBA8 render target that can be sampled and read back.
    pub fn target(&self, w: u32, h: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    pub fn read(&self, tex: &wgpu::Texture) -> image::RgbaImage {
        let (w, h) = (tex.width(), tex.height());
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::Maintain::Wait);
        let data = buf.slice(..).get_mapped_range();
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            let s = &data[(y * row) as usize..(y * row + w * 4) as usize];
            img.as_mut()[(y * w * 4) as usize..((y + 1) * w * 4) as usize].copy_from_slice(s);
        }
        img
    }

    /// One-shot helper: render `regions` over `wall` and read the result back.
    pub fn render_once(
        &self,
        wall: &image::RgbaImage,
        regions: &[MaterialRegion],
        display_scale: f32,
        tint: [f32; 3],
    ) -> image::RgbaImage {
        let (w, h) = wall.dimensions();
        let src = self.upload(wall);
        let dst = self.target(w, h);
        let mut r =
            MaterialRenderer::new(&self.device, &self.queue, wgpu::TextureFormat::Rgba8Unorm);
        let (sv, dv) = (
            src.create_view(&Default::default()),
            dst.create_view(&Default::default()),
        );
        let mut enc = self.device.create_command_encoder(&Default::default());
        r.render(
            &mut enc,
            &FrameDesc {
                source: &sv,
                source_size: (w, h),
                target: &dv,
                target_size: (w, h),
                display_scale,
                damage_generation: 1,
                tint_color: tint,
                init: TargetInit::CopySource,
            },
            regions,
        );
        self.queue.submit([enc.finish()]);
        self.read(&dst)
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
