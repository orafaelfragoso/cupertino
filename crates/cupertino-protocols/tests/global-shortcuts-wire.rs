//! One protocol over a real wire: a server and a client in this process, joined by a socket pair, with no compositor.
//! The client binds the global, and requests and events cross in both directions through the generated code.

use std::os::unix::net::UnixStream;
use std::sync::Arc;

use cupertino_protocols::client::global_shortcuts::cupertino_global_shortcuts_v1 as c;
use cupertino_protocols::server::global_shortcuts::cupertino_global_shortcuts_v1 as s;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::{Display, DisplayHandle, GlobalDispatch, New, Resource};

#[derive(Default)]
struct Server {
    binds: Vec<(u32, u32, u32)>,
    unbinds: Vec<u32>,
    objects: Vec<s::CupertinoGlobalShortcutsV1>,
    destroyed: u32,
}

struct Data;

impl ClientData for Data {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

impl GlobalDispatch<s::CupertinoGlobalShortcutsV1, ()> for Server {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &wayland_server::Client,
        resource: New<s::CupertinoGlobalShortcutsV1>,
        _: &(),
        init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        let object = init.init(resource, ());
        object.reserved(s::Modifier::Ctrl | s::Modifier::Alt, 0x74);
        object.done();
        state.objects.push(object);
    }
}

impl wayland_server::Dispatch<s::CupertinoGlobalShortcutsV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &wayland_server::Client,
        _: &s::CupertinoGlobalShortcutsV1,
        request: s::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut wayland_server::DataInit<'_, Self>,
    ) {
        match request {
            s::Request::Bind {
                id,
                modifiers,
                keysym,
            } => state.binds.push((id, modifiers.into(), keysym)),
            s::Request::Unbind { id } => state.unbinds.push(id),
            s::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, _: &s::CupertinoGlobalShortcutsV1, _: &()) {
        state.destroyed += 1;
    }
}

#[derive(Default)]
struct Client {
    shortcuts: Option<c::CupertinoGlobalShortcutsV1>,
    reserved: Vec<(u32, u32)>,
    done: u32,
    pressed: Vec<(u32, u32)>,
    released: Vec<(u32, u32)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == c::CupertinoGlobalShortcutsV1::interface().name {
                state.shortcuts = Some(registry.bind(name, version, qh, ()));
            }
        }
    }
}

impl Dispatch<c::CupertinoGlobalShortcutsV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &c::CupertinoGlobalShortcutsV1,
        event: c::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            c::Event::Reserved { modifiers, keysym } => {
                state.reserved.push((modifiers.into(), keysym))
            }
            c::Event::Done => state.done += 1,
            c::Event::Pressed { id, time } => state.pressed.push((id, time)),
            c::Event::Released { id, time } => state.released.push((id, time)),
            _ => unreachable!(),
        }
    }
}

struct Wire {
    display: Display<Server>,
    server: Server,
    connection: Connection,
    queue: EventQueue<Client>,
    client: Client,
}

impl Wire {
    fn new() -> Wire {
        let (client_end, server_end) = UnixStream::pair().expect("socket pair");
        client_end.set_nonblocking(true).expect("nonblocking");
        let display = Display::<Server>::new().expect("display");
        let mut handle = display.handle();
        handle.create_global::<Server, s::CupertinoGlobalShortcutsV1, ()>(1, ());
        handle
            .insert_client(server_end, Arc::new(Data))
            .expect("client");
        let connection = Connection::from_socket(client_end).expect("connection");
        let queue = connection.new_event_queue::<Client>();
        let _registry = connection.display().get_registry(&queue.handle(), ());
        let mut wire = Wire {
            display,
            server: Server::default(),
            connection,
            queue,
            client: Client::default(),
        };
        wire.pump();
        wire
    }

    /// Carry everything written so far to the other side, both ways, until nothing moves.
    fn pump(&mut self) {
        for _ in 0..4 {
            self.connection.flush().expect("client flush");
            self.display
                .dispatch_clients(&mut self.server)
                .expect("server dispatch");
            self.display.flush_clients().expect("server flush");
            if let Some(guard) = self.connection.prepare_read() {
                // the socket is not blocking: no data yet is not an error
                let _ = guard.read();
            }
            self.queue
                .dispatch_pending(&mut self.client)
                .expect("client dispatch");
        }
    }
}

#[test]
fn the_global_is_advertised_and_bound_at_version_one() {
    let wire = Wire::new();
    let object = wire.client.shortcuts.as_ref().expect("bound");
    assert_eq!(object.version(), 1);
    assert_eq!(wire.server.objects.len(), 1);
    assert_eq!(wire.server.objects[0].version(), 1);
}

#[test]
fn events_sent_on_bind_arrive_in_order() {
    let wire = Wire::new();
    assert_eq!(wire.client.reserved, [(2 | 4, 0x74)]);
    assert_eq!(wire.client.done, 1);
}

#[test]
fn requests_arrive_with_their_arguments_and_bitfields_intact() {
    let mut wire = Wire::new();
    let object = wire.client.shortcuts.clone().expect("bound");
    object.bind(7, c::Modifier::Shift | c::Modifier::Logo, 0x61);
    object.bind(8, c::Modifier::empty(), u32::MAX);
    object.unbind(7);
    wire.pump();
    assert_eq!(wire.server.binds, [(7, 1 | 8, 0x61), (8, 0, u32::MAX)]);
    assert_eq!(wire.server.unbinds, [7]);
}

#[test]
fn pressed_and_released_carry_the_id_and_time() {
    let mut wire = Wire::new();
    let object = wire.server.objects[0].clone();
    object.pressed(3, 1000);
    object.released(3, 1042);
    wire.pump();
    assert_eq!(wire.client.pressed, [(3, 1000)]);
    assert_eq!(wire.client.released, [(3, 1042)]);
}

#[test]
fn the_destructor_ends_the_object_on_both_sides() {
    let mut wire = Wire::new();
    let object = wire.client.shortcuts.take().expect("bound");
    object.destroy();
    wire.pump();
    assert_eq!(wire.server.destroyed, 1);
    assert!(!object.is_alive());
    assert!(!wire.server.objects[0].is_alive());
}
