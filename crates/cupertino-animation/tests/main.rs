//! The integration tests of `cupertino-animation`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod animator;
mod layer;
mod momentum;
#[path = "rubber-band.rs"]
mod rubber_band;
mod scroll;
mod spring;
mod timing;
