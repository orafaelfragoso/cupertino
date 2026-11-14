// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod common;

#[path = "caches.rs"]
mod caches;
#[path = "external.rs"]
mod external;
#[path = "fences.rs"]
mod fences;
#[path = "loss.rs"]
mod loss;
#[path = "orientation.rs"]
mod orientation;
#[path = "pipelines.rs"]
mod pipelines;
#[path = "vectors.rs"]
mod vectors;
