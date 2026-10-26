//! DHCPv4 (RFC 2131, RFC 2132): the message codec, the client state machine and the transport.
//!
//! Addresses of IPv6 come from the kernel's SLAAC; there is no DHCPv6.

pub mod client;
pub mod message;
pub mod transport;
