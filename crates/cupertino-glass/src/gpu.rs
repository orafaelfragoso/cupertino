//! Headless helpers for the binary, tests and benchmarks: a `cupertino-gpu` device of its own, texture upload and
//! read-back, a procedural wallpaper.

use crate::renderer::{FrameDesc, MaterialRegion, MaterialRenderer, TargetInit};
use cupertino_gpu::{
    Device, DeviceDesc, Platform, Region, Result, Texture, TextureDesc, TextureFormat, TextureUsage,
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
