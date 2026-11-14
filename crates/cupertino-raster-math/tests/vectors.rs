//! The committed vectors are what the definitions compute. A definition that changes fails here by the file it
//! changes; `CUPERTINO_WRITE_VECTORS=1` writes the files again, a change to the look that names the pixels it changes.

use cupertino_raster_math::blur::{gaussian_weights, plan_blur, Edge, Precision};
use cupertino_raster_math::coverage::{coverage, rounded_distance};
use cupertino_raster_math::shadow::{drop_shadow, inner_shadow, Caster, Shadow};
use cupertino_raster_math::{dither, vectors};

/// Largest difference allowed between a committed number and the one computed now: the platform's `exp` and `powf`
/// may round their last bit differently.
const TOLERANCE: f64 = 1e-5;

fn line(values: &[f32]) -> String {
    values
        .iter()
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn check(file: &str, committed: &str, header: &str, lines: Vec<String>) {
    let fresh = format!("{header}\n{}\n", lines.join("\n"));
    if std::env::var_os("CUPERTINO_WRITE_VECTORS").is_some() {
        let path = format!("{}/vectors/{file}", env!("CARGO_MANIFEST_DIR"));
        std::fs::write(&path, &fresh).expect("write vectors");
        return;
    }
    let (want, got) = (vectors::rows(committed), vectors::rows(&fresh));
    assert_eq!(want.len(), got.len(), "{file}: number of vectors");
    for (i, (w, g)) in want.iter().zip(&got).enumerate() {
        assert_eq!(
            w.len(),
            g.len(),
            "{file}: vector {i} has {} numbers, now {}",
            w.len(),
            g.len()
        );
        for (a, b) in w.iter().zip(g) {
            assert!(
                (a - b).abs() <= TOLERANCE * a.abs().max(1.0),
                "{file}: vector {i} is now {g:?}, committed {w:?}"
            );
        }
    }
}

#[test]
fn coverage_vectors() {
    let shapes = [
        ([40.0f32, 20.0], 0.0f32, 2.0f32),
        ([40.0, 20.0], 8.0, 2.0),
        ([40.0, 20.0], 12.2, 3.0),
        ([12.0, 12.0], 12.0, 5.0),
    ];
    let points = [
        [0.0f32, 0.0],
        [39.5, 0.0],
        [40.25, 3.0],
        [-38.0, 18.0],
        [36.0, -17.5],
        [-40.0, -20.0],
        [10.5, 11.75],
        [41.0, 21.0],
    ];
    let mut lines = vec![];
    for (half, extent, exponent) in shapes {
        for p in points {
            for px in [1.0f32, 1.5, 2.0] {
                let d = rounded_distance(p, half, extent, exponent);
                lines.push(line(&[
                    p[0],
                    p[1],
                    half[0],
                    half[1],
                    extent,
                    exponent,
                    px,
                    d,
                    coverage(d, px),
                ]));
            }
        }
    }
    check(
        "coverage.txt",
        vectors::COVERAGE,
        "# px py hx hy extent exponent pixels distance coverage",
        lines,
    );
}

#[test]
fn kernel_and_plan_vectors() {
    let kernel = [0.1f32, 0.3, 0.5, 1.0, 2.5, 4.0, 7.9, 8.0, 12.0, 40.0]
        .iter()
        .map(|&s| {
            let w = gaussian_weights(s);
            let mut v = vec![s, (w.len() - 1) as f32];
            v.extend(w);
            line(&v)
        })
        .collect();
    check(
        "kernel.txt",
        vectors::KERNEL,
        "# sigma radius w0 w1 ... wr",
        kernel,
    );
    let plan = [
        (1920u32, 1080u32, 0.5f32, 180.0f32, 1.0f32),
        (1920, 1080, 0.5, 30.0, 1.0),
        (3840, 2160, 0.5, 30.0, 1.5),
        (2560, 1600, 0.5, 20.0, 2.0),
        (1023, 641, 1.0, 10.0, 1.0),
        (100, 100, 1.0, 0.0, 1.0),
        (40, 24, 0.25, 50.0, 2.0),
    ]
    .iter()
    .map(|&(w, h, scale, sigma, display)| {
        let p = plan_blur(w, h, scale, sigma, display);
        let mut v = vec![
            w as f32,
            h as f32,
            scale,
            sigma,
            display,
            p.levels.len() as f32,
        ];
        v.extend(p.levels.iter().flat_map(|l| [l.0 as f32, l.1 as f32]));
        v.extend([p.sigma, (p.weights.len() - 1) as f32]);
        line(&v)
    })
    .collect();
    check(
        "plan.txt",
        vectors::PLAN,
        "# w h backdrop_scale sigma_pt display_scale levels (lw lh)... sigma radius",
        plan,
    );
}

#[test]
fn blur_vectors() {
    let source = vectors::pattern();
    let plan = plan_blur(16, 16, 0.5, 6.0, 1.0);
    let mut lines = vec![];
    for (e, edge) in [Edge::Clamp, Edge::Transparent].into_iter().enumerate() {
        for (p, precision) in [Precision::Half, Precision::Byte].into_iter().enumerate() {
            let out = source.blur(&plan, edge, precision);
            for (i, px) in out.pixels.iter().enumerate() {
                let (x, y) = (i % out.width, i / out.width);
                let mut v = vec![e as f32, p as f32, x as f32, y as f32];
                v.extend(px);
                lines.push(line(&v));
            }
        }
    }
    check(
        "blur.txt",
        vectors::BLUR,
        "# edge precision x y r g b a",
        lines,
    );
}

#[test]
fn shadow_vectors() {
    let casters = [
        Caster {
            half: [60.0, 40.0],
            radius: 10.0,
            extent: 10.0,
            exponent: 2.0,
        },
        Caster {
            half: [60.0, 40.0],
            radius: 12.0,
            extent: 18.3,
            exponent: 3.2,
        },
        Caster {
            half: [8.0, 30.0],
            radius: 20.0,
            extent: 20.0,
            exponent: 2.0,
        },
    ];
    let shadows = [
        Shadow {
            sigma: 4.0,
            offset: [0.0, 2.0],
            density: 1.0,
        },
        Shadow {
            sigma: 20.0,
            offset: [0.0, 12.0],
            density: 1.6,
        },
    ];
    let points = [
        [0.0f32, 0.0],
        [0.0, 39.0],
        [0.0, 46.0],
        [61.5, 0.0],
        [58.0, 38.0],
        [70.0, 55.0],
        [-9.0, -31.0],
    ];
    let mut lines = vec![];
    for c in casters {
        for s in shadows {
            for p in points {
                let px = 1.5;
                lines.push(line(&[
                    p[0],
                    p[1],
                    c.half[0],
                    c.half[1],
                    c.radius,
                    c.extent,
                    c.exponent,
                    s.sigma,
                    s.offset[0],
                    s.offset[1],
                    s.density,
                    px,
                    drop_shadow(p, &c, &s, false, px),
                    drop_shadow(p, &c, &s, true, px),
                    inner_shadow(p, &c, &s, px),
                ]));
            }
        }
    }
    check(
        "shadow.txt",
        vectors::SHADOW,
        "# px py hx hy radius extent exponent sigma ox oy density pixels drop drop_cut inner",
        lines,
    );
}

#[test]
fn gradient_vectors() {
    let mut lines = vec![];
    for (g, stops) in vectors::gradient_stops().iter().enumerate() {
        for i in -1..=12 {
            let t = i as f32 / 10.0 - 0.05;
            let mut v = vec![g as f32, t];
            v.extend(stops.at(t));
            lines.push(line(&v));
        }
    }
    check(
        "gradient.txt",
        vectors::GRADIENT,
        "# gradient t r g b a",
        lines,
    );
}

#[test]
fn dither_vectors() {
    let mut lines = vec![];
    for y in 0..dither::SIZE {
        for x in 0..dither::SIZE {
            let v = (100.0 + (x * 8 + y) as f32 / 64.0 + 1.0 / 128.0) / 255.0;
            lines.push(line(&[
                x as f32,
                y as f32,
                v,
                dither::offset(x, y),
                f32::from(dither::store(v, x, y)),
            ]));
        }
    }
    for (x, y) in [(8u32, 0u32), (9, 17), (1919, 1079), (3839, 2159)] {
        let v = 0.3;
        lines.push(line(&[
            x as f32,
            y as f32,
            v,
            dither::offset(x, y),
            f32::from(dither::store(v, x, y)),
        ]));
    }
    check(
        "dither.txt",
        vectors::DITHER,
        "# x y v offset stored",
        lines,
    );
}
