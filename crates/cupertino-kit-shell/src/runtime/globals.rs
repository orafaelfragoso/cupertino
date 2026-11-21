//! The versions of the globals a component's loop binds on its connection (`shell-host.md#versions`): the runtime
//! compares them with what the connection's registry offers before the component's thread starts, so a component
//! never runs on a server older than it needs.

use std::collections::HashMap;

use crate::{ComponentName, Error};

/// `cupertino_shell_v1`, the session object every component observes: `activate_app` came with version 20.
pub(crate) const SHELL_SESSION_VERSION: u32 = 20;
/// `cupertino_shell_v1` as the menu bar speaks it: the menu bar's messages without `session_agent` came with 31, which
/// moved the opcodes of the later events, so a menu bar never runs on an older server.
pub(crate) const SHELL_MENU_BAR_VERSION: u32 = 31;
/// `cupertino_shell_v1` as the dock speaks it: the dock's messages without the overview and the pictures of minimized
/// windows came with 28, which moved the opcodes of the later ones, so a dock never runs on an older server.
pub(crate) const SHELL_DOCK_VERSION: u32 = 28;
/// `cupertino_shell_v1` as the window manager speaks it: the window manager's messages without the strip placement,
/// its rectangle and `strip_pressed` came with 29, which moved the opcode of `done`, so a window manager never runs
/// on an older server.
pub(crate) const SHELL_WINDOW_MANAGER_VERSION: u32 = 29;

/// `cupertino_shell_v1` as the on-screen displays speak it: the keyboard's panels (`get_keyboard_panels`) came with 30.
pub(crate) const SHELL_KEYBOARD_PANELS_VERSION: u32 = 30;
/// `zwlr_layer_shell_v1`: on-demand keyboard interactivity came with version 4.
pub(crate) const LAYER_SHELL_VERSION: u32 = 4;
/// `cupertino_transaction_manager_v1`, which every component's connection sees.
pub(crate) const TRANSACTION_VERSION: u32 = 1;
/// `cupertino_overview_manager_v1`, the overview's role.
pub(crate) const OVERVIEW_VERSION: u32 = 1;

pub(crate) const SHELL: &str = "cupertino_shell_v1";
pub(crate) const LAYER_SHELL: &str = "zwlr_layer_shell_v1";
pub(crate) const TRANSACTION_MANAGER: &str = "cupertino_transaction_manager_v1";
pub(crate) const OVERVIEW_MANAGER: &str = "cupertino_overview_manager_v1";

/// The globals `component` binds, each with the oldest version it works with.
pub(crate) fn needs(component: ComponentName) -> Vec<(&'static str, u32)> {
    let shell = if component == ComponentName::DOCK {
        SHELL_DOCK_VERSION
    } else if component == ComponentName::WINDOWMANAGER {
        SHELL_WINDOW_MANAGER_VERSION
    } else if component == ComponentName::MENUBAR {
        SHELL_MENU_BAR_VERSION
    } else if component == ComponentName::OSD {
        SHELL_KEYBOARD_PANELS_VERSION
    } else {
        SHELL_SESSION_VERSION
    };
    let mut needs = vec![
        (SHELL, shell),
        (LAYER_SHELL, LAYER_SHELL_VERSION),
        (TRANSACTION_MANAGER, TRANSACTION_VERSION),
    ];
    if component == ComponentName::OVERVIEW {
        needs.push((OVERVIEW_MANAGER, OVERVIEW_VERSION));
    }
    needs
}

/// `Error::Version` when `offered`, the registry of `component`'s connection, lacks a global the component binds or
/// offers it older than the component needs; the first such global is named in the log.
pub(crate) fn check(component: ComponentName, offered: &HashMap<String, u32>) -> Result<(), Error> {
    for (interface, expected) in needs(component) {
        let found = offered.get(interface).copied().unwrap_or(0);
        if found < expected {
            tracing::warn!(
                component = component.as_str(),
                interface,
                expected,
                found,
                "the server's global is older than the component needs"
            );
            return Err(Error::Version { expected, found });
        }
    }
    Ok(())
}
