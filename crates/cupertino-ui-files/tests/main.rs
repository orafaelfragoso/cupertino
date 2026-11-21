//! The integration tests of `cupertino-ui-files`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;
#[allow(dead_code)]
mod support;

#[path = "file-columns.rs"]
mod file_columns;
#[path = "file-list.rs"]
mod file_list;
#[path = "file-picker.rs"]
mod file_picker;
#[path = "icon-grid.rs"]
mod icon_grid;
#[path = "icon-layout.rs"]
mod icon_layout;
mod mime;
mod model;
mod names;
mod notices;
mod open;
mod operations;
mod order;
mod panel;
mod places;
mod tags;
mod thumbnails;
mod transfer;
