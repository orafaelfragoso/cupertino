//! The integration tests of `cupertino-peer`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod caller;
#[path = "component-caller.rs"]
mod component_caller;
mod process;
mod signal;
