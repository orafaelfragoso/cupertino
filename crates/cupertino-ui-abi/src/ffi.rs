//! The only `unsafe` boundary: the panic guard every exported function runs in, the thread's last error, and one checked
//! reader or writer per pointer shape. Nothing here forms a reference to the caller's memory.

use std::any::Any;
use std::cell::RefCell;
use std::mem::{self, MaybeUninit};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;

use crate::{ck_status, registry, CkStr};

/// The longest string an id, a key or display text may be.
pub(crate) const MAX_STR: usize = 1 << 20;

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Error {
    NullPointer(&'static str),
    InvalidArgument(String),
    StaleHandle,
    WrongThread,
    BufferTooSmall(&'static str),
    Version(String),
    Unavailable(String),
    Unsupported(&'static str),
    State(&'static str),
    QueueFull,
    Internal,
}

impl Error {
    fn status(&self) -> ck_status {
        match self {
            Error::NullPointer(_) => crate::CK_ERROR_NULL_POINTER,
            Error::InvalidArgument(_) => crate::CK_ERROR_INVALID_ARGUMENT,
            Error::StaleHandle => crate::CK_ERROR_STALE_HANDLE,
            Error::WrongThread => crate::CK_ERROR_WRONG_THREAD,
            Error::BufferTooSmall(_) => crate::CK_ERROR_BUFFER_TOO_SMALL,
            Error::Version(_) => crate::CK_ERROR_VERSION,
            Error::Unavailable(_) => crate::CK_ERROR_UNAVAILABLE,
            Error::Unsupported(_) => crate::CK_ERROR_UNSUPPORTED,
            Error::State(_) => crate::CK_ERROR_STATE,
            Error::QueueFull => crate::CK_ERROR_QUEUE_FULL,
            Error::Internal => crate::CK_ERROR_INTERNAL,
        }
    }

    fn message(&self) -> String {
        match self {
            Error::NullPointer(what) => format!("{what} is null"),
            Error::InvalidArgument(why) | Error::Version(why) | Error::Unavailable(why) => {
                why.clone()
            }
            Error::StaleHandle => "the handle is zero, released or of another runtime".into(),
            Error::WrongThread => "the runtime belongs to another thread".into(),
            Error::BufferTooSmall(why) | Error::Unsupported(why) | Error::State(why) => {
                (*why).into()
            }
            Error::QueueFull => {
                "1024 posted messages wait; post again after the owner polls".into()
            }
            Error::Internal => "a panic was caught; the runtime is poisoned".into(),
        }
    }
}

/// Runs an exported function's body: an error is recorded as the thread's last error, and a panic returns
/// `CK_ERROR_INTERNAL` and poisons the runtime of `handle` (a runtime's or a waker's bits, 0 for none).
pub(crate) fn guard(handle: u64, body: impl FnOnce() -> Result<(), Error>) -> ck_status {
    let error = match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => return crate::CK_OK,
        Ok(Err(error)) => error,
        Err(payload) => {
            drop_payload(payload);
            registry::poison(handle);
            Error::Internal
        }
    };
    let _ = LAST_ERROR.try_with(|last| *last.borrow_mut() = error.message());
    error.status()
}

/// Drops a panic payload, whose destructor may panic too; that second payload is forgotten.
pub(crate) fn drop_payload(payload: Box<dyn Any + Send>) {
    if let Err(again) = panic::catch_unwind(AssertUnwindSafe(|| drop(payload))) {
        mem::forget(again);
    }
}

/// Drops `value`, or leaks what is left of it if its destructor panics: never twice, and no panic leaves.
pub(crate) fn drop_or_leak<T>(value: T) {
    if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(move || drop(value))) {
        drop_payload(payload);
    }
}

/// The thread's last error; empty while the thread's thread-locals are being destroyed.
pub(crate) fn last_error() -> String {
    LAST_ERROR
        .try_with(|last| last.borrow().clone())
        .unwrap_or_default()
}

/// A test makes the function named `at` panic.
pub(crate) fn hook(at: &'static str) {
    #[cfg(test)]
    if crate::tests::PANIC_AT
        .try_with(|p| p.get() == Some(at))
        .unwrap_or(false)
    {
        panic!("injected panic at {at}");
    }
    #[cfg(not(test))]
    let _ = at;
}

/// An input struct of the ABI: `#[repr(C)]`, starting with `uint32_t struct_size`, with no field for which all-zero
/// bytes are invalid (no reference, `bool`, enum or `char`).
///
/// # Safety
///
/// Implement it only for such a struct.
pub(crate) unsafe trait Input: Copy {
    /// The size in ABI 1.0; a caller's `struct_size` may not be smaller.
    const V1_0: usize;
}

/// Copies the caller's struct, `min(struct_size, sizeof)` bytes into a zeroed local, so fields a 1.0 caller doesn't
/// know stay zero. A `struct_size` below the 1.0 size or above this library's is refused.
///
/// # Safety
///
/// `input` is null or points to `struct_size` readable bytes, at any alignment.
pub(crate) unsafe fn read_input<T: Input>(input: *const T, what: &'static str) -> Result<T, Error> {
    if input.is_null() {
        return Err(Error::NullPointer(what));
    }
    let size = input.cast::<u32>().read_unaligned() as usize;
    if !(T::V1_0..=mem::size_of::<T>()).contains(&size) {
        return Err(Error::InvalidArgument(format!(
            "{what}: struct_size {size} is not between {} and {}",
            T::V1_0,
            mem::size_of::<T>()
        )));
    }
    let mut local = MaybeUninit::<T>::zeroed();
    ptr::copy_nonoverlapping(input.cast::<u8>(), local.as_mut_ptr().cast::<u8>(), size);
    Ok(local.assume_init())
}

/// Copies an id or a key: UTF-8 of at most [`MAX_STR`] bytes, without a NUL.
///
/// # Safety
///
/// `s.ptr` points to `s.len` readable bytes, or `s.len` is 0.
pub(crate) unsafe fn read_id(s: CkStr, what: &'static str) -> Result<String, Error> {
    if s.len == 0 {
        return Ok(String::new());
    }
    if s.len > MAX_STR {
        return Err(Error::InvalidArgument(format!(
            "{what} is longer than 1 MiB"
        )));
    }
    if s.ptr.is_null() {
        return Err(Error::NullPointer(what));
    }
    let bytes = std::slice::from_raw_parts(s.ptr, s.len);
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::InvalidArgument(format!("{what} is not UTF-8")))?;
    if text.contains('\0') {
        return Err(Error::InvalidArgument(format!("{what} holds a NUL")));
    }
    Ok(text.to_owned())
}

/// An out-parameter, checked for null before any work so that it is written only on success.
pub(crate) struct Out<T>(*mut T);

impl<T> Out<T> {
    pub(crate) fn new(out: *mut T, what: &'static str) -> Result<Out<T>, Error> {
        if out.is_null() {
            Err(Error::NullPointer(what))
        } else {
            Ok(Out(out))
        }
    }

    /// # Safety
    ///
    /// The pointer given to [`Out::new`] is writable for a `T`, at any alignment.
    pub(crate) unsafe fn write(self, value: T) {
        self.0.write_unaligned(value);
    }
}

/// Whether the byte ranges `a` and `b` share no byte; a range that wraps around the address space is refused as if it
/// overlapped.
pub(crate) fn disjoint(a: *const u8, a_len: usize, b: *const u8, b_len: usize) -> bool {
    if a_len == 0 || b_len == 0 {
        return true;
    }
    let (a, b) = (a as usize, b as usize);
    match (a.checked_add(a_len), b.checked_add(b_len)) {
        (Some(a_end), Some(b_end)) => a_end <= b || b_end <= a,
        _ => false,
    }
}
