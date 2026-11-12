// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "app_chooser.rs"]
mod app_chooser;
#[path = "diag_events.rs"]
mod diag_events;
#[path = "file_chooser.rs"]
mod file_chooser;
#[path = "portals_conf.rs"]
mod portals_conf;
#[path = "screen_cast.rs"]
mod screen_cast;
#[path = "screenshot.rs"]
mod screenshot;
#[path = "settings.rs"]
mod settings;
