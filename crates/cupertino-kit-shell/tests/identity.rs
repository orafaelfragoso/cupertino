//! A component's session-bus identity (D20, `shell-host.md#identity`) against a private bus daemon, a stand-in of
//! `org.cupertino.Session1` and the stand-in server: `bus::session` announces the connection's unique name on the
//! component's Wayland connection, returns the server's nonce from that bus connection, says the server's answer as
//! `Event::BusIdentity`, and only then hands the connection over; a development build's debug interface answers on the
//! connection before the announcement does; `Running::shut` cuts the connection off.

mod support;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};
use std::sync::Mutex;

use cupertino_kit::core::{Application, Command, Window, WindowKey};
use cupertino_kit_shell::bus;
use cupertino_kit_shell::hosting::{HostEvent, Kind, Runtime};
use cupertino_kit_shell::{Component, ComponentName, Entry, Event, Program, Role, Surface};
use support::{component_connection, connect_host, eventually, wait_for, Judge, Seen, Server};

/// The bus names the stand-in judged, in order.
static JUDGED: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// A component's connection when it got it: its unique name, and whether its announcement was judged by then.
type Connected = (ComponentName, Result<String, String>, bool);

/// Each component's connection when it got it.
static CONNECTED: Mutex<Vec<Connected>> = Mutex::new(Vec::new());
/// Each component's identity, as `Event::BusIdentity` said it.
static IDENTITY: Mutex<Vec<(ComponentName, bool)>> = Mutex::new(Vec::new());
/// The connections, kept open.
static KEPT: Mutex<Vec<zbus::Connection>> = Mutex::new(Vec::new());
/// Each announcing bus name, and whether its debug interface answered before the stand-in answered the announcement.
static DEBUG_SERVED: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

/// The object path of a development build's debug interface, and the interface.
const DEBUG_PATH: &str = "/org/cupertino/ShellComponentDebug1";
const DEBUG_INTERFACE: &str = "org.cupertino.ShellComponentDebug1";

/// The stand-in server accepts every component but the dialogs.
fn accept(component: &str) -> bool {
    component != "dialogs"
}

struct Session {
    judge: Judge,
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl Session {
    async fn announce_component(
        &self,
        nonce: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        // a caller that finds the component by the name about to be accepted reaches its debug interface already
        let debug = connection
            .call_method(
                Some(sender.as_str()),
                DEBUG_PATH,
                Some("org.freedesktop.DBus.Introspectable"),
                "Introspect",
                &(),
            )
            .await
            .ok()
            .and_then(|reply| reply.body().deserialize::<String>().ok())
            .is_some_and(|xml| xml.contains(DEBUG_INTERFACE));
        DEBUG_SERVED.lock().unwrap().push((sender.clone(), debug));
        JUDGED.lock().unwrap().push(sender.clone());
        if self.judge.judge(&sender, &nonce, accept) {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AccessDenied(
                "not this component's name".into(),
            ))
        }
    }
}

/// The tasks a test component's `init` gives its loop: the connection made, proved and handed over, beside the
/// connection's own work, which the loop runs for as long as the component runs.
fn connect_in_init(name: ComponentName) -> Vec<Command<()>> {
    bus::session(
        move |connection| {
            let unique = connection.map(|connection| {
                let unique = connection
                    .unique_name()
                    .map(|n| n.to_string())
                    .unwrap_or_default();
                KEPT.lock().unwrap().push(connection);
                unique
            });
            let judged = unique
                .as_ref()
                .is_ok_and(|unique| JUDGED.lock().unwrap().contains(unique));
            CONNECTED
                .lock()
                .unwrap()
                .push((name, unique.map_err(|e| e.to_string()), judged));
        },
        (),
    )
}

macro_rules! component {
    ($ty:ident, $name:expr) => {
        struct $ty;

        impl Application for $ty {
            type Model = ();
            type Message = ();

            fn init() -> ((), Vec<Command<()>>) {
                ((), connect_in_init($name))
            }

            fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
                Vec::new()
            }

            fn view(_: &()) -> Vec<Window<()>> {
                Vec::new()
            }
        }

        impl Program for $ty {
            fn surface(_: &(), _: &WindowKey) -> Surface {
                Surface {
                    role: Role::Window,
                    persistent: None,
                    holes: Vec::new(),
                }
            }

            fn event(event: Event) -> Option<()> {
                if let Event::BusIdentity { accepted } = event {
                    IDENTITY.lock().unwrap().push(($name, accepted));
                }
                None
            }
        }

        impl Component for $ty {
            const NAME: ComponentName = $name;
        }
    };
}

component!(Agent, ComponentName::AGENT);
component!(Dialogs, ComponentName::DIALOGS);
component!(Notifications, ComponentName::NOTIFICATIONS);

static AGENT: Entry = Entry::of::<Agent>();
static DIALOGS: Entry = Entry::of::<Dialogs>();
static NOTIFICATIONS: Entry = Entry::of::<Notifications>();

/// A private session bus daemon, stopped when dropped.
struct Daemon(Child);

impl Daemon {
    fn start() -> Daemon {
        // a socket's path is short: the daemon draws a name of its own in the temporary directory
        let dir = std::env::temp_dir();
        let mut child = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:tmpdir={}", dir.display()))
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        assert!(!address.trim().is_empty(), "dbus-daemon gives its address");
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", address.trim());
        Daemon(child)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn connected(name: ComponentName) -> Option<(Result<String, String>, bool)> {
    CONNECTED
        .lock()
        .unwrap()
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, unique, judged)| (unique.clone(), *judged))
}

fn identity(name: ComponentName) -> Option<bool> {
    IDENTITY
        .lock()
        .unwrap()
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, accepted)| *accepted)
}

#[test]
fn a_component_proves_its_bus_name_before_it_gets_its_connection() {
    let _daemon = Daemon::start();
    let (server, host_end) = Server::start(Kind::Shared, &["agent", "dialogs", support::NO_NONCE]);
    let session = async_io::block_on(async {
        zbus::connection::Builder::session()?
            .name("org.cupertino.Session1")?
            .serve_at(
                "/org/cupertino/Session1",
                Session {
                    judge: server.judge(),
                },
            )?
            .build()
            .await
    })
    .expect("the stand-in of org.cupertino.Session1 serves");
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();

    let connection = component_connection(&mut host, "agent");
    let agent = runtime.start(&AGENT, connection).unwrap();
    eventually("the agent gets its connection and its identity", || {
        connected(ComponentName::AGENT).is_some() && identity(ComponentName::AGENT).is_some()
    });
    let (unique, judged) = connected(ComponentName::AGENT).unwrap();
    let unique = unique.expect("the agent's bus connection");
    assert!(
        judged,
        "the agent got its connection before its identity was settled"
    );
    assert_eq!(identity(ComponentName::AGENT), Some(true));
    // a development build serves the debug interface before it announces the name, so no call that finds the
    // component by its accepted name comes before the interface
    assert!(
        DEBUG_SERVED
            .lock()
            .unwrap()
            .contains(&(unique.clone(), cfg!(debug_assertions))),
        "the agent's debug interface is served as its announcement is judged"
    );
    // the name announced on the agent's Wayland connection is its bus connection's
    server.expect(|seen| {
        matches!(seen, Seen::Announced { component, name, .. } if component == "agent" && *name == unique)
    });

    let connection = component_connection(&mut host, "dialogs");
    let mut dialogs = runtime.start(&DIALOGS, connection).unwrap();
    eventually(
        "the dialogs get their connection and their identity",
        || {
            connected(ComponentName::DIALOGS).is_some()
                && identity(ComponentName::DIALOGS).is_some()
        },
    );
    let (refused, judged) = connected(ComponentName::DIALOGS).unwrap();
    assert!(
        refused.is_ok() && judged,
        "a refused component still gets its connection, after the answer"
    );
    assert_eq!(identity(ComponentName::DIALOGS), Some(false));

    // a name the server refuses without a nonce (it fails closed) ends the proof: the component hears it is
    // refused and still gets its connection
    let connection = component_connection(&mut host, support::NO_NONCE);
    let mut notifications = runtime.start(&NOTIFICATIONS, connection).unwrap();
    eventually(
        "the notifications get their connection and their identity",
        || {
            connected(ComponentName::NOTIFICATIONS).is_some()
                && identity(ComponentName::NOTIFICATIONS).is_some()
        },
    );
    let (refused, _) = connected(ComponentName::NOTIFICATIONS).unwrap();
    assert!(
        refused.is_ok(),
        "a refused component still gets its connection"
    );
    assert_eq!(identity(ComponentName::NOTIFICATIONS), Some(false));
    notifications.stop();
    eventually("the notifications stop", || notifications.ended().is_some());

    // a shut component's bus connection is cut off: the bus daemon drops its name
    let has_owner = |name: &str| -> bool {
        async_io::block_on(session.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &name,
        ))
        .and_then(|reply| reply.body().deserialize::<bool>())
        .unwrap_or(false)
    };
    assert!(has_owner(&unique));
    agent.shut();
    eventually("the bus daemon drops the shut agent's name", || {
        !has_owner(&unique)
    });

    dialogs.stop();
    eventually("the dialogs stop", || dialogs.ended().is_some());
}
