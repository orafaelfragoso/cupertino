//! An allocator that zeroes every block it frees, so no copy of a password outlives the memory that held it. A password
//! is copied on its way (the frame, the `CString` handed to PAM, which PAM's Rust binding drops without clearing), and
//! the libraries that make the copies do not clear them. A binary that handles passwords installs it as its global
//! allocator: `cupertino-sessiond` (manager and worker), `cupertino-login` (the lock screen and the login screen) and
//! `cupertino-polkit-agent`'s two prompts (the authentication agent and the keyring's prompter), where what the person
//! types also passes through the toolkit's keyboard handling, which may hold it in blocks of its own.
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
