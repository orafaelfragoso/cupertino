//! The integration tests of `cupertino-powerd`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "battery-warning.rs"]
mod battery_warning;
#[path = "power-service.rs"]
mod power_service;
