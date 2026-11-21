//! The manager and its workers write no core file, and the programs they start get back the core file limit the
//! process was started with. Its own test binary: hardening lowers this process's hard limit for good.

use std::process::Command;

use cupertino_sessiond::core_limit;
use rustix::process::{getrlimit, Resource, Rlimit};

/// The core file limit, soft and hard, of a `cat` started by `command`, read from its own `/proc/self/limits`.
fn child_limit(mut command: Command) -> (String, String) {
    let out = command.arg("/proc/self/limits").output().expect("cat runs");
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("Max core file size"))
        .expect("a core file line");
    let mut words = line["Max core file size".len()..].split_whitespace();
    (
        words.next().unwrap().to_owned(),
        words.next().unwrap().to_owned(),
    )
}

/// How `/proc/<pid>/limits` writes `value`.
fn shown(value: Option<u64>) -> String {
    value.map_or_else(|| "unlimited".into(), |v| v.to_string())
}

#[test]
fn hardening_zeroes_the_core_limit_and_a_started_program_gets_the_original_back() {
    let before = getrlimit(Resource::Core);
    assert_eq!(core_limit::original(), None);
    core_limit::restore().expect("nothing to restore before hardening");

    core_limit::harden().expect("hardening");
    let zero = Rlimit {
        current: Some(0),
        maximum: Some(0),
    };
    assert_eq!(getrlimit(Resource::Core), zero);
    assert_eq!(core_limit::original(), Some(before));
    // a later hardening keeps the limit from before the first
    core_limit::harden().expect("hardening again");
    assert_eq!(core_limit::original(), Some(before));

    // a program started plainly inherits the zero limit
    assert_eq!(child_limit(Command::new("cat")), ("0".into(), "0".into()));

    // given back: raising a hard limit takes root, which the manager and the worker are; elsewhere only a hard
    // limit of zero to begin with can be given back, and any other is refused rather than left at zero
    let mut command = Command::new("cat");
    core_limit::restore_in_child(&mut command);
    // Safety: no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    if root || before.maximum == Some(0) {
        assert_eq!(
            child_limit(command),
            (shown(before.current), shown(before.maximum))
        );
    } else {
        assert!(command.arg("/proc/self/limits").output().is_err());
    }
}
