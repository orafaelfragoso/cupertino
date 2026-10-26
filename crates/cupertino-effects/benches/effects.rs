//! Minimal dependency-free benchmark of the per-frame effect paths.
use cupertino_effects::raster::{draw_mesh, window_test_pattern};
use cupertino_effects::*;
use cupertino_geometry::Rect;
use std::hint::black_box;
use std::time::Instant;
use tiny_skia::Pixmap;

fn bench<F: FnMut(usize) -> f64>(name: &str, n: usize, mut f: F) {
    let t0 = Instant::now();
    let mut acc = 0.0;
    for i in 0..n {
        acc += f(i);
    }
    let dt = t0.elapsed().as_secs_f64();
    black_box(acc);
    println!(
        "{name:<44} {:>12.1} ns/op  ({:.2} M op/s)",
        dt * 1e9 / n as f64,
        n as f64 / dt / 1e6
    );
}

fn main() {
    let window = Rect::new(300.0, 100.0, 1200.0, 800.0);
    let slot = Rect::new(1200.0, 1000.0, 64.0, 64.0);
    let params = WarpParams::default();
    let p = |i: usize| (i % 1000) as f64 / 999.0;

    bench("warp geometry setup", 200_000, |_| {
        WarpGeometry::new(black_box(window), slot, WarpDirection::Down, &params).slide
    });
    let g = WarpGeometry::new(window, slot, WarpDirection::Down, &params);
    bench("warp mesh (per frame, 1200x800 window)", 200_000, |i| {
        g.mesh(p(i)).vertex_count() as f64
    });
    bench("warp subdivision only", 500_000, |i| {
        subdivide(800.0, 600.0 * p(i), &params).len() as f64
    });
    let fp = FunnelParams::default();
    bench("funnel mesh (12x8 cells)", 200_000, |i| {
        funnel_mesh(window, slot, p(i), &fp).vertex_count() as f64
    });
    let big = Rect::new(0.0, 0.0, 3840.0, 2160.0);
    bench("funnel mesh (19x19 cells, budget case)", 100_000, |i| {
        funnel_mesh(big, slot, p(i), &fp).vertex_count() as f64
    });
    bench("scale mesh", 5_000_000, |i| {
        scale_mesh(window, slot, p(i), Ease::Cosine).vertex_count() as f64
    });
    let e = MinimizeEffect::new(MinimizeKind::Warp, window, slot, DockEdge::Bottom);
    let mut anim = MinimizeAnimation::new(e, false);
    anim.minimize(0.0);
    bench("minimize animation mesh at time t", 200_000, |i| {
        anim.mesh(p(i) * 0.5).vertex_count() as f64
    });
    let sd = ShowDesktop::default();
    let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    bench("show desktop placement", 5_000_000, |i| {
        sd.placement(window, screen, p(i)).opacity
    });

    let tex = window_test_pattern(600, 400);
    let mut dst = Pixmap::new(960, 540).unwrap();
    let mesh = MinimizeEffect::new(
        MinimizeKind::Warp,
        Rect::new(150.0, 50.0, 600.0, 400.0),
        Rect::new(600.0, 480.0, 32.0, 32.0),
        DockEdge::Bottom,
    )
    .mesh(0.4);
    bench("CPU raster of a warp mesh (960x540)", 50, |_| {
        draw_mesh(&mut dst, &tex, &mesh, (0.0, 0.0), 1.0);
        dst.data()[0] as f64
    });
}
