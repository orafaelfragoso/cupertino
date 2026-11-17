//! The committed test vectors against the definitions: every line is computed again.

use cupertino_raster_math::blur::{gaussian_weights, plan_blur, Edge, Precision};
use cupertino_raster_math::coverage::{coverage, rounded_distance};
use cupertino_raster_math::dither;
use cupertino_raster_math::shadow::{drop_shadow, inner_shadow, Caster, Shadow};
use cupertino_raster_math::vectors::{self, rows};

fn close(actual: f32, expected: f64, tolerance: f32, what: &str) {
    assert!(
        (f64::from(actual) - expected).abs() <= f64::from(tolerance),
        "{what}: {actual} against {expected}"
    );
}

#[test]
fn coverage_vectors_hold() {
    let lines = rows(vectors::COVERAGE);
    assert!(lines.len() > 50);
    for r in lines {
        let d = rounded_distance(
            [r[0] as f32, r[1] as f32],
            [r[2] as f32, r[3] as f32],
            r[4] as f32,
            r[5] as f32,
        );
        close(d, r[7], 1e-4, &format!("distance of {r:?}"));
        close(
            coverage(d, r[6] as f32),
            r[8],
            1e-4,
            &format!("coverage of {r:?}"),
        );
    }
}

#[test]
fn kernel_vectors_hold() {
    let lines = rows(vectors::KERNEL);
    assert!(!lines.is_empty());
    for r in lines {
        let w = gaussian_weights(r[0] as f32);
        assert_eq!(w.len() - 1, r[1] as usize, "radius at sigma {}", r[0]);
        assert_eq!(w.len(), r.len() - 2);
        for (i, (a, e)) in w.iter().zip(&r[2..]).enumerate() {
            close(*a, *e, 1e-6, &format!("weight {i} at sigma {}", r[0]));
        }
    }
}

#[test]
fn plan_vectors_hold() {
    let lines = rows(vectors::PLAN);
    assert!(!lines.is_empty());
    for r in lines {
        let plan = plan_blur(
            r[0] as u32,
            r[1] as u32,
            r[2] as f32,
            r[3] as f32,
            r[4] as f32,
        );
        let n = r[5] as usize;
        assert_eq!(plan.levels.len(), n, "levels of {r:?}");
        for (i, level) in plan.levels.iter().enumerate() {
            assert_eq!(
                (f64::from(level.0), f64::from(level.1)),
                (r[6 + 2 * i], r[7 + 2 * i]),
                "level {i} of {r:?}"
            );
        }
        close(plan.sigma, r[6 + 2 * n], 1e-4, "sigma");
        assert_eq!(plan.weights.len() - 1, r[7 + 2 * n] as usize, "radius");
    }
}

#[test]
fn blur_vectors_hold() {
    let lines = rows(vectors::BLUR);
    assert_eq!(lines.len(), 4 * 8 * 8);
    let plan = plan_blur(16, 16, 0.5, 6.0, 1.0);
    let pattern = vectors::pattern();
    let mut done = std::collections::HashMap::new();
    for r in lines {
        let edge = if r[0] == 0.0 {
            Edge::Clamp
        } else {
            Edge::Transparent
        };
        let precision = if r[1] == 0.0 {
            Precision::Half
        } else {
            Precision::Byte
        };
        let image = done
            .entry((r[0] as u8, r[1] as u8))
            .or_insert_with(|| pattern.blur(&plan, edge, precision));
        assert_eq!((image.width, image.height), (8, 8));
        let pixel = image.pixels[r[3] as usize * image.width + r[2] as usize];
        for c in 0..4 {
            close(pixel[c], r[4 + c], 1e-4, &format!("channel {c} of {r:?}"));
        }
    }
}

#[test]
fn shadow_vectors_hold() {
    let lines = rows(vectors::SHADOW);
    assert!(lines.len() > 30);
    for r in lines {
        let caster = Caster {
            half: [r[2] as f32, r[3] as f32],
            radius: r[4] as f32,
            extent: r[5] as f32,
            exponent: r[6] as f32,
        };
        let shadow = Shadow {
            sigma: r[7] as f32,
            offset: [r[8] as f32, r[9] as f32],
            density: r[10] as f32,
        };
        let p = [r[0] as f32, r[1] as f32];
        let px = r[11] as f32;
        close(
            drop_shadow(p, &caster, &shadow, false, px),
            r[12],
            1e-4,
            &format!("drop {r:?}"),
        );
        close(
            drop_shadow(p, &caster, &shadow, true, px),
            r[13],
            1e-4,
            &format!("cut {r:?}"),
        );
        close(
            inner_shadow(p, &caster, &shadow, px),
            r[14],
            1e-4,
            &format!("inner {r:?}"),
        );
    }
}

#[test]
fn gradient_vectors_hold() {
    let stops = vectors::gradient_stops();
    let lines = rows(vectors::GRADIENT);
    assert!(lines.len() > 30);
    for r in lines {
        let c = stops[r[0] as usize].at(r[1] as f32);
        for k in 0..4 {
            close(c[k], r[2 + k], 1e-5, &format!("channel {k} of {r:?}"));
        }
    }
}

#[test]
fn dither_vectors_hold() {
    let lines = rows(vectors::DITHER);
    assert!(lines.len() > 30);
    for r in lines {
        let (x, y, v) = (r[0] as u32, r[1] as u32, r[2] as f32);
        close(
            dither::offset(x, y),
            r[3],
            1e-6,
            &format!("offset of {r:?}"),
        );
        assert_eq!(f64::from(dither::store(v, x, y)), r[4], "stored {r:?}");
    }
}

#[test]
fn rows_skip_comments_and_blank_lines() {
    let parsed = rows("# a b\n\n  1 2.5  \n# note\n-3 4e1\n");
    assert_eq!(parsed, vec![vec![1.0, 2.5], vec![-3.0, 40.0]]);
}

#[test]
#[should_panic(expected = "not a number")]
fn rows_reject_text_in_a_number_column() {
    rows("1 x 3");
}
