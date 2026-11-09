//! The extras task against stand-ins for the network, power, Bluetooth and notification services on a private bus
//! (`dbus-daemon --session`, used as both the system and the session bus); skipped when the daemon is not available.
#![cfg(feature = "app")]

use cupertino_menubar::extras::{
    BatteryState, BluetoothDevice, ExtraAction, Extras, FocusLifetime, OpenPanels, SessionAction,
    WifiNetwork,
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

    #[zbus(property)]
    fn centre_shown(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn centre_output(&self) -> String {
        String::new()
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
    let (extras, events) = ExtrasBus::with_system(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
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
    let (_extras, events) = ExtrasBus::with_system(
        zbus::block_on(bus.connect_async()),
        std::future::ready(Ok(system)),
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
