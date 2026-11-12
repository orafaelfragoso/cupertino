//! The heap a Virtual List holds once scrolled to its end, beyond its model, is the same within a fixed margin for
//! 1,000 and 50,000 rows: rows away from the viewport cost nothing. A test binary of its own, for its counting allocator.

use crate::support;

use crate::examples::virtual_list::VirtualList;
use cupertino_kit::core::Application;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use support::{Run, END, TAB};

/// Stated before measuring: what 49,000 more rows may cost.
const MARGIN: isize = 64 * 1024;

thread_local! {
    /// Bytes this thread allocated and has not freed.
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

fn count(bytes: isize) {
    LIVE.with(|live| live.set(live.get() + bytes));
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size() as isize);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size as isize - layout.size() as isize);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The bytes a run holds after its first frame, Tab and End, less what its model holds once built.
fn held_at_end<const N: usize>() -> isize {
    let before = LIVE.with(Cell::get);
    let model = VirtualList::<N>::init();
    let model_bytes = LIVE.with(Cell::get) - before;
    drop(model);
    let before = LIVE.with(Cell::get);
    let mut run = Run::<VirtualList<N>>::new();
    run.key(TAB, None);
    run.key(END, None);
    let held = LIVE.with(Cell::get) - before - model_bytes;
    drop(run);
    held
}

#[test]
fn a_list_of_50_000_rows_holds_as_much_memory_as_one_of_1_000() {
    held_at_end::<1_000>();
    let small = held_at_end::<1_000>();
    let large = held_at_end::<50_000>();
    println!("held after End: {small} bytes at 1,000 rows, {large} at 50,000");
    assert!(
        (large - small).abs() <= MARGIN,
        "{large} bytes at 50,000 rows, {small} at 1,000"
    );
}
