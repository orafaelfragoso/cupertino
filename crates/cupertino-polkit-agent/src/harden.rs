//! The process holds a typed password in memory: it never dumps core.

use std::io;

/// Make the process undumpable and its core file size zero.
pub fn harden() -> io::Result<()> {
    // Safety: `prctl(PR_SET_DUMPABLE)` takes integers only.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let none = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // Safety: `none` is a valid `rlimit` that outlives the call.
    if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &none) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
