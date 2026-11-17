//! Instruction counts of the CPU model of one glass sample ([`Container::shade`]): inside the face, on the straight
//! edge, in a continuous corner and in the neck between two shapes of a joined container, each one call.

use cupertino_glass::glass::{Container, GlassMode, GlassShape, Rgb};
use gungraun::prelude::*;
use std::hint::black_box;

/// A smooth backdrop: each blur level a ramp of its own across the points it is read at.
fn backdrop(level: usize, q: [f32; 2]) -> Rgb {
    let l = level as f32 * 0.05;
    [
        (q[0] / 640.0 + l).fract(),
        (q[1] / 400.0 + l).fract(),
        0.5 + 0.25 * l,
    ]
}

/// A capsule button of 120 x 36 points, and the point it is sampled at.
fn button(at: [f32; 2]) -> (Container, [f32; 2]) {
    let shape = GlassShape::capsule([40.0, 40.0, 120.0, 36.0]);
    (Container::new(vec![shape], 0.0, GlassMode::default()), at)
}

/// A panel of 320 x 200 points with continuous corners of radius 16, and the point it is sampled at.
fn panel(at: [f32; 2]) -> (Container, [f32; 2]) {
    let shape = GlassShape::new([20.0, 20.0, 320.0, 200.0], 16.0);
    (Container::new(vec![shape], 0.0, GlassMode::default()), at)
}

/// Three toolbar buttons of 44 x 32 points, 6 points apart, joined with a blend width of 12, and the point sampled.
fn toolbar(at: [f32; 2]) -> (Container, [f32; 2]) {
    let shapes = (0..3)
        .map(|i| GlassShape::capsule([20.0 + i as f32 * 50.0, 20.0, 44.0, 32.0]))
        .collect();
    (Container::new(shapes, 12.0, GlassMode::default()), at)
}

#[library_benchmark]
#[bench::button_inside(setup = button, args = ([100.5, 58.5]))]
#[bench::button_edge(setup = button, args = ([100.5, 40.5]))]
#[bench::panel_inside(setup = panel, args = ([180.5, 120.5]))]
#[bench::panel_edge(setup = panel, args = ([180.5, 20.5]))]
#[bench::panel_corner(setup = panel, args = ([24.5, 24.5]))]
#[bench::toolbar_neck(setup = toolbar, args = ([67.0, 36.5]))]
fn shade((container, at): (Container, [f32; 2])) -> (Rgb, f32) {
    black_box(black_box(&container).shade(black_box(at), &backdrop))
}

library_benchmark_group!(name = sample, benchmarks = [shade]);
main!(library_benchmark_groups = sample);
