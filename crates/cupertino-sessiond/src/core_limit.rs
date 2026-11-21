//! The core file limit. The manager and its workers hold passwords and root, so they write no core file and accept no
//! ptrace attach ([`harden`]); the programs they start are given back the limit the manager was started with, so a
//! session's programs dump core as the system's do. The manager gives it to each worker it starts
//! ([`restore_in_child`]), and a worker, which hardens itself again, to the session's leader before dropping root
//! ([`restore`]): only root may raise a hard limit.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::OnceLock;

use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};

/// The limit before the first [`harden`]: what the programs started from here are given back.
static ORIGINAL: OnceLock<Rlimit> = OnceLock::new();

/// Make the process undumpable (`PR_SET_DUMPABLE` 0) and its core file size zero (`RLIMIT_CORE` 0, soft and hard),
/// keeping the limit it had before for [`restore`] and [`restore_in_child`].
pub fn harden() -> io::Result<()> {
    // Safety: `prctl(PR_SET_DUMPABLE)` takes integers only.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(io::Error::other(format!(
            "PR_SET_DUMPABLE: {}",
            io::Error::last_os_error()
        )));
    }
    let _ = ORIGINAL.set(getrlimit(Resource::Core));
    setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )
    .map_err(|e| io::Error::other(format!("RLIMIT_CORE: {e}")))
}

/// The core file limit the process had before its first [`harden`], if it hardened.
pub fn original() -> Option<Rlimit> {
    ORIGINAL.get().copied()
}

/// Give this process back the limit it had before [`harden`] (nothing when it did not harden). Raising the hard limit
/// back takes root. It makes one system call and allocates nothing, so the child of `fork` may call it.
pub fn restore() -> io::Result<()> {
    match ORIGINAL.get() {
        Some(original) => setrlimit(Resource::Core, *original).map_err(Into::into),
        None => Ok(()),
    }
}

/// Make the process `command` starts run with the limit this one had before [`harden`] (nothing when it did not harden).
pub fn restore_in_child(command: &mut Command) {
    // Safety: the closure runs in the child between fork and exec and calls only `setrlimit`, which is a system call.
    unsafe {
        command.pre_exec(restore);
    }
}
