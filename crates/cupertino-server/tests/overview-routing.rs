//! How the person's ways to the overview reach the overview component (W6.8): the overview shortcuts, the named
//! actions and the dock's tile command arrive at the output's `cupertino_overview_v1` object as `toggle`s, the dock is
//! told nothing of them, and a dock bound below the shell version of the dock's present messages is left inert.

use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_protocols::client::cupertino_dock_v1::{self, AppCommand, CupertinoDockV1};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1};
use cupertino_protocols::client::overview::cupertino_overview_manager_v1::CupertinoOverviewManagerV1;
use cupertino_protocols::client::overview::cupertino_overview_v1::{
    self, CupertinoOverviewV1, Mode,
};
use cupertino_server::authorization::{Holder, DETACHED_LIFETIME};
use cupertino_server::host::HostKind;
use cupertino_server::state::{insert_client, new_output, ClientState};
use cupertino_server::{Config, Cupertino};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// The shell version of the dock's present messages: 28 took the overview and the pictures of minimized windows out.
const DOCK_MESSAGES: u32 = 28;
/// The application id of the test's window.
const APP_ID: &str = "org.cupertino.test.overview";

/// The shell host's side: its global and the component connections it was handed.
#[derive(Default)]
struct HostClient {
    host: Option<CupertinoHostV1>,
    connections: Vec<(String, OwnedFd)>,
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

impl Dispatch<CupertinoHostComponentV1, String> for HostClient {
    fn event(
        state: &mut Self,
        _: &CupertinoHostComponentV1,
        event: cupertino_host_component_v1::Event,
        name: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_host_component_v1::Event::Connection { fd } = event {
            state.connections.push((name.clone(), fd));
        }
    }
}

/// A component's connection: the dock or the overview, with what each was told.
#[derive(Default)]
struct Component {
    /// The highest `cupertino_shell_v1` version this component binds.
    shell_version: u32,
    compositor: Option<wl_compositor::WlCompositor>,
    output: Option<wl_output::WlOutput>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    shell: Option<CupertinoShellV1>,
    overviews: Option<CupertinoOverviewManagerV1>,
    /// The names of the dock's events other than `done`, in the order they came.
    dock_events: Vec<&'static str>,
    /// The dock's groups (its `done` events).
    dock_groups: usize,
    /// The overview's `toggle` events.
    toggles: Vec<(Mode, String)>,
    /// The application id of each window the overview was told of.
    overview_apps: Vec<String>,
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
            "cupertino_overview_manager_v1" => {
                state.overviews = Some(reg.bind(name, version.min(1), qh, ()));
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

impl Dispatch<CupertinoDockV1, ()> for Component {
    fn event(
        state: &mut Self,
        _: &CupertinoDockV1,
        event: cupertino_dock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use cupertino_dock_v1::Event as E;
        let name = match event {
            E::Done => {
                state.dock_groups += 1;
                return;
            }
            E::Window { .. } => "window",
            E::WindowClosed { .. } => "window_closed",
            E::WindowIcon { .. } => "window_icon",
            E::AppResident { .. } => "app_resident",
            E::Launching { .. } => "launching",
            E::HotCorner { .. } => "hot_corner",
            E::Gesture { .. } => "gesture",
            _ => "another",
        };
        state.dock_events.push(name);
    }
}

impl Dispatch<CupertinoOverviewV1, ()> for Component {
    fn event(
        state: &mut Self,
        _: &CupertinoOverviewV1,
        event: cupertino_overview_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_overview_v1::Event::Toggle {
                mode: WEnum::Value(mode),
                app_id,
            } => state.toggles.push((mode, app_id)),
            cupertino_overview_v1::Event::Window { app_id, .. } => state.overview_apps.push(app_id),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Component: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Component: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Component: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(Component: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
wayland_client::delegate_noop!(Component: ignore CupertinoShellV1);
wayland_client::delegate_noop!(Component: ignore CupertinoOverviewManagerV1);

/// An ordinary client with one window.
#[derive(Default)]
struct WindowClient {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    configured: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for WindowClient {
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
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(reg.bind(name, 4, qh, ())),
                "wl_shm" => state.shm = Some(reg.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(reg.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WindowClient {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for WindowClient {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

wayland_client::delegate_noop!(WindowClient: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(WindowClient: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(WindowClient: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(WindowClient: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(WindowClient: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(WindowClient: ignore xdg_toplevel::XdgToplevel);

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
        let mut config = Config::default();
        config.shell_processes.clear();
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

    /// One turn of the compositor's loop.
    fn turn(&mut self) {
        let _ = self
            .event_loop
            .dispatch(Some(Duration::from_millis(2)), &mut self.state);
        self.state.tick();
        let _ = self.state.display_handle.flush_clients();
    }

    /// A few turns, each with the exchange of every side in `sides`.
    fn settle(&mut self, sides: &mut [&mut dyn Exchange]) {
        for _ in 0..8 {
            for side in sides.iter_mut() {
                side.exchange();
            }
            self.turn();
            for side in sides.iter_mut() {
                side.exchange();
            }
        }
    }

    /// A shared host running `components`, and the connection of each, binding `cupertino_shell_v1` up to
    /// `shell_version`.
    fn components(
        &mut self,
        components: &[&'static str],
        shell_version: u32,
    ) -> (Side<HostClient>, Vec<Side<Component>>) {
        let fd = self
            .state
            .attach_host_connection(
                HostKind::Shared,
                components.to_vec(),
                Holder::Detached(Instant::now() + DETACHED_LIFETIME),
            )
            .unwrap();
        let mut host = Side::new(
            Connection::from_socket(UnixStream::from(fd)).unwrap(),
            HostClient::default(),
        );
        self.settle(&mut [&mut host]);
        let qh = host.queue.handle();
        let global = host.client.host.clone().expect("the host global");
        for name in components {
            global
                .get_component((*name).into(), &qh, (*name).to_string())
                .connect();
        }
        self.settle(&mut [&mut host]);
        let mut sides = Vec::new();
        for name in components {
            let at = host
                .client
                .connections
                .iter()
                .position(|(n, _)| n == name)
                .expect("a connection for each component");
            let (_, fd) = host.client.connections.remove(at);
            let mut side = Side::new(
                Connection::from_socket(UnixStream::from(fd)).unwrap(),
                Component {
                    shell_version,
                    ..Component::default()
                },
            );
            self.settle(&mut [&mut side]);
            sides.push(side);
        }
        (host, sides)
    }

    /// Make the dock component's layer surface the dock of the output.
    fn dock(&mut self, side: &mut Side<Component>) -> CupertinoDockV1 {
        let qh = side.queue.handle();
        let c = &side.client;
        let surface = c.compositor.as_ref().unwrap().create_surface(&qh, ());
        let layer = c.layer_shell.as_ref().unwrap().get_layer_surface(
            &surface,
            c.output.as_ref(),
            zwlr_layer_shell_v1::Layer::Top,
            "cupertino-dock".into(),
            &qh,
            (),
        );
        layer.set_size(400, 60);
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::Bottom);
        surface.commit();
        self.settle(&mut [side]);
        let c = &side.client;
        let dock = c.shell.as_ref().expect("the shell global").get_dock(
            &surface,
            c.output.as_ref().unwrap(),
            &qh,
            (),
        );
        self.settle(&mut [side]);
        dock
    }

    /// The overview component's object for the output.
    fn overview(&mut self, side: &mut Side<Component>) -> CupertinoOverviewV1 {
        let qh = side.queue.handle();
        let c = &side.client;
        let overview = c
            .overviews
            .as_ref()
            .expect("the overview manager")
            .get_overview(c.output.as_ref().unwrap(), &qh, ());
        self.settle(&mut [side]);
        overview
    }

    /// An ordinary client's window of application `APP_ID`, mapped and given the keyboard.
    fn window(&mut self) -> (Side<WindowClient>, xdg_toplevel::XdgToplevel) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        insert_client(&self.state.display_handle, ours, ClientState::default()).unwrap();
        let mut side = Side::new(
            Connection::from_socket(theirs).unwrap(),
            WindowClient::default(),
        );
        self.settle(&mut [&mut side]);
        let qh = side.queue.handle();
        let surface = side
            .client
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let xdg = side
            .client
            .wm_base
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title("overview routing".into());
        toplevel.set_app_id(APP_ID.into());
        surface.commit();
        self.settle(&mut [&mut side]);
        assert!(side.client.configured, "the window is configured");
        let size = (200, 150);
        let buffer = shm_buffer(side.client.shm.as_ref().unwrap(), &qh, size);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        self.settle(&mut [&mut side]);
        (side, toplevel)
    }
}

/// A side the rig pumps along with the compositor.
trait Exchange {
    fn exchange(&mut self);
}

impl<T: 'static> Exchange for Side<T> {
    fn exchange(&mut self) {
        Side::exchange(self);
    }
}

/// A `width` × `height` buffer of opaque grey.
fn shm_buffer<D>(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<D>,
    (width, height): (i32, i32),
) -> wl_buffer::WlBuffer
where
    D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, ()> + 'static,
{
    let len = width * height * 4;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![0x80; len as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), len, qh, ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();
    buffer
}

#[test]
fn the_overview_shortcuts_actions_and_tile_command_toggle_the_overview_object_and_tell_the_dock_nothing(
) {
    let mut rig = Rig::new();
    let (mut host, mut sides) = rig.components(&["dock", "overview"], DOCK_MESSAGES);
    let mut overview_side = sides.pop().unwrap();
    let mut dock_side = sides.pop().unwrap();
    let dock = rig.dock(&mut dock_side);
    let _overview = rig.overview(&mut overview_side);
    assert!(
        dock_side.client.dock_groups >= 1,
        "the dock got its first group"
    );

    // the overview shortcut, with no application in front the application overview shortcut too, is the whole overview
    rig.state.scripted_key("Ctrl+Up").unwrap();
    rig.state.scripted_key("Ctrl+Down").unwrap();
    rig.state.dock_run_action("overview");
    rig.settle(&mut [&mut host, &mut dock_side, &mut overview_side]);
    let all = (Mode::All, String::new());
    assert_eq!(
        overview_side.client.toggles,
        vec![all.clone(), all.clone(), all]
    );

    let (mut window, _toplevel) = rig.window();
    rig.settle(&mut [&mut host, &mut dock_side, &mut overview_side, &mut window]);
    let app_id = overview_side
        .client
        .overview_apps
        .last()
        .cloned()
        .expect("the overview was told of the window");
    assert!(!app_id.is_empty());
    assert!(
        rig.state.wm.front_app().is_some(),
        "the window's application is in front"
    );
    overview_side.client.toggles.clear();
    dock_side.client.dock_events.clear();
    let groups = dock_side.client.dock_groups;

    // the application overview shortcut, the dock's named action and its tile command name the front application
    rig.state.scripted_key("Ctrl+Down").unwrap();
    dock.run_action("app-overview".into());
    dock.app_command(APP_ID.into(), AppCommand::ShowWindows);
    rig.settle(&mut [&mut host, &mut dock_side, &mut overview_side, &mut window]);
    let app = (Mode::App, app_id);
    assert_eq!(
        overview_side.client.toggles,
        vec![app.clone(), app.clone(), app]
    );
    // a toggle is the component's to read: the shortcut again toggles again, and the server keeps no overview open
    rig.state.scripted_key("Ctrl+Up").unwrap();
    rig.settle(&mut [&mut host, &mut dock_side, &mut overview_side, &mut window]);
    assert_eq!(overview_side.client.toggles.len(), 4);
    assert_eq!(dock_side.client.dock_events, Vec::<&str>::new());
    assert_eq!(
        dock_side.client.dock_groups, groups,
        "the dock heard nothing of the overview"
    );
}

#[test]
fn a_dock_bound_below_the_shell_version_of_its_messages_is_left_inert() {
    for (version, live) in [(DOCK_MESSAGES - 1, false), (DOCK_MESSAGES, true)] {
        let mut rig = Rig::new();
        let (mut host, mut sides) = rig.components(&["dock"], version);
        let mut dock_side = sides.pop().unwrap();
        let _dock = rig.dock(&mut dock_side);
        let (mut window, _toplevel) = rig.window();
        rig.settle(&mut [&mut host, &mut dock_side, &mut window]);
        assert!(
            dock_side.conn.protocol_error().is_none(),
            "version {version}: the connection stays"
        );
        assert_eq!(
            dock_side.client.dock_groups > 0,
            live,
            "version {version}: the dock is told the windows"
        );
        assert_eq!(
            dock_side.client.dock_events.contains(&"window"),
            live,
            "version {version}: the window"
        );
    }
}
