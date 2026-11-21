//! The integration tests of `cupertino-icons`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[allow(dead_code)]
mod support;

mod document;
mod loading;
mod lookup;
#[path = "system-icons.rs"]
mod system_icons;
mod thumbnail;
