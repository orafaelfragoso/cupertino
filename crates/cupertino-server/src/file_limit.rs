//! The limit on open files. Every client connection, buffer fence and device the server holds is a descriptor, and the
//! soft limit of a login session is often 1024. The server raises its own soft limit and gives the programs it starts the
//! old one back: a program that uses `select` breaks at descriptor 1024 and above.

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::OnceLock;

use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};

/// The limit before [`raise`]: what the programs the server starts are given back.
static ORIGINAL: OnceLock<Rlimit> = OnceLock::new();

/// What to set when the soft limit is below the hard one: the soft limit moves up to the hard limit.
fn raised(limit: Rlimit) -> Option<Rlimit> {
    (limit.current != limit.maximum).then_some(Rlimit {
        current: limit.maximum,
        maximum: limit.maximum,
    })
}

fn show(value: Option<u64>) -> String {
    value.map_or_else(|| "unlimited".into(), |v| v.to_string())
}

/// Raises the soft limit on open files to the hard limit and logs both values.
pub fn raise() {
    let before = getrlimit(Resource::Nofile);
    let _ = ORIGINAL.set(before);
    let Some(wanted) = raised(before) else {
        tracing::info!(limit = show(before.current), "open file limit");
        return;
    };
    match setrlimit(Resource::Nofile, wanted) {
        Ok(()) => tracing::info!(
            soft = show(before.current),
            hard = show(before.maximum),
            "raised the soft open file limit to the hard limit"
        ),
        Err(e) => tracing::warn!(
            soft = show(before.current),
            hard = show(before.maximum),
            error = %e,
            "could not raise the soft open file limit"
        ),
    }
}

/// Makes the process `command` starts run with the soft limit it had before [`raise`] (nothing when `raise` was not
/// called).
pub fn restore_in_child(command: &mut Command) {
    // Safety: the closure runs in the child between fork and exec and calls only `setrlimit`, which is a system call.
    unsafe {
        command.pre_exec(|| match ORIGINAL.get() {
            Some(original) => setrlimit(Resource::Nofile, *original).map_err(Into::into),
            None => Ok(()),
        });
    }
}
