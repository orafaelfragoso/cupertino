//! A device for the tests, both renderers, and the comparison of their frames.
//!
//! The tests run on the first render node through GBM, or on the surfaceless platform without one or with
//! `LIBGL_ALWAYS_SOFTWARE` set (llvmpipe, as CI runs them). Without any EGL device a test is skipped with a message,
//! unless `CUPERTINO_EGL_REQUIRED=1` (CI) or `CUPERTINO_GPU_REQUIRED=1` (the local GPU tier) makes that a failure;
//! the latter also fails on a software device.

use std::path::PathBuf;
use std::rc::Rc;

use cupertino_gpu::{Device, DeviceDesc, Platform};
use cupertino_ui_gpu::{GpuConfig, GpuRenderer};
use cupertino_ui_render::{
    CpuConfig, CpuRenderer, DamageRegion, DisplayList, PaintResources, RenderRequest, UiRenderer,
};
use cupertino_ui_text::GlyphRasterizer;

/// The most a channel of one renderer's pixel may differ from the other's.
pub const TOLERANCE: u8 = 2;

pub fn gpu_required() -> bool {
    std::env::var("CUPERTINO_GPU_REQUIRED").as_deref() == Ok("1")
}

fn egl_required() -> bool {
    gpu_required() || std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() == Ok("1")
}

fn render_node() -> Option<PathBuf> {
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_some() {
        return None;
    }
    let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().next()
}

/// A device for a test, or None (with a message) when the machine has none and none is required.
pub fn device() -> Option<Rc<Device>> {
    let mut why = String::new();
    if let Some(node) = render_node() {
        match Device::new(&DeviceDesc {
            platform: Platform::RenderNode(node.clone()),
        }) {
            Ok(d) => return Some(checked(d)),
            Err(e) => {
                assert!(
                    !gpu_required(),
                    "CUPERTINO_GPU_REQUIRED=1: no device on {}: {e}",
                    node.display()
                );
                why = format!("{}: {e}; ", node.display());
            }
        }
    }
    match Device::new(&DeviceDesc {
        platform: Platform::Surfaceless,
    }) {
        Ok(d) => Some(checked(d)),
        Err(e) => {
            assert!(
                !egl_required(),
                "no EGL device and CUPERTINO_EGL_REQUIRED=1: {why}{e}"
            );
            eprintln!(
                "SKIPPED: no EGL device ({why}{e}); set CUPERTINO_EGL_REQUIRED=1 to fail instead"
            );
            None
        }
    }
}

fn checked(d: Device) -> Rc<Device> {
    eprintln!("device: {}", d.info().renderer);
    if gpu_required() {
        assert!(
            !d.info().software,
            "CUPERTINO_GPU_REQUIRED=1 but the device is {}",
            d.info().renderer
        );
    }
    Rc::new(d)
}

pub fn cpu<R: GlyphRasterizer>(rasterizer: R) -> CpuRenderer<R> {
    CpuRenderer::new(
        CpuConfig {
            max_pixels: 1 << 24,
            glyph_cache_bytes: 1 << 22,
            image_cache_bytes: 1 << 22,
            font_smoothing: true,
        },
        rasterizer,
    )
}

pub fn gpu<R: GlyphRasterizer>(device: &Rc<Device>, rasterizer: R) -> GpuRenderer<R> {
    GpuRenderer::new(
        Rc::clone(device),
        GpuConfig {
            max_pixels: 1 << 24,
            ..GpuConfig::default()
        },
        rasterizer,
    )
    .expect("the renderer's programs")
}

/// One frame request.
pub struct Frame<'a> {
    pub list: &'a DisplayList,
    pub resources: &'a PaintResources,
    pub scale: f64,
    pub size: (u32, u32),
    pub damage: &'a DamageRegion,
}

impl Frame<'_> {
    pub fn request(&self) -> RenderRequest<'_> {
        RenderRequest {
            list: self.list,
            resources: self.resources,
            scale: self.scale,
            width: self.size.0,
            height: self.size.1,
            damage: self.damage,
        }
    }
}

pub fn draw_cpu<R: GlyphRasterizer>(r: &mut CpuRenderer<R>, frame: &Frame<'_>) -> Vec<u8> {
    let id = r.render(&frame.request()).expect("the CPU frame");
    r.frame(id).unwrap().pixels.clone()
}

pub fn draw_gpu<R: GlyphRasterizer>(r: &mut GpuRenderer<R>, frame: &Frame<'_>) -> Vec<u8> {
    r.render(&frame.request()).expect("the GPU frame");
    r.read_pixels().expect("the GPU frame's pixels")
}

/// The pixels beyond the tolerance, the largest channel difference, and the first pixel beyond with both its values.
pub type Difference = (usize, u8, Option<(u32, u32, [u8; 4], [u8; 4])>);

/// The pixels beyond the tolerance and the largest channel difference, and the first pixel beyond it.
pub fn difference(a: &[u8], b: &[u8], width: u32) -> Difference {
    assert_eq!(a.len(), b.len());
    let mut beyond = 0;
    let mut largest = 0;
    let mut first = None;
    for (i, (p, q)) in a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .enumerate()
    {
        let delta = p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
        largest = largest.max(delta);
        if delta > TOLERANCE {
            beyond += 1;
            if first.is_none() {
                first = Some((i as u32 % width, i as u32 / width, *p, *q));
            }
        }
    }
    (beyond, largest, first)
}
