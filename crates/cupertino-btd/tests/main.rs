//! The integration tests of `cupertino-btd`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod address;
mod command;
#[path = "device-lifecycle.rs"]
mod device_lifecycle;
mod eir;
mod event;
#[path = "pairing-agent.rs"]
mod pairing_agent;
mod reply;
