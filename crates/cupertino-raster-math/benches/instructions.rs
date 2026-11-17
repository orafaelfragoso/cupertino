//! Instruction counts of the drawing maths a frame spends its time in: the blur's plan and kernel for a 4K-sized
//! region and the blur of one, the coverage of a continuous-corner rounded rectangle over every pixel of a panel at 2x,
//! and the dithered store of a gradient over the same pixels.

use cupertino_geometry::CornerStyle;
use cupertino_raster_math::blur::{self, BlurPlan, Edge, Image, Precision};
use cupertino_raster_math::{coverage, dither};
use gungraun::prelude::*;
use std::hint::black_box;

/// A 4K output in pixels.
const UHD: (u32, u32) = (3840, 2160);
/// The scale the blur reads its backdrop at.
const BACKDROP_SCALE: f32 = 0.5;
/// The display scale the frames are drawn at.
const DISPLAY_SCALE: f32 = 2.0;
/// A panel of 360 x 240 points at 2x, in pixels.
const PANEL: (u32, u32) = (720, 480);
/// The panel's corner radius in points.
const PANEL_RADIUS: f32 = 16.0;

#[library_benchmark]
#[bench::sigma_15pt(15.0)]
#[bench::sigma_30pt(30.0)]
#[bench::sigma_90pt(90.0)]
fn plan_4k(sigma_pt: f32) -> BlurPlan {
    black_box(blur::plan_blur(
        black_box(UHD.0),
        black_box(UHD.1),
        BACKDROP_SCALE,
        black_box(sigma_pt),
        DISPLAY_SCALE,
    ))
}

#[library_benchmark]
#[bench::sigma_4(4.0)]
#[bench::widest(11.7)]
fn kernel(sigma: f32) -> Vec<f32> {
    black_box(blur::gaussian_weights(black_box(sigma)))
}

/// A 4K backdrop of smooth colour ramps, opaque, and the plan of a 30-point blur over it.
fn backdrop_4k() -> (Image, BlurPlan) {
    let (w, h) = (UHD.0 as usize, UHD.1 as usize);
    let pixels = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
            [x, y, 1.0 - x * y, 1.0]
        })
        .collect();
    let plan = blur::plan_blur(UHD.0, UHD.1, BACKDROP_SCALE, 30.0, DISPLAY_SCALE);
    (Image::new(w, h, pixels), plan)
}

#[library_benchmark]
#[bench::sigma_30pt(setup = backdrop_4k)]
fn blur_4k((image, plan): (Image, BlurPlan)) -> Image {
    black_box(black_box(&image).blur(&plan, Edge::Clamp, Precision::Half))
}

/// The coverage of every pixel of the panel, its corners continuous.
fn panel_coverage() -> f32 {
    let half = [PANEL.0 as f32 * 0.5, PANEL.1 as f32 * 0.5];
    let style = CornerStyle::continuous();
    let extent = PANEL_RADIUS * DISPLAY_SCALE * style.expansion() as f32;
    let exponent = style.exponent() as f32;
    let mut sum = 0.0;
    for y in 0..PANEL.1 {
        for x in 0..PANEL.0 {
            let p = [x as f32 + 0.5 - half[0], y as f32 + 0.5 - half[1]];
            let d = coverage::rounded_distance(black_box(p), half, extent, exponent);
            sum += coverage::coverage(d, 1.0);
        }
    }
    sum
}

#[library_benchmark]
fn rounded_rect_coverage() -> f32 {
    black_box(panel_coverage())
}

/// A diagonal gradient over the panel's pixels, each channel dithered and stored in 8 bits.
fn panel_dither() -> Vec<u8> {
    let mut out = Vec::with_capacity((PANEL.0 * PANEL.1 * 3) as usize);
    for y in 0..PANEL.1 {
        for x in 0..PANEL.0 {
            let t = (x + y) as f32 / (PANEL.0 + PANEL.1) as f32;
            for v in [t, 0.5 * t + 0.25, 1.0 - t] {
                out.push(dither::store(black_box(v), x, y));
            }
        }
    }
    out
}

#[library_benchmark]
fn dither_store() -> Vec<u8> {
    black_box(panel_dither())
}

library_benchmark_group!(name = blur_group, benchmarks = [plan_4k, kernel, blur_4k]);
library_benchmark_group!(
    name = coverage_group,
    benchmarks = [rounded_rect_coverage, dither_store]
);
main!(library_benchmark_groups = blur_group, coverage_group);
