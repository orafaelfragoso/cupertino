//! The diagnostic events of the power service, read from a journal socket of the test's own: a start that fails on a
//! name another daemon holds, refused brightness requests, and the binary's panic and exit with an error. Each test
//! also checks that what the caller sees is unchanged.
#![cfg(feature = "dbus")]

mod common;
mod diag;

use common::{write_tree, BACKLIGHT, KEYBOARD_BACKLIGHT};
use cupertino_diagnostic_events::{self as events, Severity};
use cupertino_powerd::auth::{AllowAll, Authorizer, Decision};
use cupertino_powerd::daemon::{Bus, Config, Daemon};
use cupertino_powerd::error::Result;
use cupertino_powerd::sysfs::Lid;
use cupertino_test_bus::{missing_tool, PrivateBus};
use diag::Journal;

const UPOWER: &str = "org.freedesktop.UPower";
const POWER: &str = "org.cupertino.Power1";
const KEYBOARD: &str = "org.freedesktop.UPower.KbdBacklight";

struct Refuse;

impl Authorizer for Refuse {
    fn authorize(&self, _pid: u32) -> Result<Decision> {
        Ok(Decision::Denied(
            "session c2 is not the active session of seat0".to_owned(),
        ))
    }
}

fn config(bus: &PrivateBus, root: &std::path::Path, authorizer: Box<dyn Authorizer>) -> Config {
    Config {
        bus: Bus::Address(bus.address.clone()),
        sysfs_root: root.to_owned(),
        lid: Lid {
            present: true,
            closed: false,
        },
        authorizer,
    }
}

fn call_error(result: zbus::Result<zbus::Message>) -> String {
    match result.unwrap_err() {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
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
        let event = journal.one("power.backend.start_failed");
        event.check("power.backend.start_failed", Severity::Error, &[]);
        assert_eq!(event.field("SYSLOG_IDENTIFIER"), "cupertino-powerd");
        assert_eq!(event.text("step"), "bus");
        assert_eq!(event.text("kind"), "bus");
        assert_eq!(
            event.text("error"),
            "requesting a bus name: name already taken on the bus"
        );
        return;
    }
    events::init(events::component!(), "power", events::Sink::Journal);
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    // UPower runs
    let other = bus.connect();
    other.request_name(UPOWER).unwrap();
    let root = tempfile::tempdir().unwrap();
    let Err(error) = Daemon::start(config(&bus, root.path(), Box::new(AllowAll))) else {
        panic!("the daemon took a name another daemon holds");
    };
    assert_eq!(
        error.to_string(),
        "requesting a bus name: name already taken on the bus"
    );
}

#[test]
fn a_refused_brightness_names_its_interface_method_and_error() {
    if !diag::is_child() {
        let journal = Journal::new();
        if diag::child(
            "a_refused_brightness_names_its_interface_method_and_error",
            &journal,
        )
        .is_none()
        {
            return;
        }
        let failed: Vec<diag::Datagram> = journal
            .read()
            .into_iter()
            .filter(|d| d.record.id == "power.request.failed")
            .collect();
        assert_eq!(failed.len(), 2);
        let display = &failed[0];
        display.check("power.request.failed", Severity::Warn, &[]);
        assert_eq!(display.text("interface"), POWER);
        assert_eq!(display.text("method"), "SetBrightness");
        assert_eq!(display.text("error"), "org.cupertino.Power1.NotAuthorized");
        assert!(display.record.fields.get("backlight").is_none());
        let keyboard = &failed[1];
        keyboard.check("power.request.failed", Severity::Warn, &[]);
        assert_eq!(keyboard.text("interface"), KEYBOARD);
        assert_eq!(keyboard.text("method"), "SetBrightness");
        assert_eq!(
            keyboard.text("error"),
            "org.freedesktop.UPower.GeneralError"
        );
        assert_eq!(keyboard.text("backlight"), "tpacpi::kbd_backlight");
        return;
    }
    events::init(events::component!(), "power", events::Sink::Journal);
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &BACKLIGHT);
    write_tree(root.path(), &KEYBOARD_BACKLIGHT);
    let _daemon = Daemon::start(config(&bus, root.path(), Box::new(Refuse))).unwrap();
    let client = bus.connect();
    // the caller sees the same refusals as before, and nothing is written
    assert_eq!(
        call_error(client.call_method(
            Some(POWER),
            "/org/cupertino/Power1",
            Some(POWER),
            "SetBrightness",
            &(70u32,),
        )),
        "org.cupertino.Power1.NotAuthorized"
    );
    assert_eq!(
        call_error(client.call_method(
            Some(UPOWER),
            "/org/freedesktop/UPower/KbdBacklight/tpacpi__kbd_backlight",
            Some(KEYBOARD),
            "SetBrightness",
            &(2i32,),
        )),
        "org.freedesktop.UPower.GeneralError"
    );
    assert_eq!(
        std::fs::read_to_string(
            root.path()
                .join("class/backlight/nvidia_wmi_ec_backlight/brightness")
        )
        .unwrap(),
        "40\n"
    );
}

#[cfg(feature = "app")]
mod binary {
    use std::process::Command;

    use super::*;

    fn powerd(journal: &Journal) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cupertino-powerd"));
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
        let out = powerd(&journal)
            .env(events::TEST_PANIC_ENV, "power")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(101));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("panicked"), "{stderr}");
        let event = journal.one("power.panic");
        event.check("power.panic", Severity::Fatal, &[]);
        assert_eq!(event.field("SYSLOG_IDENTIFIER"), "cupertino-powerd");
        assert_eq!(
            event.text("panic"),
            "CUPERTINO_DIAG_TEST_PANIC asked this process to panic"
        );
    }

    #[test]
    fn an_exit_with_an_error_is_recorded_beside_the_same_message_and_status() {
        let journal = Journal::new();
        let out = powerd(&journal).arg("--no-auth").output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "cupertino-powerd: --no-auth is only allowed with --session-bus\n\
             usage: cupertino-powerd [--session-bus [--no-auth]] [--sysfs-root PATH]\n"
        );
        let event = journal.one("power.exit_error");
        event.check("power.exit_error", Severity::Error, &[]);
        assert_eq!(
            event.text("error"),
            "--no-auth is only allowed with --session-bus"
        );
    }
}
