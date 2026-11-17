//! An allocator that zeroes every block it frees, the agent's global allocator: what the person types reaches the agent
//! through the toolkit's keyboard handling, which may hold it in blocks of its own (a composed character's text), and no
//! copy of it should outlive the memory that held it. The agent's own buffers are zeroed without it (the `zeroing` test).
//! Growing a block goes through the default `realloc`, which frees the old block here too.

use std::alloc::{GlobalAlloc, Layout};

use zeroize::Zeroize;

/// `A`, with every freed block zeroed first.
pub struct ZeroOnFree<A>(pub A);

// Safety: every call is forwarded to `A` with the same arguments; the zeroing writes only inside the block `dealloc`
// is about to give back.
unsafe impl<A: GlobalAlloc> GlobalAlloc for ZeroOnFree<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.0.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        std::slice::from_raw_parts_mut(ptr, layout.size()).zeroize();
        self.0.dealloc(ptr, layout);
    }
}
