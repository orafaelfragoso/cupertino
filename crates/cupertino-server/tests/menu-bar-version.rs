//! The menu bar's version: a menu bar created through a shell global bound below the shell version of the menu bar's
//! present messages, whose events lie at other opcodes, is left inert, and one bound at that version is fed.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_protocols::client::cupertino_menu_bar_v1::{self, CupertinoMenuBarV1};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1};
use cupertino_server::authorization::{Holder, DETACHED_LIFETIME};
use cupertino_server::host::HostKind;
use cupertino_server::state::new_output;
use cupertino_server::{Config, Cupertino};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::protocol::{wl_compositor, wl_output, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// The shell version of the menu bar's present messages: 31 took `session_agent` out.
const MENU_BAR_MESSAGES: u32 = 31;

/// The shell host's side: its global and the component connections it was handed.
#[derive(Default)]
struct HostClient {
    host: Option<CupertinoHostV1>,
    connections: Vec<OwnedFd>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for HostClient {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
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
                state.host = Some(reg.bind::<CupertinoHostV1, _, _>(name, 1, qh, ()));
            }
        }
    }
}

impl Dispatch<CupertinoHostV1, ()> for HostClient {
    fn event(
        _: &mut Self,
        _: &CupertinoHostV1,
        _: cupertino_host_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoHostComponentV1, ()> for HostClient {
    fn event(
        state: &mut Self,
        _: &CupertinoHostComponentV1,
        event: cupertino_host_component_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_host_component_v1::Event::Connection { fd } = event {
            state.connections.push(fd);
        }
    }
}

/// The menu bar component's connection and what its bar was told.
#[derive(Default)]
struct Component {
    /// The highest `cupertino_shell_v1` version this component binds.
    shell_version: u32,
    compositor: Option<wl_compositor::WlCompositor>,
    output: Option<wl_output::WlOutput>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    shell: Option<CupertinoShellV1>,
    /// The bar's groups (its `done` events).
    groups: usize,
    /// Whether the bar was told the active application.
    active_app: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Component {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_compositor" => state.compositor = Some(reg.bind(name, version.min(6), qh, ())),
            "wl_output" => state.output = Some(reg.bind(name, version.min(4), qh, ())),
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(reg.bind(name, version.min(4), qh, ()))
            }
            "cupertino_shell_v1" => {
                state.shell = Some(reg.bind(name, version.min(state.shell_version), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Component {
    fn event(
        _: &mut Self,
        surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            surface.ack_configure(serial);
        }
    }
}

impl Dispatch<CupertinoMenuBarV1, ()> for Component {
    fn event(
        state: &mut Self,
        _: &CupertinoMenuBarV1,
        event: cupertino_menu_bar_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_menu_bar_v1::Event::Done => state.groups += 1,
            cupertino_menu_bar_v1::Event::ActiveApp { .. } => state.active_app = true,
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Component: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Component: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Component: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(Component: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
wayland_client::delegate_noop!(Component: ignore CupertinoShellV1);

/// One client connection of a test: its queue and its state.
struct Side<T> {
    conn: Connection,
    queue: EventQueue<T>,
    client: T,
}

impl<T: 'static> Side<T> {
    fn new(conn: Connection, client: T) -> Self
    where
        T: Dispatch<wl_registry::WlRegistry, ()>,
    {
        let queue = conn.new_event_queue();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        Self {
            conn,
            queue,
            client,
        }
    }

    /// Send what the client queued and read what the server answered.
    fn exchange(&mut self) {
        let _ = self.conn.flush();
        if let Some(guard) = self.conn.prepare_read() {
            let _ = guard.read();
        }
        let _ = self.queue.dispatch_pending(&mut self.client);
    }
}

/// The compositor with one output.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
}

impl Rig {
    fn new() -> Self {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let config = Config::default();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .unwrap();
        state.add_output(new_output(
            "HEADLESS-1",
            PhysicalProperties {
                size: (400, 300).into(),
                subpixel: Subpixel::None,
                make: "Cupertino".into(),
                model: "Test".into(),
            },
            OutputMode {
                size: (1280, 800).into(),
                refresh: 60_000,
            },
            1.0,
            Transform::Normal,
            (0, 0),
        ));
        Self { event_loop, state }
    }

    /// A few turns of the compositor's loop, each with the exchange of `side`.
    fn settle<T: 'static>(&mut self, side: &mut Side<T>) {
        for _ in 0..8 {
            side.exchange();
            let _ = self
                .event_loop
                .dispatch(Some(Duration::from_millis(2)), &mut self.state);
            self.state.tick();
            let _ = self.state.display_handle.flush_clients();
            side.exchange();
        }
    }

    /// The `menubar` component's connection from a shared host, binding `cupertino_shell_v1` up to `shell_version`.
    fn menu_bar_component(&mut self, shell_version: u32) -> (Side<HostClient>, Side<Component>) {
        let fd = self
            .state
            .attach_host_connection(
                HostKind::Shared,
                vec!["menubar"],
                Holder::Detached(Instant::now() + DETACHED_LIFETIME),
            )
            .unwrap();
        let mut host = Side::new(
            Connection::from_socket(UnixStream::from(fd)).unwrap(),
            HostClient::default(),
        );
        self.settle(&mut host);
        let qh = host.queue.handle();
        host.client
            .host
            .clone()
            .expect("the host global")
            .get_component("menubar".into(), &qh, ())
            .connect();
        self.settle(&mut host);
        let fd = host
            .client
            .connections
            .pop()
            .expect("the component's connection");
        let mut side = Side::new(
            Connection::from_socket(UnixStream::from(fd)).unwrap(),
            Component {
                shell_version,
                ..Component::default()
            },
        );
        self.settle(&mut side);
        (host, side)
    }

    /// Make the component's top-anchored layer surface the menu bar of the output.
    fn menu_bar(&mut self, side: &mut Side<Component>) -> CupertinoMenuBarV1 {
        let qh = side.queue.handle();
        let c = &side.client;
        let surface = c.compositor.as_ref().unwrap().create_surface(&qh, ());
        let layer = c.layer_shell.as_ref().unwrap().get_layer_surface(
            &surface,
            c.output.as_ref(),
            zwlr_layer_shell_v1::Layer::Top,
            "cupertino-menubar".into(),
            &qh,
            (),
        );
        layer.set_size(0, 24);
        layer.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        surface.commit();
        self.settle(side);
        let c = &side.client;
        let bar = c.shell.as_ref().expect("the shell global").get_menu_bar(
            &surface,
            c.output.as_ref().unwrap(),
            &qh,
            (),
        );
        self.settle(side);
        bar
    }
}

#[test]
fn a_menu_bar_bound_below_the_shell_version_of_its_messages_is_left_inert() {
    for (version, live) in [(MENU_BAR_MESSAGES - 1, false), (MENU_BAR_MESSAGES, true)] {
        let mut rig = Rig::new();
        let (_host, mut side) = rig.menu_bar_component(version);
        let _bar = rig.menu_bar(&mut side);
        assert!(
            side.conn.protocol_error().is_none(),
            "version {version}: the connection stays"
        );
        assert_eq!(
            side.client.groups > 0,
            live,
            "version {version}: the bar is sent its groups"
        );
        assert_eq!(
            side.client.active_app, live,
            "version {version}: the bar is told the active application"
        );
    }
}
