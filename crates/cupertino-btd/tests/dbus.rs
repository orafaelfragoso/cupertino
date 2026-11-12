//! The daemon on a private bus with a fake management interface behind it: a private
//! `dbus-daemon` is started for the test (skipped when the binary is not installed), the daemon
//! connects to it and a second connection plays the shell.
#![cfg(feature = "dbus")]

use crate::common;

use std::collections::HashMap;
use std::fmt::Debug;
use std::os::unix::fs::PermissionsExt;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use common::controller::{FakeController, PASSKEY};
use common::{device, phone, OFF, POWERED};
use cupertino_bluetooth::{AddressType, ClassOfDevice, Command, DeviceId, Eir, EirName};
use cupertino_btd::auth::{Authorizer, Decision};
use cupertino_btd::daemon::{Bus, Config, Daemon};
use cupertino_btd::error::{Error, Result};
use cupertino_btd::transport::NoBluetooth;
use cupertino_test_bus::PrivateBus;
use tempfile::TempDir;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedValue};

const NAME: &str = "org.cupertino.Bluetooth1";
const PATH: &str = "/org/cupertino/Bluetooth1";
const AGENT_PATH: &str = "/test/agent";
const WAIT: Duration = Duration::from_secs(5);

type Dict = HashMap<String, OwnedValue>;

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

/// A daemon on a private bus with a fake controller. Dropping it stops the daemon's loop and
/// checks that it ended because the controller went away.
struct Running {
    client: Connection,
    controller: Arc<FakeController>,
    keys: TempDir,
    asked: Arc<Mutex<Vec<u32>>>,
    bus: PrivateBus,
    daemon: Option<JoinHandle<Error>>,
}

impl Running {
    /// `None` when `dbus-daemon` is not installed.
    fn start(settings: u32, nearby: Vec<(DeviceId, Eir)>, decision: Decision) -> Option<Self> {
        let bus = PrivateBus::start()?;
        let controller = Arc::new(FakeController::new(settings, nearby));
        let keys = tempfile::tempdir().unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let daemon = Daemon::start(Config {
            bus: Bus::Address(bus.address.clone()),
            keys_dir: keys.path().join("bluetooth"),
            read_only: false,
            authorizer: Box::new(Scripted {
                decision,
                asked: asked.clone(),
            }),
            transport: controller.clone(),
        })
        .unwrap();
        let thread = std::thread::spawn(move || daemon.run().unwrap_err());
        let running = Self {
            client: bus.connect(),
            controller,
            keys,
            asked,
            bus,
            daemon: Some(thread),
        };
        // The adapter appears once the daemon has read it and its keys.
        wait_for("the adapter to be available", || {
            running.proxy().get_property::<bool>("Available").unwrap()
        });
        Some(running)
    }

    fn proxy(&self) -> Proxy<'_> {
        proxy_on(&self.client)
    }

    fn keys_file(&self) -> std::path::PathBuf {
        self.keys.path().join("bluetooth/AA-BB-CC-DD-EE-01.keys")
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.controller.close();
        let error = self.daemon.take().unwrap().join().unwrap();
        if !std::thread::panicking() {
            assert!(
                matches!(error, Error::Bluetooth(_)),
                "the loop ended with {error}"
            );
        }
    }
}

/// A proxy for the daemon, without a property cache: the tests read what the daemon changed.
fn proxy_on(client: &Connection) -> Proxy<'_> {
    zbus::blocking::proxy::Builder::<Proxy>::new(client)
        .destination(NAME)
        .unwrap()
        .path(PATH)
        .unwrap()
        .interface(NAME)
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .unwrap()
}

macro_rules! running {
    ($settings:expr, $nearby:expr, $decision:expr) => {
        match Running::start($settings, $nearby, $decision) {
            Some(running) => running,
            None => {
                eprintln!("skipping: dbus-daemon is not available");
                return;
            }
        }
    };
}

/// Polls until `condition` holds.
fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn field<T: TryFrom<OwnedValue>>(device: &Dict, name: &str) -> Option<T>
where
    T::Error: Debug,
{
    device
        .get(name)
        .map(|value| T::try_from(value.try_clone().unwrap()).unwrap())
}

/// `(Id, Name, Icon, Paired)` of each device.
fn summary(devices: &[Dict]) -> Vec<(String, Option<String>, String, bool)> {
    devices
        .iter()
        .map(|d| {
            (
                field(d, "Id").unwrap(),
                field(d, "Name"),
                field(d, "Icon").unwrap(),
                field(d, "Paired").unwrap(),
            )
        })
        .collect()
}

fn devices(proxy: &Proxy<'_>) -> Vec<Dict> {
    proxy.get_property("Devices").unwrap()
}

fn call_error<T: Debug>(result: zbus::Result<T>) -> String {
    match result.unwrap_err() {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        other => panic!("expected a method error, got {other}"),
    }
}

/// A phone, found by name and class, and a low-energy keyboard that shows only its appearance.
fn nearby() -> Vec<(DeviceId, Eir)> {
    let phone_eir = Eir {
        name: Some(EirName {
            text: "Phone".to_owned(),
            complete: true,
        }),
        class: Some(ClassOfDevice::from_wire([0x0C, 0x02, 0x5A])),
        ..Eir::default()
    };
    let keyboard_eir = Eir {
        appearance: Some(0x03C1),
        ..Eir::default()
    };
    vec![
        (phone(), phone_eir),
        (device(0x02, AddressType::LePublic), keyboard_eir),
    ]
}

fn start_discovery_and_wait(running: &Running) -> Vec<Dict> {
    running
        .proxy()
        .call::<_, _, ()>("StartDiscovery", &())
        .unwrap();
    wait_for("both devices", || devices(&running.proxy()).len() == 2);
    devices(&running.proxy())
}

/// The shell's pairing agent: records what it is asked and accepts or refuses.
struct TestAgent {
    accept: bool,
    calls: Arc<Mutex<Vec<String>>>,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Bluetooth1.Agent")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
}

#[zbus::interface(name = "org.cupertino.Bluetooth1.Agent")]
impl TestAgent {
    fn confirm_passkey(
        &self,
        id: u64,
        device: Dict,
        passkey: u32,
        just_works: bool,
    ) -> std::result::Result<bool, AgentError> {
        let name: String = field(&device, "Name").unwrap();
        self.calls
            .lock()
            .unwrap()
            .push(format!("confirm {id} {name} {passkey} {just_works}"));
        if self.accept {
            Ok(true)
        } else {
            Err(AgentError::Rejected("the user said no".to_owned()))
        }
    }

    fn cancel(&self, id: u64) {
        self.calls.lock().unwrap().push(format!("cancel {id}"));
    }
}

fn register_agent(client: &Connection, accept: bool) -> Arc<Mutex<Vec<String>>> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    client
        .object_server()
        .at(
            AGENT_PATH,
            TestAgent {
                accept,
                calls: calls.clone(),
            },
        )
        .unwrap();
    proxy_on(client)
        .call::<_, _, ()>(
            "RegisterAgent",
            &(ObjectPath::try_from(AGENT_PATH).unwrap(),),
        )
        .unwrap();
    calls
}

#[test]
fn a_kernel_without_bluetooth_leaves_the_name_owned_with_no_adapter() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let keys = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(Config {
        bus: Bus::Address(bus.address.clone()),
        keys_dir: keys.path().join("bluetooth"),
        read_only: false,
        authorizer: Box::new(Scripted {
            decision: Decision::Allowed,
            asked: Arc::new(Mutex::new(Vec::new())),
        }),
        transport: Arc::new(NoBluetooth),
    })
    .unwrap();
    std::thread::spawn(move || daemon.run());
    let client = bus.connect();
    let proxy = proxy_on(&client);
    assert!(!proxy.get_property::<bool>("Available").unwrap());
    assert!(!proxy.get_property::<bool>("Powered").unwrap());
    assert!(proxy.set_property("Powered", true).is_err());
}

#[test]
fn an_authorised_caller_powers_the_adapter_and_everyone_hears_it() {
    let running = running!(OFF, Vec::new(), Decision::Allowed);
    let proxy = running.proxy();
    assert!(!proxy.get_property::<bool>("Powered").unwrap());

    // PropertiesChanged on the object, collected before the write.
    let properties = Proxy::new(
        &running.client,
        NAME,
        PATH,
        "org.freedesktop.DBus.Properties",
    )
    .unwrap();
    let mut signals = properties.receive_signal("PropertiesChanged").unwrap();
    let (sender, changed) = mpsc::channel();
    std::thread::spawn(move || {
        for message in signals.by_ref() {
            let (interface, changed, _): (String, Dict, Vec<String>) =
                message.body().deserialize().unwrap();
            if sender
                .send((interface, field::<bool>(&changed, "Powered")))
                .is_err()
            {
                return;
            }
        }
    });

    proxy.set_property("Powered", true).unwrap();
    assert!(proxy.get_property::<bool>("Powered").unwrap());
    assert_eq!(
        changed.recv_timeout(WAIT).unwrap(),
        (NAME.to_owned(), Some(true))
    );
    assert_eq!(
        running
            .controller
            .sent()
            .iter()
            .filter(|command| matches!(command, Command::SetPowered(_)))
            .collect::<Vec<_>>(),
        [&Command::SetPowered(true)]
    );

    proxy.set_property("Powered", false).unwrap();
    assert!(!proxy.get_property::<bool>("Powered").unwrap());
    // The authorizer saw the pid of the caller, taken from the bus daemon.
    let me = std::process::id();
    assert_eq!(*running.asked.lock().unwrap(), [me, me]);
}

#[test]
fn a_caller_outside_the_active_session_is_refused_but_may_read() {
    let running = running!(
        POWERED,
        nearby(),
        Decision::Denied("session c2 is not the active session of seat0".to_owned())
    );
    let proxy = running.proxy();

    let error = proxy.set_property("Powered", false).unwrap_err();
    assert_eq!(
        error.to_string(),
        "org.freedesktop.DBus.Error.AuthFailed: session c2 is not the active session of seat0"
    );
    assert_eq!(
        call_error(proxy.call::<_, _, ()>("StartDiscovery", &())),
        "org.cupertino.Bluetooth1.NotAuthorized"
    );
    assert_eq!(
        call_error(proxy.call::<_, _, ()>("Pair", &("11:22:33:44:55:01",))),
        "org.cupertino.Bluetooth1.NotAuthorized"
    );
    assert_eq!(
        call_error(proxy.call::<_, _, ()>(
            "RegisterAgent",
            &(ObjectPath::try_from(AGENT_PATH).unwrap(),)
        )),
        "org.cupertino.Bluetooth1.NotAuthorized"
    );

    // Nothing reached the controller, and reads are open.
    assert!(running.controller.sent().iter().all(|command| !matches!(
        command,
        Command::SetPowered(_) | Command::StartDiscovery(_) | Command::PairDevice { .. }
    )));
    assert!(proxy.get_property::<bool>("Powered").unwrap());
    assert!(!proxy.get_property::<bool>("Discovering").unwrap());
    assert!(devices(&proxy).is_empty());
}

#[test]
fn discovery_lists_the_devices_that_the_controller_finds_and_announces_them() {
    let running = running!(POWERED, nearby(), Decision::Allowed);
    let announced = Proxy::new(&running.client, NAME, PATH, NAME)
        .unwrap()
        .receive_signal("DeviceAdded")
        .unwrap();
    let (sender, added) = mpsc::channel();
    std::thread::spawn(move || {
        for message in announced {
            let device: Dict = message.body().deserialize().unwrap();
            if sender
                .send(field::<String>(&device, "Id").unwrap())
                .is_err()
            {
                return;
            }
        }
    });

    let found = start_discovery_and_wait(&running);
    assert!(running.proxy().get_property::<bool>("Discovering").unwrap());
    assert_eq!(
        summary(&found),
        [
            (
                "11:22:33:44:55:01/bredr".to_owned(),
                Some("Phone".to_owned()),
                "phone".to_owned(),
                false
            ),
            // No name was reported: the field is absent, not empty.
            (
                "11:22:33:44:55:02/le-public".to_owned(),
                None,
                "keyboard".to_owned(),
                false
            ),
        ]
    );
    assert_eq!(field::<i16>(&found[0], "Rssi"), Some(-52));
    assert_eq!(field::<u32>(&found[0], "Class"), Some(0x5A020C));
    assert_eq!(
        field::<String>(&found[1], "AddressType"),
        Some("le-public".to_owned())
    );
    assert_eq!(field::<u16>(&found[1], "Appearance"), Some(0x03C1));
    assert_eq!(
        [
            added.recv_timeout(WAIT).unwrap(),
            added.recv_timeout(WAIT).unwrap()
        ],
        ["11:22:33:44:55:01/bredr", "11:22:33:44:55:02/le-public"]
    );

    running
        .proxy()
        .call::<_, _, ()>("StopDiscovery", &())
        .unwrap();
    wait_for("discovery to stop", || {
        !running.proxy().get_property::<bool>("Discovering").unwrap()
    });
}

#[test]
fn pairing_asks_the_registered_agent_and_stores_the_key() {
    let running = running!(POWERED, nearby(), Decision::Allowed);
    let agent = running.bus.connect();
    let calls = register_agent(&agent, true);
    start_discovery_and_wait(&running);

    running
        .proxy()
        .call::<_, _, ()>("Pair", &("11:22:33:44:55:01",))
        .unwrap();

    assert_eq!(
        *calls.lock().unwrap(),
        [format!("confirm 1 Phone {PASSKEY} false")]
    );
    let paired = summary(&devices(&running.proxy()));
    assert_eq!(
        paired[0],
        (
            "11:22:33:44:55:01/bredr".to_owned(),
            Some("Phone".to_owned()),
            "phone".to_owned(),
            true
        )
    );
    assert!(running
        .controller
        .sent()
        .contains(&Command::UserConfirmationReply {
            device: phone(),
            accept: true
        }));

    // The key is on disk, in the documented form, readable by the owner only.
    let file = running.keys_file();
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("cupertino-bluetooth-keys 1\ndevice 11:22:33:44:55:01 bredr\nname 50686f6e65\nlink 08 00 {}\n", "42".repeat(16)));
}

#[test]
fn an_agent_that_refuses_fails_the_pairing_as_rejected() {
    let running = running!(POWERED, nearby(), Decision::Allowed);
    let agent = running.bus.connect();
    let calls = register_agent(&agent, false);
    start_discovery_and_wait(&running);

    assert_eq!(
        call_error(
            running
                .proxy()
                .call::<_, _, ()>("Pair", &("11:22:33:44:55:01",))
        ),
        "org.cupertino.Bluetooth1.Rejected"
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(!summary(&devices(&running.proxy()))[0].3);
    assert!(!running.keys_file().exists());
}

#[test]
fn pairing_without_an_agent_fails_and_says_why() {
    let running = running!(POWERED, nearby(), Decision::Allowed);
    start_discovery_and_wait(&running);

    assert_eq!(
        call_error(
            running
                .proxy()
                .call::<_, _, ()>("Pair", &("11:22:33:44:55:01",))
        ),
        "org.cupertino.Bluetooth1.Agent"
    );
    assert!(running
        .controller
        .sent()
        .contains(&Command::UserConfirmationReply {
            device: phone(),
            accept: false
        }));
}

#[test]
fn a_second_client_cannot_take_over_a_live_agent_but_may_replace_a_dead_one() {
    let running = running!(POWERED, Vec::new(), Decision::Allowed);
    let first = running.bus.connect();
    register_agent(&first, true);

    let second = running.bus.connect();
    let path = ObjectPath::try_from(AGENT_PATH).unwrap();
    assert_eq!(
        call_error(proxy_on(&second).call::<_, _, ()>("RegisterAgent", &(path.clone(),))),
        "org.cupertino.Bluetooth1.AgentExists"
    );

    drop(first);
    // The bus tells the daemon the owner is gone; the next registration succeeds.
    wait_for("the dead agent to be replaced", || {
        proxy_on(&second)
            .call::<_, _, ()>("RegisterAgent", &(path.clone(),))
            .is_ok()
    });
}
