//! The manager's and the worker's global allocator, [`ZeroOnFree`]: every block they free is zeroed first, so no copy of a
//! password outlives the memory that held it. The PAM answer is copied several times on its way (the frame, the `CString`
//! handed to PAM), and the libraries that make the copies do not clear them.

pub use cupertino_login_ipc::zeroing::ZeroOnFree;

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Only this module's test writes this text.
    const MARKER: &[u8] = b"zeroing-marker-5c1e9a";

    /// Blocks freed with the marker still in them.
    static LEFT_BEHIND: AtomicUsize = AtomicUsize::new(0);

    /// What the scan looks for: `CString`'s drop overwrites the first byte, so the rest of the marker is what survives.
    const TRAIL: &[u8] = MARKER.split_at(1).1;

    /// Counts the blocks that reach the system allocator with [`TRAIL`] in them.
    struct Watch;

    // Safety: forwards to `System`; the scan only reads the block being freed.
    unsafe impl GlobalAlloc for Watch {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            System.alloc(layout)
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if layout.size() >= TRAIL.len()
                && std::slice::from_raw_parts(ptr, layout.size())
                    .windows(TRAIL.len())
                    .any(|window| window == TRAIL)
            {
                LEFT_BEHIND.fetch_add(1, Ordering::SeqCst);
            }
            System.dealloc(ptr, layout);
        }
    }

    #[global_allocator]
    static ALLOCATOR: ZeroOnFree<Watch> = ZeroOnFree(Watch);

    /// SEC-06: the `CString` the worker hands PAM is zeroed when PAM's conversation drops it, and so is every other
    /// copy of the answer the worker made on the way.
    #[cfg(feature = "app")]
    #[test]
    fn no_copy_of_the_answer_is_freed_with_the_password_in_it() {
        use crate::channel::Channel;
        use crate::worker::Relay;
        use crate::worker_protocol::{ToWorker, MAX_FRAME};
        use cupertino_login_ipc::{encode_secret, Secret};
        use pam_client2::ConversationHandler;
        use zeroize::Zeroizing;

        let before = LEFT_BEHIND.load(Ordering::SeqCst);
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut manager = Channel::new(ours, MAX_FRAME);
        let mut relay = Relay::new(Channel::new(theirs, MAX_FRAME));
        let text = String::from_utf8(MARKER.to_vec()).unwrap();
        let secret = Secret::new(Zeroizing::new(text)).unwrap();
        manager
            .send_frame(&encode_secret(&ToWorker::Answer { secret }, MAX_FRAME).unwrap())
            .unwrap();
        let answer = relay.prompt_echo_off(c"Password: ").unwrap();
        assert_eq!(answer.as_bytes(), MARKER);
        drop(answer);
        drop(relay);
        assert_eq!(LEFT_BEHIND.load(Ordering::SeqCst), before);
    }
}
