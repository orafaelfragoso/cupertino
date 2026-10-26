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

const LIGHT: Session = Session { dark: false, display: DisplayOptions { reduce_motion: false, reduce_transparency: false, increase_contrast: false, differentiate_without_color: false } };

/// The calls a fake received, in order.
type Log = Arc<Mutex<Vec<String>>>;

fn settings(dir: &std::path::Path) -> Settings {
    let paths = Paths { dock: None, compositor: None, menubar: None, keyboard: None, power: None, accessibility: Some(dir.join("accessibility.conf")) };
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
            return Err(fdo::Error::AccessDenied("Not authorized to change Wi-Fi".into()));
        }
        self.log.lock().unwrap().push(format!("WirelessEnabled={on}"));
        self.enabled = on;
        Ok(())
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<(u32, String, String, String, String, String, String)> {
        vec![(2, "wlan0".into(), "wifi".into(), "connected".into(), "home".into(), String::new(), "10.0.0.2".into())]
    }

    #[zbus(property)]
    fn state(&self) -> String {
        "connected".into()
    }

    fn get_networks(&self) -> Vec<(String, i32, String, bool, bool)> {
        vec![("home".into(), -40, "wpa2".into(), true, true), ("office".into(), -60, "wpa2".into(), true, false), ("cafe".into(), -70, "open".into(), false, false)]
    }

    fn scan(&self) {
        self.log.lock().unwrap().push("Scan".into());
    }

    fn connect(&self, ssid: &str, passphrase: &str) {
        self.log.lock().unwrap().push(format!("Connect({ssid},{passphrase:?})"));
    }

    fn disconnect(&self) {
        self.log.lock().unwrap().push("Disconnect".into());
    }
}

fn serve_network(bus: &common::PrivateBus, refuse: bool) -> (zbus::blocking::Connection, Log) {
    let log = Log::default();
    let fake = FakeNetwork { enabled: true, refuse, log: log.clone() };
    let c = zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap().serve_at("/org/cupertino/Network1", fake).unwrap().name("org.cupertino.Network1").unwrap().build().unwrap();
    (c, log)
}

#[test]
fn st10_wifi_switch_join_and_a_refused_write() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (daemon, log) = serve_network(&bus, false);
    let mut services = services(&bus);
    let mut live = Live { wifi: services.wifi(None), ..Live::default() };
    let s = sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live);
    assert!(switch_on(&s[0]));
    assert_eq!(s.iter().map(labels).collect::<Vec<_>>(), [vec!["Wi-Fi"], vec!["home (secured)", "office (secured)"], vec!["cafe"]], "GetNetworks order, known first");

    // the switch writes WirelessEnabled; the shown value is the daemon's on the next read
    services.call(&call_of(ControlId::WifiPower, Value::Bool(false), &live, dir.path())).unwrap();
    live.wifi = services.wifi(live.wifi.ready());
    assert!(!switch_on(&sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live)[0]));
    // a change made by another client shows too
    daemon.object_server().interface::<_, FakeNetwork>("/org/cupertino/Network1").unwrap().get_mut().enabled = true;
    live.wifi = services.wifi(live.wifi.ready());
    assert!(switch_on(&sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live)[0]));

    // a saved network joins at once with an empty passphrase, once
    let join = call_of(ControlId::WifiNetwork(1), Value::Index(0), &live, dir.path());
    assert_eq!(join, Call::Join { ssid: "office".into(), passphrase: Some(cupertino_settings::sheet::Secret::new()) });
    services.call(&join).unwrap();
    services.scan().unwrap();
    assert_eq!(*log.lock().unwrap(), ["WirelessEnabled=false", "Connect(office,\"\")", "Scan"]);
    drop(daemon);

    // a refused write: the error for the row, and the switch stays at the daemon's value
    let (_daemon, log) = serve_network(&bus, true);
    let mut services = self::services(&bus);
    let live = Live { wifi: services.wifi(None), ..Live::default() };
    let error = services.call(&call_of(ControlId::WifiPower, Value::Bool(false), &live, dir.path())).unwrap_err();
    assert_eq!(error, "Not authorized to change Wi-Fi");
    let live = Live { wifi: services.wifi(live.wifi.ready()), ..Live::default() };
    assert!(switch_on(&sections(PaneId::Wifi, &settings(dir.path()), LIGHT, &live)[0]));
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
        vec![device("hp", "Headphones", true), device("kb", "Keyboard", false)]
    }

    fn start_discovery(&self) {
        self.log.lock().unwrap().push("StartDiscovery".into());
    }

    fn stop_discovery(&self) {
        self.log.lock().unwrap().push("StopDiscovery".into());
    }

    fn register_agent(&mut self, #[zbus(header)] header: Header<'_>, path: ObjectPath<'_>) -> Result<(), FakeBtError> {
        if self.taken {
            return Err(FakeBtError::AgentExists("another client's pairing agent is registered".into()));
        }
        self.log.lock().unwrap().push(format!("RegisterAgent({path})"));
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
    async fn pair(&self, #[zbus(connection)] connection: &zbus::Connection, id: &str) -> Result<(), FakeBtError> {
        let (sender, path) = self.agent.clone().expect("an agent is registered");
        let agent = zbus::Proxy::new(connection, sender, path, "org.cupertino.Bluetooth1.Agent").await?;
        match agent.call::<_, _, bool>("ConfirmPasskey", &(7u64, device(id, "Keyboard", false), 123456u32, false)).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(FakeBtError::Rejected("refused".into())),
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.cupertino.Bluetooth1.Agent.Rejected" => Err(FakeBtError::Rejected("The pairing was refused.".into())),
            Err(e) => Err(e.into()),
        }
    }
}

fn serve_bluetooth(bus: &common::PrivateBus, taken: bool) -> (zbus::blocking::Connection, Log) {
    let log = Log::default();
    let fake = FakeBluetooth { taken, agent: None, log: log.clone() };
    let c = zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap().serve_at("/org/cupertino/Bluetooth1", fake).unwrap().name("org.cupertino.Bluetooth1").unwrap().build().unwrap();
    (c, log)
}

/// An agent whose questions arrive on the returned channel.
fn agent() -> (Agent, mpsc::Receiver<AgentEvent>) {
    let (tx, rx) = mpsc::channel();
    (Agent { on_event: Box::new(move |e| tx.send(e).unwrap()) }, rx)
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
    let live = Live { bluetooth: services.bluetooth(None), ..Live::default() };
    let s = sections(PaneId::Bluetooth, &settings(dir.path()), LIGHT, &live);
    assert_eq!((labels(&s[1]), labels(&s[2])), (vec!["Headphones"], vec!["Keyboard"]), "My Devices, then Nearby Devices");
    assert_eq!(s[0].footer, None);

    // pairing asks the agent; the answer of its sheet ends the pairing
    for (answer, outcome) in [(Answer::Confirm, Ok(())), (Answer::Reject, Err("The pairing was refused.".to_string()))] {
        let (done, result) = mpsc::channel();
        services.pair("kb", move |r| done.send(r).unwrap()).unwrap();
        match questions.recv_timeout(Duration::from_secs(5)).expect("the daemon asks the agent") {
            AgentEvent::Prompt { id: 7, device, prompt: Prompt::Confirm { passkey: Some(123456) }, answer: Some(reply) } => {
                assert_eq!(device, "Keyboard");
                reply.send_blocking(answer).unwrap();
            }
            other => panic!("a passkey to confirm, got {other:?}"),
        }
        assert_eq!(result.recv_timeout(Duration::from_secs(5)).unwrap(), outcome);
    }

    // Forget asks first: no call until the sheet confirms it
    let forget = call_of(ControlId::BluetoothDevice(0), Value::Index(1), &live, dir.path());
    assert_eq!(forget, Call::Remove { id: "hp".into(), confirmed: false });
    services.call(&Call::Remove { id: "hp".into(), confirmed: true }).unwrap();
    services.discovery(false).unwrap();
    services.unregister_agent().unwrap();
    assert_eq!(*log.lock().unwrap(), ["RegisterAgent(/org/cupertino/Settings1/Agent)", "StartDiscovery", "Remove(hp)", "StopDiscovery", "UnregisterAgent"]);
}

#[test]
fn the_agent_answers_only_the_bluetooth_daemon() {
    let Some(bus) = common::bus() else { return };
    let (_daemon, _log) = serve_bluetooth(&bus, false);
    let mut services = services(&bus);
    let (agent, questions) = agent();
    assert_eq!(services.register_agent(agent), Ok(true));
    let ours = services.system.connection().unwrap().unique_name().expect("connected").to_string();
    // another client of the bus forges a pairing question
    let intruder = bus.connect();
    let forged = intruder.call_method(Some(ours.as_str()), "/org/cupertino/Settings1/Agent", Some("org.cupertino.Bluetooth1.Agent"), "ConfirmPasskey", &(7u64, device("kb", "Keyboard", false), 123456u32, false));
    match forged {
        Err(zbus::Error::MethodError(name, _, _)) => assert_eq!(name.as_str(), "org.cupertino.Bluetooth1.Agent.Rejected"),
        other => panic!("a forged question is rejected, got {other:?}"),
    }
    let cancel = intruder.call_method(Some(ours.as_str()), "/org/cupertino/Settings1/Agent", Some("org.cupertino.Bluetooth1.Agent"), "Cancel", &(7u64,));
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
    let mut live = Live { bluetooth: services.bluetooth(None), ..Live::default() };
    if let Reading::Ready(b) = &mut live.bluetooth {
        b.agent_elsewhere = true;
    }
    let s = sections(PaneId::Bluetooth, &settings(dir.path()), LIGHT, &live);
    assert_eq!(s[0].footer.as_deref(), Some("Pairing is handled by another application."));
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
        vec![("hour".into(), "For 1 hour".into()), ("tonight".into(), "Until this evening".into()), ("tomorrow".into(), "Until tomorrow morning".into())]
    }

    fn set_do_not_disturb(&self, on: bool) {
        self.log.lock().unwrap().push(format!("SetDoNotDisturb({on})"));
    }

    fn set_do_not_disturb_for(&self, id: &str) {
        self.log.lock().unwrap().push(format!("SetDoNotDisturbFor({id})"));
    }
}

#[test]
fn st13_focus_rows_are_the_lifetimes_and_call_the_centre() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let log = Log::default();
    let _centre = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/NotificationCenter1", FakeCentre { log: log.clone() })
        .unwrap()
        .name("org.cupertino.NotificationCenter1")
        .unwrap()
        .build()
        .unwrap();
    let mut services = services(&bus);
    let live = Live { focus: services.focus(), ..Live::default() };
    let s = sections(PaneId::Focus, &settings(dir.path()), LIGHT, &live);
    assert_eq!(labels(&s[1]), ["For 1 hour", "Until this evening", "Until tomorrow morning"], "exactly FocusLifetimes, in order");
    services.call(&call_of(ControlId::DoNotDisturb, Value::Bool(true), &live, dir.path())).unwrap();
    for i in 0..3 {
        services.call(&call_of(ControlId::FocusFor(i), Value::Index(0), &live, dir.path())).unwrap();
    }
    assert_eq!(*log.lock().unwrap(), ["SetDoNotDisturb(true)", "SetDoNotDisturbFor(hour)", "SetDoNotDisturbFor(tonight)", "SetDoNotDisturbFor(tomorrow)"]);
}

// ---- no daemon ---------------------------------------------------------------------------------------------------

#[test]
fn st19_a_missing_daemon_disables_the_switch_with_the_reason() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut services = services(&bus);
    let live = Live { wifi: services.wifi(None), bluetooth: services.bluetooth(None), focus: services.focus(), brightness: services.brightness(), ..Live::default() };
    for (pane, reason) in [
        (PaneId::Wifi, "Wi-Fi is not available"),
        (PaneId::Bluetooth, "Bluetooth is not available"),
        (PaneId::Focus, "The notification centre is not running"),
        (PaneId::Network, "Wi-Fi is not available"),
    ] {
        let s = sections(pane, &settings(dir.path()), LIGHT, &live);
        assert_eq!((s.len(), s[0].rows.len()), (1, 1), "{pane:?}: the switch row only, no empty list");
        assert_eq!(s[0].rows[0].disabled.as_deref(), Some(reason), "{pane:?}");
    }
    assert!(matches!(live.brightness, Reading::Unavailable(_)));
}
