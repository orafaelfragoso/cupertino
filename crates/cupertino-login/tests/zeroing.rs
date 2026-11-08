//! The lock screen's answer to PAM leaves no copy of the password in the memory it frees, with the allocator the
//! `cupertino-login` process installs: pam-client2 takes the answer as a `CString` and drops it after copying it into
//! PAM's buffer, and a `CString`'s drop clears only its first byte, so the allocator zeroing every freed block is what
//! clears the rest. The watch below the allocator finds a block freed with the password in it: the test shows it does
//! for a block that bypasses the zeroing, and finds none from the attempt.
#![cfg(feature = "app")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use cupertino_login::auth::PasswordConversation;
use cupertino_login::model::password;
use cupertino_login_ipc::zeroing::ZeroOnFree;
use pam_client2::ConversationHandler;

/// Only this test types it.
const MARKER: &[u8] = b"zeroing-marker-lock-2b7e41";
/// What the scan looks for: a `CString`'s drop overwrites the first byte, so the rest of the marker is what survives.
const TRAIL: &[u8] = MARKER.split_at(1).1;

/// Blocks freed with the marker still in them.
static LEFT_BEHIND: AtomicUsize = AtomicUsize::new(0);

/// The system allocator, counting the blocks given back with [`TRAIL`] in them.
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

#[test]
fn the_answer_pam_was_given_is_zeroed_when_it_is_dropped() {
    // the watch sees a block that is not zeroed
    let before = LEFT_BEHIND.load(Ordering::SeqCst);
    let layout = Layout::array::<u8>(MARKER.len()).unwrap();
    // Safety: the block is allocated with `layout`, filled within it and freed once.
    unsafe {
        let block = Watch.alloc(layout);
        block.copy_from(MARKER.as_ptr(), MARKER.len());
        Watch.dealloc(block, layout);
    }
    assert_eq!(LEFT_BEHIND.load(Ordering::SeqCst), before + 1);

    let before = LEFT_BEHIND.load(Ordering::SeqCst);
    let on_notice = |_: String| {};
    let mut conversation =
        PasswordConversation::new(password(std::str::from_utf8(MARKER).unwrap()), &on_notice);
    let answer = conversation.prompt_echo_off(c"Password: ").unwrap();
    assert_eq!(answer.as_bytes(), MARKER);
    // what pam-client2 does once the answer is in PAM's buffer
    drop(answer);
    drop(conversation);
    assert_eq!(LEFT_BEHIND.load(Ordering::SeqCst), before);
}
