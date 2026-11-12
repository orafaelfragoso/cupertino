// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod support;

#[path = "emitter.rs"]
mod emitter;
#[path = "journal.rs"]
mod journal;
#[path = "model.rs"]
mod model;
#[path = "redaction.rs"]
mod redaction;
#[path = "registry.rs"]
mod registry;
#[path = "store.rs"]
mod store;
