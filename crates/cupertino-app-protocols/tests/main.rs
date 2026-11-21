//! The integration tests of `cupertino-app-protocols`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "layer-limits.rs"]
mod layer_limits;
#[path = "semantics-wire.rs"]
mod semantics_wire;
