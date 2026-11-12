// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "agent.rs"]
mod agent;
#[path = "diag_events.rs"]
mod diag_events;
#[path = "flow.rs"]
mod flow;
#[path = "keyring.rs"]
mod keyring;
#[path = "keyring_daemon.rs"]
mod keyring_daemon;
#[path = "zeroing.rs"]
mod zeroing;
