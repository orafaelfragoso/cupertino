//! The integration tests of `cupertino-ui-controls`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[allow(dead_code)]
mod support;

mod dialog;
mod lists;
mod menus;
#[path = "press-controls.rs"]
mod press_controls;
#[path = "text-fields.rs"]
mod text_fields;
mod typeahead;
#[path = "value-controls.rs"]
mod value_controls;
