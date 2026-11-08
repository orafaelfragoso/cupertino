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

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::System;
    use std::sync::Mutex;

    /// The system allocator, keeping a copy of the last block it was given back.
    struct Keep(Mutex<Vec<u8>>);

    // Safety: forwards to `System`; the copy only reads the block being freed.
    unsafe impl GlobalAlloc for Keep {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            System.alloc(layout)
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            *self.0.lock().unwrap() = std::slice::from_raw_parts(ptr, layout.size()).to_vec();
            System.dealloc(ptr, layout);
        }
    }

    #[test]
    fn a_block_reaches_the_allocator_below_zeroed_also_when_it_grows() {
        let allocator = ZeroOnFree(Keep(Mutex::new(Vec::new())));
        let freed = || allocator.0 .0.lock().unwrap().clone();
        let layout = Layout::from_size_align(16, 1).unwrap();
        // Safety: the blocks are allocated with `layout` (then the grown size) and freed once each.
        unsafe {
            let block = allocator.alloc(layout);
            block.copy_from(b"a secret, 16 b.!".as_ptr(), 16);
            let grown = allocator.realloc(block, layout, 64);
            assert_eq!(freed(), [0; 16], "the block it grew out of");
            assert_eq!(std::slice::from_raw_parts(grown, 16), b"a secret, 16 b.!");
            allocator.dealloc(grown, Layout::from_size_align(64, 1).unwrap());
        }
        assert_eq!(freed(), [0; 64]);
    }
}
