// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod common;

#[path = "focus.rs"]
mod focus;
#[path = "focus_on_map.rs"]
mod focus_on_map;
#[path = "interaction.rs"]
mod interaction;
#[path = "kinds.rs"]
mod kinds;
#[path = "lifecycle_wm.rs"]
mod lifecycle_wm;
#[path = "magnet.rs"]
mod magnet;
#[path = "transitions.rs"]
mod transitions;
#[path = "window_controls.rs"]
mod window_controls;
#[path = "workspaces.rs"]
mod workspaces;
