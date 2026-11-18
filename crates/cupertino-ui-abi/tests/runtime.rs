//! The C interface as a caller sees it: what it refuses, who may call it, and what the queue of posted messages does.
//!
//! A process has one runtime, so every test takes `exclusive` for as long as it holds one.

use cupertinokit::*;
use std::mem::size_of;
use std::sync::{Mutex, MutexGuard};
use std::thread;

static ONE_RUNTIME: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    ONE_RUNTIME.lock().unwrap_or_else(|e| e.into_inner())
}

const APP_ID: &[u8] = b"org.cupertino.tests.Abi";

fn text(bytes: &[u8]) -> CkStr {
    CkStr {
        ptr: bytes.as_ptr(),
        len: bytes.len(),
    }
}

fn options() -> CkRuntimeOptions {
    CkRuntimeOptions {
        struct_size: size_of::<CkRuntimeOptions>() as u32,
        abi_major: CUPERTINOKIT_ABI_MAJOR,
        abi_minor: CUPERTINOKIT_ABI_MINOR,
        backend: CK_BACKEND_HEADLESS,
        app_id: text(APP_ID),
    }
}

fn new_runtime(options: &CkRuntimeOptions) -> (ck_status, CkRuntime) {
    let mut runtime = CkRuntime { bits: 0 };
    let status = unsafe { ck_runtime_new(options, &mut runtime) };
    (status, runtime)
}

/// A runtime that is released when the test ends, however it ends.
struct Started(CkRuntime);

impl Started {
    fn new() -> Started {
        let (status, runtime) = new_runtime(&options());
        assert_eq!(status, CK_OK);
        Started(runtime)
    }

    fn waker(&self) -> CkWaker {
        let mut waker = CkWaker { bits: 0 };
        assert_eq!(unsafe { ck_runtime_waker(self.0, &mut waker) }, CK_OK);
        waker
    }

    /// The messages a poll with room for `cap` events gives.
    fn poll(&self, cap: usize) -> Vec<u64> {
        let mut events = vec![[0u8; size_of::<CkEvent>()]; cap.max(1)];
        let (mut n_events, mut n_bytes) = (usize::MAX, usize::MAX);
        let status = unsafe {
            ck_poll_events(
                self.0,
                events.as_mut_ptr().cast(),
                cap,
                std::ptr::null_mut(),
                0,
                &mut n_events,
                &mut n_bytes,
            )
        };
        assert_eq!(status, CK_OK);
        assert_eq!(n_bytes, 0);
        events[..n_events]
            .iter()
            .map(|raw| {
                let event: CkEvent = unsafe { std::ptr::read_unaligned(raw.as_ptr().cast()) };
                assert_eq!(event.kind, CK_EVENT_MESSAGE);
                event.message
            })
            .collect()
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        let _ = unsafe { ck_runtime_release(self.0) };
    }
}

/// The calling thread's last error message.
fn last_error() -> String {
    let mut buf = [0u8; 512];
    let mut len = 0;
    assert_eq!(
        unsafe { ck_last_error_message(buf.as_mut_ptr(), buf.len(), &mut len) },
        CK_OK
    );
    String::from_utf8(buf[..len].to_vec()).unwrap()
}

#[test]
fn a_caller_the_library_cannot_serve_is_refused_and_no_runtime_starts() {
    let _one = exclusive();
    const WITH_NUL: &[u8] = b"org.cupertino\0Abi";
    const NOT_UTF8: &[u8] = &[0xff, 0xfe, b'a'];
    type Change = fn(&mut CkRuntimeOptions);
    let rows: [(&str, Change, ck_status, &str); 11] = [
        (
            "another major version",
            |o| o.abi_major = 2,
            CK_ERROR_VERSION,
            "2.0",
        ),
        (
            "a newer minor version",
            |o| o.abi_minor += 1,
            CK_ERROR_VERSION,
            "1.1",
        ),
        (
            "a backend the library lacks",
            |o| o.backend = CK_BACKEND_WAYLAND,
            CK_ERROR_UNSUPPORTED,
            "Wayland",
        ),
        (
            "an unknown backend",
            |o| o.backend = 77,
            CK_ERROR_INVALID_ARGUMENT,
            "77",
        ),
        (
            "an empty application id",
            |o| o.app_id.len = 0,
            CK_ERROR_INVALID_ARGUMENT,
            "empty",
        ),
        (
            "an id that is not UTF-8",
            |o| o.app_id = text(NOT_UTF8),
            CK_ERROR_INVALID_ARGUMENT,
            "UTF-8",
        ),
        (
            "an id with a NUL",
            |o| o.app_id = text(WITH_NUL),
            CK_ERROR_INVALID_ARGUMENT,
            "NUL",
        ),
        (
            "an id over 1 MiB",
            |o| o.app_id.len = (1 << 20) + 1,
            CK_ERROR_INVALID_ARGUMENT,
            "1 MiB",
        ),
        (
            "an id with a null pointer",
            |o| o.app_id.ptr = std::ptr::null(),
            CK_ERROR_NULL_POINTER,
            "app_id",
        ),
        (
            "a struct smaller than 1.0's",
            |o| o.struct_size = 8,
            CK_ERROR_INVALID_ARGUMENT,
            "struct_size",
        ),
        (
            "a struct larger than the library's",
            |o| o.struct_size += 8,
            CK_ERROR_INVALID_ARGUMENT,
            "struct_size",
        ),
    ];
    for (what, change, status, says) in rows {
        let mut asked = options();
        change(&mut asked);
        let (got, runtime) = new_runtime(&asked);
        assert_eq!(got, status, "{what}");
        assert_eq!(runtime.bits, 0, "{what}: nothing is written on an error");
        assert!(last_error().contains(says), "{what}: {}", last_error());
    }
    let mut runtime = CkRuntime { bits: 0 };
    assert_eq!(
        unsafe { ck_runtime_new(std::ptr::null(), &mut runtime) },
        CK_ERROR_NULL_POINTER
    );
    assert_eq!(
        unsafe { ck_runtime_new(&options(), std::ptr::null_mut()) },
        CK_ERROR_NULL_POINTER
    );
    // none of them took the process's one runtime
    let _ = Started::new();
}

#[test]
fn a_runtime_belongs_to_the_thread_that_made_it_and_one_exists_per_process() {
    let _one = exclusive();
    let runtime = Started::new();
    let waker = runtime.waker();
    assert_eq!(new_runtime(&options()).0, CK_ERROR_STATE);
    thread::scope(|other| {
        other
            .spawn(|| {
                let mut out = CkWaker { bits: 0 };
                assert_eq!(
                    unsafe { ck_runtime_waker(runtime.0, &mut out) },
                    CK_ERROR_WRONG_THREAD
                );
                assert_eq!(
                    unsafe { ck_runtime_release(runtime.0) },
                    CK_ERROR_WRONG_THREAD
                );
                let (mut n_events, mut n_bytes) = (0, 0);
                assert_eq!(
                    unsafe {
                        ck_poll_events(
                            runtime.0,
                            std::ptr::null_mut(),
                            0,
                            std::ptr::null_mut(),
                            0,
                            &mut n_events,
                            &mut n_bytes,
                        )
                    },
                    CK_ERROR_WRONG_THREAD
                );
                // a waker is the handle every thread may use
                assert_eq!(unsafe { ck_waker_post(waker, 5) }, CK_OK);
            })
            .join()
            .unwrap();
    });
    assert_eq!(runtime.poll(4), [5]);
    drop(runtime);

    // a thread that ends without releasing its runtime gives the process's slot back, and the waker it made goes stale
    let (abandoned, waker) = thread::spawn(|| {
        let runtime = Started::new();
        let waker = runtime.waker();
        let handle = runtime.0;
        std::mem::forget(runtime);
        (handle, waker)
    })
    .join()
    .unwrap();
    assert_eq!(unsafe { ck_waker_post(waker, 1) }, CK_ERROR_STALE_HANDLE);
    assert_eq!(
        unsafe { ck_runtime_release(abandoned) },
        CK_ERROR_STALE_HANDLE
    );
    let again = Started::new();
    assert_ne!(again.0, abandoned, "a handle value does not come back");
}

#[test]
fn a_zero_released_or_foreign_handle_is_stale() {
    let _one = exclusive();
    let runtime = Started::new();
    let live = runtime.waker();
    let released = runtime.waker();
    assert_eq!(unsafe { ck_waker_release(released) }, CK_OK);
    let mut out = CkWaker { bits: 0 };
    let as_runtime = CkRuntime { bits: live.bits };
    let as_waker = CkWaker {
        bits: runtime.0.bits,
    };
    let statuses = [
        ("a zero waker", unsafe {
            ck_waker_post(CkWaker { bits: 0 }, 1)
        }),
        ("a released waker", unsafe { ck_waker_post(released, 1) }),
        ("releasing it again", unsafe { ck_waker_release(released) }),
        ("a waker as a waker's runtime", unsafe {
            ck_runtime_waker(as_runtime, &mut out)
        }),
        ("a runtime as a waker", unsafe {
            ck_waker_post(as_waker, 1)
        }),
        ("a zero runtime", unsafe {
            ck_runtime_release(CkRuntime { bits: 0 })
        }),
    ];
    for (what, status) in statuses {
        assert_eq!(status, CK_ERROR_STALE_HANDLE, "{what}");
    }
    assert_eq!(out.bits, 0);

    // a waker made for a runtime that is gone is refused, and may still be released
    assert_eq!(unsafe { ck_runtime_release(runtime.0) }, CK_OK);
    assert_eq!(
        unsafe { ck_runtime_release(runtime.0) },
        CK_ERROR_STALE_HANDLE
    );
    assert_eq!(unsafe { ck_waker_post(live, 1) }, CK_ERROR_STALE_HANDLE);
    assert_eq!(
        unsafe { ck_runtime_waker(runtime.0, &mut out) },
        CK_ERROR_STALE_HANDLE
    );
    assert_eq!(unsafe { ck_waker_release(live) }, CK_OK);
}

#[test]
fn messages_from_any_thread_come_out_oldest_first_and_the_rest_stay_queued() {
    let _one = exclusive();
    let runtime = Started::new();
    let waker = runtime.waker();
    thread::scope(|threads| {
        threads.spawn(|| {
            for message in 1..=5 {
                assert_eq!(unsafe { ck_waker_post(waker, message) }, CK_OK);
            }
        });
    });
    assert_eq!(runtime.poll(2), [1, 2]);
    assert_eq!(runtime.poll(10), [3, 4, 5]);
    assert_eq!(runtime.poll(10), [] as [u64; 0]);

    // the events are written wherever the caller's buffer starts, and the room is the caller's to declare
    assert_eq!(unsafe { ck_waker_post(waker, 9) }, CK_OK);
    let mut raw = vec![0u8; size_of::<CkEvent>() + 1];
    let (mut n_events, mut n_bytes) = (0, 0);
    let mut poll = |events: *mut CkEvent, cap, bytes: *mut u8, bytes_cap| unsafe {
        ck_poll_events(
            runtime.0,
            events,
            cap,
            bytes,
            bytes_cap,
            &mut n_events,
            &mut n_bytes,
        )
    };
    let mut bytes = [0u8; 16];
    assert_eq!(
        poll(raw.as_mut_ptr().cast(), 0, bytes.as_mut_ptr(), 16),
        CK_ERROR_BUFFER_TOO_SMALL,
        "room for no event while one waits"
    );
    assert_eq!(
        poll(std::ptr::null_mut(), 1, bytes.as_mut_ptr(), 16),
        CK_ERROR_NULL_POINTER
    );
    assert_eq!(
        poll(raw.as_mut_ptr().cast(), 1, std::ptr::null_mut(), 16),
        CK_ERROR_NULL_POINTER
    );
    let overlapping = unsafe { raw.as_mut_ptr().add(8) };
    assert_eq!(
        poll(raw.as_mut_ptr().cast(), 1, overlapping, 16),
        CK_ERROR_INVALID_ARGUMENT,
        "the two buffers may not overlap"
    );
    let unaligned = unsafe { raw.as_mut_ptr().add(1) };
    assert_eq!(poll(unaligned.cast(), 1, bytes.as_mut_ptr(), 16), CK_OK);
    assert_eq!(n_events, 1);
    let event: CkEvent = unsafe { std::ptr::read_unaligned(unaligned.cast()) };
    assert_eq!((event.kind, event.message), (CK_EVENT_MESSAGE, 9));
}

#[test]
fn a_full_queue_refuses_posts_until_the_owner_polls() {
    let _one = exclusive();
    let runtime = Started::new();
    let waker = runtime.waker();
    for message in 0..1024 {
        assert_eq!(unsafe { ck_waker_post(waker, message) }, CK_OK);
    }
    assert_eq!(unsafe { ck_waker_post(waker, 1024) }, CK_ERROR_QUEUE_FULL);
    assert_eq!(runtime.poll(1), [0]);
    assert_eq!(unsafe { ck_waker_post(waker, 1024) }, CK_OK);
    let rest = runtime.poll(2000);
    assert_eq!(rest.len(), 1024);
    assert_eq!(
        rest.last(),
        Some(&1024),
        "the refused post was not queued, the retried one was"
    );
}

#[test]
fn the_last_error_is_the_calling_threads_and_is_cut_to_the_buffer_it_fills() {
    let _one = exclusive();
    let mut asking_too_much = options();
    asking_too_much.abi_minor = 9;
    assert_eq!(new_runtime(&asking_too_much).0, CK_ERROR_VERSION);
    let full = last_error();
    assert!(full.contains("1.9"), "{full}");

    let mut buf = [0xAAu8; 8];
    let mut len = 0;
    assert_eq!(
        unsafe { ck_last_error_message(buf.as_mut_ptr(), buf.len(), &mut len) },
        CK_OK
    );
    assert_eq!(len, full.len(), "the length is the whole message's");
    assert_eq!(&buf[..7], &full.as_bytes()[..7]);
    assert_eq!(buf[7], 0, "what is copied ends in a NUL");

    let mut untouched = [0xAAu8; 4];
    assert_eq!(
        unsafe { ck_last_error_message(untouched.as_mut_ptr(), 0, &mut len) },
        CK_OK
    );
    assert_eq!(
        untouched, [0xAA; 4],
        "a buffer of no capacity is left alone"
    );
    assert_eq!(len, full.len());

    thread::spawn(|| assert_eq!(last_error(), ""))
        .join()
        .unwrap();
}

#[test]
fn the_library_says_what_it_has() {
    let ask = |name: &[u8]| {
        let mut has = 7;
        let status = unsafe { ck_has_capability(text(name), &mut has) };
        (status, has)
    };
    assert_eq!(ask(b"backend.headless"), (CK_OK, 1));
    assert_eq!(ask(b"backend.nothing"), (CK_OK, 0));
    assert_eq!(ask(b""), (CK_OK, 0));
    assert_eq!(
        unsafe { ck_has_capability(text(b"x"), std::ptr::null_mut()) },
        CK_ERROR_NULL_POINTER
    );
}
