//! The live panes against fake daemons on a private bus (ST-10, ST-11, ST-13, ST-19): what the panes show of each
//! reading, and that their controls make the daemons' calls. The pane's open and close hooks (scan, discovery, agent
//! registration) run in the window process; here the `Services` calls they make are driven directly.
#![cfg(feature = "app")]

mod common;

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_settings::agent::{Agent, AgentEvent};
use cupertino_settings::controls::{Control, Value};
use cupertino_settings::live::{Live, Reading};
use cupertino_settings::panes::{change, sections, Call, ControlId, Effect, Section, Session};
use cupertino_settings::services::{Bus, Services};
use cupertino_settings::settings::{Paths, Settings};
use cupertino_settings::sheet::{Answer, Prompt};
use cupertino_settings::PaneId;
use zbus::fdo;
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value as Variant};

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
        screen_reader: None,
    };
    Settings::load(&paths, 1080.0).expect("defaults load without files")
}

/// The services of the window on `bus` (as system and session bus).
fn services(bus: &common::PrivateBus) -> Services {
    Services::on(Bus::at(bus.connect()), Bus::at(bus.connect()))
}

/// The one service call `id` changed to `value` makes.
fn call_of(id: ControlId, value: Value, live: &Live, dir: &std::path::Path) -> Call {
    match change(id, value, &mut settings(dir), live).as_slice() {
        [Effect::Service(call)] => call.clone(),
        other => panic!("one service call, got {other:?}"),
    }
}

fn labels(s: &Section) -> Vec<&str> {
    s.rows.iter().map(|r| r.label.as_str()).collect()
}

fn switch_on(s: &Section) -> bool {
    match s.rows[0].control {
        Control::Switch { on } => on,
        ref other => panic!("a switch, got {other:?}"),
    }
}

// ---- Wi-Fi -------------------------------------------------------------------------------------------------------

struct FakeNetwork {
    enabled: bool,
    refuse: bool,
    log: Log,
}

#[zbus::interface(name = "org.cupertino.Network1")]
impl FakeNetwork {
    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        self.enabled
    }

    #[zbus(property)]
    fn set_wireless_enabled(&mut self, on: bool) -> fdo::Result<()> {
        if self.refuse {
            return Err(fdo::Error::AccessDenied(
                "Not authorized to change Wi-Fi".into(),
            ));
        }
        self.log
            .lock()
            .unwrap()
            .push(format!("WirelessEnabled={on}"));
        self.enabled = on;
        Ok(())
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<(u32, String, String, String, String, String, String)> {
        vec![(
            2,
            "wlan0".into(),
            "wifi".into(),
            "connected".into(),
            "home".into(),
            String::new(),
            "10.0.0.2".into(),
        )]
    }

    #[zbus(property)]
    fn state(&self) -> String {
        "connected".into()
    }

    fn get_networks(&self) -> Vec<(String, i32, String, bool, bool)> {
        vec![
            ("home".into(), -40, "wpa2".into(), true, true),
            ("office".into(), -60, "wpa2".into(), true, false),
            ("cafe".into(), -70, "open".into(), false, false),
        ]
    }

    fn scan(&self) {
        self.log.lock().unwrap().push("Scan".into());
    }

    fn connect(&self, ssid: &str, passphrase: &str) {
        self.log
            .lock()
            .unwrap()
            .push(format!("Connect({ssid},{passphrase:?})"));
    }

    fn disconnect(&self) {
        self.log.lock().unwrap().push("Disconnect".into());
    }
}

fn serve_network(bus: &common::PrivateBus, refuse: bool) -> (zbus::blocking::Connection, Log) {
    let log = Log::default();
    let fake = FakeNetwork {
        enabled: true,
        refuse,
        log: log.clone(),
    };
    let c = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/Network1", fake)
        .unwrap()
        .name("org.cupertino.Network1")
        .unwrap()
        .build()
        .unwrap();
    (c, log)
}

#[test]
fn st10_wifi_switch_join_and_a_refused_write() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (daemon, log) = serve_network(&bus, false);
    let mut services = services(&bus);
    let mut live = Live {
        wifi: services.wifi(None),
        ..Live::default()
    };
    let s = sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live);
    assert!(switch_on(&s[0]));
    assert_eq!(
        s.iter().map(labels).collect::<Vec<_>>(),
        [
            vec!["Wi-Fi"],
            vec!["home (secured)", "office (secured)"],
            vec!["cafe"]
        ],
        "GetNetworks order, known first"
    );

    // the switch writes WirelessEnabled; the shown value is the daemon's on the next read
    services
        .call(&call_of(
            ControlId::WifiPower,
            Value::Bool(false),
            &live,
            dir.path(),
        ))
        .unwrap();
    live.wifi = services.wifi(live.wifi.ready());
    assert!(!switch_on(
        &sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live)[0]
    ));
    // a change made by another client shows too
    daemon
        .object_server()
        .interface::<_, FakeNetwork>("/org/cupertino/Network1")
        .unwrap()
        .get_mut()
        .enabled = true;
    live.wifi = services.wifi(live.wifi.ready());
    assert!(switch_on(
        &sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live)[0]
    ));

    // a saved network joins at once with an empty passphrase, once
    let join = call_of(
        ControlId::WifiNetwork(1),
        Value::Index(0),
        &live,
        dir.path(),
    );
    assert_eq!(
        join,
        Call::Join {
            ssid: "office".into(),
            passphrase: Some(cupertino_settings::sheet::Secret::new())
        }
    );
    services.call(&join).unwrap();
    services.scan().unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        ["WirelessEnabled=false", "Connect(office,\"\")", "Scan"]
    );
    drop(daemon);

    // a refused write: the error for the row, and the switch stays at the daemon's value
    let (_daemon, log) = serve_network(&bus, true);
    let mut services = self::services(&bus);
    let live = Live {
        wifi: services.wifi(None),
        ..Live::default()
    };
    let error = services
        .call(&call_of(
            ControlId::WifiPower,
            Value::Bool(false),
            &live,
            dir.path(),
        ))
        .unwrap_err();
    assert_eq!(error, "Not authorized to change Wi-Fi");
    let live = Live {
        wifi: services.wifi(live.wifi.ready()),
        ..Live::default()
    };
    assert!(switch_on(
        &sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live)[0]
    ));
    assert!(log.lock().unwrap().is_empty());
}

// ---- Bluetooth ---------------------------------------------------------------------------------------------------

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Bluetooth1")]
enum FakeBtError {
    #[zbus(error)]
    ZBus(zbus::Error),
    AgentExists(String),
    Rejected(String),
}

struct FakeBluetooth {
    /// Another client's agent holds pairing.
    taken: bool,
    agent: Option<(String, OwnedObjectPath)>,
    log: Log,
}

fn device(id: &str, name: &str, paired: bool) -> HashMap<String, OwnedValue> {
    let v = |x: Variant<'_>| x.try_to_owned().unwrap();
    HashMap::from([
        ("Id".to_string(), v(Variant::from(id))),
        ("Address".to_string(), v(Variant::from("00:11:22:33:44:55"))),
        ("Name".to_string(), v(Variant::from(name))),
        ("Icon".to_string(), v(Variant::from("input-keyboard"))),
        ("Paired".to_string(), v(Variant::from(paired))),
        ("Connected".to_string(), v(Variant::from(paired))),
        ("Pairing".to_string(), v(Variant::from(false))),
    ])
}

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
        vec![
            device("hp", "Headphones", true),
            device("kb", "Keyboard", false),
        ]
    }

    fn start_discovery(&self) {
        self.log.lock().unwrap().push("StartDiscovery".into());
    }

    fn stop_discovery(&self) {
        self.log.lock().unwrap().push("StopDiscovery".into());
    }

    fn register_agent(
        &mut self,
        #[zbus(header)] header: Header<'_>,
        path: ObjectPath<'_>,
    ) -> Result<(), FakeBtError> {
        if self.taken {
            return Err(FakeBtError::AgentExists(
                "another client's pairing agent is registered".into(),
            ));
        }
        self.log
            .lock()
            .unwrap()
            .push(format!("RegisterAgent({path})"));
        self.agent = Some((header.sender().unwrap().to_string(), path.into()));
        Ok(())
    }

    fn unregister_agent(&mut self) {
        self.log.lock().unwrap().push("UnregisterAgent".into());
        self.agent = None;
    }

    fn remove(&self, id: &str) {
        self.log.lock().unwrap().push(format!("Remove({id})"));
    }

    /// Pair by asking the registered agent to compare 123456, as the daemon does.
    async fn pair(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: &str,
    ) -> Result<(), FakeBtError> {
        let (sender, path) = self.agent.clone().expect("an agent is registered");
        let agent =
            zbus::Proxy::new(connection, sender, path, "org.cupertino.Bluetooth1.Agent").await?;
        match agent
            .call::<_, _, bool>(
                "ConfirmPasskey",
                &(7u64, device(id, "Keyboard", false), 123456u32, false),
            )
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(FakeBtError::Rejected("refused".into())),
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.cupertino.Bluetooth1.Agent.Rejected" =>
            {
                Err(FakeBtError::Rejected("The pairing was refused.".into()))
            }
            Err(e) => Err(e.into()),
        }
    }
}

fn serve_bluetooth(bus: &common::PrivateBus, taken: bool) -> (zbus::blocking::Connection, Log) {
    let log = Log::default();
    let fake = FakeBluetooth {
        taken,
        agent: None,
        log: log.clone(),
    };
    let c = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/Bluetooth1", fake)
        .unwrap()
        .name("org.cupertino.Bluetooth1")
        .unwrap()
        .build()
        .unwrap();
    (c, log)
}

/// An agent whose questions arrive on the returned channel.
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
fn st11_bluetooth_groups_registers_the_agent_pairs_and_forgets_only_after_confirmation() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, log) = serve_bluetooth(&bus, false);
    let mut services = services(&bus);
    let (agent, questions) = agent();
    assert_eq!(services.register_agent(agent), Ok(true));
    services.discovery(true).unwrap();
    let live = Live {
        bluetooth: services.bluetooth(None),
        ..Live::default()
    };
    let s = sections(PaneId::Bluetooth, &settings(dir.path()), LIGHT, &live);
    assert_eq!(
        (labels(&s[1]), labels(&s[2])),
        (vec!["Headphones"], vec!["Keyboard"]),
        "My Devices, then Nearby Devices"
    );
    assert_eq!(s[0].footer, None);

    // pairing asks the agent; the answer of its sheet ends the pairing
    for (answer, outcome) in [
        (Answer::Confirm, Ok(())),
        (Answer::Reject, Err("The pairing was refused.".to_string())),
    ] {
        let (done, result) = mpsc::channel();
        services.pair("kb", move |r| done.send(r).unwrap()).unwrap();
        match questions
            .recv_timeout(Duration::from_secs(5))
            .expect("the daemon asks the agent")
        {
            AgentEvent::Prompt {
                id: 7,
                device,
                prompt:
                    Prompt::Confirm {
                        passkey: Some(123456),
                    },
                answer: Some(reply),
            } => {
                assert_eq!(device, "Keyboard");
                reply.send_blocking(answer).unwrap();
            }
            other => panic!("a passkey to confirm, got {other:?}"),
        }
        assert_eq!(
            result.recv_timeout(Duration::from_secs(5)).unwrap(),
            outcome
        );
    }

    // Forget asks first: no call until the sheet confirms it
    let forget = call_of(
        ControlId::BluetoothDevice(0),
        Value::Index(1),
        &live,
        dir.path(),
    );
    assert_eq!(
        forget,
        Call::Remove {
            id: "hp".into(),
            confirmed: false
        }
    );
    services
        .call(&Call::Remove {
            id: "hp".into(),
            confirmed: true,
        })
        .unwrap();
    services.discovery(false).unwrap();
    services.unregister_agent().unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        [
            "RegisterAgent(/org/cupertino/Settings1/Agent)",
            "StartDiscovery",
            "Remove(hp)",
            "StopDiscovery",
            "UnregisterAgent"
        ]
    );
}

#[test]
fn the_agent_answers_only_the_bluetooth_daemon() {
    let Some(bus) = common::bus() else { return };
    let (_daemon, _log) = serve_bluetooth(&bus, false);
    let mut services = services(&bus);
    let (agent, questions) = agent();
    assert_eq!(services.register_agent(agent), Ok(true));
    let ours = services
        .system
        .connection()
        .unwrap()
        .unique_name()
        .expect("connected")
        .to_string();
    // another client of the bus forges a pairing question
    let intruder = bus.connect();
    let forged = intruder.call_method(
        Some(ours.as_str()),
        "/org/cupertino/Settings1/Agent",
        Some("org.cupertino.Bluetooth1.Agent"),
        "ConfirmPasskey",
        &(7u64, device("kb", "Keyboard", false), 123456u32, false),
    );
    match forged {
        Err(zbus::Error::MethodError(name, _, _)) => {
            assert_eq!(name.as_str(), "org.cupertino.Bluetooth1.Agent.Rejected")
        }
        other => panic!("a forged question is rejected, got {other:?}"),
    }
    let cancel = intruder.call_method(
        Some(ours.as_str()),
        "/org/cupertino/Settings1/Agent",
        Some("org.cupertino.Bluetooth1.Agent"),
        "Cancel",
        &(7u64,),
    );
    assert!(cancel.is_err(), "a forged Cancel is rejected too");
    assert!(questions.try_recv().is_err(), "no sheet for either");
}

#[test]
fn st11_another_clients_agent_means_no_dialog_here() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, log) = serve_bluetooth(&bus, true);
    let mut services = services(&bus);
    let (agent, questions) = agent();
    assert_eq!(services.register_agent(agent), Ok(false));
    let mut live = Live {
        bluetooth: services.bluetooth(None),
        ..Live::default()
    };
    if let Reading::Ready(b) = &mut live.bluetooth {
        b.agent_elsewhere = true;
    }
    let s = sections(PaneId::Bluetooth, &settings(dir.path()), LIGHT, &live);
    assert_eq!(
        s[0].footer.as_deref(),
        Some("Pairing is handled by another application.")
    );
    assert!(questions.try_recv().is_err());
    assert!(log.lock().unwrap().is_empty());
}

// ---- Focus -------------------------------------------------------------------------------------------------------

struct FakeCentre {
    log: Log,
}

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl FakeCentre {
    #[zbus(property)]
    fn do_not_disturb(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn focus_lifetimes(&self) -> Vec<(String, String)> {
        vec![
            ("hour".into(), "For 1 hour".into()),
            ("tonight".into(), "Until this evening".into()),
            ("tomorrow".into(), "Until tomorrow morning".into()),
        ]
    }

    fn set_do_not_disturb(&self, on: bool) {
        self.log
            .lock()
            .unwrap()
            .push(format!("SetDoNotDisturb({on})"));
    }

    fn set_do_not_disturb_for(&self, id: &str) {
        self.log
            .lock()
            .unwrap()
            .push(format!("SetDoNotDisturbFor({id})"));
    }

    fn applications(&self) -> Vec<(String, String)> {
        vec![("org.example.Mail".into(), "Mail".into())]
    }
}

#[test]
fn st13_focus_rows_are_the_lifetimes_and_call_the_centre() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let log = Log::default();
    let _centre = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at(
            "/org/cupertino/NotificationCenter1",
            FakeCentre { log: log.clone() },
        )
        .unwrap()
        .name("org.cupertino.NotificationCenter1")
        .unwrap()
        .build()
        .unwrap();
    let mut services = services(&bus);
    let live = Live {
        focus: services.focus(),
        ..Live::default()
    };
    let s = sections(PaneId::Focus, &settings(dir.path()), LIGHT, &live);
    assert_eq!(
        labels(&s[1]),
        ["For 1 hour", "Until this evening", "Until tomorrow morning"],
        "exactly FocusLifetimes, in order"
    );
    services
        .call(&call_of(
            ControlId::DoNotDisturb,
            Value::Bool(true),
            &live,
            dir.path(),
        ))
        .unwrap();
    for i in 0..3 {
        services
            .call(&call_of(
                ControlId::FocusFor(i),
                Value::Index(0),
                &live,
                dir.path(),
            ))
            .unwrap();
    }
    assert_eq!(
        *log.lock().unwrap(),
        [
            "SetDoNotDisturb(true)",
            "SetDoNotDisturbFor(hour)",
            "SetDoNotDisturbFor(tonight)",
            "SetDoNotDisturbFor(tomorrow)"
        ]
    );
}

#[test]
fn the_notifications_pane_lists_the_applications_the_centre_holds() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut services = services(&bus);
    assert!(
        services.notifying_apps().is_empty(),
        "no centre: no applications"
    );
    let _centre = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at(
            "/org/cupertino/NotificationCenter1",
            FakeCentre {
                log: Log::default(),
            },
        )
        .unwrap()
        .name("org.cupertino.NotificationCenter1")
        .unwrap()
        .build()
        .unwrap();
    let live = Live {
        notifying_apps: services.notifying_apps(),
        ..Live::default()
    };
    assert_eq!(
        live.notifying_apps,
        [("org.example.Mail".to_string(), "Mail".to_string())]
    );
    let s = sections(PaneId::Notifications, &settings(dir.path()), LIGHT, &live);
    assert_eq!(s[2].header.as_deref(), Some("Mail"));
    assert_eq!(labels(&s[2]), ["Show on lock screen", "Show previews"]);
}

// ---- no daemon ---------------------------------------------------------------------------------------------------

#[test]
fn st19_a_missing_daemon_disables_the_switch_with_the_reason() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut services = services(&bus);
    let live = Live {
        wifi: services.wifi(None),
        bluetooth: services.bluetooth(None),
        focus: services.focus(),
        brightness: services.brightness(),
        ..Live::default()
    };
    for (pane, reason) in [
        (PaneId::Wifi, "Wi-Fi is not available"),
        (PaneId::Bluetooth, "Bluetooth is not available"),
        (PaneId::Focus, "The notification centre is not running"),
        (PaneId::Network, "Wi-Fi is not available"),
    ] {
        let s = sections(pane, &settings(dir.path()), LIGHT, &live);
        assert_eq!(
            (s.len(), s[0].rows.len()),
            (1, 1),
            "{pane:?}: the switch row only, no empty list"
        );
        assert_eq!(s[0].rows[0].disabled.as_deref(), Some(reason), "{pane:?}");
    }
    assert!(matches!(live.brightness, Reading::Unavailable(_)));
}

// ---- Mouse resolution --------------------------------------------------------------------------------------------

struct FakeSession {
    mice: Vec<(String, String, u32, bool)>,
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl FakeSession {
    #[zbus(property)]
    fn mice(&self) -> Vec<(String, String, u32, bool)> {
        self.mice.clone()
    }
}

fn mouse(key: &str, name: &str, dpi: u32, stored: bool) -> (String, String, u32, bool) {
    (key.into(), name.into(), dpi, stored)
}

/// The mouse pane with `mice` listed by a fake session on a private bus.
fn mouse_pane(
    mice: Vec<(String, String, u32, bool)>,
    dir: &std::path::Path,
) -> (Vec<Section>, Live) {
    let bus = common::bus().expect("a private bus");
    let _session = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/Session1", FakeSession { mice })
        .unwrap()
        .name("org.cupertino.Session1")
        .unwrap()
        .build()
        .unwrap();
    let live = Live {
        mice: services(&bus).mice(),
        ..Live::default()
    };
    (sections(PaneId::Mouse, &settings(dir), LIGHT, &live), live)
}

fn popup_of(row: &cupertino_settings::panes::Row) -> (&[String], usize) {
    match &row.control {
        Control::Popup { choices, selected } => (choices, *selected),
        other => panic!("a pop-up, got {other:?}"),
    }
}

#[test]
fn a_single_mouse_gets_one_resolution_row_and_none_gets_no_row() {
    if common::bus().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (none, _) = mouse_pane(vec![], dir.path());
    assert_eq!(none.len(), 1, "no mouse, no resolution row");
    let (one, _) = mouse_pane(
        vec![mouse("046d:c077:Mouse", "Mouse", 800, false)],
        dir.path(),
    );
    assert_eq!(one.len(), 2);
    assert_eq!(one[1].header, None);
    assert_eq!(labels(&one[1]), ["Mouse resolution"]);
    let (choices, selected) = popup_of(&one[1].rows[0]);
    assert_eq!(
        choices,
        [
            "Automatic (800 dpi)",
            "400 dpi",
            "800 dpi",
            "1200 dpi",
            "1600 dpi",
            "3200 dpi",
            "6400 dpi",
            "Custom\u{2026}"
        ]
    );
    assert_eq!(selected, 0, "automatic is selected until a value is stored");
}

#[test]
fn several_mice_get_a_row_each_and_a_stored_value_outside_the_list_is_an_extra_item() {
    if common::bus().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (s, _) = mouse_pane(
        vec![
            mouse("1:1:Mouse", "USB Mouse", 1600, true),
            mouse("1:2:Mouse", "USB Mouse", 2400, true),
            mouse("2:3:Gamer", "Gamer", 1000, false),
        ],
        dir.path(),
    );
    assert_eq!(s[1].header.as_deref(), Some("Mouse resolution"));
    assert_eq!(labels(&s[1]), ["USB Mouse (1)", "USB Mouse (2)", "Gamer"]);
    let (first, selected) = popup_of(&s[1].rows[0]);
    assert_eq!((first[selected].as_str(), first.len()), ("1600 dpi", 8));
    let (second, selected) = popup_of(&s[1].rows[1]);
    assert_eq!(
        (
            second[selected].as_str(),
            second.last().map(String::as_str),
            second.len()
        ),
        ("2400 dpi", Some("Custom\u{2026}"), 9)
    );
    assert_eq!(
        popup_of(&s[1].rows[2]).0[popup_of(&s[1].rows[2]).1],
        "Automatic (1000 dpi)"
    );
}

#[test]
fn choosing_a_resolution_stores_it_for_that_mouse_and_automatic_forgets_it() {
    if common::bus().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (_, live) = mouse_pane(
        vec![
            mouse("1:1:A", "A", 1000, false),
            mouse("1:2:B", "B", 800, true),
        ],
        dir.path(),
    );
    let mut settings = settings(dir.path());
    let effects = change(
        ControlId::MouseResolution(0),
        Value::Index(4),
        &mut settings,
        &live,
    );
    assert_eq!(
        effects,
        [Effect::Store(
            cupertino_settings::settings::File::Compositor
        )]
    );
    assert_eq!(
        settings.compositor.mouse_dpi,
        Some([("1:1:A".to_string(), 1600)].into())
    );
    change(
        ControlId::MouseResolution(1),
        Value::Index(2),
        &mut settings,
        &live,
    );
    assert_eq!(
        settings.compositor.mouse_dpi,
        Some([("1:1:A".to_string(), 1600), ("1:2:B".to_string(), 800)].into())
    );
    change(
        ControlId::MouseResolution(0),
        Value::Index(0),
        &mut settings,
        &live,
    );
    assert_eq!(
        settings.compositor.mouse_dpi,
        Some([("1:2:B".to_string(), 800)].into()),
        "Automatic forgets only that mouse's value"
    );
    let custom = change(
        ControlId::MouseResolution(0),
        Value::Index(7),
        &mut settings,
        &live,
    );
    assert_eq!(
        custom,
        [Effect::AskResolution {
            key: "1:1:A".into(),
            name: "A".into()
        }],
        "Custom asks in a sheet and changes nothing yet"
    );
}

// ---- Accessibility held by the server ----------------------------------------------------------------------------

struct FakeAccessibility(HashMap<String, String>);

#[zbus::interface(name = "org.cupertino.Session1")]
impl FakeAccessibility {
    #[zbus(property)]
    fn accessibility(&self) -> HashMap<String, String> {
        self.0.clone()
    }
}

#[test]
fn the_colour_filter_button_shows_what_the_server_holds_not_the_file() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let _session = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at(
            "/org/cupertino/Session1",
            FakeAccessibility([("color_filter".to_string(), "grayscale".to_string())].into()),
        )
        .unwrap()
        .name("org.cupertino.Session1")
        .unwrap()
        .build()
        .unwrap();
    let colour_filter = |settings: &Settings| {
        let s = sections(PaneId::Accessibility, settings, LIGHT, &Live::default());
        let row = s
            .iter()
            .flat_map(|s| &s.rows)
            .find(|r| r.label == "Colour filter")
            .expect("the Colour filter row")
            .clone();
        let (choices, selected) = popup_of(&row);
        choices[selected].clone()
    };
    let mut settings = settings(dir.path());
    assert_eq!(colour_filter(&settings), "None", "the file has no filter");
    let held = match services(&bus).accessibility() {
        Reading::Ready(held) => held,
        other => panic!("the session's property, got {other:?}"),
    };
    assert!(settings.apply_session_accessibility(&held).is_empty());
    assert_eq!(colour_filter(&settings), "Grayscale");
    let paths = Paths {
        dock: None,
        compositor: None,
        menubar: None,
        keyboard: None,
        power: None,
        accessibility: Some(dir.path().join("accessibility.conf")),
        system_accessibility: None,
        data_control: None,
        screen_sharing: None,
        desktop_control: None,
        notifications: None,
        screen_reader: None,
    };
    settings.reload(&paths).unwrap();
    settings.apply_session_accessibility(&held);
    assert_eq!(
        colour_filter(&settings),
        "Grayscale",
        "a re-read of the file does not undo it"
    );
}

#[test]
fn a_key_the_server_holds_that_this_build_does_not_know_is_named_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = settings(dir.path());
    let held: HashMap<String, String> = [
        ("color_filter".to_string(), "grayscale".to_string()),
        ("future_option".to_string(), "on".to_string()),
    ]
    .into();
    let problems = settings.apply_session_accessibility(&held);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("future_option"), "{problems:?}");
    assert_eq!(
        settings.accessibility.color_filter,
        Some(cupertino_defaults::accessibility::ColorFilter::Grayscale),
        "the known keys still apply"
    );
}

// ---- Sound -------------------------------------------------------------------------------------------------------

#[test]
fn the_sound_pane_reads_and_sets_the_default_pipewire_sink() {
    use cupertino_pipewire::fake::FakeWpctl;
    use cupertino_pipewire::{Target, Volume as Node, Wpctl};
    use cupertino_settings::live::Volume;

    const NOWHERE: &str = "unix:path=/nonexistent/cupertino-test-bus";
    let half = Node {
        level: 0.5,
        muted: false,
    };
    let fake = FakeWpctl::new(half, half);
    let no_cards = tempfile::tempdir().unwrap();
    let mut services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE))
        .with_sound(Wpctl::new(fake.program()), no_cards.path().to_path_buf());
    // a reading asked for after a change behind the worker's back, and done
    let read_again = |services: &mut Services| {
        services.volume();
        services.wait_sound();
    };
    assert_eq!(
        services.volume(),
        Reading::Ready(Volume {
            level: 0.5,
            muted: false
        })
    );
    services
        .call(&Call::SetVolume {
            level: 0.25,
            muted: true,
        })
        .unwrap();
    assert_eq!(
        services.volume(),
        Reading::Ready(Volume {
            level: 0.25,
            muted: true
        }),
        "a reading shows the change asked at once"
    );
    let quiet = Node {
        level: 0.25,
        muted: true,
    };
    services.wait_sound();
    assert_eq!(fake.volume(Target::DefaultSink), quiet);
    assert_eq!(fake.volume(Target::DefaultSource), half);

    fake.set_sinks(&[(52, "Speakers"), (53, "Headphones")], 52);
    read_again(&mut services);
    assert_eq!(
        services
            .outputs()
            .iter()
            .map(|o| (o.id, o.name.as_str(), o.default))
            .collect::<Vec<_>>(),
        [(52, "Speakers", true), (53, "Headphones", false)]
    );
    services.call(&Call::SetOutput(53)).unwrap();
    assert_eq!(fake.default_sink(), 53);
    services.wait_sound();
    assert!(services.outputs()[1].default);

    fake.set_running(false);
    read_again(&mut services);
    assert_eq!(
        services.volume(),
        Reading::Unavailable("No sound output is available".into()),
        "without PipeWire the mixer is read, and there is no card"
    );
    assert_eq!(services.outputs(), [], "and no output is listed");
}

// ---- Plain failure texts -----------------------------------------------------------------------------------------

#[test]
fn a_bus_that_cannot_be_reached_shows_the_plain_state_and_never_the_error() {
    const NOWHERE: &str = "unix:path=/nonexistent/cupertino-test-bus";
    let mut services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
    fn reason<T: std::fmt::Debug>(reading: Reading<T>) -> String {
        match reading {
            Reading::Unavailable(reason) => reason,
            other => panic!("unavailable, got {other:?}"),
        }
    }
    assert_eq!(reason(services.wifi(None)), "Wi-Fi is not available");
    assert_eq!(
        reason(services.bluetooth(None)),
        "Bluetooth is not available"
    );
    assert_eq!(
        reason(services.battery()),
        "The power service is not running"
    );
    assert_eq!(
        reason(services.focus()),
        "The notification centre is not running"
    );
}

struct RefusingSession;

#[zbus::interface(name = "org.cupertino.Session1")]
impl RefusingSession {
    #[zbus(property)]
    fn mice(&self) -> zbus::fdo::Result<Vec<(String, String, u32, bool)>> {
        Err(zbus::fdo::Error::AccessDenied(
            "Rejected send message, 0 matched rules; interface=\"org.example\" (uid 1000)".into(),
        ))
    }
}

#[test]
fn the_bus_daemons_own_errors_never_reach_the_pane_but_a_services_plain_refusal_does() {
    use cupertino_settings::services::{failure, SERVICE_UNAVAILABLE};
    let fdo = |e: zbus::fdo::Error| zbus::Error::FDO(Box::new(e));
    for bus_error in [
        zbus::fdo::Error::NoReply("Did not receive a reply. Possible causes include: the remote application did not send a reply".into()),
        zbus::fdo::Error::AccessDenied(
            "Rejected send message, 0 matched rules; interface=\"org.x\" (uid 1000)".into(),
        ),
        zbus::fdo::Error::AccessDenied(
            "Sender is not authorized to send message to destination=:1.7 (interface=org.x)".into(),
        ),
        zbus::fdo::Error::AccessDenied(
            "Receiver is not authorized to receive message from :1.3".into(),
        ),
        zbus::fdo::Error::TimedOut("Activation timed out".into()),
        zbus::fdo::Error::LimitsExceeded("too many".into()),
    ] {
        assert_eq!(failure(&fdo(bus_error)), SERVICE_UNAVAILABLE);
    }
    assert_eq!(
        failure(&fdo(zbus::fdo::Error::Failed(
            "The radio is switched off.".into()
        ))),
        "The radio is switched off.",
        "a daemon's own refusal keeps its plain text"
    );
    let Some(bus) = common::bus() else { return };
    let _session = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/Session1", RefusingSession)
        .unwrap()
        .name("org.cupertino.Session1")
        .unwrap()
        .build()
        .unwrap();
    match services(&bus).mice() {
        Reading::Unavailable(reason) => assert_eq!(reason, "The session is not running"),
        other => panic!("unavailable, got {other:?}"),
    }
}

// ---- Login screen ------------------------------------------------------------------------------------------------

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.LoginManager1.Error")]
enum FakeLoginError {
    #[zbus(error)]
    ZBus(zbus::Error),
    AccessDenied(String),
    InvalidArgs(String),
    Failed(String),
    NotAdministrator(String),
    NotAtConsole(String),
    TooManyWrites(String),
}

/// The options sessiond wrote, in order.
type Written = Arc<Mutex<Vec<HashMap<String, String>>>>;

/// sessiond's machine settings: `can_set` `None` fails the question, `refuse` names the refusal of a write.
struct FakeLoginManager {
    can_set: Option<bool>,
    refuse: Option<&'static str>,
    written: Written,
}

#[zbus::interface(name = "org.cupertino.LoginManager1")]
impl FakeLoginManager {
    fn can_set_system_accessibility(&self) -> Result<bool, FakeLoginError> {
        self.can_set
            .ok_or_else(|| FakeLoginError::Failed("logind did not answer".into()))
    }

    fn set_system_accessibility(
        &self,
        options: HashMap<String, String>,
    ) -> Result<(), FakeLoginError> {
        // the journal's wording, which the row never shows
        let detail = "uid 1000 refused: see the journal".to_string();
        match self.refuse {
            None => {
                self.written.lock().unwrap().push(options);
                Ok(())
            }
            Some("AccessDenied") => Err(FakeLoginError::AccessDenied(detail)),
            Some("InvalidArgs") => Err(FakeLoginError::InvalidArgs(detail)),
            Some("NotAdministrator") => Err(FakeLoginError::NotAdministrator(detail)),
            Some("NotAtConsole") => Err(FakeLoginError::NotAtConsole(detail)),
            Some("TooManyWrites") => Err(FakeLoginError::TooManyWrites(detail)),
            Some(_) => Err(FakeLoginError::Failed(detail)),
        }
    }
}

fn serve_login(
    bus: &common::PrivateBus,
    can_set: Option<bool>,
) -> (zbus::blocking::Connection, Written) {
    let written = Arc::default();
    let fake = FakeLoginManager {
        can_set,
        refuse: None,
        written: Arc::clone(&written),
    };
    let c = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/LoginManager1", fake)
        .unwrap()
        .name("org.cupertino.LoginManager1")
        .unwrap()
        .build()
        .unwrap();
    (c, written)
}

fn can_set(services: &mut Services) -> bool {
    let (done, answer) = mpsc::channel();
    match services.can_set_system_accessibility(move |allowed| done.send(allowed).unwrap()) {
        Ok(()) => answer.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(_) => false,
    }
}

fn save(services: &mut Services, options: HashMap<String, String>) -> Result<(), String> {
    let (done, outcome) = mpsc::channel();
    services.set_system_accessibility(options, move |result| done.send(result).unwrap())?;
    outcome.recv_timeout(Duration::from_secs(5)).unwrap()
}

#[test]
fn the_login_screen_is_offered_only_when_sessiond_says_yes() {
    let Some(bus) = common::bus() else { return };
    let mut services = services(&bus);
    assert!(!can_set(&mut services), "no sessiond");
    for (answer, offered) in [(Some(true), true), (Some(false), false), (None, false)] {
        let (_daemon, _) = serve_login(&bus, answer);
        assert_eq!(can_set(&mut services), offered, "{answer:?}");
    }
}

#[test]
fn use_my_settings_sends_the_allow_listed_values_and_a_refusal_gets_its_sentence() {
    use cupertino_settings::panes::login_screen_options;
    use cupertino_settings::services::SERVICE_UNAVAILABLE;
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut s = settings(dir.path());
    s.accessibility.invert_colors = true;
    let session = Session {
        display: DisplayOptions {
            reduce_motion: true,
            ..DisplayOptions::default()
        },
        ..LIGHT
    };
    let mut services = services(&bus);
    assert_eq!(
        save(&mut services, login_screen_options(session, &s)),
        Err(SERVICE_UNAVAILABLE.to_string()),
        "no sessiond"
    );

    let (daemon, written) = serve_login(&bus, Some(true));
    save(&mut services, login_screen_options(session, &s)).unwrap();
    assert_eq!(
        *written.lock().unwrap(),
        [HashMap::from([(
            "reduce_motion".to_string(),
            "on".to_string()
        )])],
        "the person's value, and no invert"
    );

    for (refusal, sentence) in [
        (
            "NotAdministrator",
            "Only an administrator can change the login screen.",
        ),
        ("NotAtConsole", "Change this at the computer itself."),
        ("TooManyWrites", "Wait a moment, then try again."),
        (
            "InvalidArgs",
            "These settings can't be used for the login screen.",
        ),
        ("AccessDenied", SERVICE_UNAVAILABLE),
        ("Failed", SERVICE_UNAVAILABLE),
    ] {
        daemon
            .object_server()
            .interface::<_, FakeLoginManager>("/org/cupertino/LoginManager1")
            .unwrap()
            .get_mut()
            .refuse = Some(refusal);
        assert_eq!(
            save(&mut services, login_screen_options(session, &s)),
            Err(sentence.to_string()),
            "{refusal}"
        );
    }
    assert_eq!(written.lock().unwrap().len(), 1);
}
