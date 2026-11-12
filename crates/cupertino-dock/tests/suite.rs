// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "activate.rs"]
mod activate;
#[path = "dock.rs"]
mod dock;
#[path = "kit.rs"]
mod kit;
#[path = "session.rs"]
mod session;
#[path = "wm_surfaces.rs"]
mod wm_surfaces;
