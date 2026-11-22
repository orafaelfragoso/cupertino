//! The application a window's launch named reaches the dock: a token made with `set_app_id` announces the launch of an
//! application the desktop shows (and of nothing else), and the window of the process started with it is named to a
//! dock of shell version 32 by `window_launch`, before its `window` event, whatever application id the window gives
//! itself.

use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_protocols::client::cupertino_dock_v1::{self, CupertinoDockV1};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1};
use cupertino_server::authorization::{Holder, DETACHED_LIFETIME};
use cupertino_server::desktop::{DesktopEntry, DesktopIndex};
use cupertino_server::host::HostKind;
use cupertino_server::state::{insert_client, new_output, ClientState};
use cupertino_server::{Config, Cupertino};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// The shell version of `window_launch`.
const WINDOW_LAUNCH: u32 = 32;
/// The desktop entry the launch names.
const EDITOR: &str = "org.example.Editor";
/// The application id the editor's windows give themselves, which names none of its entry.
const EDITOR_WINDOWS: &str = "electron-editor";
/// A helper's hidden entry.
const PORTAL: &str = "org.example.Portal";

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

/// The dock component's connection, with the dock's events of a window or an application.
#[derive(Default)]
struct DockClient {
    shell_version: u32,
    compositor: Option<wl_compositor::WlCompositor>,
    output: Option<wl_output::WlOutput>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    shell: Option<CupertinoShellV1>,
    /// `launching`, `window_launch` and `window` events in the order they came: the event's name and its application id.
    heard: Vec<(&'static str, String)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for DockClient {
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

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for DockClient {
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

impl Dispatch<CupertinoDockV1, ()> for DockClient {
    fn event(
        state: &mut Self,
        _: &CupertinoDockV1,
        event: cupertino_dock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use cupertino_dock_v1::Event as E;
        match event {
            E::Launching { app_id } => state.heard.push(("launching", app_id)),
            E::WindowLaunch { app_id, .. } => state.heard.push(("window_launch", app_id)),
            E::Window { app_id, .. } => state.heard.push(("window", app_id)),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(DockClient: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(DockClient: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(DockClient: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(DockClient: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
wayland_client::delegate_noop!(DockClient: ignore CupertinoShellV1);

/// An application's connection: one window, and the activation tokens it was given.
#[derive(Default)]
struct AppClient {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    activation: Option<xdg_activation_v1::XdgActivationV1>,
    configured: bool,
    tokens: Vec<String>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for AppClient {
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
                "xdg_activation_v1" => state.activation = Some(reg.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for AppClient {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for AppClient {
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

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for AppClient {
    fn event(
        state: &mut Self,
        _: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.tokens.push(token);
        }
    }
}

wayland_client::delegate_noop!(AppClient: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(AppClient: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(AppClient: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(AppClient: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(AppClient: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(AppClient: ignore xdg_toplevel::XdgToplevel);
wayland_client::delegate_noop!(AppClient: ignore xdg_activation_v1::XdgActivationV1);

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
}

/// A side the rig pumps along with the compositor.
trait Exchange {
    fn exchange(&mut self);
}

impl<T: 'static> Exchange for Side<T> {
    /// Send what the client queued and read what the server answered.
    fn exchange(&mut self) {
        let _ = self.conn.flush();
        if let Some(guard) = self.conn.prepare_read() {
            let _ = guard.read();
        }
        let _ = self.queue.dispatch_pending(&mut self.client);
    }
}

/// The compositor with one output and the desktop entries of the editor and of a helper.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
}

fn entry(id: &str, exec: &str, no_display: bool) -> DesktopEntry {
    DesktopEntry {
        id: id.to_owned(),
        name: id.to_owned(),
        exec: exec.to_owned(),
        no_display,
        ..DesktopEntry::default()
    }
}

impl Rig {
    fn new() -> Self {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
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
        state.desktop = DesktopIndex {
            entries: vec![
                entry(EDITOR, "/opt/editor/start-editor %F", false),
                entry(PORTAL, "/usr/lib/example-portal", true),
            ],
        };
        Self { event_loop, state }
    }

    fn settle(&mut self, sides: &mut [&mut dyn Exchange]) {
        for _ in 0..8 {
            for side in sides.iter_mut() {
                side.exchange();
            }
            let _ = self
                .event_loop
                .dispatch(Some(Duration::from_millis(2)), &mut self.state);
            self.state.tick();
            let _ = self.state.display_handle.flush_clients();
            for side in sides.iter_mut() {
                side.exchange();
            }
        }
    }

    /// The dock component's connection, binding `cupertino_shell_v1` up to `shell_version`, with its layer surface made
    /// the dock of the output; and the shell host's, which must stay.
    fn dock(
        &mut self,
        shell_version: u32,
    ) -> (Side<HostClient>, Side<DockClient>, CupertinoDockV1) {
        let fd = self
            .state
            .attach_host_connection(
                HostKind::Shared,
                vec!["dock"],
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
        global
            .get_component("dock".into(), &qh, "dock".to_owned())
            .connect();
        self.settle(&mut [&mut host]);
        let (_, fd) = host
            .client
            .connections
            .pop()
            .expect("the dock's connection");
        let mut side = Side::new(
            Connection::from_socket(UnixStream::from(fd)).unwrap(),
            DockClient {
                shell_version,
                ..DockClient::default()
            },
        );
        self.settle(&mut [&mut side]);
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
        self.settle(&mut [&mut side]);
        let c = &side.client;
        let dock = c.shell.as_ref().expect("the shell global").get_dock(
            &surface,
            c.output.as_ref().unwrap(),
            &qh,
            (),
        );
        self.settle(&mut [&mut side]);
        (host, side, dock)
    }

    /// An application's connection.
    fn app(&mut self) -> Side<AppClient> {
        let (ours, theirs) = UnixStream::pair().unwrap();
        insert_client(&self.state.display_handle, ours, ClientState::default()).unwrap();
        let mut side = Side::new(
            Connection::from_socket(theirs).unwrap(),
            AppClient::default(),
        );
        self.settle(&mut [&mut side]);
        side
    }
}

/// An activation token made by `app` with the application id `app_id`.
fn token(app: &mut Side<AppClient>, app_id: &str) -> xdg_activation_token_v1::XdgActivationTokenV1 {
    let qh = app.queue.handle();
    let token = app
        .client
        .activation
        .as_ref()
        .expect("the activation global")
        .get_activation_token(&qh, ());
    token.set_app_id(app_id.into());
    token.commit();
    token
}

/// Map a window of application id `app_id` on `app`'s connection: its toplevel and its surface.
fn map_window(
    rig: &mut Rig,
    app: &mut Side<AppClient>,
    app_id: &str,
) -> (xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
    let qh = app.queue.handle();
    let surface = app
        .client
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&qh, ());
    let xdg = app
        .client
        .wm_base
        .as_ref()
        .unwrap()
        .get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_app_id(app_id.into());
    surface.commit();
    rig.settle(&mut [app]);
    assert!(app.client.configured, "the window is configured");
    let (width, height) = (200, 150);
    let len = width * height * 4;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![0x80; len as usize]).unwrap();
    let shm = app.client.shm.as_ref().unwrap();
    let pool = shm.create_pool(file.as_fd(), len, &qh, ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wl_shm::Format::Argb8888,
        &qh,
        (),
    );
    pool.destroy();
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
    (toplevel, surface)
}

#[test]
fn a_token_announces_the_launch_of_an_application_the_desktop_shows_only() {
    let mut rig = Rig::new();
    let (mut host, mut dock_side, _dock) = rig.dock(WINDOW_LAUNCH);
    let mut app = rig.app();
    let _editor = token(&mut app, EDITOR);
    let _portal = token(&mut app, PORTAL);
    let _unknown = token(&mut app, "some-script");
    rig.settle(&mut [&mut host, &mut dock_side, &mut app]);
    assert_eq!(app.client.tokens.len(), 3, "every token is made");
    assert_eq!(
        dock_side.client.heard,
        [("launching", EDITOR.to_owned())],
        "only the editor's launch is announced"
    );
}

#[test]
fn the_window_of_a_process_started_with_a_token_is_named_by_its_launch_to_a_new_dock() {
    for (version, named) in [(WINDOW_LAUNCH - 1, false), (WINDOW_LAUNCH, true)] {
        let mut rig = Rig::new();
        let (mut host, mut dock_side, _dock) = rig.dock(version);
        let mut app = rig.app();
        let _token = token(&mut app, EDITOR);
        rig.settle(&mut [&mut host, &mut dock_side, &mut app]);
        let made = app.client.tokens.pop().expect("the token");
        // the process was started with the token in its environment and never presents it
        rig.state.launch_token_of = Box::new(move |_, _| Some(made.clone()));
        dock_side.client.heard.clear();
        let _window = map_window(&mut rig, &mut app, EDITOR_WINDOWS);
        rig.settle(&mut [&mut host, &mut dock_side, &mut app]);
        let window = ("window", EDITOR_WINDOWS.to_owned());
        let expected = if named {
            vec![("window_launch", EDITOR.to_owned()), window]
        } else {
            vec![window]
        };
        assert_eq!(dock_side.client.heard, expected, "shell version {version}");
        assert!(dock_side.conn.protocol_error().is_none());
    }
}

#[test]
fn a_window_first_activated_with_a_token_is_named_by_it_once() {
    let mut rig = Rig::new();
    let (mut host, mut dock_side, _dock) = rig.dock(WINDOW_LAUNCH);
    rig.state.launch_token_of = Box::new(|_, _| None);
    let mut app = rig.app();
    let (_toplevel, surface) = map_window(&mut rig, &mut app, EDITOR_WINDOWS);
    rig.settle(&mut [&mut host, &mut dock_side, &mut app]);
    let named = |dock: &Side<DockClient>| -> Vec<String> {
        dock.client
            .heard
            .iter()
            .filter(|(event, _)| *event == "window_launch")
            .map(|(_, app_id)| app_id.clone())
            .collect()
    };
    assert!(
        named(&dock_side).is_empty(),
        "the window mapped with no launch"
    );
    for app_id in [EDITOR, "org.example.Other"] {
        let _token = token(&mut app, app_id);
        rig.settle(&mut [&mut host, &mut dock_side, &mut app]);
        let made = app.client.tokens.pop().expect("the token");
        let activation = app
            .client
            .activation
            .clone()
            .expect("the activation global");
        activation.activate(made, &surface);
        rig.settle(&mut [&mut host, &mut dock_side, &mut app]);
    }
    assert_eq!(
        named(&dock_side),
        [EDITOR],
        "the first activation names the launch, once"
    );
}
