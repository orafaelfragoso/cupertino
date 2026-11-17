//! The generated interface tables: both roles see the same protocol, and every message dates from a version the
//! interface has.

use cupertino_protocols::{client, server};
use wayland_backend::protocol::{ArgumentType, Interface, MessageDesc};
use wayland_client::Proxy;
use wayland_server::Resource;

type Shape = Vec<(&'static str, u32, Vec<ArgumentType>, bool)>;

fn shape(messages: &'static [MessageDesc]) -> Shape {
    messages
        .iter()
        .map(|m| (m.name, m.since, m.signature.to_vec(), m.is_destructor))
        .collect()
}

#[test]
fn every_interface_agrees_between_client_and_server_and_dates_its_messages_within_its_version() {
    let pairs: [(&Interface, &Interface); 10] = [
        (
            client::cupertino_dock_v1::CupertinoDockV1::interface(),
            server::cupertino_dock_v1::CupertinoDockV1::interface(),
        ),
        (
            client::cupertino_menu_bar_v1::CupertinoMenuBarV1::interface(),
            server::cupertino_menu_bar_v1::CupertinoMenuBarV1::interface(),
        ),
        (
            client::cupertino_session_v1::CupertinoSessionV1::interface(),
            server::cupertino_session_v1::CupertinoSessionV1::interface(),
        ),
        (
            client::cupertino_shell_v1::CupertinoShellV1::interface(),
            server::cupertino_shell_v1::CupertinoShellV1::interface(),
        ),
        (
            client::cupertino_window_manager_v1::CupertinoWindowManagerV1::interface(),
            server::cupertino_window_manager_v1::CupertinoWindowManagerV1::interface(),
        ),
        (
            client::screenshot::cupertino_screenshot_v1::CupertinoScreenshotV1::interface(),
            server::screenshot::cupertino_screenshot_v1::CupertinoScreenshotV1::interface(),
        ),
        (
            client::control::cupertino_control_v1::CupertinoControlV1::interface(),
            server::control::cupertino_control_v1::CupertinoControlV1::interface(),
        ),
        (
            client::control::cupertino_control_session_v1::CupertinoControlSessionV1::interface(),
            server::control::cupertino_control_session_v1::CupertinoControlSessionV1::interface(),
        ),
        (
            client::control::cupertino_control_authority_v1::CupertinoControlAuthorityV1::interface(),
            server::control::cupertino_control_authority_v1::CupertinoControlAuthorityV1::interface(),
        ),
        (
            client::global_shortcuts::cupertino_global_shortcuts_v1::CupertinoGlobalShortcutsV1::interface(),
            server::global_shortcuts::cupertino_global_shortcuts_v1::CupertinoGlobalShortcutsV1::interface(),
        ),
    ];
    for (client, server) in pairs {
        let name = client.name;
        assert_eq!(name, server.name);
        assert_eq!(client.version, server.version, "version of {name}");
        assert_eq!(
            shape(client.requests),
            shape(server.requests),
            "requests of {name}"
        );
        assert_eq!(
            shape(client.events),
            shape(server.events),
            "events of {name}"
        );
        for message in client.requests.iter().chain(client.events) {
            assert!(
                (1..=client.version).contains(&message.since),
                "{name}.{} since {} of {}",
                message.name,
                message.since,
                client.version
            );
        }
    }
}
