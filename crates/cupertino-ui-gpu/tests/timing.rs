//! A large window's frames on both renderers, timed: a window of 1600 by 1000 points at scale 1.5 tiled with the
//! golden scenes (controls with strokes and gradients, a card with a shadow, text, clips and a faded group), drawn
//! whole and then repainting a control's worth of damage. A GPU frame is
//! timed from the request to the device finishing it, so its time holds the recording on the CPU as well. On the
//! local GPU tier (`CUPERTINO_GPU_REQUIRED=1`) a damaged frame must stay within the client's GPU budget per frame,
//! and a whole frame's recording and issuing of its draws within its CPU budget.

use std::time::{Duration, Instant};

use cupertino_geometry::Rect;
use cupertino_ui_render::{DamageRegion, DisplayList, PaintNodeId, PaintResources, UiRenderer};
use cupertino_ui_text::{GlyphRaster, TextSystem};

use crate::common::{self, Frame};
use crate::goldens::scenes;

/// The window, in points, and its scale.
const WINDOW: (f64, f64) = (1600.0, 1000.0);
const SCALE: f64 = 1.5;

/// The most a frame repainting a control's worth of damage may take on the GPU, request to finish.
const DAMAGED_BUDGET: Duration = Duration::from_millis(4);

/// The most the recording and issuing of a whole frame's draws may take on the CPU, request to return.
const WHOLE_RECORDING_BUDGET: Duration = Duration::from_millis(4);

/// The window tiled with the golden scenes, each moved to its place.
fn window(system: &TextSystem) -> DisplayList {
    let mut nodes = Vec::new();
    let (columns, rows) = (
        (WINDOW.0 / scenes::WIDTH).ceil() as u64,
        (WINDOW.1 / scenes::HEIGHT).ceil() as u64,
    );
    for row in 0..rows {
        for column in 0..columns {
            let tile = row * columns + column;
            let (dx, dy) = (column as f64 * scenes::WIDTH, row as f64 * scenes::HEIGHT);
            let list = match tile % 4 {
                0 => scenes::chrome(),
                1 => scenes::text_scene(system),
                2 => scenes::clipped(system),
                _ => scenes::faded(0.0),
            };
            for mut n in list.nodes {
                n.id = PaintNodeId(tile * 1000 + n.id.0);
                n.parent = n.parent.map(|p| PaintNodeId(tile * 1000 + p.0));
                if n.parent.is_none() {
                    n.transform.0[4] += dx;
                    n.transform.0[5] += dy;
                }
                n.bounds = Rect::new(
                    n.bounds.min_x() + dx,
                    n.bounds.min_y() + dy,
                    n.bounds.width(),
                    n.bounds.height(),
                );
                nodes.push(n);
            }
        }
    }
    DisplayList { nodes }
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
fn a_large_windows_damaged_frame_stays_within_the_gpu_budget() {
    let Some(device) = common::device() else {
        return;
    };
    let system = scenes::text();
    let list = window(&system);
    let resources = PaintResources::default();
    let size = (
        (WINDOW.0 * SCALE).round() as u32,
        (WINDOW.1 * SCALE).round() as u32,
    );
    let full = DamageRegion::Full;
    // a control's worth: a button-sized rectangle in the middle of the window
    let damaged = DamageRegion::Rects(vec![Rect::new(700.0, 480.0, 120.0, 28.0)]);
    let made = Instant::now();
    let mut gpu = common::gpu(&device, GlyphRaster::new(system.clone()));
    let made = made.elapsed();
    let mut cpu = common::cpu(GlyphRaster::new(system.clone()));
    let time = |damage: &DamageRegion, runs: usize, draw: &mut dyn FnMut(&Frame<'_>)| {
        let f = Frame {
            list: &list,
            resources: &resources,
            scale: SCALE,
            size,
            damage,
        };
        (0..runs)
            .map(|_| {
                let start = Instant::now();
                draw(&f);
                start.elapsed()
            })
            .collect::<Vec<_>>()
    };
    // what of a frame is spent before the request returns: recording, uploads and issuing the passes
    let submitted = std::cell::RefCell::new(Vec::new());
    let last_stats = std::cell::Cell::new(cupertino_ui_gpu::GpuStats::default());
    let mut on_gpu = |f: &Frame<'_>| {
        let start = Instant::now();
        gpu.render(&cupertino_ui_render::RenderRequest {
            list: f.list,
            resources: f.resources,
            scale: f.scale,
            width: f.size.0,
            height: f.size.1,
            damage: f.damage,
        })
        .expect("the GPU frame");
        submitted.borrow_mut().push(start.elapsed());
        last_stats.set(*gpu.stats());
        device.finish().expect("the frame finished");
    };
    // the first frame fills the caches and atlases
    let first_gpu = time(&full, 1, &mut on_gpu)[0];
    submitted.borrow_mut().clear();
    let whole_gpu = median(time(&full, 9, &mut on_gpu));
    let whole_submitted = median(submitted.take());
    let whole_stats = last_stats.get();
    let damaged_gpu = median(time(&damaged, 21, &mut on_gpu));
    let damaged_submitted = median(submitted.take());
    let mut on_cpu = |f: &Frame<'_>| {
        common::draw_cpu(&mut cpu, f);
    };
    let first_cpu = time(&full, 1, &mut on_cpu)[0];
    let whole_cpu = median(time(&full, 5, &mut on_cpu));
    let damaged_cpu = median(time(&damaged, 21, &mut on_cpu));
    eprintln!(
        "renderer made in {made:?} (programs compiled); {} nodes, {}x{} pixels: first frame gpu {first_gpu:?} cpu {first_cpu:?}; whole frame (median) gpu \
         {whole_gpu:?} (returned after {whole_submitted:?}) cpu {whole_cpu:?}; damaged frame (median) gpu \
         {damaged_gpu:?} (returned after {damaged_submitted:?}) cpu {damaged_cpu:?}; whole {whole_stats:?}; damaged {:?}",
        list.nodes.len(),
        size.0,
        size.1,
        gpu.stats()
    );
    // a frame's offscreen targets come from the pool and from the targets it finished with, never made anew
    assert_eq!(whole_stats.targets_made, 0, "{whole_stats:?}");
    assert!(whole_stats.layers > 100, "{whole_stats:?}");
    if common::gpu_required() {
        assert!(
            damaged_gpu < DAMAGED_BUDGET,
            "a damaged frame took {damaged_gpu:?} on the GPU"
        );
        assert!(
            whole_submitted < WHOLE_RECORDING_BUDGET,
            "a whole frame's draws took {whole_submitted:?} to record and issue"
        );
    }
}
