//! The network and Bluetooth panes on NetworkManager and BlueZ (fakes on a private bus), and hidden without either.
#![cfg(feature = "app")]

mod common;

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_settings::agent::{Agent, AgentEvent};
use cupertino_settings::live::{Live, Reading};
use cupertino_settings::panes::{sections, Call, Section, Session};
use cupertino_settings::search::search;
use cupertino_settings::services::{Backends, BluetoothBackend, Bus, NetworkBackend, Services};
use cupertino_settings::settings::{Paths, Settings};
use cupertino_settings::sheet::{Answer, Prompt, Secret};
use cupertino_settings::{sidebar, PaneId};
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};
use zbus::{fdo, interface};

const LIGHT: Session = Session {
    dark: false,
    display: DisplayOptions {
        reduce_motion: false,
        reduce_transparency: false,
        increase_contrast: false,
        differentiate_without_color: false,
    },
};

/// The calls a fake received, in order.
type Log = Arc<Mutex<Vec<String>>>;

fn settings(dir: &std::path::Path) -> Settings {
    let paths = Paths {
        dock: None,
        compositor: None,
        menubar: None,
        keyboard: None,
        power: None,
        accessibility: Some(dir.join("accessibility.conf")),
        system_accessibility: None,
        data_control: None,
        screen_sharing: None,
        desktop_control: None,
        notifications: None,
        appearance: None,
        launcher: None,
        screen_reader: None,
    };
    Settings::load(&paths, 1080.0).expect("defaults load without files")
}

fn services(bus: &common::PrivateBus) -> Services {
    Services::on(Bus::at(bus.connect()), Bus::at(bus.connect()))
}

fn labels(s: &Section) -> Vec<&str> {
    s.rows.iter().map(|r| r.label.as_str()).collect()
}

fn path(p: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(p).unwrap()
}

fn push(log: &Log, entry: String) {
    log.lock().unwrap().push(entry);
}

// ---- NetworkManager ----------------------------------------------------------------------------------------------

const NM: &str = "/org/freedesktop/NetworkManager";
const WLAN: &str = "/org/freedesktop/NetworkManager/Devices/1";
const ETH: &str = "/org/freedesktop/NetworkManager/Devices/2";

struct FakeNm {
    enabled: bool,
    log: Log,
}

#[interface(name = "org.freedesktop.NetworkManager")]
impl FakeNm {
    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        self.enabled
    }

    #[zbus(property)]
    fn set_wireless_enabled(&mut self, on: bool) {
        push(&self.log, format!("WirelessEnabled={on}"));
        self.enabled = on;
    }

    /// `NM_STATE_CONNECTED_GLOBAL`.
    #[zbus(property)]
    fn state(&self) -> u32 {
        70
    }

    fn get_devices(&self) -> Vec<OwnedObjectPath> {
        vec![path(WLAN), path(ETH)]
    }

    fn activate_connection(
        &self,
        connection: ObjectPath<'_>,
        device: ObjectPath<'_>,
        ap: ObjectPath<'_>,
    ) -> OwnedObjectPath {
        push(
            &self.log,
            format!("ActivateConnection({connection},{device},{ap})"),
        );
        path("/org/freedesktop/NetworkManager/ActiveConnection/1")
    }

    fn add_and_activate_connection(
        &self,
        settings: HashMap<String, HashMap<String, OwnedValue>>,
        device: ObjectPath<'_>,
        ap: ObjectPath<'_>,
    ) -> (OwnedObjectPath, OwnedObjectPath) {
        let security = settings.get("802-11-wireless-security").map(|s| {
            let text = |k: &str| String::try_from(s[k].try_clone().unwrap()).unwrap();
            format!("{},{}", text("key-mgmt"), text("psk"))
        });
        push(
            &self.log,
            format!("AddAndActivateConnection({security:?},{device},{ap})"),
        );
        (
            path("/org/freedesktop/NetworkManager/Settings/9"),
            path("/org/freedesktop/NetworkManager/ActiveConnection/1"),
        )
    }
}

struct FakeDevice {
    interface: &'static str,
    kind: u32,
    state: u32,
    address: &'static str,
    log: Log,
}

#[interface(name = "org.freedesktop.NetworkManager.Device")]
impl FakeDevice {
    #[zbus(property)]
    fn interface(&self) -> String {
        self.interface.into()
    }

    #[zbus(property)]
    fn device_type(&self) -> u32 {
        self.kind
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.state
    }

    #[zbus(property)]
    fn state_reason(&self) -> (u32, u32) {
        (self.state, 0)
    }

    #[zbus(property)]
    fn hw_address(&self) -> String {
        self.address.into()
    }

    fn disconnect(&self) {
        push(&self.log, format!("Disconnect({})", self.interface));
    }
}

struct FakeWireless {
    access_points: usize,
    log: Log,
}

#[interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl FakeWireless {
    #[zbus(property)]
    fn active_access_point(&self) -> OwnedObjectPath {
        path("/org/freedesktop/NetworkManager/AccessPoint/1")
    }

    fn get_all_access_points(&self) -> Vec<OwnedObjectPath> {
        (1..=self.access_points)
            .map(|i| path(&format!("/org/freedesktop/NetworkManager/AccessPoint/{i}")))
            .collect()
    }

    fn request_scan(&self, _options: HashMap<String, OwnedValue>) {
        push(&self.log, "RequestScan".into());
    }
}

struct FakeAccessPoint {
    ssid: &'static [u8],
    strength: u8,
    /// Privacy, WPA and RSN flags.
    flags: (u32, u32, u32),
}

#[interface(name = "org.freedesktop.NetworkManager.AccessPoint")]
impl FakeAccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> Vec<u8> {
        self.ssid.to_vec()
    }

    #[zbus(property)]
    fn strength(&self) -> u8 {
        self.strength
    }

    #[zbus(property)]
    fn flags(&self) -> u32 {
        self.flags.0
    }

    #[zbus(property)]
    fn wpa_flags(&self) -> u32 {
        self.flags.1
    }

    #[zbus(property)]
    fn rsn_flags(&self) -> u32 {
        self.flags.2
    }
}

struct FakeSettings;

#[interface(name = "org.freedesktop.NetworkManager.Settings")]
impl FakeSettings {
    fn list_connections(&self) -> Vec<OwnedObjectPath> {
        vec![
            path("/org/freedesktop/NetworkManager/Settings/1"),
            path("/org/freedesktop/NetworkManager/Settings/2"),
            path("/org/freedesktop/NetworkManager/Settings/3"),
        ]
    }
}

/// A saved connection: Wi-Fi with an SSID, else wired.
struct FakeConnection(Option<&'static str>);

#[interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl FakeConnection {
    fn get_settings(&self) -> HashMap<String, HashMap<String, OwnedValue>> {
        let value = |v: zbus::zvariant::Value<'_>| v.try_to_owned().unwrap();
        match self.0 {
            Some(ssid) => HashMap::from([(
                "802-11-wireless".to_string(),
                HashMap::from([("ssid".to_string(), value(ssid.as_bytes().into()))]),
            )]),
            None => HashMap::from([(
                "802-3-ethernet".to_string(),
                HashMap::from([("mtu".to_string(), value(0u32.into()))]),
            )]),
        }
    }
}

const PSK: u32 = 0x100;
const SAE: u32 = 0x400;

/// SSID, strength and flags of an access point.
type AccessPointRecord = (&'static [u8], u8, (u32, u32, u32));

const ACCESS_POINTS: [AccessPointRecord; 5] = [
    (b"home", 80, (1, 0, PSK)),
    (b"office", 60, (1, 0, PSK)),
    (b"cafe", 90, (0, 0, 0)),
    (b"lab", 50, (1, 0, SAE)),
    // a second radio of the office network, weaker: one row with the stronger signal
    (b"office", 30, (1, 0, PSK)),
];

fn serve_nm(bus: &common::PrivateBus) -> (zbus::blocking::Connection, Log) {
    serve_nm_with(bus, &ACCESS_POINTS)
}

fn serve_nm_with(
    bus: &common::PrivateBus,
    access_points: &[AccessPointRecord],
) -> (zbus::blocking::Connection, Log) {
    let log = Log::default();
    let mut c = bus
        .blocking_builder()
        .serve_at(
            NM,
            FakeNm {
                enabled: true,
                log: log.clone(),
            },
        )
        .unwrap()
        .serve_at(
            WLAN,
            FakeDevice {
                interface: "wlan0",
                kind: 2,
                state: 100,
                address: "AA:BB:CC:DD:EE:01",
                log: log.clone(),
            },
        )
        .unwrap()
        .serve_at(
            WLAN,
            FakeWireless {
                access_points: access_points.len(),
                log: log.clone(),
            },
        )
        .unwrap()
        .serve_at(
            ETH,
            FakeDevice {
                interface: "eth0",
                kind: 1,
                state: 30,
                address: "AA:BB:CC:DD:EE:02",
                log: log.clone(),
            },
        )
        .unwrap()
        .serve_at("/org/freedesktop/NetworkManager/Settings", FakeSettings)
        .unwrap();
    for (i, saved) in [Some("home"), Some("office"), None].into_iter().enumerate() {
        c = c
            .serve_at(
                format!("/org/freedesktop/NetworkManager/Settings/{}", i + 1),
                FakeConnection(saved),
            )
            .unwrap();
    }
    for (i, &(ssid, strength, flags)) in access_points.iter().enumerate() {
        c = c
            .serve_at(
                format!("/org/freedesktop/NetworkManager/AccessPoint/{}", i + 1),
                FakeAccessPoint {
                    ssid,
                    strength,
                    flags,
                },
            )
            .unwrap();
    }
    let c = c
        .name("org.freedesktop.NetworkManager")
        .unwrap()
        .build()
        .unwrap();
    (c, log)
}

#[test]
fn the_wifi_and_network_panes_read_and_change_networkmanager() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (_nm, log) = serve_nm(&bus);
    let mut services = services(&bus);
    assert_eq!(
        services.detect().network,
        Some(NetworkBackend::NetworkManager)
    );
    let live = Live {
        wifi: services.wifi(None),
        ..Live::default()
    };
    let wifi = live.wifi.ready().expect("NetworkManager is read");
    assert_eq!(
        wifi.networks
            .iter()
            .map(|n| (
                n.ssid.as_str(),
                n.signal,
                n.security.as_str(),
                n.saved,
                n.connected
            ))
            .collect::<Vec<_>>(),
        [
            ("home", -60, "wpa2", true, true),
            ("cafe", -55, "open", false, false),
            ("office", -70, "wpa2", true, false),
            ("lab", -75, "wpa3", false, false),
        ],
        "the connected network first, then by signal"
    );
    assert_eq!(
        wifi.devices
            .iter()
            .map(|d| (
                d.name.as_str(),
                d.kind.as_str(),
                d.state.as_str(),
                d.ssid.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            ("wlan0", "wifi", "connected", "home"),
            ("eth0", "ethernet", "disconnected", "")
        ]
    );
    assert_eq!((wifi.enabled, wifi.state.as_str()), (true, "connected"));
    let s = sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live);
    assert_eq!(
        s.iter().map(labels).collect::<Vec<_>>(),
        [
            vec!["Wi-Fi"],
            vec!["home", "office"],
            vec!["cafe", "lab (secured)"],
            vec!["Network", "Security"]
        ]
    );

    let mut passphrase = Secret::new();
    passphrase.push_str("hunter22");
    for call in [
        Call::SetWirelessEnabled(false),
        Call::Join {
            ssid: "office".into(),
            passphrase: Some(Secret::new()),
        },
        Call::Join {
            ssid: "cafe".into(),
            passphrase: Some(Secret::new()),
        },
        Call::Join {
            ssid: "lab".into(),
            passphrase: Some(passphrase),
        },
        Call::Disconnect,
    ] {
        services.call(&call).unwrap();
    }
    services.scan().unwrap();
    assert_eq!(
        services.call(&Call::Join {
            ssid: "gone".into(),
            passphrase: Some(Secret::new()),
        }),
        Err("“gone” is out of range.".to_string())
    );
    assert_eq!(
        *log.lock().unwrap(),
        [
            "WirelessEnabled=false",
            "ActivateConnection(/org/freedesktop/NetworkManager/Settings/2,\
             /org/freedesktop/NetworkManager/Devices/1,/org/freedesktop/NetworkManager/AccessPoint/2)",
            "AddAndActivateConnection(None,/org/freedesktop/NetworkManager/Devices/1,\
             /org/freedesktop/NetworkManager/AccessPoint/3)",
            "AddAndActivateConnection(Some(\"sae,hunter22\"),/org/freedesktop/NetworkManager/Devices/1,\
             /org/freedesktop/NetworkManager/AccessPoint/4)",
            "Disconnect(wlan0)",
            "RequestScan",
        ]
    );
}

#[test]
fn ssids_that_differ_only_in_bytes_that_are_not_text_stay_two_networks_and_names_are_cleaned() {
    let Some(bus) = common::bus() else { return };
    let (_nm, log) = serve_nm_with(
        &bus,
        &[
            (b"caf\xe9", 90, (0, 0, 0)),
            (b"caf\xe8", 40, (0, 0, 0)),
            (b"\xe2\x80\xaeLobby\x1b[2J\n", 60, (0, 0, 0)),
        ],
    );
    let mut services = services(&bus);
    services.detect();
    let wifi = services.wifi(None);
    let wifi = wifi.ready().expect("NetworkManager is read");
    assert_eq!(
        wifi.networks
            .iter()
            .map(|n| (n.name.as_str(), n.signal))
            .collect::<Vec<_>>(),
        [
            ("caf\u{fffd}", -55),
            ("Lobby[2J", -70),
            ("caf\u{fffd}", -80)
        ],
        "two rows for the two networks, and a clean name for the third"
    );
    assert_eq!(wifi.devices[0].ssid, "caf\u{fffd}");
    assert_ne!(wifi.networks[0].ssid, wifi.networks[2].ssid);
    services
        .call(&Call::Join {
            ssid: wifi.networks[2].ssid.clone(),
            passphrase: Some(Secret::new()),
        })
        .unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        [
            "AddAndActivateConnection(None,/org/freedesktop/NetworkManager/Devices/1,\
          /org/freedesktop/NetworkManager/AccessPoint/2)"
        ],
        "the weaker network's Join reaches its own access point"
    );
}

// ---- BlueZ -------------------------------------------------------------------------------------------------------

const HCI0: &str = "/org/bluez/hci0";
const HEADPHONES: &str = "/org/bluez/hci0/dev_AA_00_00_00_00_01";
const KEYBOARD: &str = "/org/bluez/hci0/dev_AA_00_00_00_00_02";

/// The agent BlueZ asks: its connection's name and path.
type Registered = Arc<Mutex<Option<(String, OwnedObjectPath)>>>;

struct FakeAdapter {
    powered: bool,
    discovering: bool,
    log: Log,
}

#[interface(name = "org.bluez.Adapter1")]
impl FakeAdapter {
    #[zbus(property)]
    fn powered(&self) -> bool {
        self.powered
    }

    #[zbus(property)]
    fn set_powered(&mut self, on: bool) {
        push(&self.log, format!("Powered={on}"));
        self.powered = on;
    }

    #[zbus(property)]
    fn discovering(&self) -> bool {
        self.discovering
    }

    fn start_discovery(&mut self) {
        push(&self.log, "StartDiscovery".into());
        self.discovering = true;
    }

    fn stop_discovery(&mut self) {
        push(&self.log, "StopDiscovery".into());
        self.discovering = false;
    }

    fn remove_device(&self, device: ObjectPath<'_>) {
        push(&self.log, format!("RemoveDevice({device})"));
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum FakeBluezError {
    #[zbus(error)]
    ZBus(zbus::Error),
    AuthenticationRejected(String),
}

struct FakeBtDevice {
    name: &'static str,
    address: &'static str,
    paired: bool,
    agent: Registered,
    log: Log,
}

#[interface(name = "org.bluez.Device1")]
impl FakeBtDevice {
    #[zbus(property)]
    fn address(&self) -> String {
        self.address.into()
    }

    #[zbus(property)]
    fn name(&self) -> String {
        self.name.into()
    }

    #[zbus(property)]
    fn alias(&self) -> String {
        self.name.into()
    }

    #[zbus(property)]
    fn icon(&self) -> String {
        "audio-headphones".into()
    }

    #[zbus(property)]
    fn paired(&self) -> bool {
        self.paired
    }

    #[zbus(property)]
    fn connected(&self) -> bool {
        self.paired
    }

    #[zbus(property)]
    fn adapter(&self) -> OwnedObjectPath {
        path(HCI0)
    }

    #[zbus(property)]
    fn trusted(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn set_trusted(&mut self, on: bool) {
        push(&self.log, format!("Trusted({})={on}", self.address));
    }

    fn connect(&self) {
        push(&self.log, format!("Connect({})", self.address));
    }

    fn disconnect(&self) {
        push(&self.log, format!("Disconnect({})", self.address));
    }

    /// Pair by asking the default agent to confirm 123456, as bluetoothd does.
    async fn pair(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), FakeBluezError> {
        let (sender, agent) = self
            .agent
            .lock()
            .unwrap()
            .clone()
            .expect("an agent is registered");
        let device = header.path().expect("a method call has a path").to_owned();
        let agent = zbus::Proxy::new(connection, sender, agent, "org.bluez.Agent1").await?;
        match agent
            .call::<_, _, ()>("RequestConfirmation", &(device, 123456u32))
            .await
        {
            Ok(()) => Ok(()),
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.bluez.Error.Rejected" =>
            {
                Err(FakeBluezError::AuthenticationRejected(
                    "The pairing was refused.".into(),
                ))
            }
            Err(e) => Err(e.into()),
        }
    }
}

struct FakeAgentManager {
    agent: Registered,
    log: Log,
}

#[interface(name = "org.bluez.AgentManager1")]
impl FakeAgentManager {
    fn register_agent(
        &self,
        #[zbus(header)] header: Header<'_>,
        agent: ObjectPath<'_>,
        capability: &str,
    ) {
        push(&self.log, format!("RegisterAgent({agent},{capability})"));
        *self.agent.lock().unwrap() = Some((header.sender().unwrap().to_string(), agent.into()));
    }

    fn request_default_agent(&self, agent: ObjectPath<'_>) {
        push(&self.log, format!("RequestDefaultAgent({agent})"));
    }

    fn unregister_agent(&self, agent: ObjectPath<'_>) {
        push(&self.log, format!("UnregisterAgent({agent})"));
        *self.agent.lock().unwrap() = None;
    }
}

fn serve_bluez(bus: &common::PrivateBus) -> (zbus::blocking::Connection, Log) {
    let log = Log::default();
    let agent = Registered::default();
    let device = |name, address, paired| FakeBtDevice {
        name,
        address,
        paired,
        agent: agent.clone(),
        log: log.clone(),
    };
    let c = bus
        .blocking_builder()
        .serve_at("/", fdo::ObjectManager)
        .unwrap()
        .serve_at(
            "/org/bluez",
            FakeAgentManager {
                agent: agent.clone(),
                log: log.clone(),
            },
        )
        .unwrap()
        .serve_at(
            HCI0,
            FakeAdapter {
                powered: true,
                discovering: false,
                log: log.clone(),
            },
        )
        .unwrap()
        .serve_at(HEADPHONES, device("Headphones", "AA:00:00:00:00:01", true))
        .unwrap()
        // the radio's name carries a right-to-left override, a bell and a newline; the rows and the prompt show neither
        .serve_at(
            KEYBOARD,
            device("Key\u{202e}board\u{7}\n", "AA:00:00:00:00:02", false),
        )
        .unwrap()
        .name("org.bluez")
        .unwrap()
        .build()
        .unwrap();
    (c, log)
}

fn agent() -> (Agent, mpsc::Receiver<AgentEvent>) {
    let (tx, rx) = mpsc::channel();
    (
        Agent {
            on_event: Box::new(move |e| tx.send(e).unwrap()),
        },
        rx,
    )
}

#[test]
fn the_bluetooth_pane_reads_bluez_pairs_through_its_agent_and_trusts_what_it_pairs() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (_bluez, log) = serve_bluez(&bus);
    let mut services = services(&bus);
    assert_eq!(services.detect().bluetooth, Some(BluetoothBackend::BlueZ));
    let (agent, questions) = agent();
    assert_eq!(services.register_agent(agent), Ok(true));
    services.discovery(true).unwrap();
    let live = Live {
        bluetooth: services.bluetooth(None),
        ..Live::default()
    };
    let b = live.bluetooth.ready().expect("BlueZ is read");
    assert_eq!((b.available, b.powered, b.discovering), (true, true, true));
    let s = sections(PaneId::Bluetooth, &settings(dir.path()), LIGHT, &live);
    assert_eq!(
        (labels(&s[1]), labels(&s[2])),
        (vec!["Headphones"], vec!["Keyboard"]),
        "My Devices, then Nearby Devices"
    );

    for (answer, outcome) in [
        (Answer::Reject, Err("The pairing was refused.".to_string())),
        (Answer::Confirm, Ok(())),
    ] {
        let (done, result) = mpsc::channel();
        services
            .pair(KEYBOARD, move |r| done.send(r).unwrap())
            .unwrap();
        match questions
            .recv_timeout(Duration::from_secs(5))
            .expect("BlueZ asks the agent")
        {
            AgentEvent::Prompt {
                device,
                prompt:
                    Prompt::Confirm {
                        passkey: Some(123456),
                    },
                answer: Some(reply),
                ..
            } => {
                assert_eq!(device, "Keyboard");
                let pairing = services.bluetooth(None);
                let keyboard = pairing
                    .ready()
                    .unwrap()
                    .devices
                    .iter()
                    .find(|d| d.id == KEYBOARD);
                assert!(
                    keyboard.unwrap().pairing,
                    "the row shows the pairing until Pair returns"
                );
                reply.send_blocking(answer).unwrap();
            }
            other => panic!("a passkey to confirm, got {other:?}"),
        }
        assert_eq!(
            result.recv_timeout(Duration::from_secs(5)).unwrap(),
            outcome
        );
    }
    let after = services.bluetooth(None);
    assert!(after.ready().unwrap().devices.iter().all(|d| !d.pairing));

    // a forged question from another client of the bus gets no sheet
    let ours = services
        .system
        .connection()
        .unwrap()
        .unique_name()
        .unwrap()
        .to_string();
    let forged = bus.connect().call_method(
        Some(ours.as_str()),
        "/org/cupertino/Settings1/Agent",
        Some("org.bluez.Agent1"),
        "RequestConfirmation",
        &(ObjectPath::try_from(KEYBOARD).unwrap(), 123456u32),
    );
    match forged {
        Err(zbus::Error::MethodError(name, _, _)) => {
            assert_eq!(name.as_str(), "org.bluez.Error.Rejected")
        }
        other => panic!("a forged question is rejected, got {other:?}"),
    }
    assert!(questions.try_recv().is_err());

    for call in [
        Call::Connect(HEADPHONES.into()),
        Call::DisconnectDevice(HEADPHONES.into()),
        Call::Remove {
            id: HEADPHONES.into(),
            confirmed: true,
        },
        Call::SetBluetoothPowered(false),
    ] {
        services.call(&call).unwrap();
    }
    assert_eq!(
        services.call(&Call::Connect("/org/bluez".into())),
        Err("The device is no longer available.".to_string()),
        "an id that is not a device of the adapter reaches no object"
    );
    services.discovery(false).unwrap();
    services.unregister_agent().unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        [
            "RegisterAgent(/org/cupertino/Settings1/Agent,KeyboardDisplay)",
            "RequestDefaultAgent(/org/cupertino/Settings1/Agent)",
            "StartDiscovery",
            "Trusted(AA:00:00:00:00:02)=true",
            "Connect(AA:00:00:00:00:01)",
            "Disconnect(AA:00:00:00:00:01)",
            &format!("RemoveDevice({HEADPHONES})"),
            "Powered=false",
            "StopDiscovery",
            "UnregisterAgent(/org/cupertino/Settings1/Agent)",
        ]
    );
}

// ---- no service --------------------------------------------------------------------------------------------------

#[test]
fn without_a_network_or_bluetooth_service_their_panes_hide_from_the_sidebar_and_the_search() {
    let Some(bus) = common::bus() else { return };
    let mut services = services(&bus);
    let backends = services.detect();
    assert_eq!(backends, Backends::default());
    let hidden = backends.hidden();
    assert_eq!(
        hidden,
        [
            PaneId::Wifi,
            PaneId::Network,
            PaneId::Vpn,
            PaneId::Bluetooth,
            PaneId::Trackpad
        ],
        "the VPN pane needs NetworkManager"
    );
    assert_eq!(sidebar::groups(&hidden)[0], [PaneId::Account]);
    assert_eq!(sidebar::groups(&hidden)[1], [PaneId::Battery]);
    assert!(search("wi-fi", &hidden)
        .iter()
        .all(|h| !hidden.contains(&h.pane)));
    assert!(search("wi-fi", &[]).iter().any(|h| h.pane == PaneId::Wifi));

    // a service that starts later brings its pane back; the Cupertino daemon wins over NetworkManager's name it claims
    let (_nm, _log) = serve_nm(&bus);
    assert_eq!(
        services.detect().hidden(),
        [PaneId::Bluetooth, PaneId::Trackpad]
    );
    let _netd = bus
        .blocking_builder()
        .name("org.cupertino.Network1")
        .unwrap()
        .build()
        .unwrap();
    let (_bluez, _log) = serve_bluez(&bus);
    assert_eq!(
        services.detect(),
        Backends {
            network: Some(NetworkBackend::Cupertino),
            bluetooth: Some(BluetoothBackend::BlueZ),
            ..Backends::default()
        }
    );
    assert!(matches!(services.bluetooth(None), Reading::Ready(_)));
}
