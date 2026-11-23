//! The integration tests of `cupertino-glass`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "display-filter.rs"]
mod display_filter;
#[path = "glass-model.rs"]
mod glass_model;
#[path = "glass-stages.rs"]
mod glass_stages;
#[path = "material-renderer.rs"]
mod material_renderer;
