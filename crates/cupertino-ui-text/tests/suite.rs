// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "corpus.rs"]
mod corpus;
#[path = "fallback.rs"]
mod fallback;
#[path = "fontconfig.rs"]
mod fontconfig;
#[path = "installed.rs"]
mod installed;
