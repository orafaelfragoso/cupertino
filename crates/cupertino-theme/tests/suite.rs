// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "moved-checkout.rs"]
mod moved_checkout;
#[path = "one-size-table.rs"]
mod one_size_table;
