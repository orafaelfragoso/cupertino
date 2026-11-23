//! The integration tests of `cupertino-server-render`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "decorations-field.rs"]
mod decorations_field;
#[path = "decorations-frame.rs"]
mod decorations_frame;
#[path = "decorations-shadow.rs"]
mod decorations_shadow;
#[path = "decorations-titlebar.rs"]
mod decorations_titlebar;
#[path = "glass-keys.rs"]
mod glass_keys;
#[path = "render-models.rs"]
mod render_models;
