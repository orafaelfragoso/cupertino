//! Seat session over systemd-logind's D-Bus protocol, replacing libseat.
//!
//! The device bookkeeping lives in [`state`] as a pure state machine; [`logind`] is the thin D-Bus
//! layer. The Smithay `Session` and calloop notifier built on top live in `cupertino-server-backend`.

pub mod error;
pub mod logind;
pub mod state;

pub use error::Error;
pub use logind::{connect, inspect, locate, SessionInfo, SessionLookup, SessionRef};
