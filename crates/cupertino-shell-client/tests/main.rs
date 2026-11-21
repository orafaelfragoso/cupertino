//! The integration tests of `cupertino-shell-client`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod support;

#[path = "control-wire.rs"]
mod control_wire;
#[path = "dock-wire.rs"]
mod dock_wire;
#[path = "idle-wire.rs"]
mod idle_wire;
#[path = "layer-shell-wire.rs"]
mod layer_shell_wire;
#[path = "menu-bar-wire.rs"]
mod menu_bar_wire;
#[path = "session-lock-wire.rs"]
mod session_lock_wire;
#[path = "shell-session-wire.rs"]
mod shell_session_wire;
#[path = "test-clock-fixed.rs"]
mod test_clock_fixed;
#[path = "test-clock-invalid.rs"]
mod test_clock_invalid;
#[path = "test-clock-unset.rs"]
mod test_clock_unset;
