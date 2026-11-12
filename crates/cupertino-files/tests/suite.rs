// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod common;

#[path = "error-notices.rs"]
mod error_notices;
#[path = "kit-accessibility.rs"]
mod kit_accessibility;
#[path = "kit-columns.rs"]
mod kit_columns;
#[path = "kit-context-menu.rs"]
mod kit_context_menu;
#[path = "kit-damage.rs"]
mod kit_damage;
#[path = "kit-drag.rs"]
mod kit_drag;
#[path = "kit-drop.rs"]
mod kit_drop;
#[path = "kit-gallery.rs"]
mod kit_gallery;
#[path = "kit-item-icons.rs"]
mod kit_item_icons;
#[path = "kit-jobs.rs"]
mod kit_jobs;
#[path = "kit-peek.rs"]
mod kit_peek;
#[path = "kit-repaint.rs"]
mod kit_repaint;
#[path = "kit-search.rs"]
mod kit_search;
#[path = "kit-select.rs"]
mod kit_select;
#[path = "kit-sidebar-icons.rs"]
mod kit_sidebar_icons;
#[path = "kit-spring.rs"]
mod kit_spring;
#[path = "kit-tabs.rs"]
mod kit_tabs;
#[path = "kit-tags.rs"]
mod kit_tags;
#[path = "kit-window.rs"]
mod kit_window;
#[path = "removable-media.rs"]
mod removable_media;
#[path = "session-handlers.rs"]
mod session_handlers;
#[path = "single-instance.rs"]
mod single_instance;
#[path = "workflow.rs"]
mod workflow;
