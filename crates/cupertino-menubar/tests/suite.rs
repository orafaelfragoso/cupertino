// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[path = "app-menus.rs"]
mod app_menus;
#[path = "bar-view.rs"]
mod bar_view;
#[path = "controller.rs"]
mod controller;
#[path = "dbus_integration.rs"]
mod dbus_integration;
#[path = "extras_bus.rs"]
mod extras_bus;
#[path = "gtk_changes.rs"]
mod gtk_changes;
#[path = "kit_export.rs"]
mod kit_export;
#[path = "worker.rs"]
mod worker;
