//! Cupertino compositor library: Smithay-based Wayland compositor with window policy from
//! `cupertino-wm`, GLES window shadows, rounded corners and material passes.
//!
//! See the crate README for the architecture and the integration points.

pub mod accessibility;
pub mod backend;
pub mod capture;
pub mod cli;
pub mod clipboard;
pub mod config;
pub mod cursor;
pub mod dbus;
pub mod decor;
pub mod desktop;
pub mod display_power;
pub mod displays;
pub mod events;
pub mod dock;
pub mod frame;
pub mod fx;
pub mod handlers;
pub mod idle;
pub mod input;
pub mod input_method;
pub mod kbd;
pub mod kbd_filters;
pub mod kbd_ui;
pub mod live;
pub mod menu_bar;
pub mod overlays;
pub mod policy;
pub mod power;
pub mod protocols;
pub mod quit;
pub mod render;
pub mod restart_policy;
pub mod scroll_v1;
pub mod session_lock;
pub mod session_v1;
pub mod shell;
pub mod shell_ctl;
pub mod shell_v1;
pub mod state;
pub mod supervisor;
pub mod window_manager;

pub use config::Config;
pub use state::Cupertino;
