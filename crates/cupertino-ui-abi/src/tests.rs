use std::cell::Cell;
use std::mem;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;

use rustix::io::Errno;

use super::*;

thread_local! {
    pub(crate) static PANIC_AT: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// One runtime per process: the tests that start one take turns.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn text(s: &[u8]) -> CkStr {
    CkStr {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

fn options() -> CkRuntimeOptions {
    CkRuntimeOptions {
        struct_size: mem::size_of::<CkRuntimeOptions>() as u32,
        abi_major: 1,
        abi_minor: 0,
        backend: CK_BACKEND_HEADLESS,
        app_id: text(b"org.example.Abi"),
    }
}

fn new_with(options: &CkRuntimeOptions) -> Result<CkRuntime, ck_status> {
    let mut out = CkRuntime { bits: 0 };
    match unsafe { ck_runtime_new(options, &mut out) } {
        CK_OK => Ok(out),
        status => Err(status),
    }
}

fn new() -> CkRuntime {
    new_with(&options()).unwrap()
}

fn waker(runtime: CkRuntime) -> CkWaker {
    let mut out = CkWaker { bits: 0 };
    assert_eq!(unsafe { ck_runtime_waker(runtime, &mut out) }, CK_OK);
    out
}

fn poll(runtime: CkRuntime, cap: usize) -> Result<Vec<u64>, ck_status> {
    let mut events = vec![
        CkEvent {
            kind: 0,
            reserved: 0,
            window: CkWindow { bits: 0 },
            message: 0,
            value: 0,
            payload_offset: 0,
            payload_len: 0
        };
        cap
    ];
    let (mut n, mut bytes) = (usize::MAX, usize::MAX);
    match unsafe {
        ck_poll_events(
            runtime,
            events.as_mut_ptr(),
            cap,
            std::ptr::null_mut(),
            0,
            &mut n,
            &mut bytes,
        )
    } {
        CK_OK => Ok(events[..n].iter().map(|e| e.message).collect()),
        status => Err(status),
    }
}

fn last_error() -> String {
    let mut buf = [0u8; 256];
    let mut len = 0;
    assert_eq!(
        unsafe { ck_last_error_message(buf.as_mut_ptr(), buf.len(), &mut len) },
        CK_OK
    );
    String::from_utf8(buf[..len].to_vec()).unwrap()
}

fn panic_at(at: Option<&'static str>) {
    PANIC_AT.with(|p| p.set(at));
}

#[test]
fn the_version_and_capabilities_are_reported() {
    let (mut major, mut minor) = (0, 7);
    assert_eq!(unsafe { ck_abi_version(&mut major, &mut minor) }, CK_OK);
    assert_eq!((major, minor), (1, 0));
    assert_eq!(
        unsafe { ck_abi_version(std::ptr::null_mut(), &mut minor) },
        CK_ERROR_NULL_POINTER
    );
    let mut has = 7;
    assert_eq!(
        unsafe { ck_has_capability(text(b"backend.headless"), &mut has) },
        CK_OK
    );
    assert_eq!(has, 1);
    assert_eq!(
        unsafe { ck_has_capability(text(b"backend.wayland"), &mut has) },
        CK_OK
    );
    assert_eq!(has, 0);
}

#[test]
fn hostile_strings_are_refused() {
    let mut has = 7;
    let cases: [(CkStr, ck_status); 4] = [
        (text(b"backend.\xff"), CK_ERROR_INVALID_ARGUMENT),
        (text(b"backend\0headless"), CK_ERROR_INVALID_ARGUMENT),
        (
            CkStr {
                ptr: std::ptr::null(),
                len: 3,
            },
            CK_ERROR_NULL_POINTER,
        ),
        (
            CkStr {
                ptr: b"x".as_ptr(),
                len: ffi::MAX_STR + 1,
            },
            CK_ERROR_INVALID_ARGUMENT,
        ),
    ];
    for (name, status) in cases {
        assert_eq!(unsafe { ck_has_capability(name, &mut has) }, status);
        assert_eq!(has, 7, "no out-parameter is written on failure");
    }
    assert_eq!(
        unsafe {
            ck_has_capability(
                CkStr {
                    ptr: std::ptr::null(),
                    len: 0,
                },
                &mut has,
            )
        },
        CK_OK
    );
}

#[test]
fn the_last_error_is_terminated_and_cut_at_a_character() {
    let mut has = 0;
    unsafe { ck_has_capability(text(b"\xff"), &mut has) };
    assert_eq!(last_error(), "name is not UTF-8");
    ffi::guard(0, || Err(Error::InvalidArgument("aé!".into())));
    let (mut out, mut len) = ([0xaau8; 4], 0);
    // "é" is two bytes, so a cut inside it backs off to before it
    assert_eq!(
        unsafe { ck_last_error_message(out.as_mut_ptr(), 3, &mut len) },
        CK_OK
    );
    assert_eq!((len, out), (4, [b'a', 0, 0xaa, 0xaa]));
    assert_eq!(
        unsafe { ck_last_error_message(std::ptr::null_mut(), 0, &mut len) },
        CK_OK
    );
    assert_eq!(len, 4);
    assert_eq!(
        unsafe { ck_last_error_message(std::ptr::null_mut(), 1, &mut len) },
        CK_ERROR_NULL_POINTER
    );
    assert_eq!(
        unsafe { ck_last_error_message(out.as_mut_ptr(), 4, std::ptr::null_mut()) },
        CK_ERROR_NULL_POINTER
    );
}

#[test]
fn bad_options_are_refused() {
    let _serial = serial();
    let mut short = options();
    short.struct_size = 8;
    let mut long = options();
    long.struct_size += 8;
    let mut backend = options();
    backend.backend = 9;
    let mut wayland = options();
    wayland.backend = CK_BACKEND_WAYLAND;
    let mut major = options();
    major.abi_major = 2;
    let mut app_id = options();
    app_id.app_id = text(b"org.example\0Abi");
    let mut empty = options();
    empty.app_id = text(b"");
    let cases = [
        (short, CK_ERROR_INVALID_ARGUMENT),
        (long, CK_ERROR_INVALID_ARGUMENT),
        (backend, CK_ERROR_INVALID_ARGUMENT),
        (wayland, CK_ERROR_UNSUPPORTED),
        (major, CK_ERROR_VERSION),
        (app_id, CK_ERROR_INVALID_ARGUMENT),
        (empty, CK_ERROR_INVALID_ARGUMENT),
    ];
    for (options, status) in cases {
        assert_eq!(new_with(&options), Err(status));
    }
    let mut out = CkRuntime { bits: 0 };
    assert_eq!(
        unsafe { ck_runtime_new(std::ptr::null(), &mut out) },
        CK_ERROR_NULL_POINTER
    );
    assert_eq!(
        unsafe { ck_runtime_new(&options(), std::ptr::null_mut()) },
        CK_ERROR_NULL_POINTER
    );
    assert_eq!(out.bits, 0);
}

#[test]
fn options_are_read_at_any_alignment() {
    let _serial = serial();
    let mut bytes = vec![0u8; mem::size_of::<CkRuntimeOptions>() + 1];
    let options = options();
    unsafe {
        std::ptr::copy_nonoverlapping(
            (&options as *const CkRuntimeOptions).cast::<u8>(),
            bytes.as_mut_ptr().add(1),
            mem::size_of::<CkRuntimeOptions>(),
        );
    }
    let unaligned = unsafe { bytes.as_ptr().add(1) }.cast::<CkRuntimeOptions>();
    let mut out = CkRuntime { bits: 0 };
    assert_eq!(unsafe { ck_runtime_new(unaligned, &mut out) }, CK_OK);
    assert_eq!(unsafe { ck_runtime_release(out) }, CK_OK);
}

#[test]
fn handles_are_refused_once_stale() {
    let _serial = serial();
    let runtime = new();
    assert_eq!(
        new_with(&options()),
        Err(CK_ERROR_STATE),
        "one runtime per process"
    );
    let waker = waker(runtime);
    assert_eq!(
        unsafe { ck_runtime_release(CkRuntime { bits: waker.bits }) },
        CK_ERROR_STALE_HANDLE
    );
    assert_eq!(
        unsafe { ck_runtime_release(CkRuntime { bits: 0 }) },
        CK_ERROR_STALE_HANDLE
    );
    assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_OK);
    assert_eq!(
        unsafe { ck_runtime_release(runtime) },
        CK_ERROR_STALE_HANDLE
    );
    assert_eq!(unsafe { ck_waker_post(waker, 1) }, CK_ERROR_STALE_HANDLE);
    assert_eq!(unsafe { ck_waker_release(waker) }, CK_OK);
    assert_eq!(unsafe { ck_waker_release(waker) }, CK_ERROR_STALE_HANDLE);

    let next = new();
    assert_ne!(next, runtime);
    assert_eq!(
        poll(runtime, 1),
        Err(CK_ERROR_STALE_HANDLE),
        "a handle of an earlier runtime"
    );
    assert_eq!(unsafe { ck_runtime_release(next) }, CK_OK);
}

#[test]
fn owner_functions_refuse_other_threads_and_posts_cross_them() {
    let _serial = serial();
    let runtime = new();
    let waker = waker(runtime);
    thread::spawn(move || {
        let mut other = CkWaker { bits: 0 };
        assert_eq!(
            unsafe { ck_runtime_waker(runtime, &mut other) },
            CK_ERROR_WRONG_THREAD
        );
        assert_eq!(poll(runtime, 1), Err(CK_ERROR_WRONG_THREAD));
        assert_eq!(
            unsafe { ck_runtime_release(runtime) },
            CK_ERROR_WRONG_THREAD
        );
        assert_eq!(unsafe { ck_waker_post(waker, 7) }, CK_OK);
        assert_eq!(unsafe { ck_waker_post(waker, 8) }, CK_OK);
    })
    .join()
    .unwrap();
    assert_eq!(poll(runtime, 1), Ok(vec![7]));
    assert_eq!(poll(runtime, 4), Ok(vec![8]));
    assert_eq!(poll(runtime, 4), Ok(vec![]));
    assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_OK);
}

#[test]
fn the_post_queue_is_bounded() {
    let _serial = serial();
    let runtime = new();
    let waker = waker(runtime);
    for message in 0..registry::MAX_POSTED as u64 {
        assert_eq!(unsafe { ck_waker_post(waker, message) }, CK_OK);
    }
    assert_eq!(unsafe { ck_waker_post(waker, 9999) }, CK_ERROR_QUEUE_FULL);
    assert_eq!(poll(runtime, 0), Err(CK_ERROR_BUFFER_TOO_SMALL));
    assert_eq!(poll(runtime, 2), Ok(vec![0, 1]));
    assert_eq!(unsafe { ck_waker_post(waker, 9999) }, CK_OK);
    assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_OK);
}

#[test]
fn poll_writes_through_raw_pointers_into_disjoint_buffers() {
    let _serial = serial();
    let runtime = new();
    let waker = waker(runtime);
    assert_eq!(unsafe { ck_waker_post(waker, 42) }, CK_OK);
    let size = mem::size_of::<CkEvent>();
    let mut memory = vec![0u8; 2 * size + 64];
    let base = memory.as_mut_ptr();
    let (mut n, mut bytes) = (0, 0);
    // overlapping buffers and null buffers with a capacity are refused
    let overlap = unsafe {
        ck_poll_events(
            runtime,
            base.add(1).cast(),
            1,
            base.add(size),
            8,
            &mut n,
            &mut bytes,
        )
    };
    assert_eq!(overlap, CK_ERROR_INVALID_ARGUMENT);
    let null = unsafe {
        ck_poll_events(
            runtime,
            std::ptr::null_mut(),
            1,
            base,
            0,
            &mut n,
            &mut bytes,
        )
    };
    assert_eq!(null, CK_ERROR_NULL_POINTER);
    // a byte capacity past u32::MAX is clamped, so the ranges are compared without wrapping
    let unaligned = unsafe { base.add(1) }.cast::<CkEvent>();
    let status = unsafe {
        ck_poll_events(
            runtime,
            unaligned,
            1,
            base.add(1 + size),
            usize::MAX,
            &mut n,
            &mut bytes,
        )
    };
    assert_eq!((status, n, bytes), (CK_OK, 1, 0));
    let event = unsafe { unaligned.read_unaligned() };
    assert_eq!(
        event,
        CkEvent {
            kind: CK_EVENT_MESSAGE,
            reserved: 0,
            window: CkWindow { bits: 0 },
            message: 42,
            value: 0,
            payload_offset: 0,
            payload_len: 0
        }
    );
    assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_OK);
}

#[test]
fn an_owner_that_exits_without_release_frees_the_slot_and_closes_the_wake_fd() {
    let _serial = serial();
    let (runtime, waker, fd) = thread::spawn(|| {
        let runtime = new();
        let waker = waker(runtime);
        (runtime, waker, registry::wake_fd(runtime))
    })
    .join()
    .unwrap();
    let closed = rustix::io::fcntl_getfd(unsafe { BorrowedFd::borrow_raw(fd) });
    assert_eq!(
        closed,
        Err(Errno::BADF),
        "the wake fd closed when its owner exited"
    );
    // a pipe's write end takes the fd number, as any later open could; a stale post must not write to it
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::NONBLOCK).unwrap();
    let moved = rustix::io::fcntl_dupfd_cloexec(&read, fd + 1).unwrap();
    drop(read);
    let read = moved;
    let _reused = (write.as_raw_fd() != fd).then(|| {
        let mut reused = unsafe { OwnedFd::from_raw_fd(fd) };
        rustix::io::dup2(write.as_fd(), &mut reused).unwrap();
        reused
    });
    assert_eq!(unsafe { ck_waker_post(waker, 1) }, CK_ERROR_STALE_HANDLE);
    assert_eq!(rustix::io::read(&read, &mut [0u8; 8]), Err(Errno::AGAIN));
    assert_eq!(
        unsafe { ck_runtime_release(runtime) },
        CK_ERROR_STALE_HANDLE
    );
    let next = new();
    assert_eq!(unsafe { ck_runtime_release(next) }, CK_OK);
    assert_eq!(unsafe { ck_waker_release(waker) }, CK_OK);
}

#[test]
fn a_runtime_whose_drop_panics_on_thread_exit_is_leaked_without_aborting() {
    let _serial = serial();
    thread::spawn(|| {
        panic_at(Some("drop"));
        new();
    })
    .join()
    .unwrap();
    let next = new();
    assert_eq!(unsafe { ck_runtime_release(next) }, CK_OK);
}

#[test]
fn a_panic_returns_internal_and_poisons_the_runtime() {
    let _serial = serial();
    for at in [
        "ck_runtime_waker",
        "ck_poll_events",
        "ck_waker_post",
        "ck_waker_release",
    ] {
        let runtime = new();
        let first = waker(runtime);
        let mut out = CkWaker { bits: 0 };
        let mut n = 0;
        panic_at(Some(at));
        let status = match at {
            "ck_runtime_waker" => unsafe { ck_runtime_waker(runtime, &mut out) },
            "ck_poll_events" => unsafe {
                ck_poll_events(
                    runtime,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut n,
                    &mut n,
                )
            },
            "ck_waker_post" => unsafe { ck_waker_post(first, 1) },
            _ => unsafe { ck_waker_release(first) },
        };
        panic_at(None);
        assert_eq!(status, CK_ERROR_INTERNAL, "{at}");
        assert_eq!(last_error(), "a panic was caught; the runtime is poisoned");
        assert_eq!(poll(runtime, 1), Err(CK_ERROR_INTERNAL), "{at}");
        assert_eq!(
            unsafe { ck_runtime_waker(runtime, &mut out) },
            CK_ERROR_INTERNAL,
            "{at}"
        );
        assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_OK, "{at}");
        unsafe { ck_waker_release(first) };
    }
    for at in [
        "ck_abi_version",
        "ck_has_capability",
        "ck_last_error_message",
        "ck_runtime_new",
    ] {
        let mut a = 0u32;
        let mut len = 0usize;
        let mut runtime = CkRuntime { bits: 0 };
        panic_at(Some(at));
        let status = match at {
            "ck_abi_version" => unsafe { ck_abi_version(&mut a, &mut a) },
            "ck_has_capability" => unsafe { ck_has_capability(text(b"x"), &mut a) },
            "ck_last_error_message" => unsafe {
                ck_last_error_message(std::ptr::null_mut(), 0, &mut len)
            },
            _ => unsafe { ck_runtime_new(&options(), &mut runtime) },
        };
        panic_at(None);
        assert_eq!(status, CK_ERROR_INTERNAL, "{at}");
    }
    let runtime = new();
    assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_OK);
}

#[test]
fn a_panic_in_release_still_frees_the_slot() {
    let _serial = serial();
    let runtime = new();
    panic_at(Some("ck_runtime_release"));
    assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_ERROR_INTERNAL);
    panic_at(None);
    assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_OK);
    panic_at(Some("drop"));
    let poisoned = new();
    assert_eq!(
        unsafe { ck_runtime_release(poisoned) },
        CK_OK,
        "a runtime whose drop panics is leaked"
    );
    panic_at(None);
    let next = new();
    assert_eq!(unsafe { ck_runtime_release(next) }, CK_OK);
}

#[test]
fn a_panic_payload_whose_drop_panics_is_forgotten() {
    struct Bomb;
    impl Drop for Bomb {
        fn drop(&mut self) {
            panic!("the payload's drop panics");
        }
    }
    let status = ffi::guard(0, || std::panic::panic_any(Bomb));
    assert_eq!(status, CK_ERROR_INTERNAL);
}

#[test]
fn a_panic_under_the_registry_lock_leaves_it_usable() {
    let _serial = serial();
    let runtime = new();
    let waker = waker(runtime);
    panic_at(Some("post locked"));
    assert_eq!(unsafe { ck_waker_post(waker, 1) }, CK_ERROR_INTERNAL);
    panic_at(None);
    assert_eq!(
        poll(runtime, 1),
        Err(CK_ERROR_INTERNAL),
        "the post poisoned its runtime"
    );
    assert_eq!(unsafe { ck_runtime_release(runtime) }, CK_OK);
    let next = new();
    let waker = super::tests::waker(next);
    assert_eq!(unsafe { ck_waker_post(waker, 2) }, CK_OK);
    assert_eq!(poll(next, 1), Ok(vec![2]));
    assert_eq!(unsafe { ck_runtime_release(next) }, CK_OK);
}

#[test]
fn panics_unwind_in_every_profile() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut manifests = vec![root.join("Cargo.toml")];
    for entry in std::fs::read_dir(root.join("crates")).unwrap() {
        manifests.push(entry.unwrap().path().join("Cargo.toml"));
    }
    manifests.push(root.join(".cargo/config.toml"));
    for path in manifests.into_iter().filter(|p| p.exists()) {
        let manifest: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        let profiles = manifest.get("profile").and_then(|p| p.as_table());
        for (name, profile) in profiles.into_iter().flatten() {
            let panic = profile.get("panic").and_then(|p| p.as_str());
            assert_ne!(
                panic,
                Some("abort"),
                "{}: profile {name} aborts on panic, so no panic is caught",
                path.display()
            );
        }
    }
}

/// A C application declares no renderer class of its own: once its windows reach a display, it takes the size and
/// repaint rule, as a third-party toolkit application does.
#[test]
fn a_c_application_takes_the_size_and_repaint_rule() {
    use cupertino_kit::core::{Application, RendererClass};
    assert_eq!(
        <registry::CApp as Application>::RENDERER,
        RendererClass::ByUse
    );
}
