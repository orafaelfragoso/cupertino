// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "control.rs"]
mod control;
#[path = "harden.rs"]
mod harden;
#[path = "lock_atspi.rs"]
mod lock_atspi;
#[path = "pam.rs"]
mod pam;
#[path = "restore.rs"]
mod restore;
#[path = "zeroing.rs"]
mod zeroing;
