//! The integration tests of `cupertino-diagnostic-events`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[allow(dead_code)]
mod support;

#[path = "build-info.rs"]
mod build_info;
mod emitter;
mod journal;
#[path = "profile-timelines.rs"]
mod profile_timelines;
#[path = "rate-limit.rs"]
mod rate_limit;
mod record;
mod redaction;
mod store;
#[path = "time-format.rs"]
mod time_format;
