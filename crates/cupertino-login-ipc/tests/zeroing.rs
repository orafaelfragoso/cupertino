//! The zeroing allocator: every freed block is all zero by the time the wrapped allocator gets it back.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use cupertino_login_ipc::zeroing::ZeroOnFree;

/// Looks at each block `ZeroOnFree` hands back and counts the ones that still held data.
struct Inspect;

static FREED: AtomicUsize = AtomicUsize::new(0);
static DIRTY: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Inspect {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        System.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREED.fetch_add(1, Ordering::SeqCst);
        if std::slice::from_raw_parts(ptr, layout.size())
            .iter()
            .any(|&b| b != 0)
        {
            DIRTY.fetch_add(1, Ordering::SeqCst);
        }
        System.dealloc(ptr, layout);
    }
}

#[test]
fn freed_blocks_are_zero_and_live_blocks_are_untouched() {
    let allocator = ZeroOnFree(Inspect);
    let layout = Layout::from_size_align(256, 8).unwrap();
    let freed_before = FREED.load(Ordering::SeqCst);
    let dirty_before = DIRTY.load(Ordering::SeqCst);
    unsafe {
        let block = allocator.alloc(layout);
        assert!(!block.is_null());
        std::slice::from_raw_parts_mut(block, 256).fill(0xa5);
        // the block keeps its contents while it is live
        assert!(std::slice::from_raw_parts(block, 256)
            .iter()
            .all(|&b| b == 0xa5));
        allocator.dealloc(block, layout);
    }
    assert!(
        FREED.load(Ordering::SeqCst) > freed_before,
        "the inner allocator saw the free"
    );
    assert_eq!(
        DIRTY.load(Ordering::SeqCst),
        dirty_before,
        "and the block was already zero"
    );
}

#[test]
fn a_zeroed_allocation_is_zero_and_a_grown_block_frees_the_old_one_clean() {
    let allocator = ZeroOnFree(Inspect);
    let layout = Layout::from_size_align(64, 16).unwrap();
    let dirty_before = DIRTY.load(Ordering::SeqCst);
    unsafe {
        let block = allocator.alloc_zeroed(layout);
        assert!(std::slice::from_raw_parts(block, 64)
            .iter()
            .all(|&b| b == 0));
        std::slice::from_raw_parts_mut(block, 64).fill(0x5a);
        let grown = allocator.realloc(block, layout, 4096);
        assert!(!grown.is_null());
        assert!(
            std::slice::from_raw_parts(grown, 64)
                .iter()
                .all(|&b| b == 0x5a),
            "the contents move"
        );
        allocator.dealloc(grown, Layout::from_size_align(4096, 16).unwrap());
    }
    assert_eq!(DIRTY.load(Ordering::SeqCst), dirty_before);
}

#[test]
fn the_smallest_block_is_zeroed_too() {
    let allocator = ZeroOnFree(Inspect);
    let layout = Layout::from_size_align(1, 1).unwrap();
    let freed_before = FREED.load(Ordering::SeqCst);
    let dirty_before = DIRTY.load(Ordering::SeqCst);
    unsafe {
        let block = allocator.alloc(layout);
        *block = 0xff;
        allocator.dealloc(block, layout);
    }
    assert!(
        FREED.load(Ordering::SeqCst) > freed_before,
        "the block was freed"
    );
    assert_eq!(
        DIRTY.load(Ordering::SeqCst),
        dirty_before,
        "it reached the allocator zeroed"
    );
}
