//! `kit::process`: a process that holds a password writes no core file and cannot be read through `/proc`.

use rustix::process::{dumpable_behavior, getrlimit, DumpableBehavior, Resource};

#[test]
fn a_hardened_process_is_undumpable_with_no_core_size() {
    cupertino_kit::process::harden().unwrap();
    assert_eq!(dumpable_behavior().unwrap(), DumpableBehavior::NotDumpable);
    let core = getrlimit(Resource::Core);
    assert_eq!((core.current, core.maximum), (Some(0), Some(0)));
}
