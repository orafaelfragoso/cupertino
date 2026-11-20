//! The host bus across hosts (`shell-host.md#the-host-bus-and-its-bridge`), through the hosting runtime and a
//! stand-in server: the bridge's handshake and retained replay, the shared host's checks of every frame, the
//! forwarding from bridge to bridge and to the host's own components, and a separate host's waiting publications.

mod support;

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use cupertino_kit::core::{Application, Command, Window, WindowKey};
use cupertino_kit_shell::bus::{self, Message, OsdKind, Panel, Sender, Topic};
use cupertino_kit_shell::hosting::Host;
use cupertino_kit_shell::hosting::{HostEvent, Kind, Runtime};
use cupertino_kit_shell::{Component, ComponentName, Entry, Event, Program, Role, Surface};
use serde_json::{json, Value};
use support::{
    component_connection, connect_host, eventually, message, read_frame, send_frame, settle,
    wait_for, Seen, Server,
};

/// What each test component heard on the bus.
static HEARD: Mutex<Vec<(ComponentName, Sender, Message)>> = Mutex::new(Vec::new());
/// The separate host's component has published while it had no bridge.
static PUBLISHED: AtomicBool = AtomicBool::new(false);

fn heard(component: ComponentName, from: Sender, wanted: &Message) -> bool {
    HEARD
        .lock()
        .unwrap()
        .iter()
        .any(|(c, f, m)| *c == component && *f == from && m == wanted)
}

macro_rules! component {
    ($ty:ident, $name:expr, $init:expr) => {
        struct $ty;

        impl Application for $ty {
            type Model = ();
            type Message = ();

            fn init() -> ((), Vec<Command<()>>) {
                $init();
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
                if let Event::Bus { from, message } = event {
                    HEARD.lock().unwrap().push(($name, from, message));
                }
                None
            }
        }

        impl Component for $ty {
            const NAME: ComponentName = $name;
        }
    };
}

component!(MenuBar, ComponentName::MENUBAR, || bus::follow(&[
    Topic::Osd,
    Topic::Panels
]));

component!(Osd, ComponentName::OSD, || {
    bus::follow(&[Topic::Panels]);
    bus::publish(Message::PanelShown {
        panel: Panel::Launcher,
        shown: true,
        output: None,
    });
    bus::publish(Message::ShowOsd {
        kind: OsdKind::Volume,
        level: 0.5,
    });
    PUBLISHED.store(true, Ordering::SeqCst);
});

static MENU_BAR: Entry = Entry::of::<MenuBar>();
static OSD: Entry = Entry::of::<Osd>();

fn hello(component: &str) -> Value {
    json!({"type": "hello", "version": 1, "component": component})
}

fn welcome() -> Value {
    json!({"type": "welcome", "version": 1})
}

fn show_osd(level: f64) -> Value {
    json!({"type": "ShowOsd", "kind": "Volume", "level": level})
}

fn panel_shown(panel: &str) -> Value {
    json!({"type": "PanelShown", "panel": panel, "shown": true, "output": null})
}

/// Says hello as the separate host of `component` and reads the shared host's welcome and replay.
fn join(
    server: &Server,
    host: &mut Host,
    component: &str,
    replayed: usize,
) -> (UnixStream, Vec<Value>) {
    let mut bridge = server.bus_peer(component);
    settle(host);
    send_frame(&mut bridge, &hello(component));
    assert_eq!(read_frame(&mut bridge), Some(welcome()));
    let replay = (0..replayed)
        .map(|_| read_frame(&mut bridge).expect("a retained publication"))
        .collect();
    (bridge, replay)
}

/// The bridge is closed by the shared host, which says so to the server.
fn dropped(server: &Server, bridge: &mut UnixStream, component: &str) {
    assert_eq!(read_frame(bridge), None);
    server.expect(|seen| *seen == Seen::BusDropped(component.into()));
}

#[test]
fn a_shared_host_checks_its_bridges_and_forwards_between_them() {
    let (server, host_end) = Server::start(Kind::Shared, &["menubar"]);
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    // the server sends a bridge's end as soon as it binds the host: before the host has its runtime
    let mut osd = server.bus_peer("osd");
    settle(&mut host);
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();
    runtime.publish(Message::PanelShown {
        panel: Panel::ControlPanel,
        shown: true,
        output: None,
    });
    let connection = component_connection(&mut host, "menubar");
    let mut menu_bar = runtime.start(&MENU_BAR, connection).unwrap();

    // hello, then welcome and every retained publication
    send_frame(&mut osd, &hello("osd"));
    assert_eq!(read_frame(&mut osd), Some(welcome()));
    assert_eq!(
        read_frame(&mut osd),
        Some(message("host", panel_shown("ControlPanel")))
    );
    let (mut dock, replay) = join(&server, &mut host, "dock", 1);
    assert_eq!(replay, vec![message("host", panel_shown("ControlPanel"))]);

    // a bridge's publication reaches the host's components and the other bridges, never its sender
    send_frame(&mut osd, &message("osd", show_osd(0.25)));
    assert_eq!(read_frame(&mut dock), Some(message("osd", show_osd(0.25))));
    let level = Message::ShowOsd {
        kind: OsdKind::Volume,
        level: 0.25,
    };
    eventually("the menu bar hears the osd", || {
        heard(
            ComponentName::MENUBAR,
            Sender::Component(ComponentName::OSD),
            &level,
        )
    });
    send_frame(&mut dock, &message("dock", panel_shown("Launcher")));
    assert_eq!(
        read_frame(&mut osd),
        Some(message("dock", panel_shown("Launcher")))
    );

    // a retained publication stays until its publisher's host is gone
    let (_notifications, mut replay) = join(&server, &mut host, "notifications", 2);
    replay.sort_by_key(|frame| frame["from"].to_string());
    assert_eq!(
        replay,
        vec![
            message("dock", panel_shown("Launcher")),
            message("host", panel_shown("ControlPanel")),
        ]
    );
    server.bus_peer_gone("dock");
    settle(&mut host);
    assert_eq!(read_frame(&mut dock), None);
    let (_launcher, replay) = join(&server, &mut host, "launcher", 1);
    assert_eq!(replay, vec![message("host", panel_shown("ControlPanel"))]);

    // another sender's name, the Host topic, or another component's hello closes the bridge
    send_frame(&mut osd, &message("dock", show_osd(0.5)));
    dropped(&server, &mut osd, "osd");
    let (mut agent, _) = join(&server, &mut host, "agent", 1);
    send_frame(
        &mut agent,
        &message(
            "agent",
            json!({"type": "ComponentStopped", "component": "agent"}),
        ),
    );
    dropped(&server, &mut agent, "agent");
    let mut dialogs = server.bus_peer("dialogs");
    settle(&mut host);
    send_frame(&mut dialogs, &hello("osd"));
    dropped(&server, &mut dialogs, "dialogs");
    let mut desktop = server.bus_peer("desktop");
    settle(&mut host);
    send_frame(
        &mut desktop,
        &json!({"type": "hello", "version": 2, "component": "desktop"}),
    );
    dropped(&server, &mut desktop, "desktop");

    menu_bar.stop();
    eventually("the menu bar stops", || menu_bar.ended().is_some());
}

#[test]
fn a_separate_host_says_hello_and_sends_what_waited_for_the_bridge() {
    let (server, host_end) = Server::start(Kind::Separate, &["osd"]);
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Separate, environment).unwrap();
    let connection = component_connection(&mut host, "osd");
    let mut osd = runtime.start(&OSD, connection).unwrap();
    eventually("the osd publishes", || PUBLISHED.load(Ordering::SeqCst));

    let mut shared = server.bus();
    settle(&mut host);
    assert_eq!(read_frame(&mut shared), Some(hello("osd")));
    send_frame(&mut shared, &welcome());
    send_frame(
        &mut shared,
        &message("host", panel_shown("NotificationCenter")),
    );
    // its component's retained publications again, then what waited for the bridge, in order
    assert_eq!(
        (0..3)
            .map(|_| read_frame(&mut shared).unwrap())
            .collect::<Vec<_>>(),
        vec![
            message("osd", panel_shown("Launcher")),
            message("osd", panel_shown("Launcher")),
            message("osd", show_osd(0.5)),
        ]
    );
    let shown = Message::PanelShown {
        panel: Panel::NotificationCenter,
        shown: true,
        output: None,
    };
    eventually("the osd hears the shared host", || {
        heard(ComponentName::OSD, Sender::Host, &shown)
    });

    // a message in the separate host's own name closes the bridge
    send_frame(&mut shared, &message("osd", show_osd(1.0)));
    dropped(&server, &mut shared, "osd");

    osd.stop();
    eventually("the osd stops", || osd.ended().is_some());
}
