// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod support;

#[path = "badge.rs"]
mod badge;
#[path = "bare_field.rs"]
mod bare_field;
#[path = "button.rs"]
mod button;
#[path = "canvas.rs"]
mod canvas;
#[path = "caret-overlay.rs"]
mod caret_overlay;
#[path = "checkbox.rs"]
mod checkbox;
#[path = "choice.rs"]
mod choice;
#[path = "contract.rs"]
mod contract;
#[path = "dialog.rs"]
mod dialog;
#[path = "disclosure.rs"]
mod disclosure;
#[path = "editor.rs"]
mod editor;
#[path = "field.rs"]
mod field;
#[path = "fill.rs"]
mod fill;
#[path = "fixed-width.rs"]
mod fixed_width;
#[path = "form.rs"]
mod form;
#[path = "grid.rs"]
mod grid;
#[path = "high-contrast-buttons.rs"]
mod high_contrast_buttons;
#[path = "icon.rs"]
mod icon;
#[path = "image.rs"]
mod image;
#[path = "list.rs"]
mod list;
#[path = "menu.rs"]
mod menu;
#[path = "menu-touch.rs"]
mod menu_touch;
#[path = "menu-tracking.rs"]
mod menu_tracking;
#[path = "module.rs"]
mod module;
#[path = "navigation.rs"]
mod navigation;
#[path = "pages.rs"]
mod pages;
#[path = "pickers.rs"]
mod pickers;
#[path = "popover.rs"]
mod popover;
#[path = "progress.rs"]
mod progress;
#[path = "rich_text.rs"]
mod rich_text;
#[path = "scroll.rs"]
mod scroll;
#[path = "search-field.rs"]
mod search_field;
#[path = "sidebar.rs"]
mod sidebar;
#[path = "sidebar-window.rs"]
mod sidebar_window;
#[path = "slider.rs"]
mod slider;
#[path = "split.rs"]
mod split;
#[path = "stepper.rs"]
mod stepper;
#[path = "table-header.rs"]
mod table_header;
#[path = "table-under-toolbar.rs"]
mod table_under_toolbar;
#[path = "tabs.rs"]
mod tabs;
#[path = "text.rs"]
mod text;
#[path = "toggle.rs"]
mod toggle;
#[path = "toolbar.rs"]
mod toolbar;
#[path = "tooltip.rs"]
mod tooltip;
#[path = "tree.rs"]
mod tree;
#[path = "window-drag.rs"]
mod window_drag;
#[path = "window-group.rs"]
mod window_group;
