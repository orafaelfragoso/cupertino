//! The network daemon for the desktop, replacing NetworkManager for what apps and the shell use.
//!
//! - [`model`]: pure state of devices, connections and the scan list.
//! - [`dhcp`]: the DHCPv4 codec, the client state machine and its packet-socket transport.
//! - [`dns`]: the resolver file written from DHCP.
//! - [`supplicant`]: what Wi-Fi authentication needs (decision D2: it is not done here), and
//!   `iwd` (feature `dbus`), the implementation over iwd's D-Bus API.
//! - [`engine`]: joins the model to the machine ([`engine::Host`]) and the supplicant.
//! - [`host`]: the real machine: rtnetlink, nl80211, rfkill and packet sockets.
//! - [`snapshot`] and [`nm`]: what the bus shows and the NetworkManager values worked out from it.
//! - `auth` (feature `dbus`): who may change settings, decided from logind.
//! - `daemon` and `interfaces` (feature `dbus`): `org.cupertino.Network1` and the
//!   `org.freedesktop.NetworkManager` subset on the bus.
//!
//! Addresses of IPv6 come from the kernel's SLAAC; there is no DHCPv6.

pub mod dhcp;
pub mod dns;
pub mod engine;
pub mod error;
pub mod host;
pub mod model;
pub mod nm;
pub mod snapshot;
pub mod supplicant;

#[cfg(feature = "dbus")]
pub mod auth;
#[cfg(feature = "dbus")]
pub mod daemon;
#[cfg(feature = "dbus")]
pub mod interfaces;
#[cfg(feature = "dbus")]
pub mod iwd;

pub use error::{Error, Result};
