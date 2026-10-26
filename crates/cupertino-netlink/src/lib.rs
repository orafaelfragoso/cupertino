//! The kernel's network interfaces without a C library: rtnetlink, generic netlink (nl80211) and
//! rfkill. The message codecs are written here over `rustix` sockets.
//!
//! - [`message`], [`attr`]: netlink framing and attributes.
//! - [`socket`]: a request socket (one request, its reply) and an event socket (multicast).
//! - [`rtnetlink`]: links, addresses and routes; dump, monitor and change.
//! - [`nl80211`], [`ie`]: wireless interfaces, the current connection, scan results with the
//!   security from the RSN and WPA elements, and scan, connect and disconnect events.
//! - [`rfkill`]: the Wi-Fi kill switch and its events.
//!
//! Every decoder takes bytes, so the tests run on replies recorded from a real kernel
//! (`tests/fixtures`, see its README) and on messages built byte by byte.

pub mod attr;
pub mod error;
pub mod ie;
pub mod mac;
pub mod message;
pub mod nl80211;
pub mod packet;
pub mod rfkill;
pub mod rtnetlink;
pub mod socket;

pub use error::{Error, ParseError, Result};
pub use ie::{Security, Ssid};
pub use mac::MacAddr;
