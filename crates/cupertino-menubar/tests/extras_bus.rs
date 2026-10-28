//! The extras task against stand-ins for the network, power, Bluetooth and notification services on a private bus
//! (`dbus-daemon --session`, used as both the system and the session bus); skipped when the daemon is not available.
#![cfg(feature = "app")]

use cupertino_menubar::extras::{
    BatteryState, BluetoothDevice, ExtraAction, Extras, FocusLifetime, SessionAction, WifiNetwork,
};
use cupertino_menubar::extras_bus::{ExtrasBus, SessionAgent};
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::block_on;
use zbus::zvariant::{OwnedValue, Value};

use cupertino_test_bus::PrivateBus;

/// Stands in for the session agent of `cupertino-login`: the properties of the system menu and the calls it records.
struct FakeLogin {
    calls: mpsc::Sender<String>,
}

#[zbus::interface(name = "org.cupertino.Login1")]
impl FakeLogin {
    #[zbus(property)]
    fn locked(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn user_name(&self) -> String {
        "Rafael".into()
    }

    #[zbus(property)]
    fn capabilities(&self) -> HashMap<String, bool> {
        [("sleep".to_string(), false)].into()
    }

    fn sleep(&self) {
        self.calls.send("Sleep".into()).unwrap();
    }

    fn request_restart(&self) {
        self.calls.send("RequestRestart".into()).unwrap();
    }

    fn refresh_capabilities(&self) {
        self.calls.send("RefreshCapabilities".into()).unwrap();
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
    let (login_tx, login_calls) = mpsc::channel();
    let services = zbus::block_on(bus.connect_async());
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
        server
            .at("/org/cupertino/Login1", FakeLogin { calls: login_tx })
            .await
            .unwrap();
        for name in [
            "org.cupertino.Network1",
            "org.freedesktop.UPower",
            "org.cupertino.Bluetooth1",
            "org.cupertino.NotificationCenter1",
            "org.cupertino.Login1",
        ] {
            services.request_name(name).await.unwrap();
        }
    });

    // the only test of this binary, so no other thread reads the environment
    std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &bus.address);
    let (extras, events) = ExtrasBus::start(zbus::block_on(bus.connect_async()), || {});
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

    // nobody was named the session agent, so the owner of its name is not read, however well it answers
    assert_eq!(first.session, None);
    assert!(
        !first.lock_without_agent,
        "no login process is named: nothing locks"
    );
    // the owner of the name is another process than the one named: still not read, and the lock still works
    let me = std::process::id();
    extras.session_agent(SessionAgent::Process(me ^ 1)).unwrap();
    let squatted = next(&events);
    assert_eq!(squatted.session, None);
    assert!(squatted.lock_without_agent);

    // the system menu's state: the name and a capability the agent says no to
    extras.session_agent(SessionAgent::Process(me)).unwrap();
    let with_agent = next(&events);
    let session = with_agent.session.as_ref().expect("the session agent");
    assert_eq!(session.user_name, "Rafael");
    assert!(!session.caps.sleep && session.caps.restart && session.caps.lock);
    // the session menu's entries go to the server over the shell protocol: the bus side never calls the agent
    for action in [SessionAction::Restart, SessionAction::RefreshCapabilities] {
        extras.act(ExtraAction::Session(action)).unwrap();
    }
    assert!(login_calls
        .recv_timeout(Duration::from_millis(300))
        .is_err());

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

    // an agent that leaves takes the system menu with it
    block_on(services.release_name("org.cupertino.Login1")).unwrap();
    let no_agent = next(&events);
    assert_eq!(no_agent.session, None);

    // a service that leaves hides its item
    block_on(services.release_name("org.cupertino.Bluetooth1")).unwrap();
    let gone = next(&events);
    assert_eq!(gone.bluetooth, None);
    assert!(gone.wifi.is_some());
}
