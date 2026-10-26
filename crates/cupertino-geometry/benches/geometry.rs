//! Minimal dependency-free benchmark for the per-frame geometry paths.
use cupertino_geometry::shadow::{gaussian_weights, ShadowSpec};
use cupertino_geometry::{Point, Rect, RoundedRect};
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
    println!("{name:<40} {:>10.1} ns/op  ({:.2} M op/s)", dt * 1e9 / n as f64, n as f64 / dt / 1e6);
}

fn main() {
    let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
    let circ = RoundedRect::circular(rect, 24.0);
    let cont = RoundedRect::continuous(rect, 24.0);
    let pt = |i: usize| Point::new((i % 97) as f64 * 0.4 + 1.0, ((i / 97) % 97) as f64 * 0.4 + 1.0);
    let n = 2_000_000;
    bench("sdf circular (corner region)", n, |i| circ.sdf(pt(i)));
    bench("sdf_approx continuous (corner region)", n, |i| cont.sdf_approx(pt(i)));
    bench("sdf exact continuous (corner region)", n / 20, |i| cont.sdf(pt(i)));
    bench("coverage_approx continuous", n, |i| cont.coverage_approx(pt(i), 1.0));
    bench("path continuous (4 segs/corner)", 2_000, |_| cont.path().segments.len() as f64);
    bench("path circular", 200_000, |_| circ.path().segments.len() as f64);
    bench("path flatten(0.05)", 20_000, |_| cont.path().flatten(0.05).len() as f64);
    bench("gaussian_weights(sigma=20)", 200_000, |i| gaussian_weights(20.0 + (i % 3) as f64).len() as f64);
    let s = ShadowSpec::from_css_blur(Point::new(0.0, 10.0), 40.0, 0.0, 0.4);
    bench("shadow rect_alpha (closed form)", n, |i| s.rect_alpha(pt(i), rect));
    let t0 = Instant::now();
    let (w, h) = (1920usize, 1080usize);
    let mut cover = 0.0;
    let big = RoundedRect::continuous(Rect::new(0.0, 0.0, w as f64, h as f64), 40.0);
    for y in 0..h {
        for x in 0..w {
            cover += big.coverage_approx(Point::new(x as f64 + 0.5, y as f64 + 0.5), 1.0);
        }
    }
    black_box(cover);
    println!("full 1920x1080 coverage raster (approx) {:?}", t0.elapsed());
}
