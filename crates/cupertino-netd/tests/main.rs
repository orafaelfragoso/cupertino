//! The integration tests of `cupertino-netd`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod framing;
mod ie;
#[path = "lease-lifecycle.rs"]
mod lease_lifecycle;
mod nl80211;
mod packet;
#[path = "primary-network.rs"]
mod primary_network;
mod rfkill;
mod rtnetlink;
