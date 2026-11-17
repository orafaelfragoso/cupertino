//! The material renderer on a headless device: regions are drawn over the source, masked by their rounded rectangle and
//! scaled by their opacity, and the CPU reference agrees with the device.
//!
//! The device is a surfaceless EGL context; a machine without one has nothing to draw with and the tests say so.

use cupertino_geometry::CornerStyle;
use cupertino_glass::cpu;
use cupertino_glass::gpu::{psnr, HeadlessGpu};
use cupertino_glass::MaterialRegion;
use cupertino_theme::{Blend, Fill, Recipe};
use image::{Rgba, RgbaImage};

const SIZE: u32 = 96;
const WALL: Rgba<u8> = Rgba([40, 80, 120, 255]);

fn gpu() -> Option<HeadlessGpu> {
    let gpu = HeadlessGpu::new();
    if gpu.is_none() {
        eprintln!("no headless graphics device: the material renderer is not exercised");
    }
    gpu
}

/// A recipe that fills with one opaque colour and has no backdrop.
fn solid(rgb: [f32; 3]) -> Recipe {
    Recipe {
        fills: vec![Fill {
            rgba: [rgb[0], rgb[1], rgb[2], 1.0],
            blend: Blend::Normal,
            tint_layer: false,
        }],
        ..Default::default()
    }
}

fn wall() -> RgbaImage {
    RgbaImage::from_pixel(SIZE, SIZE, WALL)
}

fn close(a: Rgba<u8>, b: Rgba<u8>, tolerance: u8) -> bool {
    a.0.iter().zip(b.0).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

#[test]
fn a_region_covers_its_rounded_rectangle_and_leaves_the_rest_of_the_source() {
    let Some(gpu) = gpu() else { return };
    let recipe = solid([1.0, 0.0, 0.0]);
    let region = MaterialRegion::new([16.0, 16.0, 64.0, 64.0], &recipe)
        .with_corner(24.0, CornerStyle::Circular);
    let out = gpu.render_once(&wall(), &[region], 1.0, [0.5; 3]).unwrap();
    let red = Rgba([255, 0, 0, 255]);
    assert!(
        close(*out.get_pixel(48, 48), red, 1),
        "the middle is the fill"
    );
    assert!(
        close(*out.get_pixel(4, 4), WALL, 0),
        "outside the region is the source"
    );
    assert!(
        close(*out.get_pixel(17, 17), WALL, 2),
        "the rounded corner is cut away"
    );
    assert!(
        close(*out.get_pixel(48, 17), red, 1),
        "the straight edge is covered"
    );
}

#[test]
fn opacity_blends_the_fill_over_the_source() {
    let Some(gpu) = gpu() else { return };
    let recipe = solid([1.0, 1.0, 1.0]);
    let region =
        MaterialRegion::new([0.0, 0.0, SIZE as f32, SIZE as f32], &recipe).with_opacity(0.5);
    let out = gpu.render_once(&wall(), &[region], 1.0, [0.5; 3]).unwrap();
    let expected = Rgba([147, 167, 187, 255]);
    assert!(
        close(*out.get_pixel(48, 48), expected, 2),
        "{:?}",
        out.get_pixel(48, 48)
    );
}

#[test]
fn the_device_agrees_with_the_cpu_reference() {
    let Some(gpu) = gpu() else { return };
    let red = solid([0.9, 0.2, 0.1]);
    let blue = solid([0.1, 0.3, 0.9]);
    let regions = [
        MaterialRegion::new([8.0, 8.0, 56.0, 48.0], &red)
            .with_corner(12.0, CornerStyle::continuous()),
        MaterialRegion::new([30.0, 30.0, 60.0, 60.0], &blue)
            .with_corner(20.0, CornerStyle::Circular)
            .with_opacity(0.7),
    ];
    let source = RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        Rgba([(x * 2) as u8, (y * 2) as u8, 90, 255])
    });
    let device = gpu.render_once(&source, &regions, 1.0, [0.5; 3]).unwrap();
    let reference = cpu::render_regions(&source, &regions, 1.0, [0.5; 3], false, false);
    let quality = psnr(&device, &reference);
    assert!(
        quality > 40.0,
        "the device differs from the CPU renderer: {quality} dB"
    );
}
