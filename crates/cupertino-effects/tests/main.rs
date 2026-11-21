//! The integration tests of `cupertino-effects`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod ease;
mod mesh;
mod minimize;
#[path = "space-slide.rs"]
mod space_slide;
mod transitions;
mod warp;
