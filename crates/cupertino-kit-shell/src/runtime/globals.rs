//! The versions of the globals a component's loop binds on its connection (`shell-host.md#versions`): the runtime
//! compares them with what the connection's registry offers before the component's thread starts.
//!
//! The session object and every role are messages of `cupertino_shell_v1` and of the interfaces it creates, which
//! take its version, so the one version that counts is the interface's. A global of `cupertino-protocols` must be
//! offered at the version this crate was built with, older or newer alike: a component never runs on a server whose
//! messages lie at other opcodes, and one of the development folder built against another server is answered
//! `unavailable` (`version`) and given back to the installed host. The layer shell, of another project, must be offered
//! at least at the oldest version the components work with.

use std::collections::HashMap;

use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::overview::cupertino_overview_manager_v1::CupertinoOverviewManagerV1;
use cupertino_protocols::client::transaction::cupertino_transaction_manager_v1::CupertinoTransactionManagerV1;
use wayland_client::Proxy;

use crate::{ComponentName, Error};

/// `cupertino_shell_v1` as the window manager binds it: the window manager's messages without the strip placement,
/// its rectangle and `strip_pressed` came with 29.
pub(crate) const SHELL_WINDOW_MANAGER_VERSION: u32 = 29;

/// `cupertino_shell_v1` as the on-screen displays bind it for the keyboard's panels (`get_keyboard_panels`), which came
/// with 30.
pub(crate) const SHELL_KEYBOARD_PANELS_VERSION: u32 = 30;
/// `zwlr_layer_shell_v1`: on-demand keyboard interactivity came with version 4, the oldest the components work with.
pub(crate) const LAYER_SHELL_VERSION: u32 = 4;
/// `cupertino_transaction_manager_v1`, which every component's connection sees, as the components bind it.
pub(crate) const TRANSACTION_VERSION: u32 = 1;
/// `cupertino_overview_manager_v1`, the overview's role, as the overview binds it.
pub(crate) const OVERVIEW_VERSION: u32 = 1;

pub(crate) const LAYER_SHELL: &str = "zwlr_layer_shell_v1";

/// The globals of `cupertino-protocols` `component` binds, each with the version this crate was built with.
fn built(component: ComponentName) -> Vec<(&'static str, u32)> {
    let mut built = vec![
        (
            CupertinoShellV1::interface().name,
            CupertinoShellV1::interface().version,
        ),
        (
            CupertinoTransactionManagerV1::interface().name,
            CupertinoTransactionManagerV1::interface().version,
        ),
    ];
    if component == ComponentName::OVERVIEW {
        built.push((
            CupertinoOverviewManagerV1::interface().name,
            CupertinoOverviewManagerV1::interface().version,
        ));
    }
    built
}

/// `Error::Version` when `offered`, the registry of `component`'s connection, lacks a global the component binds,
/// offers one of `cupertino-protocols` at another version than this crate was built with, or offers the layer shell
/// older than [`LAYER_SHELL_VERSION`]; the first such global is named in the log.
pub(crate) fn check(component: ComponentName, offered: &HashMap<String, u32>) -> Result<(), Error> {
    let found = |interface: &str| offered.get(interface).copied().unwrap_or(0);
    let refuse = |interface: &str, expected: u32, found: u32, why: &str| {
        tracing::warn!(
            component = component.as_str(),
            interface,
            expected,
            found,
            "{why}"
        );
        Err(Error::Version { expected, found })
    };
    for (interface, expected) in built(component) {
        let found = found(interface);
        if found != expected {
            return refuse(
                interface,
                expected,
                found,
                "the server offers another version of the global than the component was built with",
            );
        }
    }
    let layer_shell = found(LAYER_SHELL);
    if layer_shell < LAYER_SHELL_VERSION {
        return refuse(
            LAYER_SHELL,
            LAYER_SHELL_VERSION,
            layer_shell,
            "the server's layer shell is older than the component needs",
        );
    }
    Ok(())
}
