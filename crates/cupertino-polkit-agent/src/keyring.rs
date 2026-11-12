//! The session's keyring prompter: the keyring daemon asks it for a password (to unlock a keyring, for a new keyring's
//! password) or a confirmation, and the prompt is drawn like the authentication prompt. The password goes back through
//! the protocol's secret exchange, never in clear on the bus.
//!
//! - [`exchange`]: the secret exchange.
//! - [`model`]: the callers, the prompt and what goes back, as a pure model.
//! - [`prompter`]: the prompter on the session bus.
//! - [`panel`]: where the prompt's parts sit and what a key or a click does.
//! - [`render`]: the panel drawn.
//! - [`access`]: what the prompt tells assistive technology.

pub mod access;
pub mod exchange;
pub mod model;
pub mod panel;
pub mod prompter;
pub mod render;
