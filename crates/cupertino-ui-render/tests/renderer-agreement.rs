//! The GPU renderer and the CPU renderer draw the same display list to the same pixels within a tolerance, whole
//! and repainting damage, at each scale.
//!
//! A machine without a GPU device has nothing to compare; the test says so, and fails only where the environment
//! says a device must be there.

use crate::support;

use cupertino_gpu::{Device, DeviceDesc, Platform};
use cupertino_ui_render::gpu::{GpuConfig, GpuRenderer};
use cupertino_ui_render::{
    CpuConfig, CpuRenderer, DamageRegion, DisplayList, PaintResources, RenderRequest, UiRenderer,
};
use std::rc::Rc;
use support::{overlay, pixels, scenes, Patterns};

/// The scales the renderers are held to agree at.
const SCALES: [f64; 3] = [1.0, 1.5, 2.0];

/// How far apart two renderers' bytes may be in one channel.
const TOLERANCE: u8 = 2;

fn required() -> bool {
    ["CUPERTINO_GPU_REQUIRED", "CUPERTINO_EGL_REQUIRED"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|v| v == "1"))
}

fn device() -> Option<Rc<Device>> {
    match Device::new(&DeviceDesc {
        platform: Platform::Surfaceless,
    }) {
        Ok(device) => Some(Rc::new(device)),
        Err(e) => {
            assert!(!required(), "no GPU device: {e:?}");
            eprintln!("skipped: no GPU device ({e:?})");
            None
        }
    }
}

/// The frames of both renderers for one list and one damage.
struct Pair {
    cpu: CpuRenderer<Patterns>,
    gpu: GpuRenderer<Patterns>,
}

impl Pair {
    fn new(device: &Rc<Device>) -> Pair {
        Pair {
            cpu: CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 24,
                    glyph_cache_bytes: 1 << 20,
                    image_cache_bytes: 1 << 22,
                    font_smoothing: true,
                },
                Patterns,
            ),
            gpu: GpuRenderer::new(Rc::clone(device), GpuConfig::default(), Patterns).unwrap(),
        }
    }

    /// The pixels each renderer leaves for `list`.
    fn draw(
        &mut self,
        list: &DisplayList,
        resources: &PaintResources,
        scale: f64,
        damage: &DamageRegion,
    ) -> (Vec<u8>, Vec<u8>) {
        let (width, height) = pixels(scale);
        let request = RenderRequest {
            list,
            resources,
            scale,
            width,
            height,
            damage,
        };
        let id = self.cpu.render(&request).unwrap();
        let cpu = self.cpu.frame(id).unwrap().pixels.clone();
        self.gpu.render(&request).unwrap();
        (cpu, self.gpu.read_pixels().unwrap())
    }
}

/// The greatest difference in a channel, and where.
fn worst(a: &[u8], b: &[u8], width: u32) -> (u8, (u32, u32, usize)) {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .enumerate()
        .map(|(i, (x, y))| {
            (
                x.abs_diff(*y),
                (i as u32 / 4 % width, i as u32 / 4 / width, i % 4),
            )
        })
        .max_by_key(|(d, _)| *d)
        .unwrap()
}

#[test]
fn the_gpu_and_cpu_renderers_draw_every_kind_of_paint_to_the_same_pixels() {
    let Some(device) = device() else { return };
    for scene in scenes() {
        for scale in SCALES {
            let list = DisplayList {
                nodes: scene.nodes.clone(),
            };
            let (cpu, gpu) =
                Pair::new(&device).draw(&list, &scene.resources, scale, &DamageRegion::Full);
            let drawn = cpu
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|px| **px != support::GROUND)
                .count();
            assert!(drawn > 100, "{} at {scale} drew {drawn} pixels", scene.name);
            let (difference, at) = worst(&cpu, &gpu, pixels(scale).0);
            assert!(
                difference <= TOLERANCE,
                "{} at {scale}: {difference} apart at {at:?}",
                scene.name
            );
        }
    }
}

#[test]
fn the_renderers_still_agree_when_a_frame_repaints_only_its_damage() {
    let Some(device) = device() else { return };
    for scene in scenes() {
        for scale in SCALES {
            let list = DisplayList {
                nodes: scene.nodes.clone(),
            };
            let mut pair = Pair::new(&device);
            pair.draw(&list, &scene.resources, scale, &DamageRegion::Full);
            let (node, area) = overlay();
            let mut changed = list.clone();
            changed.nodes.push(node);
            let (cpu, gpu) = pair.draw(
                &changed,
                &scene.resources,
                scale,
                &DamageRegion::Rects(vec![area]),
            );
            let (difference, at) = worst(&cpu, &gpu, pixels(scale).0);
            assert!(
                difference <= TOLERANCE,
                "{} at {scale}: {difference} apart at {at:?}",
                scene.name
            );
        }
    }
}
