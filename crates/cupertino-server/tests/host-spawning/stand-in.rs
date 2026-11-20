//! A stand-in for `cupertino-shell` in the server's tests: the supervisor starts the test binary of
//! `tests/host-spawning.rs` as the host program, and its test `stand_in_host` runs [`run`]. As a host it binds `cupertino_host_v1` on its `WAYLAND_SOCKET`, asks for a connection for every component of its list
//! and runs each as a component that says it is ready at once. It answers `unavailable` for the components it was
//! "built without" ([`BUILT_WITHOUT`]), starts a component again when the server asks for a restart, exits when the
//! server sends `restart`, and exits successfully as a separate host whose list became empty.
//!
//! With `--process` it is a plain shell process instead: it holds its connection until the server closes it.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;

use cupertino_protocols::client::host::cupertino_component_v1::{self, CupertinoComponentV1};
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1, UnavailableReason,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1, Kind};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

/// The components this stand-in host was built without: it answers `unavailable` with `absent` for each.
const BUILT_WITHOUT: [&str; 1] = ["dialogs"];

/// Be the host, or with `--process` a plain shell process, on the connection of `WAYLAND_SOCKET`.
pub fn run() {
    let conn = Connection::connect_to_env().expect("started with a WAYLAND_SOCKET");
    if std::env::args().any(|arg| arg == "--process") {
        let mut queue = conn.new_event_queue::<Plain>();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        while queue.blocking_dispatch(&mut Plain).is_ok() {}
        return;
    }
    let mut queue = conn.new_event_queue::<Host>();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    let mut host = Host::default();
    while !host.exit && queue.blocking_dispatch(&mut host).is_ok() {}
}

/// A shell process with nothing to do.
struct Plain;

impl Dispatch<wl_registry::WlRegistry, ()> for Plain {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[derive(Default)]
struct Host {
    host: Option<CupertinoHostV1>,
    kind: Option<Kind>,
    /// The names of the list being sent.
    incoming: Vec<String>,
    /// The component objects of the list in force.
    objects: HashMap<String, CupertinoHostComponentV1>,
    exit: bool,
}

impl Host {
    /// Make the list just sent the one in force: objects for the new names, none for the names that left.
    fn apply_list(&mut self, qh: &QueueHandle<Self>) {
        let list = std::mem::take(&mut self.incoming);
        self.objects.retain(|name, object| {
            let keep = list.contains(name);
            if !keep {
                object.destroy();
            }
            keep
        });
        let Some(host) = self.host.clone() else {
            return;
        };
        for name in &list {
            if self.objects.contains_key(name) {
                continue;
            }
            let object = host.get_component(name.clone(), qh, name.clone());
            if BUILT_WITHOUT.contains(&name.as_str()) {
                object.unavailable(UnavailableReason::Absent);
            } else {
                object.connect();
            }
            self.objects.insert(name.clone(), object);
        }
        if self.kind == Some(Kind::Separate) && list.is_empty() {
            self.exit = true;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Host {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "cupertino_host_v1" {
                state.host = Some(registry.bind::<CupertinoHostV1, _, _>(name, 1, qh, ()));
            }
        }
    }
}

impl Dispatch<CupertinoHostV1, ()> for Host {
    fn event(
        state: &mut Self,
        _: &CupertinoHostV1,
        event: cupertino_host_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_host_v1::Event::Kind {
                kind: WEnum::Value(kind),
            } => state.kind = Some(kind),
            cupertino_host_v1::Event::Component { name } => state.incoming.push(name),
            cupertino_host_v1::Event::Done => state.apply_list(qh),
            cupertino_host_v1::Event::Restart => state.exit = true,
            // the bus bridge's descriptors are closed as they are dropped
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHostComponentV1, String> for Host {
    fn event(
        _: &mut Self,
        object: &CupertinoHostComponentV1,
        event: cupertino_host_component_v1::Event,
        name: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_host_component_v1::Event::Connection { fd } => {
                let name = name.clone();
                std::thread::spawn(move || run_component(&name, UnixStream::from(fd)));
            }
            cupertino_host_component_v1::Event::Restart => object.connect(),
            _ => {}
        }
    }
}

/// A component on its own connection: it binds `cupertino_component_v1`, says it is ready, and runs until the server
/// closes the connection.
fn run_component(name: &str, socket: UnixStream) {
    let Ok(conn) = Connection::from_socket(socket) else {
        return;
    };
    let mut queue = conn.new_event_queue::<Component>();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    let mut component = Component::default();
    if queue.roundtrip(&mut component).is_err() {
        return;
    }
    match &component.object {
        Some(object) => object.ready(),
        None => eprintln!("stand-in host: {name} sees no cupertino_component_v1"),
    }
    while queue.blocking_dispatch(&mut component).is_ok() {}
}

#[derive(Default)]
struct Component {
    object: Option<CupertinoComponentV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Component {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "cupertino_component_v1" {
                state.object = Some(registry.bind::<CupertinoComponentV1, _, _>(name, 1, qh, ()));
            }
        }
    }
}

impl Dispatch<CupertinoComponentV1, ()> for Component {
    fn event(
        _: &mut Self,
        _: &CupertinoComponentV1,
        _: cupertino_component_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
