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
    let pairs: [(&Interface, &Interface); 18] = [
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
        (
            client::host::cupertino_host_v1::CupertinoHostV1::interface(),
            server::host::cupertino_host_v1::CupertinoHostV1::interface(),
        ),
        (
            client::host::cupertino_host_component_v1::CupertinoHostComponentV1::interface(),
            server::host::cupertino_host_component_v1::CupertinoHostComponentV1::interface(),
        ),
        (
            client::host::cupertino_component_v1::CupertinoComponentV1::interface(),
            server::host::cupertino_component_v1::CupertinoComponentV1::interface(),
        ),
        (
            client::transaction::cupertino_transaction_manager_v1::CupertinoTransactionManagerV1::interface(),
            server::transaction::cupertino_transaction_manager_v1::CupertinoTransactionManagerV1::interface(),
        ),
        (
            client::transaction::cupertino_transaction_v1::CupertinoTransactionV1::interface(),
            server::transaction::cupertino_transaction_v1::CupertinoTransactionV1::interface(),
        ),
        (
            client::transaction::cupertino_hole_v1::CupertinoHoleV1::interface(),
            server::transaction::cupertino_hole_v1::CupertinoHoleV1::interface(),
        ),
        (
            client::overview::cupertino_overview_manager_v1::CupertinoOverviewManagerV1::interface(),
            server::overview::cupertino_overview_manager_v1::CupertinoOverviewManagerV1::interface(),
        ),
        (
            client::overview::cupertino_overview_v1::CupertinoOverviewV1::interface(),
            server::overview::cupertino_overview_v1::CupertinoOverviewV1::interface(),
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

#[test]
fn transaction_limit_constants_match_protocol_xml() {
    use cupertino_protocols::transaction;

    let xml = include_str!("../protocols/cupertino-transaction-v1.xml");
    assert!(xml.contains(&format!(
        "{} milliseconds",
        transaction::DEFAULT_DEADLINE_EXTRA_MS
    )));
    assert!(xml.contains(&format!("{} milliseconds", transaction::MAX_DEADLINE_MS)));
    assert!(xml.contains(&format!("{} seconds", transaction::ENDED_RETENTION_SECS)));
    assert!(xml.contains(&format!(
        "{} transactions",
        transaction::MAX_ACTIVE_TRANSACTIONS
    )));
    assert!(xml.contains(&format!(
        "{} staged surfaces",
        transaction::MAX_STAGED_SURFACES
    )));
    assert!(xml.contains(&format!(
        "{} expected components",
        transaction::MAX_EXPECTED_COMPONENTS
    )));
    assert!(xml.contains(&format!("{} holes", transaction::MAX_HOLES_PER_CONNECTION)));
    assert!(xml.contains(&format!(
        "{} to {} seconds",
        transaction::SPRING_RESPONSE.0,
        transaction::SPRING_RESPONSE.1 as u32
    )));
    assert!(xml.contains(&format!(
        "{} to {}",
        transaction::SPRING_DAMPING.0,
        transaction::SPRING_DAMPING.1 as u32
    )));
}
