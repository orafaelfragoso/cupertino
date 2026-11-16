//! The extras task against stand-ins for the network, power, Bluetooth and notification services on a private bus
//! (`dbus-daemon --session`, used as both the system and the session bus); skipped when the daemon is not available.
//! The network and the Bluetooth adapter are served once as the Cupertino daemons serve them and once as
//! NetworkManager and BlueZ do.
#![cfg(feature = "app")]

use cupertino_menubar::extras::{
    BatteryState, BluetoothDevice, ExtraAction, Extras, FocusLifetime, OpenPanels, Passphrase,
    SessionAction, WifiNetwork,
};
use cupertino_menubar::extras_bus::{ExtrasBus, SessionAgent, Waits};
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::block_on;
use zbus::zvariant::{OwnedValue, Value};

use cupertino_radios::fake::{FakeBluez, FakeNetworkManager, Silent, HEADPHONES, KEYBOARD};
use cupertino_test_bus::PrivateBus;

/// Stands in for the session agent of `cupertino-login`: the properties of the system menu and the calls it records.
struct FakeLogin {
    calls: mpsc::Sender<String>,
    /// The next read of `Capabilities` fails.
    fail_capabilities: Arc<std::sync::atomic::AtomicBool>,
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
    fn capabilities(&self) -> zbus::fdo::Result<HashMap<String, bool>> {
        if self
            .fail_capabilities
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(zbus::fdo::Error::Failed("not ready".into()));
        }
        // shut-down, log-out and switch-user are not reported
        Ok([
            ("sleep".to_string(), false),
            ("restart".to_string(), true),
            ("lock".to_string(), true),
        ]
        .into())
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

    #[zbus(property)]
    fn state(&self) -> String {
        "connected".into()
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
    fn discovering(&self) -> bool {
        false
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
        device.insert("Icon".to_owned(), value(Value::from("headphones")));
        device.insert("Paired".to_owned(), value(Value::from(true)));
        device.insert("Pairing".to_owned(), value(Value::from(false)));
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

    #[zbus(property)]
    fn centre_shown(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn centre_output(&self) -> String {
        String::new()
    }
}

/// The waits of a test that reads the state its services have now: as long as a busy machine may take.
const PATIENT: Waits = Waits {
    fresh: Duration::from_secs(20),
    silent: Duration::from_secs(60),
};

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
    let fail_capabilities = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
            .at(
                "/org/cupertino/Login1",
                FakeLogin {
                    calls: login_tx,
                    fail_capabilities: fail_capabilities.clone(),
                },
            )
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

    let system = zbus::block_on(bus.connect_async());
    let (extras, events) = ExtrasBus::with_waits(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
        PATIENT,
        || {},
    );
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
    assert!(
        !session.caps.shut_down && !session.caps.log_out && !session.caps.switch_user,
        "a capability the agent does not report is not offered"
    );
    // a read of the capabilities that fails is no answer: with nothing read before, every entry stays offered, so Lock and
    // Log Out remain reachable
    extras.session_agent(SessionAgent::Process(me ^ 1)).unwrap();
    assert_eq!(next(&events).session, None);
    fail_capabilities.store(true, std::sync::atomic::Ordering::SeqCst);
    extras.session_agent(SessionAgent::Process(me)).unwrap();
    let unread = next(&events);
    let caps = unread.session.as_ref().expect("the session agent").caps;
    assert!(caps.lock && caps.log_out, "{caps:?}");
    // the next read that works gives the agent's answer again
    extras.session_agent(SessionAgent::Process(me)).unwrap();
    let reread = next(&events);
    let caps = reread.session.as_ref().expect("the session agent").caps;
    assert!(caps.lock && !caps.shut_down, "{caps:?}");

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

/// The first extras from now on that `wanted` accepts; the ones before it are states on the way there.
fn until(events: &mpsc::Receiver<Extras>, what: &str, wanted: impl Fn(&Extras) -> bool) -> Extras {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut last = None;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match events.recv_timeout(left) {
            Ok(extras) if wanted(&extras) => return extras,
            Ok(extras) => last = Some(extras),
            Err(_) => panic!("the extras never showed {what}; the last were {last:#?}"),
        }
    }
}

#[test]
fn the_wifi_and_bluetooth_items_follow_networkmanager_and_bluez_and_act_on_them() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    let bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    let system = zbus::block_on(bus.connect_async());
    let (extras, events) = ExtrasBus::with_waits(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
        PATIENT,
        || {},
    );

    let first = next(&events);
    let wifi = first.wifi.as_ref().expect("a Wi-Fi device");
    assert!(wifi.enabled);
    assert_eq!(
        wifi.networks,
        [
            WifiNetwork {
                ssid: "Home".into(),
                signal_dbm: Some(-60),
                security: "wpa2".into(),
                known: true,
                connected: false
            },
            WifiNetwork {
                ssid: "Cafe".into(),
                signal_dbm: Some(-70),
                security: "open".into(),
                known: false,
                connected: false
            },
            WifiNetwork {
                ssid: "Office".into(),
                signal_dbm: Some(-80),
                security: "wpa2".into(),
                known: false,
                connected: false
            },
        ]
    );
    assert_eq!(first.wifi_link.state, "disconnected");
    let bluetooth = first.bluetooth.as_ref().expect("an adapter");
    assert!(bluetooth.powered);
    assert_eq!(
        bluetooth.devices,
        [
            BluetoothDevice {
                id: HEADPHONES.into(),
                address: "AA:00:00:00:00:01".into(),
                name: Some("Headphones".into()),
                paired: true,
                connected: false
            },
            BluetoothDevice {
                id: KEYBOARD.into(),
                address: "AA:00:00:00:00:02".into(),
                name: Some("Keyboard".into()),
                paired: false,
                connected: false
            },
        ]
    );

    let enabled = |e: &Extras| e.wifi.as_ref().is_some_and(|w| w.enabled);
    let connected = |e: &Extras, ssid: &str| {
        e.wifi
            .as_ref()
            .is_some_and(|w| w.networks.iter().any(|n| n.ssid == ssid && n.connected))
    };
    // the device joins a network by itself: the service's signal has the bar read again
    nm.set_joined(Some("Home"));
    let joined = until(&events, "the joined network", |e| connected(e, "Home"));
    assert!(joined.wifi_link.joined("Home"));

    // the switch, from the bar and from elsewhere
    extras.act(ExtraAction::SetWifiEnabled(false)).unwrap();
    let off = until(&events, "the radio off", |e| {
        e.wifi.is_some() && !enabled(e)
    });
    assert!(!nm.enabled());
    assert!(off.wifi.unwrap().networks.is_empty());
    nm.set_enabled(true);
    until(&events, "the radio on again", enabled);

    // an open network joins as it is, a secured one with its passphrase, and a refusal is the service's own words
    extras.act(ExtraAction::JoinNetwork("Cafe".into())).unwrap();
    until(&events, "the open network joined", |e| connected(e, "Cafe"));
    extras
        .join("Office".into(), Passphrase::new("correct horse"))
        .unwrap();
    until(&events, "the secured network joined", |e| {
        connected(e, "Office")
    });
    assert!(
        nm.calls()
            .iter()
            .any(|c| c.starts_with("AddAndActivateConnection(Some(\"correct horse\"),")),
        "{:?}",
        nm.calls()
    );
    extras.join("Nowhere".into(), Passphrase::new("x")).unwrap();
    let refused = until(&events, "the refusal", |e| e.wifi_link.refused.is_some());
    assert_eq!(
        refused.wifi_link.refused.as_deref(),
        Some("“Nowhere” is out of range.")
    );
    extras.act(ExtraAction::CancelJoin).unwrap();
    until(&events, "the network left", |e| {
        e.wifi_link.state == "disconnected"
    });
    assert_eq!(nm.joined(), None);

    // a device connects from the bar and goes by itself; the adapter is switched from the bar and from elsewhere
    let linked = |e: &Extras| {
        e.bluetooth
            .as_ref()
            .is_some_and(|b| b.devices.iter().any(|d| d.id == HEADPHONES && d.connected))
    };
    let powered = |e: &Extras| e.bluetooth.as_ref().is_some_and(|b| b.powered);
    extras
        .act(ExtraAction::ConnectDevice(HEADPHONES.into()))
        .unwrap();
    until(&events, "the device connected", linked);
    assert!(bluez.connected(HEADPHONES));
    bluez.set_connected(HEADPHONES, false);
    until(&events, "the device gone", |e| {
        e.bluetooth.is_some() && !linked(e)
    });
    extras.act(ExtraAction::SetBluetoothPowered(false)).unwrap();
    until(&events, "the adapter off", |e| {
        e.bluetooth.is_some() && !powered(e)
    });
    assert!(!bluez.powered());
    bluez.set_powered(true);
    until(&events, "the adapter on again", powered);

    // a service that stops hides its item
    drop(nm);
    let gone = until(&events, "no Wi-Fi item", |e| e.wifi.is_none());
    assert!(gone.bluetooth.is_some());
}

#[test]
fn a_machine_without_a_wifi_device_or_an_adapter_has_neither_item() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let _nm = FakeNetworkManager::serve_wired(bus.blocking_builder()).unwrap();
    let _bluez = FakeBluez::serve_without_adapter(bus.blocking_builder()).unwrap();
    let system = zbus::block_on(bus.connect_async());
    let (_extras, events) = ExtrasBus::with_waits(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
        PATIENT,
        || {},
    );
    let first = next(&events);
    assert_eq!(first.wifi, None);
    assert_eq!(first.wifi_link.state, "");
    assert_eq!(first.bluetooth, None);
}

/// Stands in for the control panel's process (`Shown`, `Output`) or the notification centre's list (`CentreShown`,
/// `CentreOutput`): whether it is open and the connector name of its output.
#[derive(Clone, Default)]
struct Presence(Arc<Mutex<(bool, String)>>);

impl Presence {
    fn set(&self, shown: bool, output: &str) {
        *self.0.lock().unwrap() = (shown, output.to_owned());
    }
}

struct FakePanel(Presence);

#[zbus::interface(name = "org.cupertino.ControlPanel1")]
impl FakePanel {
    #[zbus(property)]
    fn shown(&self) -> bool {
        self.0 .0.lock().unwrap().0
    }

    #[zbus(property)]
    fn output(&self) -> String {
        self.0 .0.lock().unwrap().1.clone()
    }
}

struct FakeList(Presence);

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl FakeList {
    #[zbus(property)]
    fn centre_shown(&self) -> bool {
        self.0 .0.lock().unwrap().0
    }

    #[zbus(property)]
    fn centre_output(&self) -> String {
        self.0 .0.lock().unwrap().1.clone()
    }
}

/// The next report whose panels are `panels`, skipping the reports before it.
fn until_panels(events: &mpsc::Receiver<Extras>, panels: OpenPanels) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut last = None;
    while std::time::Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(extras) if extras.panels == panels => return,
            Ok(extras) => last = Some(extras.panels),
            Err(_) => {}
        }
    }
    panic!("the panels never read {panels:?}; last {last:?}");
}

/// The extras follow whether the control panel and the notification centre's list are open, and on which output, as
/// their processes announce it; a process that goes away takes its panel with it.
#[test]
fn the_open_panels_follow_their_processes() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (panel, list) = (Presence::default(), Presence::default());
    let panel_process = zbus::block_on(bus.connect_async());
    let list_process = zbus::block_on(bus.connect_async());
    block_on(async {
        panel_process
            .object_server()
            .at("/org/cupertino/ControlPanel1", FakePanel(panel.clone()))
            .await
            .unwrap();
        panel_process
            .request_name("org.cupertino.ControlPanel1")
            .await
            .unwrap();
        list_process
            .object_server()
            .at("/org/cupertino/NotificationCenter1", FakeList(list.clone()))
            .await
            .unwrap();
        list_process
            .request_name("org.cupertino.NotificationCenter1")
            .await
            .unwrap();
    });
    let announce_panel = |shown: bool, output: &str| {
        panel.set(shown, output);
        block_on(async {
            let iface = panel_process
                .object_server()
                .interface::<_, FakePanel>("/org/cupertino/ControlPanel1")
                .await
                .unwrap();
            let fake = iface.get().await;
            fake.shown_changed(iface.signal_emitter()).await.unwrap();
            fake.output_changed(iface.signal_emitter()).await.unwrap();
        });
    };
    let announce_list = |shown: bool, output: &str| {
        list.set(shown, output);
        block_on(async {
            let iface = list_process
                .object_server()
                .interface::<_, FakeList>("/org/cupertino/NotificationCenter1")
                .await
                .unwrap();
            let fake = iface.get().await;
            fake.centre_shown_changed(iface.signal_emitter())
                .await
                .unwrap();
            fake.centre_output_changed(iface.signal_emitter())
                .await
                .unwrap();
        });
    };

    let system = zbus::block_on(bus.connect_async());
    let (_extras, events) = ExtrasBus::with_waits(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
        PATIENT,
        || {},
    );
    until_panels(&events, OpenPanels::default());

    announce_panel(true, "");
    until_panels(
        &events,
        OpenPanels {
            control_panel: Some(String::new()),
            centre: None,
        },
    );
    announce_panel(true, "DP-1");
    until_panels(
        &events,
        OpenPanels {
            control_panel: Some("DP-1".into()),
            centre: None,
        },
    );
    // the panel is dismissed (Escape, a click outside, the keyboard going elsewhere): the list opens on another output
    announce_panel(false, "");
    announce_list(true, "HDMI-A-1");
    until_panels(
        &events,
        OpenPanels {
            control_panel: None,
            centre: Some("HDMI-A-1".into()),
        },
    );
    announce_panel(true, "DP-1");
    until_panels(
        &events,
        OpenPanels {
            control_panel: Some("DP-1".into()),
            centre: Some("HDMI-A-1".into()),
        },
    );
    // the panel's process ends while it is open: its name goes, and the panel with it
    drop(panel_process);
    until_panels(
        &events,
        OpenPanels {
            control_panel: None,
            centre: Some("HDMI-A-1".into()),
        },
    );
    drop(list_process);
    until_panels(&events, OpenPanels::default());
}

#[test]
fn a_network_service_that_never_answers_hides_its_item_and_holds_no_other() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let _nm = Silent::network_manager(bus.blocking_builder()).unwrap();
    let bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    let system = zbus::block_on(bus.connect_async());
    let started = std::time::Instant::now();
    let (extras, events) = ExtrasBus::with_waits(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
        Waits {
            fresh: Duration::from_millis(20),
            silent: Duration::from_millis(200),
        },
        || {},
    );
    // the adapter is read and shown while the network's reading is under way, and the Wi-Fi item stays hidden
    let shown = until(&events, "the Bluetooth item", |e| e.bluetooth.is_some());
    assert_eq!(shown.wifi, None);
    assert_eq!(shown.wifi_link.state, "");

    // the task carries out what it is asked meanwhile: the session agent is named, the adapter is switched
    extras.session_agent(SessionAgent::Any).unwrap();
    until(&events, "the session agent", |e| e.lock_without_agent);
    extras.act(ExtraAction::SetBluetoothPowered(false)).unwrap();
    let off = until(&events, "the adapter off", |e| {
        e.bluetooth.as_ref().is_some_and(|b| !b.powered)
    });
    assert!(!bluez.powered());
    assert_eq!(off.wifi, None);
    // more requests for the silent service start no reading beside the one under way, and hold nothing
    for _ in 0..5 {
        extras.refresh().unwrap();
        extras.act(ExtraAction::ScanWifi).unwrap();
    }
    extras.act(ExtraAction::SetBluetoothPowered(true)).unwrap();
    until(&events, "the adapter on again", |e| {
        e.bluetooth.as_ref().is_some_and(|b| b.powered)
    });
    assert!(
        started.elapsed() < Duration::from_millis(1800),
        "nothing waited for the silent service: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_network_service_that_hangs_keeps_its_item_until_the_bound_then_hides_it() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    let system = zbus::block_on(bus.connect_async());
    let silent = Duration::from_millis(300);
    // the bar gives a service 300 ms: it reads the radio, then the service hangs and says it changed
    let (extras, events) = ExtrasBus::with_waits(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
        Waits {
            fresh: Duration::from_millis(20),
            silent,
        },
        || {},
    );
    until(&events, "the Wi-Fi item", |e| e.wifi.is_some());
    nm.hang();
    let hung = std::time::Instant::now();
    nm.announce();
    // the task still answers, with the item as it was last read
    extras.session_agent(SessionAgent::Any).unwrap();
    let meanwhile = until(&events, "the session agent", |e| e.lock_without_agent);
    if hung.elapsed() < silent {
        assert!(meanwhile.wifi.is_some(), "the last reading is shown");
    }
    // within the bound the item is hidden, without anything else asking
    let hidden = until(&events, "no Wi-Fi item", |e| e.wifi.is_none());
    assert_eq!(hidden.wifi_link.state, "");
    assert!(hung.elapsed() >= silent, "{:?}", hung.elapsed());
    assert!(
        hung.elapsed() < Duration::from_millis(1500),
        "{:?}",
        hung.elapsed()
    );
}

#[test]
fn a_radios_signal_has_that_radio_read_and_the_networks_listed_only_when_wanted() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    let bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    let system = zbus::block_on(bus.connect_async());
    let (extras, events) = ExtrasBus::with_waits(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
        PATIENT,
        || {},
    );
    let first = next(&events);
    assert!(first.wifi.is_some() && first.bluetooth.is_some());
    let powered = |e: &Extras| e.bluetooth.as_ref().is_some_and(|b| b.powered);
    // the services' start may be told after the first reading: let its readings pass
    bluez.set_powered(false);
    until(&events, "the adapter off", |e| {
        e.bluetooth.is_some() && !powered(e)
    });
    let (network, listed, adapter) = (nm.readings(), nm.listings(), bluez.readings());

    // a device heard again and again while the adapter discovers reads nothing; the adapter's switch reads the
    // adapter once, and the network not at all
    for step in 0..200 {
        bluez.advertise(KEYBOARD, -90 + (step % 40));
    }
    bluez.set_powered(true);
    until(&events, "the adapter on", powered);
    assert_eq!(bluez.readings(), adapter + 1);
    assert_eq!((nm.readings(), nm.listings()), (network, listed));

    // a signal of the network service that changes nothing the bar shows reads the radio and its device, not the
    // networks in range; the adapter's switch after it proves the signal was handled
    nm.announce();
    bluez.set_powered(false);
    until(&events, "the adapter off again", |e| {
        e.bluetooth.is_some() && !powered(e)
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while nm.readings() == network && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(nm.readings(), network + 1, "the radio and its device");
    assert_eq!(nm.listings(), listed, "not the networks in range");

    // the device joins a network: what the bar shows changed, so the networks are read with it
    nm.set_joined(Some("Home"));
    until(&events, "the joined network", |e| {
        e.wifi_link.joined("Home")
    });
    assert!(nm.listings() > listed);
    // the Wi-Fi menu opens: the networks in range are read though nothing changed
    let listed = nm.listings();
    extras.act(ExtraAction::ScanWifi).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while nm.listings() == listed && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(nm.listings() > listed, "the list is read as the menu opens");
}
