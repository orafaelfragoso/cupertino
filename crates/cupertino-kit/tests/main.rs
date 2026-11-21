//! The integration tests of `cupertino-kit`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[allow(dead_code)]
#[cfg(feature = "test")]
mod support;
#[cfg(feature = "test")]
#[path = "../../cupertino-wayland-client/tests/support/mod.rs"]
mod wire;

mod account;
#[cfg(feature = "test")]
mod bus;
#[cfg(feature = "test")]
#[path = "bus-ended.rs"]
mod bus_ended;
#[cfg(feature = "test")]
mod capture;
#[cfg(feature = "test")]
mod children;
#[cfg(feature = "test")]
mod clipboard;
#[cfg(feature = "test")]
#[path = "frame-away.rs"]
mod frame_away;
mod host;
mod image;
#[cfg(feature = "test")]
#[path = "io-burst.rs"]
mod io_burst;
#[cfg(feature = "test")]
#[path = "layers-handed-over.rs"]
mod layers_handed_over;
mod motion;
#[cfg(feature = "test")]
mod picture;
mod pixels;
#[cfg(feature = "test")]
mod preferences;
mod process;
#[cfg(feature = "test")]
mod settings;
#[path = "shared-caches.rs"]
mod shared_caches;
#[path = "shared-installed.rs"]
mod shared_installed;
#[cfg(feature = "test")]
#[path = "submitted-panic.rs"]
mod submitted_panic;
#[cfg(feature = "test")]
#[path = "suspended-in-flight.rs"]
mod suspended_in_flight;
#[path = "system-color.rs"]
mod system_color;
#[cfg(feature = "test")]
mod task;
mod time;
#[cfg(feature = "test")]
#[path = "two-instances.rs"]
mod two_instances;
