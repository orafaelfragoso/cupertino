//! The daemon on a private bus: a private `dbus-daemon` is started for the test (skipped when the
//! binary is not installed), the daemon connects to it and a second connection plays the client.
#![cfg(feature = "dbus")]

use crate::common;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use common::{write_tree, BACKLIGHT, DISCHARGING_BAT0, KEYBOARD_BACKLIGHT};
use cupertino_powerd::auth::{Authorizer, Decision, LogindAuthorizer};
use cupertino_powerd::daemon::{Bus, Config, Daemon};
use cupertino_powerd::error::Result;
use cupertino_powerd::sysfs::Lid;
use cupertino_test_bus::PrivateBus;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

const UPOWER: &str = "org.freedesktop.UPower";
const POWER: &str = "org.cupertino.Power1";
const WAIT: Duration = Duration::from_secs(5);

/// Answers with a fixed decision and records the pids it was asked about.
struct Scripted {
    decision: Decision,
    asked: Arc<Mutex<Vec<u32>>>,
}

impl Authorizer for Scripted {
    fn authorize(&self, pid: u32) -> Result<Decision> {
        self.asked.lock().unwrap().push(pid);
        Ok(self.decision.clone())
    }
}

/// The authorizer and the list of pids it will be asked about.
fn scripted(decision: Decision) -> (Scripted, Arc<Mutex<Vec<u32>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    (
        Scripted {
            decision,
            asked: asked.clone(),
        },
        asked,
    )
}

fn start_daemon(bus: &PrivateBus, root: &Path, authorizer: Box<dyn Authorizer>) -> Daemon {
    Daemon::start(Config {
        bus: Bus::Address(bus.address.clone()),
        sysfs_root: root.to_owned(),
        lid: Lid {
            present: true,
            closed: false,
        },
        authorizer,
        history_path: None,
    })
    .unwrap()
}

fn proxy<'a>(client: &'a Connection, path: &'a str, interface: &'a str) -> Proxy<'a> {
    let name = if interface.starts_with(POWER) {
        POWER
    } else {
        UPOWER
    };
    // Uncached: the tests read properties after the daemon changed them.
    zbus::blocking::proxy::Builder::<Proxy>::new(client)
        .destination(name)
        .unwrap()
        .path(path)
        .unwrap()
        .interface(interface)
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .unwrap()
}

/// Forwards each `PropertiesChanged` of `path` as `(interface, changed properties)`.
fn property_changes(
    client: &Connection,
    path: &'static str,
) -> mpsc::Receiver<(String, HashMap<String, OwnedValue>)> {
    let properties = Proxy::new(client, UPOWER, path, "org.freedesktop.DBus.Properties").unwrap();
    let mut signals = properties.receive_signal("PropertiesChanged").unwrap();
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        for message in signals.by_ref() {
            let (interface, changed, _invalidated): (
                String,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize().unwrap();
            if sender.send((interface, changed)).is_err() {
                return;
            }
        }
    });
    received
}

/// Forwards each emission of `member` on `interface` at `path`, decoded by `decode`.
fn signals<T: Send + 'static>(
    client: &Connection,
    name: &'static str,
    path: &'static str,
    interface: &'static str,
    member: &'static str,
    decode: fn(&zbus::Message) -> T,
) -> mpsc::Receiver<T> {
    let mut stream = Proxy::new(client, name, path, interface)
        .unwrap()
        .receive_signal(member)
        .unwrap();
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        for message in stream.by_ref() {
            if sender.send(decode(&message)).is_err() {
                return;
            }
        }
    });
    received
}

fn decode_path(message: &zbus::Message) -> OwnedObjectPath {
    message.body().deserialize().unwrap()
}

fn call_error(result: zbus::Result<()>) -> String {
    match result.unwrap_err() {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        other => panic!("expected a method error, got {other}"),
    }
}

macro_rules! private_bus {
    () => {
        match PrivateBus::start() {
            Some(bus) => bus,
            None => {
                eprintln!("skipping: dbus-daemon is not available");
                return;
            }
        }
    };
}

#[test]
fn the_display_device_reports_percentage_and_state() {
    let bus = private_bus!();
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &DISCHARGING_BAT0);
    let _daemon = start_daemon(&bus, root.path(), Box::new(scripted(Decision::Allowed).0));
    let client = bus.connect();

    let manager = proxy(&client, "/org/freedesktop/UPower", UPOWER);
    let display: OwnedObjectPath = manager.call("GetDisplayDevice", &()).unwrap();
    assert_eq!(
        display.as_str(),
        "/org/freedesktop/UPower/devices/DisplayDevice"
    );
    let device = proxy(&client, display.as_str(), "org.freedesktop.UPower.Device");
    assert_eq!(device.get_property::<f64>("Percentage").unwrap(), 50.0);
    assert_eq!(device.get_property::<u32>("State").unwrap(), 2);
    assert_eq!(device.get_property::<i64>("TimeToEmpty").unwrap(), 9000);

    let devices: Vec<OwnedObjectPath> = manager.call("EnumerateDevices", &()).unwrap();
    let paths: Vec<&str> = devices.iter().map(|path| path.as_str()).collect();
    assert_eq!(paths, ["/org/freedesktop/UPower/devices/battery_BAT0"]);
    let battery = proxy(&client, paths[0], "org.freedesktop.UPower.Device");
    assert_eq!(
        battery.get_property::<String>("NativePath").unwrap(),
        "BAT0"
    );
    assert_eq!(battery.get_property::<String>("Model").unwrap(), "L19M4PF4");
    assert!(manager.get_property::<bool>("OnBattery").unwrap());
    assert!(manager.get_property::<bool>("LidIsPresent").unwrap());
    assert!(!manager.get_property::<bool>("LidIsClosed").unwrap());
}

#[test]
fn the_warning_level_rises_from_low_to_critical_and_external_power_clears_it() {
    let bus = private_bus!();
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &DISCHARGING_BAT0);
    // 4.5 Wh of 50 Wh: 9 percent, the early warning
    write_tree(
        root.path(),
        &[("class/power_supply/BAT0/energy_now", "4500000\n")],
    );
    let daemon = start_daemon(&bus, root.path(), Box::new(scripted(Decision::Allowed).0));
    let client = bus.connect();
    let display = proxy(
        &client,
        "/org/freedesktop/UPower/devices/DisplayDevice",
        "org.freedesktop.UPower.Device",
    );
    let battery = proxy(
        &client,
        "/org/freedesktop/UPower/devices/battery_BAT0",
        "org.freedesktop.UPower.Device",
    );
    assert_eq!(
        display.get_property::<u32>("WarningLevel").unwrap(),
        3,
        "low"
    );
    assert_eq!(
        battery.get_property::<u32>("WarningLevel").unwrap(),
        3,
        "the battery shares the machine's level"
    );
    let changes = property_changes(&client, "/org/freedesktop/UPower/devices/DisplayDevice");

    // 1 Wh: 2 percent, the final warning
    write_tree(
        root.path(),
        &[("class/power_supply/BAT0/energy_now", "1000000\n")],
    );
    daemon.refresh().unwrap();
    let level = |changes: &mpsc::Receiver<(String, HashMap<String, OwnedValue>)>| loop {
        let (_, changed) = changes.recv_timeout(WAIT).expect("a WarningLevel change");
        if let Some(v) = changed.get("WarningLevel") {
            break u32::try_from(v.try_clone().unwrap()).unwrap();
        }
    };
    assert_eq!(level(&changes), 4, "critical");

    write_tree(
        root.path(),
        &[
            ("class/power_supply/AC/type", "Mains\n"),
            ("class/power_supply/AC/online", "1\n"),
            ("class/power_supply/BAT0/status", "Charging\n"),
        ],
    );
    daemon.refresh().unwrap();
    assert_eq!(level(&changes), 1, "external power clears the warning");
    assert!(
        daemon.next_deadline().is_some(),
        "the change of the charging state opened the ignore window"
    );
}

#[test]
fn changed_files_and_a_closed_lid_become_properties_changed() {
    let bus = private_bus!();
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &DISCHARGING_BAT0);
    let daemon = start_daemon(&bus, root.path(), Box::new(scripted(Decision::Allowed).0));
    let client = bus.connect();
    let display = property_changes(&client, "/org/freedesktop/UPower/devices/DisplayDevice");
    let manager = property_changes(&client, "/org/freedesktop/UPower");

    // 20 Wh of 50 Wh, now charging: the percentage drops to 40 and the machine is on mains.
    write_tree(
        root.path(),
        &[
            ("class/power_supply/BAT0/energy_now", "20000000\n"),
            ("class/power_supply/BAT0/status", "Charging\n"),
        ],
    );
    write_tree(
        root.path(),
        &[
            ("class/power_supply/AC/type", "Mains\n"),
            ("class/power_supply/AC/online", "1\n"),
        ],
    );
    daemon.refresh().unwrap();

    let (interface, changed) = display
        .recv_timeout(WAIT)
        .expect("the display device reports the change");
    assert_eq!(interface, "org.freedesktop.UPower.Device");
    let mut names: Vec<&str> = changed.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "Energy",
            "IconName",
            "Percentage",
            "State",
            "TimeToEmpty",
            "TimeToFull"
        ]
    );
    assert_eq!(
        f64::try_from(changed["Percentage"].try_clone().unwrap()).unwrap(),
        40.0
    );
    assert_eq!(
        u32::try_from(changed["State"].try_clone().unwrap()).unwrap(),
        1
    );
    assert_eq!(
        i64::try_from(changed["TimeToFull"].try_clone().unwrap()).unwrap(),
        10800
    );
    let (interface, changed) = manager
        .recv_timeout(WAIT)
        .expect("the manager reports the change");
    assert_eq!(interface, "org.freedesktop.UPower");
    assert_eq!(changed.keys().collect::<Vec<_>>(), ["OnBattery"]);

    daemon
        .set_lid(Lid {
            present: true,
            closed: true,
        })
        .unwrap();
    let (_, changed) = manager
        .recv_timeout(WAIT)
        .expect("the manager reports the lid");
    assert_eq!(changed.keys().collect::<Vec<_>>(), ["LidIsClosed"]);
    assert!(bool::try_from(changed["LidIsClosed"].try_clone().unwrap()).unwrap());
}

#[test]
fn a_new_supply_is_announced_and_a_removed_one_is_withdrawn() {
    let bus = private_bus!();
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &DISCHARGING_BAT0);
    let daemon = start_daemon(&bus, root.path(), Box::new(scripted(Decision::Allowed).0));
    let client = bus.connect();
    let added = signals(
        &client,
        UPOWER,
        "/org/freedesktop/UPower",
        UPOWER,
        "DeviceAdded",
        decode_path,
    );
    let removed = signals(
        &client,
        UPOWER,
        "/org/freedesktop/UPower",
        UPOWER,
        "DeviceRemoved",
        decode_path,
    );

    write_tree(
        root.path(),
        &[
            ("class/power_supply/BAT1/type", "Battery\n"),
            ("class/power_supply/BAT1/status", "Full\n"),
            ("class/power_supply/BAT1/capacity", "100\n"),
        ],
    );
    daemon.refresh().unwrap();
    assert_eq!(
        added.recv_timeout(WAIT).unwrap().as_str(),
        "/org/freedesktop/UPower/devices/battery_BAT1"
    );
    let new = proxy(
        &client,
        "/org/freedesktop/UPower/devices/battery_BAT1",
        "org.freedesktop.UPower.Device",
    );
    assert_eq!(new.get_property::<f64>("Percentage").unwrap(), 100.0);

    fs::remove_dir_all(root.path().join("class/power_supply/BAT1")).unwrap();
    daemon.refresh().unwrap();
    assert_eq!(
        removed.recv_timeout(WAIT).unwrap().as_str(),
        "/org/freedesktop/UPower/devices/battery_BAT1"
    );
    assert!(new.get_property::<f64>("Percentage").is_err());
}

#[test]
fn set_brightness_writes_the_file_and_signals() {
    let bus = private_bus!();
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &BACKLIGHT);
    write_tree(root.path(), &KEYBOARD_BACKLIGHT);
    let (authorizer, asked) = scripted(Decision::Allowed);
    let _daemon = start_daemon(&bus, root.path(), Box::new(authorizer));
    let client = bus.connect();
    let display_changed = signals(
        &client,
        POWER,
        "/org/cupertino/Power1",
        POWER,
        "BrightnessChanged",
        |m| m.body().deserialize::<u32>().unwrap(),
    );
    let keyboard_changed = signals(
        &client,
        UPOWER,
        "/org/freedesktop/UPower/KbdBacklight",
        "org.freedesktop.UPower.KbdBacklight",
        "BrightnessChanged",
        |m| m.body().deserialize::<i32>().unwrap(),
    );

    let power = proxy(&client, "/org/cupertino/Power1", POWER);
    assert_eq!(
        power.call::<_, _, u32>("GetMaxBrightness", &()).unwrap(),
        100
    );
    power.call::<_, _, ()>("SetBrightness", &(70u32,)).unwrap();
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join("class/backlight/nvidia_wmi_ec_backlight/brightness")
        )
        .unwrap(),
        "70"
    );
    assert_eq!(power.call::<_, _, u32>("GetBrightness", &()).unwrap(), 70);
    assert_eq!(display_changed.recv_timeout(WAIT).unwrap(), 70);
    let too_bright = call_error(power.call::<_, _, ()>("SetBrightness", &(101u32,)));
    assert_eq!(too_bright, "org.cupertino.Power1.OutOfRange");
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join("class/backlight/nvidia_wmi_ec_backlight/brightness")
        )
        .unwrap(),
        "70"
    );

    let keyboard = proxy(
        &client,
        "/org/freedesktop/UPower/KbdBacklight",
        "org.freedesktop.UPower.KbdBacklight",
    );
    assert_eq!(
        keyboard.call::<_, _, i32>("GetMaxBrightness", &()).unwrap(),
        2
    );
    keyboard
        .call::<_, _, ()>("SetBrightness", &(2i32,))
        .unwrap();
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join("class/leds/tpacpi::kbd_backlight/brightness")
        )
        .unwrap(),
        "2"
    );
    assert_eq!(keyboard_changed.recv_timeout(WAIT).unwrap(), 2);
    let listed: Vec<OwnedObjectPath> = proxy(&client, "/org/freedesktop/UPower", UPOWER)
        .call("EnumerateKbdBacklights", &())
        .unwrap();
    assert_eq!(
        listed.iter().map(|path| path.as_str()).collect::<Vec<_>>(),
        ["/org/freedesktop/UPower/KbdBacklight/tpacpi__kbd_backlight"]
    );

    // The authorizer sees the pid of the caller, taken from the bus daemon.
    let me = std::process::id();
    assert_eq!(*asked.lock().unwrap(), [me, me, me]);
}

#[test]
fn an_unauthorised_caller_is_refused_but_may_read() {
    let bus = private_bus!();
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &BACKLIGHT);
    write_tree(root.path(), &KEYBOARD_BACKLIGHT);
    let _daemon = start_daemon(
        &bus,
        root.path(),
        Box::new(
            scripted(Decision::Denied(
                "session c2 is not the active session of seat0".to_owned(),
            ))
            .0,
        ),
    );
    let client = bus.connect();

    let power = proxy(&client, "/org/cupertino/Power1", POWER);
    assert_eq!(
        call_error(power.call::<_, _, ()>("SetBrightness", &(70u32,))),
        "org.cupertino.Power1.NotAuthorized"
    );
    let keyboard = proxy(
        &client,
        "/org/freedesktop/UPower/KbdBacklight",
        "org.freedesktop.UPower.KbdBacklight",
    );
    assert_eq!(
        call_error(keyboard.call::<_, _, ()>("SetBrightness", &(2i32,))),
        "org.freedesktop.UPower.GeneralError"
    );
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join("class/backlight/nvidia_wmi_ec_backlight/brightness")
        )
        .unwrap(),
        "40\n"
    );
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join("class/leds/tpacpi::kbd_backlight/brightness")
        )
        .unwrap(),
        "1\n"
    );
    assert_eq!(power.call::<_, _, u32>("GetBrightness", &()).unwrap(), 40);
}

/// A stand-in for logind: answers `GetSessionByPID` and serves one session.
mod fake_logind {
    use super::*;

    pub const SESSION_PATH: &str = "/org/freedesktop/login1/session/_32";
    /// The pid that belongs to no session.
    pub const STRANGER: u32 = 999;

    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.freedesktop.login1")]
    pub enum LoginError {
        #[zbus(error)]
        ZBus(zbus::Error),
        NoSessionForPID(String),
    }

    pub struct Manager;

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Manager {
        #[zbus(name = "GetSessionByPID")]
        fn get_session_by_pid(&self, pid: u32) -> Result<ObjectPath<'static>, LoginError> {
            if pid == STRANGER {
                return Err(LoginError::NoSessionForPID(format!(
                    "PID {pid} does not belong to any known session"
                )));
            }
            Ok(ObjectPath::from_static_str_unchecked(SESSION_PATH))
        }
    }

    pub struct Session {
        pub active: Arc<AtomicBool>,
        pub seat: &'static str,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl Session {
        #[zbus(property)]
        fn id(&self) -> String {
            "c2".to_owned()
        }
        #[zbus(property, name = "Type")]
        fn session_type(&self) -> String {
            "wayland".to_owned()
        }
        #[zbus(property)]
        fn active(&self) -> bool {
            self.active.load(Ordering::SeqCst)
        }
        #[zbus(property, name = "VTNr")]
        fn vt_nr(&self) -> u32 {
            2
        }
        #[zbus(property)]
        fn seat(&self) -> (String, OwnedObjectPath) {
            let path = if self.seat.is_empty() {
                "/"
            } else {
                "/org/freedesktop/login1/seat/seat0"
            };
            (
                self.seat.to_owned(),
                OwnedObjectPath::try_from(path).unwrap(),
            )
        }
    }

    pub fn start(bus: &PrivateBus, active: Arc<AtomicBool>, seat: &'static str) -> Connection {
        let connection = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .serve_at("/org/freedesktop/login1", Manager)
            .unwrap()
            .serve_at(SESSION_PATH, Session { active, seat })
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .build()
            .unwrap();
        connection
    }
}

#[test]
fn logind_allows_only_a_session_active_on_a_seat() {
    let bus = private_bus!();
    let active = Arc::new(AtomicBool::new(true));
    let _logind = fake_logind::start(&bus, active.clone(), "seat0");
    let authorizer = LogindAuthorizer::new(bus.connect());

    assert_eq!(authorizer.authorize(4242).unwrap(), Decision::Allowed);
    active.store(false, Ordering::SeqCst);
    assert_eq!(
        authorizer.authorize(4242).unwrap(),
        Decision::Denied("session c2 is not the active session of seat0".to_owned())
    );
    assert_eq!(
        authorizer.authorize(fake_logind::STRANGER).unwrap(),
        Decision::Denied("the caller is not in a login session".to_owned())
    );
}

#[test]
fn a_session_without_a_seat_is_refused() {
    let bus = private_bus!();
    let _logind = fake_logind::start(&bus, Arc::new(AtomicBool::new(true)), "");
    let authorizer = LogindAuthorizer::new(bus.connect());
    assert_eq!(
        authorizer.authorize(4242).unwrap(),
        Decision::Denied("session c2 has no seat".to_owned())
    );
}

#[test]
fn the_daemon_asks_logind_while_serving_a_call() {
    let bus = private_bus!();
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &BACKLIGHT);
    let active = Arc::new(AtomicBool::new(true));
    let _logind = fake_logind::start(&bus, active.clone(), "seat0");
    let _daemon = start_daemon(
        &bus,
        root.path(),
        Box::new(LogindAuthorizer::new(bus.connect())),
    );
    let client = bus.connect();
    let power = proxy(&client, "/org/cupertino/Power1", POWER);

    power.call::<_, _, ()>("SetBrightness", &(55u32,)).unwrap();
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join("class/backlight/nvidia_wmi_ec_backlight/brightness")
        )
        .unwrap(),
        "55"
    );
    active.store(false, Ordering::SeqCst);
    assert_eq!(
        call_error(power.call::<_, _, ()>("SetBrightness", &(10u32,))),
        "org.cupertino.Power1.NotAuthorized"
    );
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join("class/backlight/nvidia_wmi_ec_backlight/brightness")
        )
        .unwrap(),
        "55"
    );
}
