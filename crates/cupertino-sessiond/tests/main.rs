//! The integration tests of `cupertino-sessiond`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "core-limit.rs"]
mod core_limit;
#[path = "greeter-policy.rs"]
mod greeter_policy;
#[path = "login-service.rs"]
mod login_service;
mod upgrade;
