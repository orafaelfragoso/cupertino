// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "header.rs"]
mod header;
#[path = "layout.rs"]
mod layout;
#[path = "symbols.rs"]
mod symbols;
