//! The integration tests of `cupertino-screenshot`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "color-picker.rs"]
mod color_picker;
mod files;
#[path = "share-picker.rs"]
mod share_picker;
mod toolbar;
