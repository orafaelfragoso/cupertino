//! Cubic-Bézier timing functions and the cosine ease.

use cupertino_animation::{cosine_ease, CubicBezier};

const EASE: CubicBezier = CubicBezier::new(0.25, 0.1, 0.25, 1.0);
const EASE_IN_OUT: CubicBezier = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);

fn samples(f: impl Fn(f64) -> f64, n: u32) -> Vec<f64> {
    (0..=n).map(|i| f(f64::from(i) / f64::from(n))).collect()
}

#[test]
fn every_curve_runs_from_zero_to_one_and_clamps_its_input() {
    for (row, c) in [
        ("ease", EASE),
        ("ease in out", EASE_IN_OUT),
        ("linear", LINEAR),
        ("overshooting", CubicBezier::new(0.3, -0.5, 0.7, 1.5)),
    ] {
        assert_eq!(c.eval(0.0), 0.0, "{row}");
        assert_eq!(c.eval(1.0), 1.0, "{row}");
        assert_eq!(c.eval(-3.0), 0.0, "{row}");
        assert_eq!(c.eval(7.0), 1.0, "{row}");
    }
}

#[test]
fn the_standard_curves_have_their_shapes() {
    for (i, v) in samples(|x| LINEAR.eval(x), 20).into_iter().enumerate() {
        assert!((v - i as f64 / 20.0).abs() < 1e-6, "linear at {i}");
    }
    let ease = samples(|x| EASE.eval(x), 200);
    assert!(ease.windows(2).all(|w| w[1] >= w[0]), "ease is monotonic");
    assert!(EASE.eval(0.25) > 0.25, "ease leads early");
    assert!((EASE.eval(0.5) - 0.8024).abs() < 1e-3, "ease at 0.5");
    assert!((EASE_IN_OUT.eval(0.5) - 0.5).abs() < 1e-6);
    for i in 1..10 {
        let x = f64::from(i) / 20.0;
        assert!(
            (EASE_IN_OUT.eval(x) + EASE_IN_OUT.eval(1.0 - x) - 1.0).abs() < 1e-6,
            "ease in out is symmetric at {x}"
        );
    }
    assert!(EASE_IN_OUT.eval(0.2) < 0.2 && EASE_IN_OUT.eval(0.8) > 0.8);
}

#[test]
fn control_points_beyond_the_unit_square_overshoot() {
    let c = CubicBezier::new(0.3, 0.0, 0.7, 1.5);
    let peak = samples(|x| c.eval(x), 100).into_iter().fold(0.0, f64::max);
    assert!(peak > 1.0);
}

#[test]
fn cosine_ease_runs_monotonically_from_zero_to_one_and_clamps() {
    for (x, y) in [(-1.0, 0.0), (0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (2.0, 1.0)] {
        assert!((cosine_ease(x) - y).abs() < 1e-12, "x = {x}");
    }
    assert!(samples(cosine_ease, 100).windows(2).all(|w| w[1] >= w[0]));
}
