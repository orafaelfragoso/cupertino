//! The diagnostic events of the network service, read from a journal socket of the test's own: a start that fails on a
//! name another daemon holds, a refused request (its SSID only as a hash), and the binary's panic and exit with an
//! error. Each test also checks that what the caller sees is unchanged.
#![cfg(feature = "dbus")]

mod diag;

use cupertino_diagnostic_events::{self as events, Severity};
use cupertino_netd::auth::{AllowAll, Authorizer, Decision};
use cupertino_netd::daemon::{Bus, Config, Daemon};
use cupertino_netd::error::Result;
use cupertino_test_bus::{missing_tool, PrivateBus};
use diag::Journal;

const NETWORK: &str = "org.cupertino.Network1";
const NETWORK_PATH: &str = "/org/cupertino/Network1";
const MANAGER: &str = "org.freedesktop.NetworkManager";
const MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";
/// An SSID and a passphrase that must never reach the journal.
const SSID: &str = "Corner Cafe Guest 5G";
const PASSPHRASE: &str = "hunter2-correct-horse";

struct Refuse;

impl Authorizer for Refuse {
    fn authorize(&self, _pid: u32) -> Result<Decision> {
        Ok(Decision::Denied("not in the active session".to_owned()))
    }
}

fn error_name<T: std::fmt::Debug>(result: zbus::Result<T>) -> String {
    match result.unwrap_err() {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        zbus::Error::FDO(error) => zbus::DBusError::name(&*error).to_string(),
        other => panic!("expected a method error, got {other}"),
    }
}

#[test]
fn a_name_another_daemon_holds_is_a_failed_start_and_the_same_error() {
    if !diag::is_child() {
        let journal = Journal::new();
        if diag::child(
            "a_name_another_daemon_holds_is_a_failed_start_and_the_same_error",
            &journal,
        )
        .is_none()
        {
            return;
        }
        let event = journal.one("network.backend.start_failed");
        event.check("network.backend.start_failed", Severity::Error, &[]);
        assert_eq!(event.field("SYSLOG_IDENTIFIER"), "cupertino-netd");
        assert_eq!(event.text("step"), "bus_name");
        assert_eq!(event.text("kind"), "name_taken");
        assert_eq!(
            event.text("error"),
            "the bus name org.freedesktop.NetworkManager is owned by another process"
        );
        return;
    }
    events::init(events::component!(), "network", events::Sink::Journal);
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    // NetworkManager runs
    let other = bus.connect();
    other.request_name(MANAGER).unwrap();
    let (daemon, _requests) = Daemon::connect(Config {
        bus: Bus::Address(bus.address.clone()),
        authorizer: Box::new(AllowAll),
    })
    .unwrap();
    let error = daemon.claim_names().unwrap_err();
    assert_eq!(
        error.to_string(),
        "the bus name org.freedesktop.NetworkManager is owned by another process"
    );
}

#[test]
fn a_refused_request_names_its_method_and_error_and_never_the_network() {
    if !diag::is_child() {
        let journal = Journal::new();
        if diag::child(
            "a_refused_request_names_its_method_and_error_and_never_the_network",
            &journal,
        )
        .is_none()
        {
            return;
        }
        let failed: Vec<diag::Datagram> = journal
            .read()
            .into_iter()
            .filter(|d| d.record.id == "network.request.failed")
            .collect();
        assert_eq!(failed.len(), 2);
        let connect = &failed[0];
        connect.check(
            "network.request.failed",
            Severity::Warn,
            &[SSID, PASSPHRASE, "Cafe Guest"],
        );
        assert_eq!(connect.text("interface"), NETWORK);
        assert_eq!(connect.text("method"), "Connect");
        assert_eq!(
            connect.text("error"),
            "org.cupertino.Network1.NotAuthorized"
        );
        let ssid = connect.text("ssid");
        assert!(
            ssid.starts_with("ssid#") && ssid.len() == "ssid#".len() + 8,
            "{ssid}"
        );
        let radio = &failed[1];
        radio.check("network.request.failed", Severity::Warn, &[]);
        assert_eq!(radio.text("interface"), MANAGER);
        assert_eq!(radio.text("method"), "WirelessEnabled");
        assert_eq!(
            radio.text("error"),
            "org.freedesktop.DBus.Error.AccessDenied"
        );
        assert!(radio.record.fields.get("ssid").is_none());
        return;
    }
    events::init(events::component!(), "network", events::Sink::Journal);
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    let (daemon, _requests) = Daemon::connect(Config {
        bus: Bus::Address(bus.address.clone()),
        authorizer: Box::new(Refuse),
    })
    .unwrap();
    daemon.claim_names().unwrap();
    let client = bus.connect();
    // the caller sees the same refusals as before
    let connect = client.call_method(
        Some(NETWORK),
        NETWORK_PATH,
        Some(NETWORK),
        "Connect",
        &(SSID, PASSPHRASE),
    );
    assert_eq!(error_name(connect), "org.cupertino.Network1.NotAuthorized");
    let radio = zbus::blocking::fdo::PropertiesProxy::builder(&client)
        .destination(MANAGER)
        .unwrap()
        .path(MANAGER_PATH)
        .unwrap()
        .build()
        .unwrap()
        .set(
            MANAGER.try_into().unwrap(),
            "WirelessEnabled",
            zbus::zvariant::Value::from(false),
        );
    assert_eq!(
        zbus::DBusError::name(&radio.unwrap_err()).to_string(),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
}

#[cfg(feature = "app")]
mod binary {
    use std::process::Command;

    use super::*;

    fn netd(journal: &Journal) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cupertino-netd"));
        command
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env(events::journal::SOCKET_ENV, journal.path())
            .env_remove(events::TEST_PANIC_ENV)
            .env_remove("STATE_DIRECTORY");
        command
    }

    #[test]
    fn a_panic_is_recorded_and_the_process_ends_with_the_panic_status() {
        let journal = Journal::new();
        let out = netd(&journal)
            .env(events::TEST_PANIC_ENV, "network")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(101));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("panicked"), "{stderr}");
        let event = journal.one("network.panic");
        event.check("network.panic", Severity::Fatal, &[]);
        assert_eq!(event.field("SYSLOG_IDENTIFIER"), "cupertino-netd");
        assert_eq!(
            event.text("panic"),
            "CUPERTINO_DIAG_TEST_PANIC asked this process to panic"
        );
    }

    #[test]
    fn an_exit_with_an_error_is_recorded_beside_the_same_message_and_status() {
        let journal = Journal::new();
        let out = netd(&journal).arg("--no-auth").output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "cupertino-netd: --no-auth is only allowed with --session-bus\n\
             usage: cupertino-netd [--session-bus [--no-auth]] [--resolv-conf PATH]\n"
        );
        let event = journal.one("network.exit_error");
        event.check("network.exit_error", Severity::Error, &[]);
        assert_eq!(
            event.text("error"),
            "--no-auth is only allowed with --session-bus"
        );
    }
}
