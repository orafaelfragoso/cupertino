//! CupertinoKit's C ABI v1, built as `libcupertinokit.so.1`, with its header generated into `abi/cupertinokit.h`.
//!
//! Every function returns a [`ck_status`] and writes its out-parameters only on [`CK_OK`]. Handles are 64-bit
//! generational values, so a zero, released or foreign handle is refused rather than dereferenced. Inputs are copied
//! before a call returns; outputs are copied into the caller's buffers, so the library never hands out memory. A panic
//! is caught in every function, returns [`CK_ERROR_INTERNAL`] and poisons the runtime. The library never calls into C.

#![allow(non_camel_case_types)]

mod ffi;
mod registry;

use std::mem;

use ffi::{guard, hook, read_id, read_input, Error, Input, Out};

pub const CUPERTINOKIT_ABI_MAJOR: u32 = 1;
pub const CUPERTINOKIT_ABI_MINOR: u32 = 0;

/// `CK_OK`, or a negative `CK_ERROR_*`.
pub type ck_status = i32;

pub const CK_OK: ck_status = 0;
/// A required pointer is null. A `CkStr` of length 0 may have a null `ptr`.
pub const CK_ERROR_NULL_POINTER: ck_status = -1;
/// Bad UTF-8, a NUL in an id or key, unknown flag bits or enum values, overlapping output buffers, a string over
/// 1 MiB, or a `struct_size` this library does not take.
pub const CK_ERROR_INVALID_ARGUMENT: ck_status = -2;
/// A zero, released or foreign handle, or one of a runtime that is gone.
pub const CK_ERROR_STALE_HANDLE: ck_status = -3;
/// An owner-thread function was called from another thread.
pub const CK_ERROR_WRONG_THREAD: ck_status = -4;
/// An output buffer cannot hold even one item.
pub const CK_ERROR_BUFFER_TOO_SMALL: ck_status = -5;
/// The major version differs, or the minor version asked for is newer than the library's.
pub const CK_ERROR_VERSION: ck_status = -6;
/// The backend failed to start.
pub const CK_ERROR_UNAVAILABLE: ck_status = -7;
/// The backend or capability is not in this library.
pub const CK_ERROR_UNSUPPORTED: ck_status = -8;
/// A call out of order, a second runtime in the process, or too many wakers.
pub const CK_ERROR_STATE: ck_status = -9;
/// 1024 posted messages wait; the post is dropped, and may be retried after the owner polls.
pub const CK_ERROR_QUEUE_FULL: ck_status = -10;
/// A panic was caught. Every later call on the runtime returns this, except `ck_runtime_release`.
pub const CK_ERROR_INTERNAL: ck_status = -100;

/// Not in this library yet; `ck_runtime_new` returns `CK_ERROR_UNSUPPORTED`.
pub const CK_BACKEND_WAYLAND: u32 = 1;
/// No display: windows are kept in memory, for tests.
pub const CK_BACKEND_HEADLESS: u32 = 2;

/// A message from `ck_waker_post`, in `message`.
pub const CK_EVENT_MESSAGE: u32 = 1;

/// The capabilities `ck_has_capability` knows in this library.
const CAPABILITIES: &[&str] = &["backend.headless"];

/// A runtime: its windows and its event loop. 0 is never a valid handle.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CkRuntime {
    pub bits: u64,
}

/// Posts messages to a runtime from any thread.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CkWaker {
    pub bits: u64,
}

/// A window of a runtime; 0 in an event that belongs to no window.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CkWindow {
    pub bits: u64,
}

/// UTF-8 bytes, copied during the call; not NUL-terminated. With `len` 0, `ptr` may be null.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CkStr {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CkRuntimeOptions {
    /// `sizeof(CkRuntimeOptions)` as the caller was compiled.
    pub struct_size: u32,
    /// `CUPERTINOKIT_ABI_MAJOR`.
    pub abi_major: u32,
    /// The lowest minor version the caller needs. Pass it: it is how a caller finds that the installed library is too
    /// old for functions added after 1.0.
    pub abi_minor: u32,
    /// `CK_BACKEND_WAYLAND` or `CK_BACKEND_HEADLESS`.
    pub backend: u32,
    /// The application's id, a desktop entry name. Not empty, no NUL.
    pub app_id: CkStr,
}

// SAFETY: repr(C), starts with struct_size, and all-zero bytes are valid for every field
unsafe impl Input for CkRuntimeOptions {
    const V1_0: usize = mem::size_of::<CkRuntimeOptions>();
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CkEvent {
    /// A `CK_EVENT_*`.
    pub kind: u32,
    /// Always 0.
    pub reserved: u32,
    pub window: CkWindow,
    pub message: u64,
    pub value: i64,
    /// Where the event's bytes start in the poll's byte buffer.
    pub payload_offset: u32,
    pub payload_len: u32,
}

/// Writes the library's ABI version.
///
/// # Safety
///
/// `major` and `minor` are null or writable.
#[no_mangle]
pub unsafe extern "C" fn ck_abi_version(major: *mut u32, minor: *mut u32) -> ck_status {
    guard(0, || {
        hook("ck_abi_version");
        let major = Out::new(major, "major")?;
        let minor = Out::new(minor, "minor")?;
        major.write(CUPERTINOKIT_ABI_MAJOR);
        minor.write(CUPERTINOKIT_ABI_MINOR);
        Ok(())
    })
}

/// Writes 1 to `out` if the library has the capability `name`, else 0. Call it before a function added after 1.0: with
/// lazy binding, a function the installed library lacks fails when it is called, not when the application loads.
///
/// # Safety
///
/// `name` is a valid `CkStr`; `out` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn ck_has_capability(name: CkStr, out: *mut u32) -> ck_status {
    guard(0, || {
        hook("ck_has_capability");
        let out = Out::new(out, "out")?;
        let name = read_id(name, "name")?;
        out.write(u32::from(CAPABILITIES.contains(&name.as_str())));
        Ok(())
    })
}

/// Copies the calling thread's last error message into `buf`. With `cap` > 0 the copy is NUL-terminated and cut at a
/// UTF-8 character boundary to fit `cap - 1` bytes. `*len` is the message's full length without the NUL.
///
/// # Safety
///
/// `buf` is null or writable for `cap` bytes; `len` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn ck_last_error_message(
    buf: *mut u8,
    cap: usize,
    len: *mut usize,
) -> ck_status {
    guard(0, || {
        hook("ck_last_error_message");
        let message = ffi::last_error();
        let len = Out::new(len, "len")?;
        if cap > 0 {
            if buf.is_null() {
                return Err(Error::NullPointer("buf"));
            }
            let mut n = message.len().min(cap - 1);
            while !message.is_char_boundary(n) {
                n -= 1;
            }
            std::ptr::copy_nonoverlapping(message.as_ptr(), buf, n);
            buf.add(n).write(0);
        }
        len.write(message.len());
        Ok(())
    })
}

/// Starts the process's runtime on the calling thread, which owns it from then on. There is one runtime per process.
///
/// # Safety
///
/// `options` is null or points to `options->struct_size` readable bytes; `out` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn ck_runtime_new(
    options: *const CkRuntimeOptions,
    out: *mut CkRuntime,
) -> ck_status {
    guard(0, || {
        hook("ck_runtime_new");
        let options = read_input(options, "options")?;
        let out = Out::new(out, "out")?;
        if options.abi_major != CUPERTINOKIT_ABI_MAJOR || options.abi_minor > CUPERTINOKIT_ABI_MINOR
        {
            return Err(Error::Version(format!(
                "the caller needs ABI {}.{}; this library is {CUPERTINOKIT_ABI_MAJOR}.{CUPERTINOKIT_ABI_MINOR}",
                options.abi_major, options.abi_minor
            )));
        }
        if read_id(options.app_id, "app_id")?.is_empty() {
            return Err(Error::InvalidArgument("app_id is empty".into()));
        }
        let make = match options.backend {
            CK_BACKEND_HEADLESS => registry::CRuntime::headless,
            CK_BACKEND_WAYLAND => {
                return Err(Error::Unsupported(
                    "this library has no Wayland backend yet",
                ))
            }
            other => {
                return Err(Error::InvalidArgument(format!(
                    "backend {other} is unknown"
                )))
            }
        };
        out.write(registry::create(make)?);
        Ok(())
    })
}

/// Releases the runtime: its windows close and its wakers go stale. A poisoned runtime may be released too.
///
/// # Safety
///
/// Any value is accepted.
#[no_mangle]
pub unsafe extern "C" fn ck_runtime_release(runtime: CkRuntime) -> ck_status {
    guard(runtime.bits, || {
        hook("ck_runtime_release");
        registry::release(runtime)
    })
}

/// Makes a waker that posts to the runtime from any thread. Call it on the runtime's thread; each waker needs its own
/// `ck_waker_release`.
///
/// # Safety
///
/// `out` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn ck_runtime_waker(runtime: CkRuntime, out: *mut CkWaker) -> ck_status {
    guard(runtime.bits, || {
        hook("ck_runtime_waker");
        let out = Out::new(out, "out")?;
        out.write(registry::new_waker(runtime)?);
        Ok(())
    })
}

/// Queues `message` for the runtime as a `CK_EVENT_MESSAGE` and wakes its loop. Any thread may call it.
///
/// # Safety
///
/// Any value is accepted.
#[no_mangle]
pub unsafe extern "C" fn ck_waker_post(waker: CkWaker, message: u64) -> ck_status {
    guard(waker.bits, || {
        hook("ck_waker_post");
        registry::post(waker, message)
    })
}

/// Releases the waker. Any thread may call it, also after its runtime is released.
///
/// # Safety
///
/// Any value is accepted.
#[no_mangle]
pub unsafe extern "C" fn ck_waker_release(waker: CkWaker) -> ck_status {
    guard(waker.bits, || {
        hook("ck_waker_release");
        registry::release_waker(waker)
    })
}

/// Copies as many whole queued events as fit into `events`, and their bytes into `bytes`, oldest first; the rest stay
/// queued. `*n_events` and `*n_bytes` say how much was written. The two buffers may not overlap.
///
/// # Safety
///
/// `events` is null or writable for `cap` events, `bytes` null or writable for `bytes_cap` bytes, at any alignment;
/// `n_events` and `n_bytes` are null or writable.
#[no_mangle]
pub unsafe extern "C" fn ck_poll_events(
    runtime: CkRuntime,
    events: *mut CkEvent,
    cap: usize,
    bytes: *mut u8,
    bytes_cap: usize,
    n_events: *mut usize,
    n_bytes: *mut usize,
) -> ck_status {
    guard(runtime.bits, || {
        hook("ck_poll_events");
        let n_events = Out::new(n_events, "n_events")?;
        let n_bytes = Out::new(n_bytes, "n_bytes")?;
        if cap > 0 && events.is_null() {
            return Err(Error::NullPointer("events"));
        }
        if bytes_cap > 0 && bytes.is_null() {
            return Err(Error::NullPointer("bytes"));
        }
        // every payload offset fits its uint32_t
        let bytes_cap = bytes_cap.min(u32::MAX as usize);
        let events_len = cap
            .checked_mul(mem::size_of::<CkEvent>())
            .ok_or_else(|| Error::InvalidArgument("cap is too large".into()))?;
        if !ffi::disjoint(events.cast(), events_len, bytes, bytes_cap) {
            return Err(Error::InvalidArgument(
                "the event and byte buffers overlap".into(),
            ));
        }
        let written = registry::take_posted(runtime, cap, |i, message| {
            let event = CkEvent {
                kind: CK_EVENT_MESSAGE,
                reserved: 0,
                window: CkWindow { bits: 0 },
                message,
                value: 0,
                payload_offset: 0,
                payload_len: 0,
            };
            events.add(i).write_unaligned(event);
        })?;
        n_events.write(written);
        n_bytes.write(0);
        Ok(())
    })
}

#[cfg(test)]
mod tests;
