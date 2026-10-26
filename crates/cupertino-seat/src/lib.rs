//! Seat session for the compositor over systemd-logind's D-Bus protocol, replacing libseat.
//!
//! [`LogindSession`] implements Smithay's [`Session`](smithay::backend::session::Session);
//! [`LogindNotifier`] is the calloop event source that reports session pause and activation. The
//! device bookkeeping lives in [`state`] as a pure state machine; [`logind`] is the thin D-Bus
//! layer. See [`session`] for how this differs from Smithay's libseat session.

pub mod error;
pub mod logind;
pub mod session;
pub mod state;

pub use error::Error;
pub use logind::{connect, inspect, locate, SessionInfo, SessionLookup, SessionRef};
pub use session::{LogindNotifier, LogindSession};
