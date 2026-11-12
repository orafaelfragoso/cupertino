// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod common;

#[path = "features.rs"]
mod features;
#[path = "layout.rs"]
mod layout;
#[path = "properties.rs"]
mod properties;
#[path = "scroll.rs"]
mod scroll;
