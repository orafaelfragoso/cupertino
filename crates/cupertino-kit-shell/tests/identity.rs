//! A component's session-bus identity (D20, `shell-host.md#identity`) against a private bus daemon, a stand-in of
//! `org.cupertino.Session1` and the stand-in server: `bus::session` announces the connection's unique name on the
//! component's Wayland connection, returns the server's nonce from that bus connection, says the server's answer as
//! `Event::BusIdentity`, and only then hands the connection over; `Running::shut` cuts the connection off.

mod support;

use std::future::Future;
use std::io::{BufRead, BufReader};
use std::pin::Pin;
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
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
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

/// The task a test component's loop would run: the connection made, proved and handed over, beside the
/// connection's own work. This crate's component loop runs no tasks yet, so the component runs them in its `init`.
fn connect_in_init(name: ComponentName) {
    let commands = bus::session(
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
    );
    let mut tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Perform(task) => Some(task),
            _ => None,
        })
        .collect();
    let mut run = tasks.pop().expect("the connection's work");
    let mut connect = tasks.pop().expect("the connection made");
    async_io::block_on(std::future::poll_fn(|cx| {
        let _ = run.as_mut().poll(cx);
        connect.as_mut().poll(cx)
    }));
}

macro_rules! component {
    ($ty:ident, $name:expr) => {
        struct $ty;

        impl Application for $ty {
            type Model = ();
            type Message = ();

            fn init() -> ((), Vec<Command<()>>) {
                connect_in_init($name);
                ((), Vec::new())
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

static AGENT: Entry = Entry::of::<Agent>();
static DIALOGS: Entry = Entry::of::<Dialogs>();

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
    let (server, host_end) = Server::start(Kind::Shared, &["agent", "dialogs"]);
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
