//! Instruction counts of the lock screen's frames at 2560 x 1440 points and scale 2, drawn as an application's windows
//! are, on a picture made for the bench: the first frame, whose pictures are made before it on the CPU pool, and the
//! frame of one typed character, which lays only the squares of the screen's picture under the field again.
//!
//! `cargo bench -p cupertino-login --bench instructions`.

mod scene;

use cupertino_kit::geometry::Size;
use gungraun::prelude::*;
use scene::Run;
use std::hint::black_box;

const OUTPUT: Size = Size::new(2560.0, 1440.0);
const SCALE: f64 = 2.0;

/// The lock screen opened, its first frame not drawn.
fn opened() -> Run {
    Run::open(OUTPUT, SCALE)
}

/// The lock screen drawn, with one character typed and its frame not drawn yet.
fn typed() -> Run {
    let mut run = Run::open(OUTPUT, SCALE);
    run.settle();
    run.type_char('a');
    run
}

#[library_benchmark]
#[bench::lock_2560x1440_at_2(setup = opened)]
fn first_frame(mut run: Run) -> Run {
    run.settle();
    black_box(run)
}

#[library_benchmark]
#[bench::lock_2560x1440_at_2(setup = typed)]
fn keystroke(mut run: Run) -> Run {
    run.settle();
    black_box(run)
}

library_benchmark_group!(name = frames, benchmarks = [first_frame, keystroke]);
main!(library_benchmark_groups = frames);
