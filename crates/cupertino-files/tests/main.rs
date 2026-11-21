//! The integration tests of `cupertino-files`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod browser;
#[cfg(feature = "app")]
mod kit;
#[path = "mounted-cache.rs"]
mod mounted_cache;
mod space;
