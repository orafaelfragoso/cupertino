//! The Bluetooth daemon, replacing the BlueZ daemon for what the shell needs: the adapter's power,
//! discovery, the list of nearby and paired devices, pairing, connecting and forgetting.
//!
//! - [`device`]: a remote device, its icon class and the merge of discovery reports.
//! - [`pairing`]: the pairing state machine.
//! - [`model`]: the adapters and devices as a pure model that turns kernel events and requests
//!   into [`request::Effect`]s.
//! - [`keys`]: the stored pairing keys and their file.
//! - [`request`]: the vocabulary between the model and its host.
//! - [`transport`]: the management socket behind a trait.
//! - `auth`, `daemon` and `interfaces` (feature `dbus`): who may write, the loop that runs the
//!   model, and `org.cupertino.Bluetooth1` on the bus.

pub mod device;
pub mod error;
pub mod keys;
pub mod model;
pub mod pairing;
pub mod request;
pub mod transport;

#[cfg(feature = "dbus")]
pub mod auth;
#[cfg(feature = "dbus")]
pub mod daemon;
#[cfg(feature = "dbus")]
pub mod interfaces;

pub use error::{Error, Result};
