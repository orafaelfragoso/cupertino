//! The integration tests of `cupertino-server`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "copy-sync.rs"]
mod copy_sync;
#[path = "cursor-reader.rs"]
mod cursor_reader;
#[path = "frame-not-ready.rs"]
mod frame_not_ready;
#[path = "host-protocol.rs"]
mod host_protocol;
#[path = "host-spawning.rs"]
mod host_spawning;
#[path = "keyboard-panels.rs"]
mod keyboard_panels;
#[path = "keymap-fallback.rs"]
mod keymap_fallback;
#[path = "live-settings-deadlines.rs"]
mod live_settings_deadlines;
#[path = "lock-screen.rs"]
mod lock_screen;
#[path = "menu-bar-version.rs"]
mod menu_bar_version;
#[path = "overview-and-config.rs"]
mod overview_and_config;
#[path = "overview-routing.rs"]
mod overview_routing;
#[path = "process-identity.rs"]
mod process_identity;
#[path = "protected-controls.rs"]
mod protected_controls;
#[path = "scripted-frames.rs"]
mod scripted_frames;
#[path = "session-and-displays.rs"]
mod session_and_displays;
#[path = "shell-components.rs"]
mod shell_components;
mod transactions;
#[path = "xwayland-watch.rs"]
mod xwayland_watch;
