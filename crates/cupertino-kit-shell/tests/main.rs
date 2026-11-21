//! The integration tests of `cupertino-kit-shell`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod support;

mod bridge;
mod capture;
mod hosting;
mod identity;
#[path = "profile-timeline.rs"]
mod profile_timeline;
mod publications;
mod runtime;
mod teardown;
