//! The diagnostic events of the agent, read from the user's event store of a state directory of the test's own: a
//! registration polkitd does not take, at the start and again after polkitd started anew, and the binary's panic and
//! exit with an error. Each test body runs in a process of its own (one emitter per process) and also checks that the
//! error is the same as before. The failed authentications are recorded in `flow.rs`, beside the prompt's flow.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use cupertino_diagnostic_events::{self as events, store, Record, Severity};
use cupertino_polkit_agent::agent::{serve, POLKIT};
use cupertino_test_bus::{missing_tool, PrivateBus};
use zbus::zvariant::OwnedValue;

const WAIT: Duration = Duration::from_secs(5);
const CHILD: &str = "DIAG_TEST_CHILD";

fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

/// Runs the test `name` of this binary again in a process of its own whose user store is under `state`; `None` when
/// the child skipped its test.
fn child(name: &str, state: &Path) -> Option<Output> {
    let output = Command::new(std::env::current_exe().unwrap())
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .args(["--exact", name, "--nocapture", "--test-threads", "1"])
        .env(CHILD, "1")
        .env("XDG_STATE_HOME", state)
        .env("HOME", state.join("home"))
        .env_remove("XDG_SESSION_ID")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the child failed: {stderr}");
    if stderr.contains("SKIPPED:") {
        eprintln!("{stderr}");
        return None;
    }
    Some(output)
}

fn of(state: &Path, id: &str) -> Vec<Record> {
    store::read(&state.join("cupertino/events"))
        .unwrap()
        .records
        .into_iter()
        .map(|stored| stored.record)
        .filter(|record| record.id == id)
        .collect()
}

fn text<'a>(record: &'a Record, key: &str) -> &'a str {
    record
        .fields
        .get(key)
        .unwrap_or_else(|| panic!("no field {key} in {}", record.to_json()))
        .as_str()
        .unwrap()
}

/// A polkitd that takes the agent's registration, or refuses it.
struct Authority {
    refuse: bool,
    registered: Sender<()>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl Authority {
    fn register_authentication_agent_with_options(
        &self,
        _subject: (String, HashMap<String, OwnedValue>),
        _locale: String,
        _object_path: String,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        let _ = self.registered.send(());
        if self.refuse {
            return Err(zbus::fdo::Error::AccessDenied(
                "an authentication agent already exists for the given subject".to_owned(),
            ));
        }
        Ok(())
    }
}

fn polkitd(bus: &PrivateBus, refuse: bool, registered: Sender<()>) -> zbus::blocking::Connection {
    bus.blocking_builder()
        .name(POLKIT)
        .unwrap()
        .serve_at(
            "/org/freedesktop/PolicyKit1/Authority",
            Authority { refuse, registered },
        )
        .unwrap()
        .build()
        .unwrap()
}

#[test]
fn a_registration_polkitd_does_not_take_is_recorded_at_the_start_and_again() {
    if !is_child() {
        let state = tempfile::tempdir().unwrap();
        let Some(out) = child(
            "a_registration_polkitd_does_not_take_is_recorded_at_the_start_and_again",
            state.path(),
        ) else {
            return;
        };
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(
                "cupertino-polkit-agent: registering again with polkitd: \
                 org.freedesktop.DBus.Error.AccessDenied: an authentication agent already exists for the given \
                 subject"
            ),
            "{stderr}"
        );
        let failed = of(state.path(), "polkit.register.failed");
        assert_eq!(failed.len(), 2);
        let start = &failed[0];
        assert_eq!(start.severity, Severity::Error);
        assert_eq!(start.component, "cupertino-polkit-agent");
        assert_eq!(text(start, "when"), "start");
        assert_eq!(
            text(start, "error_name"),
            "org.freedesktop.DBus.Error.ServiceUnknown"
        );
        assert!(
            text(start, "error").starts_with("org.freedesktop.DBus.Error.ServiceUnknown: "),
            "{}",
            text(start, "error")
        );
        let again = &failed[1];
        assert_eq!(text(again, "when"), "again");
        assert_eq!(
            text(again, "error_name"),
            "org.freedesktop.DBus.Error.AccessDenied"
        );
        assert_eq!(
            text(again, "error"),
            "org.freedesktop.DBus.Error.AccessDenied: an authentication agent already exists for the given subject"
        );
        return;
    }
    events::init(events::component!(), "polkit", events::Sink::UserStore);
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    // no polkitd: the agent does not start, with the same error as before
    let agent = bus.connect();
    let Err(error) = serve(&agent, "c2", "C", |_| {}) else {
        panic!("the agent registered without a polkitd");
    };
    let zbus::Error::MethodError(name, _, _) = &error else {
        panic!("expected a method error, got {error}");
    };
    assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown");

    // polkitd takes the registration, then starts anew and refuses it
    let agent = bus.connect();
    let (registered, registrations) = channel();
    let first = polkitd(&bus, false, registered.clone());
    serve(&agent, "c2", "C", |_| {}).unwrap();
    registrations.recv_timeout(WAIT).unwrap();
    drop(first);
    let _second = polkitd(&bus, true, registered);
    registrations
        .recv_timeout(WAIT)
        .expect("the agent registered again");
    // the refusal is answered before the agent's thread records it
    let deadline = std::time::Instant::now() + WAIT;
    while !events::recent()
        .iter()
        .any(|line| line.contains("\"when\":\"again\""))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "no second refusal recorded"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "app")]
mod binary {
    use super::*;

    fn agent(state: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cupertino-polkit-agent"));
        command
            .env_clear()
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env("XDG_STATE_HOME", state)
            .env("HOME", state.join("home"))
            .env("XDG_RUNTIME_DIR", state.join("runtime"));
        command
    }

    #[test]
    fn a_panic_is_recorded_and_the_process_ends_with_the_panic_status() {
        let state = tempfile::tempdir().unwrap();
        let out = agent(state.path())
            .env(events::TEST_PANIC_ENV, "polkit")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(101));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("panicked"), "{stderr}");
        let panics = of(state.path(), "polkit.panic");
        assert_eq!(panics.len(), 1);
        assert_eq!(panics[0].severity, Severity::Fatal);
        assert_eq!(panics[0].component, "cupertino-polkit-agent");
        assert_eq!(
            text(&panics[0], "panic"),
            "CUPERTINO_DIAG_TEST_PANIC asked this process to panic"
        );
    }

    #[test]
    fn an_exit_without_a_server_is_recorded_beside_the_same_message_and_status() {
        let state = tempfile::tempdir().unwrap();
        let out = agent(state.path()).output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        let message = stderr
            .strip_prefix("cupertino-polkit-agent: ")
            .and_then(|rest| rest.strip_suffix('\n'))
            .unwrap_or_else(|| panic!("{stderr}"));
        let exits = of(state.path(), "polkit.exit_error");
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].severity, Severity::Error);
        assert_eq!(text(&exits[0], "error"), message);
    }
}
