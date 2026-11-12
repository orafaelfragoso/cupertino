//! The diagnostic events of the Bluetooth service, read from a journal socket of the test's own: a start that fails on
//! a name another process holds, an adapter whose keys cannot be read, a refused request (its device address only as
//! a hash), and the binary's panic and exit with an error. Each test also checks that what the caller sees is
//! unchanged.
#![cfg(feature = "dbus")]

use crate::common;
use crate::diag;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::controller::FakeController;
use common::{OFF, POWERED};
use cupertino_btd::auth::{AllowAll, Authorizer, Decision};
use cupertino_btd::daemon::{Bus, Config, Daemon};
use cupertino_btd::error::Result;
use cupertino_btd::transport::NoBluetooth;
use cupertino_diagnostic_events::{self as events, Severity};
use cupertino_test_bus::{missing_tool, PrivateBus, WAIT};
use diag::Journal;
use zbus::blocking::Proxy;

const NAME: &str = "org.cupertino.Bluetooth1";
const PATH: &str = "/org/cupertino/Bluetooth1";
/// The test adapter's address, as the daemon writes it and as its key file is named.
const ADAPTER: &str = "AA:BB:CC:DD:EE:01";
const ADAPTER_FILE: &str = "AA-BB-CC-DD-EE-01";
/// A device's address that must reach the journal only as a hash.
const DEVICE: &str = "11:22:33:44:55:01";

struct Refuse;

impl Authorizer for Refuse {
    fn authorize(&self, _pid: u32) -> Result<Decision> {
        Ok(Decision::Denied(
            "session c2 is not the active session of seat0".to_owned(),
        ))
    }
}

fn config(
    bus: &PrivateBus,
    keys: &std::path::Path,
    transport: Arc<dyn cupertino_btd::transport::Transport>,
    authorizer: Box<dyn Authorizer>,
) -> Config {
    Config {
        bus: Bus::Address(bus.address.clone()),
        keys_dir: keys.to_owned(),
        read_only: false,
        authorizer,
        transport,
    }
}

fn proxy(client: &zbus::blocking::Connection) -> Proxy<'_> {
    zbus::blocking::proxy::Builder::<Proxy>::new(client)
        .destination(NAME)
        .unwrap()
        .path(PATH)
        .unwrap()
        .interface(NAME)
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .unwrap()
}

fn call_error<T: std::fmt::Debug>(result: zbus::Result<T>) -> String {
    match result.unwrap_err() {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        other => panic!("expected a method error, got {other}"),
    }
}

/// Waits until this process has recorded an event `id`.
fn recorded(id: &str) {
    let deadline = Instant::now() + WAIT;
    let needle = format!("\"id\":\"{id}\"");
    while !events::recent().iter().any(|line| line.contains(&needle)) {
        assert!(Instant::now() < deadline, "no {id} was recorded");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_name_another_process_holds_is_a_failed_start_and_the_same_error() {
    if !diag::is_child() {
        let journal = Journal::new();
        if diag::child(
            "a_name_another_process_holds_is_a_failed_start_and_the_same_error",
            &journal,
        )
        .is_none()
        {
            return;
        }
        let event = journal.one("bluetooth.backend.start_failed");
        event.check("bluetooth.backend.start_failed", Severity::Error, &[]);
        assert_eq!(event.field("SYSLOG_IDENTIFIER"), "cupertino-btd");
        assert_eq!(event.text("step"), "bus");
        assert_eq!(event.text("kind"), "bus");
        assert_eq!(
            event.text("error"),
            "requesting the bus name: name already taken on the bus"
        );
        return;
    }
    events::init(events::component!(), "bluetooth", events::Sink::Journal);
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    let other = bus.connect();
    other.request_name(NAME).unwrap();
    let keys = tempfile::tempdir().unwrap();
    let started = Daemon::start(config(
        &bus,
        keys.path(),
        Arc::new(NoBluetooth),
        Box::new(AllowAll),
    ));
    let Err(error) = started else {
        panic!("the daemon took a name another process holds");
    };
    assert_eq!(
        error.to_string(),
        "requesting the bus name: name already taken on the bus"
    );
}

#[test]
fn keys_others_may_read_leave_the_adapter_unavailable_and_name_it_only_as_a_hash() {
    if !diag::is_child() {
        let journal = Journal::new();
        let Some(out) = diag::child(
            "keys_others_may_read_leave_the_adapter_unavailable_and_name_it_only_as_a_hash",
            &journal,
        ) else {
            return;
        };
        // the line it printed before is there as it was
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&format!(
                "cupertino-btd: adapter {ADAPTER} stays unavailable: "
            )),
            "{stderr}"
        );
        let event = journal.one("bluetooth.backend.start_failed");
        event.check(
            "bluetooth.backend.start_failed",
            Severity::Error,
            &[ADAPTER, ADAPTER_FILE],
        );
        assert_eq!(event.text("step"), "keys");
        assert_eq!(event.text("kind"), "keys");
        let adapter = event.text("adapter");
        assert!(
            adapter.starts_with("bluetooth#") && adapter.len() == "bluetooth#".len() + 8,
            "{adapter}"
        );
        assert!(
            event
                .text("error")
                .ends_with(" 0644 lets others read the keys; fix it to 0600"),
            "{}",
            event.text("error")
        );
        return;
    }
    events::init(events::component!(), "bluetooth", events::Sink::Journal);
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    let keys = tempfile::tempdir().unwrap();
    let file = keys.path().join(format!("{ADAPTER_FILE}.keys"));
    std::fs::write(&file, "").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let controller = Arc::new(FakeController::new(POWERED, Vec::new()));
    let daemon = Daemon::start(config(
        &bus,
        keys.path(),
        controller.clone(),
        Box::new(AllowAll),
    ))
    .unwrap();
    let thread = std::thread::spawn(move || daemon.run().unwrap_err());
    recorded("bluetooth.backend.start_failed");
    let client = bus.connect();
    assert!(!proxy(&client).get_property::<bool>("Available").unwrap());
    controller.close();
    thread.join().unwrap();
}

#[test]
fn a_refused_request_names_its_method_and_error_and_the_device_only_as_a_hash() {
    if !diag::is_child() {
        let journal = Journal::new();
        if diag::child(
            "a_refused_request_names_its_method_and_error_and_the_device_only_as_a_hash",
            &journal,
        )
        .is_none()
        {
            return;
        }
        let failed: Vec<diag::Datagram> = journal
            .read()
            .into_iter()
            .filter(|d| d.record.id == "bluetooth.request.failed")
            .collect();
        assert_eq!(failed.len(), 2);
        let pair = &failed[0];
        pair.check(
            "bluetooth.request.failed",
            Severity::Warn,
            &[DEVICE, ADAPTER],
        );
        assert_eq!(pair.text("interface"), NAME);
        assert_eq!(pair.text("method"), "Pair");
        assert_eq!(pair.text("error"), "org.cupertino.Bluetooth1.NotAuthorized");
        let address = pair.text("address");
        assert!(
            address.starts_with("bluetooth#") && address.len() == "bluetooth#".len() + 8,
            "{address}"
        );
        let powered = &failed[1];
        powered.check("bluetooth.request.failed", Severity::Warn, &[]);
        assert_eq!(powered.text("method"), "Powered");
        assert_eq!(
            powered.text("error"),
            "org.freedesktop.DBus.Error.AuthFailed"
        );
        return;
    }
    events::init(events::component!(), "bluetooth", events::Sink::Journal);
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    let keys = tempfile::tempdir().unwrap();
    let controller = Arc::new(FakeController::new(OFF, Vec::new()));
    let daemon = Daemon::start(config(
        &bus,
        keys.path(),
        controller.clone(),
        Box::new(Refuse),
    ))
    .unwrap();
    let thread = std::thread::spawn(move || daemon.run().unwrap_err());
    let client = bus.connect();
    let proxy = proxy(&client);
    // the caller sees the same refusals as before
    assert_eq!(
        call_error(proxy.call::<_, _, ()>("Pair", &(DEVICE,))),
        "org.cupertino.Bluetooth1.NotAuthorized"
    );
    assert_eq!(
        proxy.set_property("Powered", true).unwrap_err().to_string(),
        "org.freedesktop.DBus.Error.AuthFailed: session c2 is not the active session of seat0"
    );
    controller.close();
    thread.join().unwrap();
}

#[cfg(feature = "app")]
mod binary {
    use std::process::Command;

    use super::*;

    fn btd(journal: &Journal) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cupertino-btd"));
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
        let out = btd(&journal)
            .env(events::TEST_PANIC_ENV, "bluetooth")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(101));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("panicked"), "{stderr}");
        let event = journal.one("bluetooth.panic");
        event.check("bluetooth.panic", Severity::Fatal, &[]);
        assert_eq!(event.field("SYSLOG_IDENTIFIER"), "cupertino-btd");
        assert_eq!(
            event.text("panic"),
            "CUPERTINO_DIAG_TEST_PANIC asked this process to panic"
        );
    }

    #[test]
    fn an_exit_with_an_error_is_recorded_beside_the_same_message_and_status() {
        let journal = Journal::new();
        let out = btd(&journal).arg("--no-auth").output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "cupertino-btd: --no-auth is only allowed with --session-bus\n\
             usage: cupertino-btd [--session-bus [--no-auth]] [--keys-dir PATH] [--read-only]\n"
        );
        let event = journal.one("bluetooth.exit_error");
        event.check("bluetooth.exit_error", Severity::Error, &[]);
        assert_eq!(
            event.text("error"),
            "--no-auth is only allowed with --session-bus"
        );
    }
}
