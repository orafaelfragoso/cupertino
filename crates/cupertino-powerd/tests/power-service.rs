//! The exported Power1 and UPower objects on a private bus, driven by changes to a fake sysfs.
#![cfg(feature = "dbus")]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cupertino_powerd::auth::{Authorizer, Decision};
use cupertino_powerd::daemon::{Bus, Config, Daemon};
use cupertino_powerd::sysfs::Lid;
use zbus::blocking::{connection::Builder, Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;

struct Fixture {
    root: PathBuf,
    bus: Child,
    address: String,
    client: Connection,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let mut bus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(bus.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        let client = Builder::address(address.as_str())
            .unwrap()
            .method_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        Self {
            root,
            bus,
            address,
            client,
        }
    }

    fn device(&self, dir: &str, attributes: &[(&str, &str)]) {
        let dir = self.root.join("class").join(dir);
        fs::create_dir_all(&dir).unwrap();
        for (name, value) in attributes {
            fs::write(dir.join(name), value).unwrap();
        }
    }

    fn start(&self, authorizer: Box<dyn Authorizer>) -> Daemon {
        Daemon::start(Config {
            bus: Bus::Address(self.address.clone()),
            sysfs_root: self.root.clone(),
            lid: Lid {
                present: false,
                closed: false,
            },
            authorizer,
            history_path: None,
        })
        .unwrap()
    }

    fn proxy(&self, name: &'static str, path: &str, interface: &'static str) -> Proxy<'_> {
        zbus::blocking::proxy::Builder::new(&self.client)
            .destination(name)
            .unwrap()
            .path(path.to_owned())
            .unwrap()
            .interface(interface)
            .unwrap()
            .cache_properties(CacheProperties::No)
            .build()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.bus.kill();
        let _ = self.bus.wait();
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn refresh_updates_the_display_battery_from_system_packs_and_keeps_the_last_good_snapshot() {
    let host = Fixture::new("display-battery");
    let daemon = host.start(Box::new(cupertino_powerd::auth::AllowAll));
    let manager = host.proxy(
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower",
        "org.freedesktop.UPower",
    );
    let path: OwnedObjectPath = manager.call("GetDisplayDevice", &()).unwrap();
    let display = host.proxy(
        "org.freedesktop.UPower",
        path.as_str(),
        "org.freedesktop.UPower.Device",
    );
    assert!(!display.get_property::<bool>("IsPresent").unwrap());
    // The second system pack is three times the size of the first. The peripheral is
    // enumerated for its owner but must not contribute any energy to the virtual battery.
    for (name, scope, now, full) in [
        ("BAT0", "System", "10000000", "20000000"),
        ("BAT1", "System", "10000000", "60000000"),
        ("mouse", "Device", "1000000", "1000000"),
    ] {
        host.device(
            &format!("power_supply/{name}"),
            &[
                ("type", "Battery"),
                ("scope", scope),
                ("status", "Discharging"),
                ("energy_now", now),
                ("energy_full", full),
            ],
        );
    }
    daemon.refresh().unwrap();
    assert!(manager.get_property::<bool>("OnBattery").unwrap());
    assert_eq!(display.get_property::<f64>("Percentage").unwrap(), 25.0);
    let inventory: Vec<OwnedObjectPath> = manager.call("EnumerateDevices", &()).unwrap();
    assert_eq!(inventory.len(), 3);
    let mouse = inventory
        .iter()
        .find(|p| p.as_str().ends_with("battery_mouse"))
        .unwrap();
    assert!(!host
        .proxy(
            "org.freedesktop.UPower",
            mouse.as_str(),
            "org.freedesktop.UPower.Device"
        )
        .get_property::<bool>("PowerSupply")
        .unwrap());
    host.device("power_supply/BAT1", &[("energy_now", "bad reading")]);
    assert!(daemon.refresh().is_err());
    assert_eq!(display.get_property::<f64>("Percentage").unwrap(), 25.0);
    fs::remove_dir_all(host.root.join("class/power_supply/BAT1")).unwrap();
    host.device("power_supply/AC", &[("type", "Mains"), ("online", "1")]);
    daemon.refresh().unwrap();
    assert_eq!(display.get_property::<f64>("Percentage").unwrap(), 50.0);
    assert!(!manager.get_property::<bool>("OnBattery").unwrap());
}

struct Foreground(Arc<AtomicBool>);
impl Authorizer for Foreground {
    fn authorize(&self, pid: u32) -> cupertino_powerd::Result<Decision> {
        assert_eq!(pid, std::process::id());
        Ok(if self.0.load(Ordering::SeqCst) {
            Decision::Allowed
        } else {
            Decision::Denied("background session".into())
        })
    }
}

#[test]
fn brightness_writes_require_a_foreground_caller_and_touch_only_the_selected_backlight() {
    let host = Fixture::new("brightness");
    for (name, kind) in [("raw0", "raw"), ("panel", "firmware")] {
        host.device(
            &format!("backlight/{name}"),
            &[
                ("type", kind),
                ("brightness", "4"),
                ("max_brightness", "10"),
            ],
        );
    }
    let foreground = Arc::new(AtomicBool::new(false));
    let daemon = host.start(Box::new(Foreground(foreground.clone())));
    let panel = host.proxy(
        "org.cupertino.Power1",
        "/org/cupertino/Power1",
        "org.cupertino.Power1",
    );
    for (active, value, refusal, after) in [
        (
            false,
            6u32,
            Some("org.cupertino.Power1.NotAuthorized"),
            4u32,
        ),
        (true, 11, Some("org.cupertino.Power1.OutOfRange"), 4),
        (true, 7, None, 7),
    ] {
        foreground.store(active, Ordering::SeqCst);
        let result = panel.call::<_, _, ()>("SetBrightness", &value);
        if let Some(expected) = refusal {
            assert!(
                matches!(result, Err(zbus::Error::MethodError(ref name,_,_)) if name.as_str() == expected),
                "{result:?}"
            );
        } else {
            result.unwrap();
        }
        assert_eq!(
            panel.call::<_, _, u32>("GetBrightness", &()).unwrap(),
            after
        );
        assert_eq!(
            fs::read_to_string(host.root.join("class/backlight/panel/brightness")).unwrap(),
            after.to_string()
        );
        assert_eq!(
            fs::read_to_string(host.root.join("class/backlight/raw0/brightness")).unwrap(),
            "4"
        );
    }
    fs::remove_dir_all(host.root.join("class/backlight/panel")).unwrap();
    daemon.refresh().unwrap();
    assert_eq!(panel.call::<_, _, u32>("GetBrightness", &()).unwrap(), 4);
}
