// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "keymap-warnings.rs"]
mod keymap_warnings;
#[path = "later-layout-keys.rs"]
mod later_layout_keys;
#[path = "oracle.rs"]
mod oracle;
