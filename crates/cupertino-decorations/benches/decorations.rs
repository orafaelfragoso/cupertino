//! Minimal dependency-free benchmark of the rasterisation paths.
use cupertino_decorations::shadow::render_direct;
use cupertino_decorations::*;
use cupertino_geometry::{CornerStyle, Size};
use cupertino_theme::window_shadow::ShadowOption;
use cupertino_theme::Appearance;
use std::hint::black_box;
use std::time::Instant;

fn bench<F: FnMut(usize) -> f64>(name: &str, n: usize, mut f: F) {
    let t0 = Instant::now();
    let mut acc = 0.0;
    for i in 0..n {
        acc += f(i);
    }
    let dt = t0.elapsed().as_secs_f64();
    black_box(acc);
    println!("{name:<48} {:>12.1} us/op", dt * 1e6 / n as f64);
}

fn main() {
    let assets = cupertino_assets::Assets::discover();
    let mut dec = Decorations::new(DecorationConfig::builtin(Appearance::Light, TitlebarStyle::Standard), &assets).unwrap_or_else(|e| panic!("{e}"));
    let state = DecorationState { title: "Document — Draft".into(), group_hovered: true, ..Default::default() };
    for scale in [1.0, 2.0] {
        bench(&format!("titlebar 1600 pt wide at {scale}x (button+title)"), 200, |_| dec.render_titlebar(1600.0, scale, &state).width() as f64);
    }
    let layout_size = Size::new(1600.0, 900.0);
    bench("hit test", 2_000_000, |i| {
        let p = cupertino_geometry::Point::new((i % 1700) as f64 - 50.0, (i % 1000) as f64 - 50.0);
        match dec.hit_test(layout_size, p, true) {
            Hit::None => 0.0,
            _ => 1.0,
        }
    });
    let params = ShadowParams::lookup(ShadowOption::Standard, true, false, false);
    bench("shadow direct render 1600x900 at 1x", 5, |_| render_direct(&params, layout_size, 12.0, CornerStyle::continuous(), 1.0).width as f64);
    let mut r = ShadowRasterizer::new();
    r.render(&params, layout_size, 12.0, CornerStyle::continuous(), 1.0);
    bench("shadow nine-slice 1600x900 at 1x (cached source)", 20, |i| r.render(&params, Size::new(1600.0 + (i % 5) as f64, 900.0), 12.0, CornerStyle::continuous(), 1.0).width as f64);
    bench("shadow nine-slice 1600x900 at 2x (cached source)", 5, |i| {
        if i == 0 {
            r.render(&params, layout_size, 12.0, CornerStyle::continuous(), 2.0);
        }
        r.render(&params, layout_size, 12.0, CornerStyle::continuous(), 2.0).width as f64
    });
}
