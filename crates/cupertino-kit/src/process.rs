//! The process itself: what a process that holds a typed password does first.

use std::io;

use rustix::process::{set_dumpable_behavior, setrlimit, DumpableBehavior, Resource, Rlimit};

/// Make the process undumpable (`PR_SET_DUMPABLE` 0) and its core file size zero (`RLIMIT_CORE` 0): it writes no core
/// file, and its owner cannot read its memory or environment through `/proc` or attach to it.
pub fn harden() -> io::Result<()> {
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )?;
    Ok(())
}
