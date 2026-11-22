//! The integration tests of `cupertino-dock`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod launch;
#[path = "ready-to-show.rs"]
mod ready_to_show;
mod session;
