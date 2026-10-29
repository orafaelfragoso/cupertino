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

#[cfg(test)]
mod tests {
    use super::*;

    const fn limit(current: Option<u64>, maximum: Option<u64>) -> Rlimit {
        Rlimit { current, maximum }
    }

    #[test]
    fn the_soft_limit_moves_up_to_the_hard_limit() {
        assert_eq!(
            raised(limit(Some(1024), Some(524_288))),
            Some(limit(Some(524_288), Some(524_288)))
        );
        assert_eq!(raised(limit(Some(1024), None)), Some(limit(None, None)));
    }

    fn soft_limit_of_a_child(command: &mut Command) -> String {
        let output = command.args(["-c", "ulimit -Sn"]).output().unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn a_started_program_gets_the_soft_limit_the_server_had_before_raising_it() {
        let start = getrlimit(Resource::Nofile);
        if start.maximum.is_some_and(|hard| hard <= 1024) {
            eprintln!("skipping: the hard limit is not above 1024");
            return;
        }
        setrlimit(
            Resource::Nofile,
            Rlimit {
                current: Some(1024),
                maximum: start.maximum,
            },
        )
        .unwrap();
        raise();
        assert_ne!(getrlimit(Resource::Nofile).current, Some(1024));

        let mut plain = Command::new("sh");
        assert_ne!(
            soft_limit_of_a_child(&mut plain),
            "1024",
            "inherited raised"
        );
        let mut restored = Command::new("sh");
        restore_in_child(&mut restored);
        assert_eq!(soft_limit_of_a_child(&mut restored), "1024");
    }

    #[test]
    fn a_soft_limit_at_the_hard_limit_is_left_alone() {
        assert_eq!(raised(limit(Some(4096), Some(4096))), None);
        assert_eq!(raised(limit(None, None)), None);
    }
}
