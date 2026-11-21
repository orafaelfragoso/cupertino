//! The session's keyring prompter: the keyring daemon asks it for a password (to unlock a keyring, for a new keyring's
//! password) or a confirmation, in an alert like the authentication prompt's. The password goes back through the
//! protocol's secret exchange, never in clear on the bus.
//!
//! - [`exchange`]: the secret exchange.
//! - [`model`]: the callers, the prompt and what goes back, as a pure model.
//! - [`prompter`]: the prompter on the session bus.
//! - [`offer`]: the offer to move a default keyring that login does not unlock into the login keyring, as a pure
//!   model; [`secrets`] is the Secret Service it works on and [`asking`] carries it out.

pub mod asking;
pub mod exchange;
pub mod model;
pub mod offer;
pub mod prompter;
pub mod secrets;
