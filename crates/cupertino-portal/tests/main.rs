//! The integration tests of `cupertino-portal`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod dialogs;
#[path = "screencast-connect.rs"]
mod screencast_connect;
#[path = "screencast-format.rs"]
mod screencast_format;
