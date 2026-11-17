//! Minimal dependency-free benchmark: evaluations per second for the hot paths used per frame.
use cupertino_animation::*;
use std::hint::black_box;
use std::time::Instant;

fn bench<F: FnMut(f64) -> f64>(name: &str, n: usize, mut f: F) {
    let t0 = Instant::now();
    let mut acc = 0.0;
    for i in 0..n {
        acc += f(i as f64 * 1e-4);
    }
    let dt = t0.elapsed().as_secs_f64();
    black_box(acc);
    println!(
        "{name:<32} {:>8.1} ns/eval  ({:.1} M eval/s)",
        dt * 1e9 / n as f64,
        n as f64 / dt / 1e6
    );
}

/// The underdamped spring the benchmark evaluates.
const SPRING: SpringParams = SpringParams {
    mass: 1.0,
    stiffness: 100.0,
    damping: 10.0,
    initial_velocity: 0.0,
    allows_overdamping: false,
};

fn main() {
    let n = 5_000_000;
    let s = Spring::new(SPRING);
    bench("spring.value (underdamped)", n, |t| s.value(t));
    let c = Spring::new(SpringParams {
        damping: 20.0,
        ..SPRING
    });
    bench("spring.value (critical)", n, |t| c.value(t));
    let b = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
    bench("cubic_bezier.eval", n, |t| b.eval(t.fract()));
    let m = Momentum::new(0.0, 3000.0, -1e9, 1e9);
    bench("momentum.position", n, |t| m.position(t));
    let mut a = AnimatedValue::new([0.0f64; 4]);
    a.animate_to([10.0, 20.0, 300.0, 400.0], Curve::Spring(SPRING), 0.0);
    bench("AnimatedValue<[f64;4]>.value", n / 5, |t| a.value(t)[2]);
    let t0 = Instant::now();
    let st = s.settling_duration();
    println!(
        "spring.settling_duration = {st:.4}s computed in {:?}",
        t0.elapsed()
    );
}
