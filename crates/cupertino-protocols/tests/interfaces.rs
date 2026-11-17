//! The generated interface tables: both roles see the same protocol, and the versions are the ones the README promises.

use cupertino_protocols::{client, server};
use wayland_backend::protocol::{Interface, MessageDesc};
use wayland_client::Proxy;
use wayland_server::Resource;

fn shape(
    messages: &'static [MessageDesc],
) -> Vec<(
    &'static str,
    u32,
    Vec<wayland_backend::protocol::ArgumentType>,
    bool,
)> {
    messages
        .iter()
        .map(|m| (m.name, m.since, m.signature.to_vec(), m.is_destructor))
        .collect()
}

fn same(client: &'static Interface, server: &'static Interface) {
    assert_eq!(client.name, server.name);
    assert_eq!(client.version, server.version);
    assert_eq!(
        shape(client.requests),
        shape(server.requests),
        "requests of {}",
        client.name
    );
    assert_eq!(
        shape(client.events),
        shape(server.events),
        "events of {}",
        client.name
    );
}

#[test]
fn shell_interfaces_agree_between_client_and_server() {
    same(
        client::cupertino_dock_v1::CupertinoDockV1::interface(),
        server::cupertino_dock_v1::CupertinoDockV1::interface(),
    );
    same(
        client::cupertino_menu_bar_v1::CupertinoMenuBarV1::interface(),
        server::cupertino_menu_bar_v1::CupertinoMenuBarV1::interface(),
    );
    same(
        client::cupertino_session_v1::CupertinoSessionV1::interface(),
        server::cupertino_session_v1::CupertinoSessionV1::interface(),
    );
    same(
        client::cupertino_shell_v1::CupertinoShellV1::interface(),
        server::cupertino_shell_v1::CupertinoShellV1::interface(),
    );
    same(
        client::cupertino_window_manager_v1::CupertinoWindowManagerV1::interface(),
        server::cupertino_window_manager_v1::CupertinoWindowManagerV1::interface(),
    );
}

#[test]
fn separate_protocol_modules_agree_between_client_and_server() {
    same(
        client::screenshot::cupertino_screenshot_v1::CupertinoScreenshotV1::interface(),
        server::screenshot::cupertino_screenshot_v1::CupertinoScreenshotV1::interface(),
    );
    same(
        client::control::cupertino_control_v1::CupertinoControlV1::interface(),
        server::control::cupertino_control_v1::CupertinoControlV1::interface(),
    );
    same(
        client::control::cupertino_control_session_v1::CupertinoControlSessionV1::interface(),
        server::control::cupertino_control_session_v1::CupertinoControlSessionV1::interface(),
    );
    same(
        client::control::cupertino_control_authority_v1::CupertinoControlAuthorityV1::interface(),
        server::control::cupertino_control_authority_v1::CupertinoControlAuthorityV1::interface(),
    );
    same(
        client::global_shortcuts::cupertino_global_shortcuts_v1::CupertinoGlobalShortcutsV1::interface(),
        server::global_shortcuts::cupertino_global_shortcuts_v1::CupertinoGlobalShortcutsV1::interface(),
    );
}

#[test]
fn versions_match_the_readme() {
    let version = |i: &Interface| (i.name, i.version);
    assert_eq!(
        version(client::cupertino_shell_v1::CupertinoShellV1::interface()),
        ("cupertino_shell_v1", 26)
    );
    assert_eq!(
        version(client::cupertino_dock_v1::CupertinoDockV1::interface()),
        ("cupertino_dock_v1", 26)
    );
    assert_eq!(
        version(client::cupertino_menu_bar_v1::CupertinoMenuBarV1::interface()),
        ("cupertino_menu_bar_v1", 25)
    );
    assert_eq!(
        version(client::cupertino_session_v1::CupertinoSessionV1::interface()),
        ("cupertino_session_v1", 20)
    );
    assert_eq!(
        version(client::control::cupertino_control_v1::CupertinoControlV1::interface()),
        ("cupertino_control_v1", 5)
    );
    assert_eq!(
        version(
            client::global_shortcuts::cupertino_global_shortcuts_v1::CupertinoGlobalShortcutsV1::interface()
        ),
        ("cupertino_global_shortcuts_v1", 1)
    );
}

#[test]
fn every_message_is_available_from_the_version_that_added_it() {
    let interfaces = [
        client::cupertino_shell_v1::CupertinoShellV1::interface(),
        client::cupertino_dock_v1::CupertinoDockV1::interface(),
        client::cupertino_menu_bar_v1::CupertinoMenuBarV1::interface(),
        client::cupertino_session_v1::CupertinoSessionV1::interface(),
        client::cupertino_window_manager_v1::CupertinoWindowManagerV1::interface(),
        client::control::cupertino_control_v1::CupertinoControlV1::interface(),
        client::control::cupertino_control_session_v1::CupertinoControlSessionV1::interface(),
        client::control::cupertino_control_authority_v1::CupertinoControlAuthorityV1::interface(),
    ];
    for interface in interfaces {
        for message in interface.requests.iter().chain(interface.events) {
            assert!(
                (1..=interface.version).contains(&message.since),
                "{}.{} since {} of {}",
                interface.name,
                message.name,
                message.since,
                interface.version
            );
        }
    }
}

#[test]
fn the_shortcuts_interface_has_one_destructor_and_the_documented_messages() {
    let shortcuts = client::global_shortcuts::cupertino_global_shortcuts_v1::CupertinoGlobalShortcutsV1::interface();
    let requests: Vec<_> = shortcuts.requests.iter().map(|m| m.name).collect();
    let events: Vec<_> = shortcuts.events.iter().map(|m| m.name).collect();
    assert_eq!(requests, ["destroy", "bind", "unbind"]);
    assert_eq!(events, ["reserved", "done", "pressed", "released"]);
    let destructors: Vec<_> = shortcuts
        .requests
        .iter()
        .filter(|m| m.is_destructor)
        .map(|m| m.name)
        .collect();
    assert_eq!(destructors, ["destroy"]);
}
