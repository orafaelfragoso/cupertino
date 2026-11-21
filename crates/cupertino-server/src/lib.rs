//! Cupertino compositor library: Smithay-based Wayland compositor with window policy from
//! `cupertino-wm`, GLES window shadows, rounded corners and material passes.
//!
//! See the crate README for the architecture and the integration points.

pub mod accessibility;
pub use cupertino_server_backend::{
    acquire_waits, drm_trace, latch, output_depth, schedule, software_render, sync_file,
};
pub use cupertino_server_input::{input, kbd};
pub use cupertino_server_render::{decor, frame_fill, fx, popup_linger, toolkit_programs};
pub mod activation_env;
pub mod app_effects_v1;
pub mod authorization;
pub mod backend;
pub mod caller;
pub mod capture;
pub mod cli;
pub mod clipboard;
pub mod config;
pub mod control;
pub mod cursor;
pub mod data_control;
pub mod dbus;
pub mod decor_access;
pub mod desktop;
pub mod display_power;
pub mod displays;
pub mod dock;
pub mod drag_return;
pub mod events;
pub mod file_limit;
pub mod force_quit;
pub mod frame;
pub mod frame_stats;
pub mod global_shortcuts;
pub mod gpu_reset;
pub mod handlers;
pub mod host;
pub mod hung;
pub mod idle;
pub mod input_method;
pub mod kbd_filters;
pub mod kbd_ui;
pub mod layers_v1;
pub mod live;
pub mod localed;
pub mod logging;
pub mod menu_bar;
pub mod overlays;
pub mod overview_v1;
pub mod owner_check;
pub mod pointer_constraints;
pub mod policy;
pub mod popup_grab;
pub mod power;
pub mod process;
pub mod protocols;
pub mod quit;
pub mod render;
pub mod restart_policy;
pub mod runtime_info;
pub mod screen_reader;
pub mod scroll_v1;
pub mod session;
pub mod session_agent;
pub mod session_lock;
pub mod session_v1;
pub mod shell;
pub mod shell_connection;
pub mod shell_ctl;
pub mod shell_v1;
pub mod shortcut_inhibit;
pub mod state;
pub mod store_later;
pub mod supervisor;
pub mod suspended;
pub mod toplevel_icon;
pub mod transaction;
pub mod window_manager;
pub mod xwayland;

pub use config::Config;
pub use state::Cupertino;
