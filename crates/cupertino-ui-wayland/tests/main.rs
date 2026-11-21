//! The integration tests of `cupertino-ui-wayland`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "../../cupertino-wayland-client/tests/support/mod.rs"]
mod wire;

mod geometry;
#[path = "keyed-surfaces.rs"]
mod keyed_surfaces;
mod materials;
mod presenter;
