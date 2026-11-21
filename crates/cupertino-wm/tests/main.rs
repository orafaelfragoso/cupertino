//! The integration tests of `cupertino-wm`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod common;

mod focus;
mod interaction;
#[path = "menu-bar.rs"]
mod menu_bar;
#[path = "overview-model.rs"]
mod overview_model;
mod placement;
mod stage;
mod tiling;
mod windows;
mod workspaces;
