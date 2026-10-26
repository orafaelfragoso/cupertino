//! The extras task against stand-ins for the network, power, Bluetooth and notification services on a private bus
//! (`dbus-daemon --session`, used as both the system and the session bus); skipped when the daemon is not available.
#![cfg(feature = "app")]

use cupertino_menubar::extras::{
    BatteryState, BluetoothDevice, ExtraAction, Extras, FocusLifetime, WifiNetwork,
};
use cupertino_menubar::extras_bus::ExtrasBus;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{block_on, connection, Connection};

struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout)
            .ok()
            .filter(|_| out.status.success())?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next()?.to_string(),
            pid: lines.next()?.to_string(),
        })
    }

    fn connect(&self) -> Connection {
        block_on(
            connection::Builder::address(self.address.as_str())
                .expect("address")
                .build(),
        )
        .expect("connect")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        Command::new("kill")
            .arg(&self.pid)
            .status()
            .expect("kill the private bus");
    }
}

type DeviceRecord = (u32, String, String, String, String, String, String);
type NetworkRecord = (String, i32, String, bool, bool);

/// Stands in for `cupertino-netd`: one Wi-Fi device joined to `Home`; `Connect` joins the network it names.
struct FakeNetwork {
    connected: Arc<Mutex<String>>,
    calls: mpsc::Sender<(String, String)>,
}

#[zbus::interface(name = "org.cupertino.Network1")]
impl FakeNetwork {
    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<DeviceRecord> {
        let ssid = self.connected.lock().unwrap().clone();
        vec![(
            3,
            "wlan0".into(),
            "wifi".into(),
            "connected".into(),
            ssid,
            String::new(),
            String::new(),
        )]
    }

    fn get_networks(&self) -> Vec<NetworkRecord> {
        let connected = self.connected.lock().unwrap().clone();
        [
            ("Home", -50, "wpa2", true),
            ("cafe", i32::MIN, "open", false),
        ]
        .into_iter()
        .map(|(ssid, signal, security, known)| {
            (
                ssid.into(),
                signal,
                security.into(),
                known,
                connected == ssid,
            )
        })
        .collect()
    }

    fn connect(&self, ssid: String, passphrase: String) {
        *self.connected.lock().unwrap() = ssid.clone();
        self.calls.send((ssid, passphrase)).unwrap();
    }
}

struct FakeDisplayDevice;

#[zbus::interface(name = "org.freedesktop.UPower.Device")]
impl FakeDisplayDevice {
    #[zbus(property)]
    fn is_present(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn percentage(&self) -> f64 {
        47.4
    }

    /// Discharging.
    #[zbus(property)]
    fn state(&self) -> u32 {
        2
    }
}

struct FakeUPower;

#[zbus::interface(name = "org.freedesktop.UPower")]
impl FakeUPower {
    #[zbus(property)]
    fn on_battery(&self) -> bool {
        true
    }
}

struct FakeBluetooth;

#[zbus::interface(name = "org.cupertino.Bluetooth1")]
impl FakeBluetooth {
    #[zbus(property)]
    fn available(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn powered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<HashMap<String, OwnedValue>> {
        let value = |v: Value<'_>| OwnedValue::try_from(v).unwrap();
        let mut device = HashMap::new();
        device.insert(
            "Id".to_owned(),
            value(Value::from("AA:BB:CC:DD:EE:FF/bredr")),
        );
        device.insert(
            "Address".to_owned(),
            value(Value::from("AA:BB:CC:DD:EE:FF")),
        );
        device.insert("Name".to_owned(), value(Value::from("Headphones")));
        device.insert("Paired".to_owned(), value(Value::from(true)));
        device.insert("Connected".to_owned(), value(Value::from(false)));
        vec![device]
    }
}

struct FakeCentre;

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl FakeCentre {
    #[zbus(property)]
    fn do_not_disturb(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn focus_lifetimes(&self) -> Vec<(String, String)> {
        vec![("one-hour".into(), "For 1 hour".into())]
    }
}

fn next(events: &mpsc::Receiver<Extras>) -> Extras {
    events
        .recv_timeout(Duration::from_secs(5))
        .expect("the extras task reports")
}

#[test]
fn the_extras_follow_the_services_and_act_on_them() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (calls_tx, calls) = mpsc::channel();
    let services = bus.connect();
    block_on(async {
        let server = services.object_server();
        server
            .at(
                "/org/cupertino/Network1",
                FakeNetwork {
                    connected: Arc::new(Mutex::new("Home".into())),
                    calls: calls_tx,
                },
            )
            .await
            .unwrap();
        server
            .at(
                "/org/freedesktop/UPower/devices/DisplayDevice",
                FakeDisplayDevice,
            )
            .await
            .unwrap();
        server
            .at("/org/freedesktop/UPower", FakeUPower)
            .await
            .unwrap();
        server
            .at("/org/cupertino/Bluetooth1", FakeBluetooth)
            .await
            .unwrap();
        server
            .at("/org/cupertino/NotificationCenter1", FakeCentre)
            .await
            .unwrap();
        for name in [
            "org.cupertino.Network1",
            "org.freedesktop.UPower",
            "org.cupertino.Bluetooth1",
            "org.cupertino.NotificationCenter1",
        ] {
            services.request_name(name).await.unwrap();
        }
    });

    // the only test of this binary, so no other thread reads the environment
    std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &bus.address);
    let (extras, events) = ExtrasBus::start(bus.connect(), || {});
    let first = next(&events);
    let wifi = first.wifi.as_ref().expect("a Wi-Fi device");
    assert!(wifi.enabled);
    assert_eq!(
        wifi.networks,
        [
            WifiNetwork {
                ssid: "Home".into(),
                signal_dbm: Some(-50),
                security: "wpa2".into(),
                known: true,
                connected: true
            },
            WifiNetwork {
                ssid: "cafe".into(),
                signal_dbm: None,
                security: "open".into(),
                known: false,
                connected: false
            },
        ]
    );
    assert_eq!(
        first.battery,
        Some(BatteryState {
            percent: 47,
            charging: false,
            on_battery: true
        })
    );
    let bluetooth = first.bluetooth.as_ref().expect("an adapter");
    assert_eq!(
        bluetooth.devices,
        [BluetoothDevice {
            id: "AA:BB:CC:DD:EE:FF/bredr".into(),
            address: "AA:BB:CC:DD:EE:FF".into(),
            name: Some("Headphones".into()),
            paired: true,
            connected: false
        }]
    );
    let focus = first.focus.as_ref().expect("the notification process");
    assert!(focus.active);
    assert_eq!(
        focus.lifetimes,
        [FocusLifetime {
            id: "one-hour".into(),
            title: "For 1 hour".into()
        }]
    );

    // joining an open network calls the service without a passphrase, and the bar reads the result back
    extras.act(ExtraAction::JoinNetwork("cafe".into())).unwrap();
    assert_eq!(
        calls.recv_timeout(Duration::from_secs(5)).unwrap(),
        ("cafe".to_owned(), String::new())
    );
    let joined = next(&events);
    let connected: Vec<&str> = joined
        .wifi
        .as_ref()
        .unwrap()
        .networks
        .iter()
        .filter(|n| n.connected)
        .map(|n| n.ssid.as_str())
        .collect();
    assert_eq!(connected, ["cafe"]);

    // a service that leaves hides its item
    block_on(services.release_name("org.cupertino.Bluetooth1")).unwrap();
    let gone = next(&events);
    assert_eq!(gone.bluetooth, None);
    assert!(gone.wifi.is_some());
}
