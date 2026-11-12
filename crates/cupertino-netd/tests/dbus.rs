//! The daemon on a private bus: a private `dbus-daemon` is started for the test (skipped when the
//! binary is not installed), the daemon connects to it and a second connection plays the client.
//! A thread plays the engine: it answers the requests the daemon forwards and the test publishes
//! the snapshots the engine would.
#![cfg(feature = "dbus")]

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cupertino_netd::auth::{Authorizer, Decision};
use cupertino_netd::daemon::{Bus, Config, Daemon, Requests};
use cupertino_netd::engine::{Command, CommandError};
use cupertino_netd::error::Result;
use cupertino_netd::model::{DeviceKind, DeviceState, Failure, Network};
use cupertino_netd::snapshot::{DeviceView, Snapshot};
use cupertino_netlink::{MacAddr, Security, Ssid};
use cupertino_test_bus::PrivateBus;
use rustix::event::{poll, PollFd, PollFlags};
use rustix::time::Timespec;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedValue;

const NETWORK: &str = "org.cupertino.Network1";
const NETWORK_PATH: &str = "/org/cupertino/Network1";
const MANAGER: &str = "org.freedesktop.NetworkManager";
const MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";
const WAIT: Duration = Duration::from_secs(5);

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

/// The engine's side: answers every request with `answer` and records the commands.
struct FakeEngine {
    commands: Arc<Mutex<Vec<Command>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeEngine {
    fn start(
        requests: Requests,
        answer: fn(&Command) -> std::result::Result<(), CommandError>,
    ) -> Self {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopping) = (commands.clone(), stop.clone());
        let thread = thread::spawn(move || {
            let timeout = Timespec {
                tv_sec: 0,
                tv_nsec: 50_000_000,
            };
            while !stopping.load(Ordering::SeqCst) {
                let fd = requests.as_fd();
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                poll(&mut fds, Some(&timeout)).unwrap();
                for request in requests.drain().unwrap() {
                    seen.lock().unwrap().push(request.command.clone());
                    let result = answer(&request.command);
                    request.reply(result);
                }
            }
        });
        Self {
            commands,
            stop,
            thread: Some(thread),
        }
    }

    fn commands(&self) -> Vec<Command> {
        self.commands.lock().unwrap().clone()
    }
}

impl Drop for FakeEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn accept(_: &Command) -> std::result::Result<(), CommandError> {
    Ok(())
}

fn needs_passphrase(command: &Command) -> std::result::Result<(), CommandError> {
    match command {
        Command::Connect { ssid, .. } => Err(CommandError::PassphraseRequired(ssid.clone())),
        _ => Ok(()),
    }
}

struct Running {
    daemon: Daemon,
    engine: FakeEngine,
    asked: Arc<Mutex<Vec<u32>>>,
}

fn start(
    bus: &PrivateBus,
    decision: Decision,
    answer: fn(&Command) -> std::result::Result<(), CommandError>,
) -> Running {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let authorizer = Box::new(Scripted {
        decision,
        asked: asked.clone(),
    });
    start_with(bus, authorizer, asked, answer)
}

fn start_with(
    bus: &PrivateBus,
    authorizer: Box<dyn Authorizer>,
    asked: Arc<Mutex<Vec<u32>>>,
    answer: fn(&Command) -> std::result::Result<(), CommandError>,
) -> Running {
    let (daemon, requests) = Daemon::connect(Config {
        bus: Bus::Address(bus.address.clone()),
        authorizer,
    })
    .unwrap();
    daemon.claim_names().unwrap();
    Running {
        daemon,
        engine: FakeEngine::start(requests, answer),
        asked,
    }
}

fn proxy<'a>(
    client: &'a Connection,
    name: &'a str,
    path: &'a str,
    interface: &'a str,
) -> Proxy<'a> {
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

fn manager(client: &Connection) -> Proxy<'_> {
    proxy(client, MANAGER, MANAGER_PATH, MANAGER)
}

fn network(client: &Connection) -> Proxy<'_> {
    proxy(client, NETWORK, NETWORK_PATH, NETWORK)
}

fn error_name<T: std::fmt::Debug>(result: zbus::Result<T>) -> String {
    match result.unwrap_err() {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        zbus::Error::FDO(error) => zbus::DBusError::name(&*error).to_string(),
        other => panic!("expected a method error, got {other}"),
    }
}

/// The bus error name of a refused property write.
fn property_error(result: zbus::fdo::Result<()>) -> String {
    zbus::DBusError::name(&result.unwrap_err()).to_string()
}

/// Forwards each emission of `member` on `interface`, decoded by `decode`.
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
    thread::spawn(move || {
        for message in stream.by_ref() {
            if sender.send(decode(&message)).is_err() {
                return;
            }
        }
    });
    received
}

/// Forwards each `PropertiesChanged` of `path` as `(interface, changed properties)`.
fn property_changes(
    client: &Connection,
    name: &'static str,
    path: &'static str,
) -> mpsc::Receiver<(String, HashMap<String, OwnedValue>)> {
    signals(
        client,
        name,
        path,
        "org.freedesktop.DBus.Properties",
        "PropertiesChanged",
        |message| {
            let (interface, changed, _invalidated): (
                String,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize().unwrap();
            (interface, changed)
        },
    )
}

fn device(ifindex: u32, name: &str, kind: DeviceKind, state: DeviceState) -> DeviceView {
    DeviceView {
        ifindex,
        name: name.to_owned(),
        kind,
        state,
        ssid: None,
        failure: None,
        address: (state == DeviceState::Connected).then_some((Ipv4Addr::new(10, 0, 0, 9), 24)),
        has_router: state == DeviceState::Connected,
        metric: if kind == DeviceKind::Ethernet {
            100
        } else {
            600
        },
    }
}

fn scanned(ssid: &str, security: Security, signal: i32, known: bool, connected: bool) -> Network {
    Network {
        ssid: Ssid(ssid.as_bytes().to_vec()),
        security,
        signal_dbm: Some(signal),
        bssid: MacAddr([2, 0, 0, 0, 0, 1]),
        frequency_mhz: 2412,
        known,
        connected,
    }
}

#[test]
fn a_client_reads_the_networkmanager_values_of_the_current_state() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, accept);
    let client = bus.connect();
    let manager = manager(&client);

    // before the first snapshot: nothing is connected
    assert_eq!(manager.get_property::<u32>("State").unwrap(), 20);
    assert_eq!(manager.get_property::<u32>("Connectivity").unwrap(), 1);

    let wired = device(2, "enp1s0", DeviceKind::Ethernet, DeviceState::Connected);
    running
        .daemon
        .publish(Snapshot {
            wireless_enabled: true,
            devices: vec![wired],
            networks: Vec::new(),
            known: Vec::new(),
        })
        .unwrap();
    assert_eq!(manager.get_property::<u32>("State").unwrap(), 70);
    assert_eq!(manager.get_property::<u32>("Connectivity").unwrap(), 0);
    assert!(manager.get_property::<bool>("NetworkingEnabled").unwrap());
    assert!(manager.get_property::<bool>("WirelessEnabled").unwrap());
    assert_eq!(
        manager
            .get_property::<String>("PrimaryConnectionType")
            .unwrap(),
        "802-3-ethernet"
    );
    assert_eq!(
        manager.call::<_, _, u32>("CheckConnectivity", &()).unwrap(),
        0
    );
    // reads need no authorisation
    assert_eq!(*running.asked.lock().unwrap(), Vec::<u32>::new());
}

#[test]
fn state_changes_reach_listeners_as_signals_and_properties() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, accept);
    let client = bus.connect();
    let states = signals(
        &client,
        MANAGER,
        MANAGER_PATH,
        MANAGER,
        "StateChanged",
        |message| message.body().deserialize::<u32>().unwrap(),
    );
    let changes = property_changes(&client, MANAGER, MANAGER_PATH);
    // let the subscriptions reach the bus daemon before the change
    manager(&client).get_property::<u32>("State").unwrap();

    let wifi = device(3, "wlan0", DeviceKind::Wifi, DeviceState::Connected);
    running
        .daemon
        .publish(Snapshot {
            wireless_enabled: true,
            devices: vec![wifi],
            networks: Vec::new(),
            known: Vec::new(),
        })
        .unwrap();

    assert_eq!(states.recv_timeout(WAIT).unwrap(), 70);
    let mut seen: HashMap<String, OwnedValue> = HashMap::new();
    while seen.len() < 4 {
        let (interface, changed) = changes.recv_timeout(WAIT).unwrap();
        assert_eq!(interface, MANAGER);
        seen.extend(changed);
    }
    // the first snapshot also switches the radio setting on (the default snapshot has it off)
    assert!(bool::try_from(seen.remove("WirelessEnabled").unwrap()).unwrap());
    assert_eq!(u32::try_from(seen.remove("State").unwrap()).unwrap(), 70);
    assert_eq!(
        u32::try_from(seen.remove("Connectivity").unwrap()).unwrap(),
        0
    );
    assert_eq!(
        String::try_from(seen.remove("PrimaryConnectionType").unwrap()).unwrap(),
        "802-11-wireless"
    );
}

#[test]
fn an_authorised_caller_switches_wifi_and_the_engine_hears_it() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, accept);
    let client = bus.connect();
    let changes = property_changes(&client, NETWORK, NETWORK_PATH);
    running
        .daemon
        .publish(Snapshot {
            wireless_enabled: true,
            devices: Vec::new(),
            networks: Vec::new(),
            known: Vec::new(),
        })
        .unwrap();

    network(&client)
        .set_property("WirelessEnabled", false)
        .unwrap();
    assert_eq!(
        running.engine.commands(),
        [Command::SetWirelessEnabled(false)]
    );
    assert_eq!(running.asked.lock().unwrap().len(), 1);

    // the engine publishes the new state when the kernel reports the switch
    running
        .daemon
        .publish(Snapshot {
            wireless_enabled: false,
            devices: Vec::new(),
            networks: Vec::new(),
            known: Vec::new(),
        })
        .unwrap();
    assert!(!network(&client)
        .get_property::<bool>("WirelessEnabled")
        .unwrap());
    // Every signal before the one that says "off" says "on": the first snapshot switched the
    // setting on, and the bus library repeats the old value after the write, before the engine has
    // published the new one.
    loop {
        let (interface, changed) = changes.recv_timeout(WAIT).unwrap();
        assert_eq!(interface, NETWORK);
        if !bool::try_from(changed.get("WirelessEnabled").unwrap().try_clone().unwrap()).unwrap() {
            break;
        }
    }
}

#[test]
fn the_networkmanager_switch_is_the_same_switch() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, accept);
    let client = bus.connect();
    manager(&client)
        .set_property("WirelessEnabled", true)
        .unwrap();
    assert_eq!(
        running.engine.commands(),
        [Command::SetWirelessEnabled(true)]
    );
}

#[test]
fn a_refused_caller_cannot_switch_wifi_scan_connect_or_disconnect() {
    let bus = private_bus!();
    let running = start(
        &bus,
        Decision::Denied("the caller is not in a login session".to_owned()),
        accept,
    );
    let client = bus.connect();
    running
        .daemon
        .publish(Snapshot {
            wireless_enabled: true,
            devices: Vec::new(),
            networks: Vec::new(),
            known: Vec::new(),
        })
        .unwrap();

    assert_eq!(
        property_error(network(&client).set_property("WirelessEnabled", false)),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    assert_eq!(
        property_error(manager(&client).set_property("WirelessEnabled", false)),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    assert_eq!(
        error_name(network(&client).call::<_, _, ()>("Scan", &())),
        "org.cupertino.Network1.NotAuthorized"
    );
    assert_eq!(
        error_name(network(&client).call::<_, _, ()>("Connect", &("home", "secret"))),
        "org.cupertino.Network1.NotAuthorized"
    );
    assert_eq!(
        error_name(network(&client).call::<_, _, ()>("Disconnect", &())),
        "org.cupertino.Network1.NotAuthorized"
    );

    assert_eq!(running.engine.commands(), []);
    assert!(network(&client)
        .get_property::<bool>("WirelessEnabled")
        .unwrap());
}

#[test]
fn the_authorizer_is_asked_about_the_callers_own_process() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, accept);
    let client = bus.connect();
    network(&client).call::<_, _, ()>("Scan", &()).unwrap();
    assert_eq!(*running.asked.lock().unwrap(), [std::process::id()]);
    assert_eq!(running.engine.commands(), [Command::Scan]);
}

#[test]
fn get_networks_lists_what_the_last_scan_found() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, accept);
    let client = bus.connect();
    let changed = signals(
        &client,
        NETWORK,
        NETWORK_PATH,
        NETWORK,
        "NetworksChanged",
        |_| (),
    );
    network(&client).get_property::<String>("State").unwrap();

    assert_eq!(
        network(&client)
            .call::<_, _, Vec<(String, i32, String, bool, bool)>>("GetNetworks", &())
            .unwrap(),
        []
    );

    let mut wifi = device(3, "wlan0", DeviceKind::Wifi, DeviceState::Connected);
    wifi.ssid = Some("home".to_owned());
    let networks = vec![
        scanned("home", Security::Wpa2, -45, true, true),
        scanned("cafe", Security::Open, -60, false, false),
        scanned("new", Security::Wpa2Wpa3, -72, false, false),
    ];
    running
        .daemon
        .publish(Snapshot {
            wireless_enabled: true,
            devices: vec![wifi],
            networks,
            known: Vec::new(),
        })
        .unwrap();

    changed.recv_timeout(WAIT).unwrap();
    assert_eq!(
        network(&client)
            .call::<_, _, Vec<(String, i32, String, bool, bool)>>("GetNetworks", &())
            .unwrap(),
        [
            ("home".to_owned(), -45, "wpa2".to_owned(), true, true),
            ("cafe".to_owned(), -60, "open".to_owned(), false, false),
            ("new".to_owned(), -72, "wpa2-wpa3".to_owned(), false, false),
        ]
    );
    assert_eq!(
        network(&client)
            .get_property::<Vec<(u32, String, String, String, String, String, String)>>("Devices")
            .unwrap(),
        [(
            3,
            "wlan0".to_owned(),
            "wifi".to_owned(),
            "connected".to_owned(),
            "home".to_owned(),
            String::new(),
            "10.0.0.9/24".to_owned()
        )]
    );
    assert_eq!(
        network(&client).get_property::<String>("State").unwrap(),
        "connected"
    );
}

#[test]
fn connect_passes_the_ssid_and_passphrase_on_and_an_empty_passphrase_is_none() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, accept);
    let client = bus.connect();
    network(&client)
        .call::<_, _, ()>("Connect", &("home", "correct horse"))
        .unwrap();
    network(&client)
        .call::<_, _, ()>("Connect", &("cafe", ""))
        .unwrap();
    network(&client)
        .call::<_, _, ()>("Disconnect", &())
        .unwrap();
    assert_eq!(
        running.engine.commands(),
        [
            Command::Connect {
                ssid: "home".to_owned(),
                passphrase: Some("correct horse".to_owned())
            },
            Command::Connect {
                ssid: "cafe".to_owned(),
                passphrase: None
            },
            Command::Disconnect,
        ]
    );
}

#[test]
fn a_request_the_engine_refuses_comes_back_as_a_named_error() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, needs_passphrase);
    let client = bus.connect();
    let result = network(&client).call::<_, _, ()>("Connect", &("home", ""));
    assert_eq!(
        error_name(result),
        "org.cupertino.Network1.PassphraseRequired"
    );
    drop(running);
}

#[test]
fn a_failed_connection_is_signalled_once_with_its_reason() {
    let bus = private_bus!();
    let running = start(&bus, Decision::Allowed, accept);
    let client = bus.connect();
    let failures = signals(
        &client,
        NETWORK,
        NETWORK_PATH,
        NETWORK,
        "ConnectionFailed",
        |message| message.body().deserialize::<(String, String)>().unwrap(),
    );
    network(&client).get_property::<String>("State").unwrap();

    let mut wifi = device(3, "wlan0", DeviceKind::Wifi, DeviceState::Disconnected);
    wifi.failure = Some(Failure::WrongPassphrase);
    let snapshot = Snapshot {
        wireless_enabled: true,
        devices: vec![wifi],
        networks: Vec::new(),
        known: Vec::new(),
    };
    running.daemon.publish(snapshot.clone()).unwrap();
    running.daemon.publish(snapshot).unwrap();

    assert_eq!(
        failures.recv_timeout(WAIT).unwrap(),
        ("wlan0".to_owned(), "wrong-passphrase".to_owned())
    );
    assert!(failures.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn a_second_daemon_cannot_take_the_names() {
    let bus = private_bus!();
    let first = start(&bus, Decision::Allowed, accept);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (second, _requests) = Daemon::connect(Config {
        bus: Bus::Address(bus.address.clone()),
        authorizer: Box::new(Scripted {
            decision: Decision::Allowed,
            asked,
        }),
    })
    .unwrap();
    let error = second.claim_names().unwrap_err();
    assert_eq!(
        error.to_string(),
        "the bus name org.freedesktop.NetworkManager is owned by another process"
    );
    drop(first);
}

/// The question from which [`Slow`] waits.
const SLOW_FROM: usize = 100;

/// Refuses every caller, as logind would for a process outside a session. From the [`SLOW_FROM`]th question on (by then
/// the daemon has asked the bus about many callers) each waits while the test holds `held`, as a slow logind keeps the
/// daemon's thread, and says on `asked` that it came.
struct Slow {
    held: Arc<Mutex<()>>,
    questions: std::sync::atomic::AtomicUsize,
    asked: Mutex<mpsc::Sender<()>>,
}

impl Authorizer for Slow {
    fn authorize(&self, _pid: u32) -> Result<Decision> {
        if self
            .questions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
            >= SLOW_FROM
        {
            let _ = self.asked.lock().unwrap().send(());
            drop(self.held.lock().unwrap());
        }
        Ok(Decision::Denied(
            "the caller is not in a login session".to_owned(),
        ))
    }
}

#[test]
fn a_flood_of_calls_while_logind_is_slow_does_not_make_the_bus_refuse_others() {
    // the system bus's own limits: the bus daemon queues few descriptors for one connection
    let dir = tempfile::tempdir().unwrap();
    let Some(bus) =
        PrivateBus::start_with_config(&cupertino_test_bus::system_limits_config(dir.path()))
    else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let held = Arc::new(Mutex::new(()));
    let (asked, questions) = mpsc::channel();
    let slow = Box::new(Slow {
        held: held.clone(),
        questions: Default::default(),
        asked: Mutex::new(asked),
    });
    let running = start_with(&bus, slow, Arc::default(), accept);
    let logind_waits = held.lock().unwrap();
    // calls that each make the daemon ask who called, sent at once without waiting for an answer
    let flooder = bus.connect();
    for _ in 0..3000 {
        let message = zbus::Message::method_call(NETWORK_PATH, "Scan")
            .and_then(|b| b.destination(NETWORK))
            .and_then(|b| b.interface(NETWORK))
            .and_then(|b| b.with_flags(zbus::message::Flags::NoReplyExpected))
            .and_then(|b| b.build(&()))
            .unwrap();
        zbus::block_on(flooder.inner().send(&message)).unwrap();
    }
    // the daemon waits for logind: the answers about the other callers come meanwhile
    questions
        .recv_timeout(WAIT)
        .expect("the daemon asked about a caller");
    thread::sleep(Duration::from_millis(300));
    let client = bus.connect();
    let answer = thread::spawn(move || error_name(network(&client).call::<_, _, ()>("Scan", &())));
    thread::sleep(Duration::from_millis(300));
    drop(logind_waits);
    // the daemon's own refusal, not the bus daemon's
    assert_eq!(
        answer.join().unwrap(),
        "org.cupertino.Network1.NotAuthorized"
    );
    assert_eq!(running.engine.commands(), []);
}
