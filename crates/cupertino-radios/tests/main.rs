//! The integration tests of `cupertino-radios`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "call-timeout-default.rs"]
mod call_timeout_default;
#[path = "call-timeout-set.rs"]
mod call_timeout_set;
mod errors;
#[path = "network-manager.rs"]
mod network_manager;
mod readings;
mod watch;
