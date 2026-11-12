// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "cpu_render.rs"]
mod cpu_render;
#[path = "goldens.rs"]
mod goldens;
#[path = "render_matrix.rs"]
mod render_matrix;
