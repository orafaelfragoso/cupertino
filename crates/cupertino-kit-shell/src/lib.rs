//! The shell's extension to CupertinoKit. The host bus uses component names, output names from the registry and
//! transaction IDs that remain meaningful on both ends of a bridge.

pub mod bus;
mod component;
pub mod outputs;
pub mod transaction;

pub use component::ComponentName;

/// Clocks for what the shell draws. Never use a display clock for authentication, waits or IPC deadlines.
pub mod test_clock {
    pub use cupertino_shell_client::test_clock::{
        display_now, display_unix_seconds, motion_settled,
    };
}
