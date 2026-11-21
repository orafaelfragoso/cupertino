//! The integration tests of `cupertino-wayland-client`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod support;

#[path = "app-effects-wire.rs"]
mod app_effects_wire;
#[path = "commit-order.rs"]
mod commit_order;
#[path = "keymap-wire.rs"]
mod keymap_wire;
#[path = "layer-values.rs"]
mod layer_values;
#[path = "layers-wire.rs"]
mod layers_wire;
#[path = "scroll-wire.rs"]
mod scroll_wire;
#[path = "semantics-publishing.rs"]
mod semantics_publishing;
#[path = "transport-connection.rs"]
mod transport_connection;
#[path = "transport-values.rs"]
mod transport_values;
