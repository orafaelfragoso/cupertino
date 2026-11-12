// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod common;
#[allow(dead_code, unused_imports, unused_macros)]
mod diag;

#[path = "dbus.rs"]
mod dbus;
#[path = "diag_events.rs"]
mod diag_events;
#[path = "fake-sysfs.rs"]
mod fake_sysfs;
