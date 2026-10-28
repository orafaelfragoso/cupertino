//! What composing and lowering a frame costs compared with the builder it replaces. The deterministic part is
//! enforced: in a steady scene the migrated path makes no more large (pixel-buffer sized) allocations than the old
//! one, so a copy of window or image pixels per frame fails the test whatever the machine is doing. Timings and small
//! allocation counts are only reported; `scene_cost_report` prints them for the size matrix when run with
//! `--ignored --nocapture`, for Probe to validate.
#![cfg(feature = "scene-oracle")]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use common::desktop::Desktop;
use cupertino_server::backend::Backend;
use cupertino_server::render::legacy_scene;
use cupertino_server::render::scene::build_scene;

/// Allocations of at least this many bytes count as pixel-buffer sized.
const LARGE: usize = 64 * 1024;

thread_local! {
    static SMALL_COUNT: Cell<usize> = const { Cell::new(0) };
    static LARGE_COUNT: Cell<usize> = const { Cell::new(0) };
    static LARGE_BYTES: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= LARGE {
            LARGE_COUNT.with(|c| c.set(c.get() + 1));
            LARGE_BYTES.with(|c| c.set(c.get() + layout.size()));
        } else {
            SMALL_COUNT.with(|c| c.set(c.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= LARGE && layout.size() < LARGE {
            LARGE_COUNT.with(|c| c.set(c.get() + 1));
            LARGE_BYTES.with(|c| c.set(c.get() + new_size));
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[derive(Clone, Copy, Debug, Default)]
struct Cost {
    micros: u128,
    small: usize,
    large: usize,
    large_bytes: usize,
}

fn measure(work: impl FnOnce()) -> Cost {
    let before = (
        SMALL_COUNT.with(Cell::get),
        LARGE_COUNT.with(Cell::get),
        LARGE_BYTES.with(Cell::get),
    );
    let start = Instant::now();
    work();
    Cost {
        micros: start.elapsed().as_micros(),
        small: SMALL_COUNT.with(Cell::get) - before.0,
        large: LARGE_COUNT.with(Cell::get) - before.1,
        large_bytes: LARGE_BYTES.with(Cell::get) - before.2,
    }
}

/// The cost of one frame by the old and by the migrated builder, alternating, after warming both.
fn frame_costs(d: &mut Desktop, rounds: usize) -> (Vec<Cost>, Vec<Cost>) {
    let mut backend = d.state.backend.take().unwrap();
    let Backend::Headless(ref mut h) = backend else {
        panic!("headless backend required")
    };
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for round in 0..rounds + 3 {
        let cost = measure(|| {
            let built = d
                .state
                .with_scene(&mut h.renderer, 0, legacy_scene::build_scene);
            assert!(built.is_some());
        });
        let migrated = measure(|| {
            let built = d.state.with_scene(&mut h.renderer, 0, build_scene);
            assert!(built.is_some());
        });
        // the first rounds fill the caches of both builders
        if round >= 3 {
            old.push(cost);
            new.push(migrated);
        }
    }
    d.state.backend = Some(backend);
    (old, new)
}

fn median<T: Copy + Ord>(mut values: Vec<T>) -> T {
    values.sort();
    values[values.len() / 2]
}

#[test]
fn the_counter_sees_a_pixel_sized_allocation() {
    let cost = measure(|| drop(std::hint::black_box(vec![1u8; LARGE])));
    assert_eq!((cost.large, cost.large_bytes), (1, LARGE));
}

#[test]
fn a_steady_frame_makes_no_more_pixel_sized_allocations_than_the_old_builder() {
    for (windows, scale) in [(1, 1.0), (10, 1.0), (10, 2.0)] {
        let mut d = Desktop::new(scale);
        for _ in 0..windows {
            d.window();
        }
        d.at(5000);
        d.at(5016);
        let (old, new) = frame_costs(&mut d, 5);
        let large = |costs: &[Cost]| median(costs.iter().map(|c| c.large).collect());
        let bytes = |costs: &[Cost]| median(costs.iter().map(|c| c.large_bytes).collect());
        eprintln!(
            "{windows} windows at {scale}x: migrated {} large / {} bytes / {} small, old {} / {} / {}",
            large(&new),
            bytes(&new),
            median(new.iter().map(|c| c.small).collect()),
            large(&old),
            bytes(&old),
            median(old.iter().map(|c| c.small).collect()),
        );
        assert!(
            large(&new) <= large(&old) && bytes(&new) <= bytes(&old),
            "{windows} windows at {scale}x: {} large allocations of {} bytes, the old builder {} of {}",
            large(&new),
            bytes(&new),
            large(&old),
            bytes(&old)
        );
    }
}

#[test]
#[ignore = "opt-in measurement for Probe: cargo test --features scene-oracle --test scene_cost -- --ignored --nocapture"]
fn scene_cost_report() {
    for size in [(1920, 1080), (3840, 2160)] {
        for scale in [1.0, 2.0] {
            for windows in [1, 10, 100] {
                let mut d = Desktop::sized(size, scale);
                for _ in 0..windows {
                    d.window();
                }
                d.at(5000);
                d.at(5016);
                let (old, new) = frame_costs(&mut d, 41);
                let micros = |costs: &[Cost]| median(costs.iter().map(|c| c.micros).collect());
                let small = |costs: &[Cost]| median(costs.iter().map(|c| c.small).collect());
                let large = |costs: &[Cost]| median(costs.iter().map(|c| c.large).collect());
                println!(
                    "{}x{} scale {scale} windows {windows}: old {} us ({} small, {} large), migrated {} us ({} small, {} large)",
                    size.0,
                    size.1,
                    micros(&old),
                    small(&old),
                    large(&old),
                    micros(&new),
                    small(&new),
                    large(&new),
                );
            }
        }
    }
}
