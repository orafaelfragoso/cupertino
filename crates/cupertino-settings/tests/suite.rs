// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod common;

#[path = "about-window.rs"]
mod about_window;
#[path = "clipboard.rs"]
mod clipboard;
#[path = "desktop-control.rs"]
mod desktop_control;
#[path = "fallback-services.rs"]
mod fallback_services;
#[path = "files.rs"]
mod files;
#[path = "instance.rs"]
mod instance;
#[path = "screen-sharing.rs"]
mod screen_sharing;
#[path = "services.rs"]
mod services;
#[path = "start-process.rs"]
mod start_process;
