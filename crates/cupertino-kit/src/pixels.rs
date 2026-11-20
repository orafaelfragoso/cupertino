//! Pixel memory for pictures a program fills itself.

use std::sync::Arc;

/// `len` zero bytes, for pixels a program fills in place (`Arc::get_mut` finds them unshared). The memory comes zeroed
/// from the allocator and is not written first, so a whole screen of it costs no pass that sets it to zero, and its pages
/// are first touched by whoever fills them.
pub fn zeroed_pixels(len: usize) -> Arc<[u8]> {
    // SAFETY: zeroed bytes are valid `u8`s.
    unsafe { Arc::<[u8]>::new_zeroed_slice(len).assume_init() }
}
