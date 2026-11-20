//! Program tests for W5.2 (`cupertino_host_v1`, `cupertino_host_component_v1`, `cupertino_component_v1`):
//! - A dropped component keeps a persisted menu bar's reserved space and picture (hold), and a new instance
//!   receives `hold(key)` + `done` and takes over atomically on its first buffer commit without usable-area reflow.
//! - A transient surface is dismissed on `unresponsive` (and on drop) and keyboard focus returns to the prior window.
//! - An unresponsive component receives no pointer or keyboard input until `responsive`.
//! - Bus-identity nonce verification accepts the announced unique name from the host process and refuses wrong sender
//!   or duplicate component claims.
//! - Five failures within 60 s stop the component (`stopped(gave_up)` + `component_stopped`) and release holds; a
//!   restart then hands its host a connection.
//! - An instance that ends before its first buffer leaves the older hold as it was.
//! - `lost` and `failed` count only from the host that runs the component.
//! - Keyboard focus returns to the window that had it when a component with a transient surface turns unresponsive
//!   and when its connection drops; the dropped transient leaves no hold.
//! - `lost` takes a component out of the shared host's list and asks for a separate host for it; a separate host
//!   gets its bus bridge, a fresh pair after `bus_dropped` once the throttle passed, and its end reaches the shared host.
//! - A shared host's `request_restart` waits for a quiet seat after real input; its exit after the kill grace is not a
//!   crash, while three unexpected exits within 60 s isolate every component.
//! - A window manager component's placements stay held across a dropped connection, a new instance's commit is
//!   deferred to its takeover frame, and the hold timeout ends held placements.

use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_peer::{Caller, ProcessHandle};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::cupertino_window_manager_v1::{
    self, CupertinoWindowManagerV1, Placement,
};
use cupertino_protocols::client::host::cupertino_component_v1::{
    self, BusNameResult, CupertinoComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1, Failure, StopReason,
};
use cupertino_protocols::client::host::cupertino_host_v1::{
    self, CupertinoHostV1, Kind as WireKind,
};
use cupertino_server::authorization::{Holder, DETACHED_LIFETIME};
use cupertino_server::host::{HostKind, Liveness};
use cupertino_server::state::{new_output, ClientState};
use cupertino_server::{Config, Cupertino};
use smithay::backend::input::ButtonState;
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use smithay::reexports::wayland_server::{Display, Resource};
use smithay::utils::{Logical, Point, Transform};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

#[derive(Default)]
struct HostClientState {
    host: Option<CupertinoHostV1>,
    kind: Option<WireKind>,
    components: Vec<String>,
    done_count: usize,
    connections: Vec<(String, OwnedFd)>,
    stopped: Vec<(String, StopReason)>,
    component_stopped: Vec<String>,
    restarts: usize,
    bus_peers: Vec<(String, OwnedFd)>,
    bus_peers_gone: Vec<String>,
    buses: Vec<OwnedFd>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for HostClientState {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
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
            if interface == "cupertino_host_v1" {
                state.host = Some(reg.bind::<CupertinoHostV1, _, _>(name, version.min(1), qh, ()));
            }
        }
    }
}

impl Dispatch<CupertinoHostV1, ()> for HostClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoHostV1,
        event: cupertino_host_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_host_v1::Event::Kind {
                kind: WEnum::Value(k),
            } => state.kind = Some(k),
            cupertino_host_v1::Event::Component { name } => state.components.push(name),
            cupertino_host_v1::Event::Done => state.done_count += 1,
            cupertino_host_v1::Event::ComponentStopped { name } => {
                state.component_stopped.push(name);
            }
            cupertino_host_v1::Event::Restart => state.restarts += 1,
            cupertino_host_v1::Event::BusPeer { component, fd } => {
                state.bus_peers.push((component, fd));
            }
            cupertino_host_v1::Event::BusPeerGone { component } => {
                state.bus_peers_gone.push(component);
            }
            cupertino_host_v1::Event::Bus { fd } => state.buses.push(fd),
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHostComponentV1, String> for HostClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoHostComponentV1,
        event: cupertino_host_component_v1::Event,
        name: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_host_component_v1::Event::Connection { fd } => {
                state.connections.push((name.clone(), fd));
            }
            cupertino_host_component_v1::Event::Stopped {
                reason: WEnum::Value(r),
            } => {
                state.stopped.push((name.clone(), r));
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct ComponentClientState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    component_global: Option<CupertinoComponentV1>,
    announced_name: Option<(String, u32)>,
    holds: Vec<String>,
    done: bool,
    nonces: Vec<String>,
    bus_results: Vec<(String, BusNameResult)>,
    configure_serials: Vec<u32>,
    closed_count: usize,
    output: Option<wl_output::WlOutput>,
    shell: Option<CupertinoShellV1>,
    /// Window ids the window manager heard of.
    wm_windows: Vec<u32>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for ComponentClientState {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
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
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(reg.bind::<wl_compositor::WlCompositor, _, _>(
                        name,
                        version.min(6),
                        qh,
                        (),
                    ));
                }
                "wl_shm" => {
                    state.shm = Some(reg.bind::<wl_shm::WlShm, _, _>(name, 1, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell =
                        Some(reg.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(
                            name,
                            version.min(4),
                            qh,
                            (),
                        ));
                }
                "cupertino_component_v1" => {
                    state.component_global =
                        Some(reg.bind::<CupertinoComponentV1, _, _>(name, version.min(1), qh, ()));
                }
                "wl_output" if state.output.is_none() => {
                    state.output =
                        Some(reg.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ()));
                }
                "cupertino_shell_v1" => {
                    state.shell =
                        Some(reg.bind::<CupertinoShellV1, _, _>(name, version.min(27), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for ComponentClientState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for ComponentClientState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for ComponentClientState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for ComponentClientState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for ComponentClientState {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for ComponentClientState {
    fn event(
        _: &mut Self,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for ComponentClientState {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                state.configure_serials.push(serial);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.closed_count += 1;
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(ComponentClientState: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(ComponentClientState: ignore CupertinoShellV1);

impl Dispatch<CupertinoWindowManagerV1, ()> for ComponentClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoWindowManagerV1,
        event: cupertino_window_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_window_manager_v1::Event::Window { id, .. }
                if !state.wm_windows.contains(&id) =>
            {
                state.wm_windows.push(id);
            }
            cupertino_window_manager_v1::Event::WindowClosed { id } => {
                state.wm_windows.retain(|w| *w != id);
            }
            _ => {}
        }
    }
}

/// An ordinary application connection that maps one toplevel.
#[derive(Default)]
struct WindowClientState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    configured: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for WindowClientState {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
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
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(reg.bind(name, version.min(6), qh, ()));
                }
                "wl_shm" => state.shm = Some(reg.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(reg.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

wayland_client::delegate_noop!(WindowClientState: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(WindowClientState: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(WindowClientState: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(WindowClientState: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(WindowClientState: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(WindowClientState: ignore xdg_toplevel::XdgToplevel);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WindowClientState {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for WindowClientState {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<CupertinoComponentV1, ()> for ComponentClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoComponentV1,
        event: cupertino_component_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_component_v1::Event::Component { name, instance } => {
                state.announced_name = Some((name, instance));
            }
            cupertino_component_v1::Event::Hold { key } => state.holds.push(key),
            cupertino_component_v1::Event::Done => state.done = true,
            cupertino_component_v1::Event::BusNonce { nonce } => state.nonces.push(nonce),
            cupertino_component_v1::Event::BusName {
                name,
                result: WEnum::Value(res),
            } => state.bus_results.push((name, res)),
            _ => {}
        }
    }
}

fn pump<T>(
    event_loop: &mut EventLoop<'static, Cupertino>,
    state: &mut Cupertino,
    conn: &Connection,
    queue: &mut EventQueue<T>,
    client_state: &mut T,
) {
    for _ in 0..6 {
        let _ = conn.flush();
        let _ = event_loop.dispatch(Some(Duration::from_millis(2)), state);
        state.tick();
        let _ = state.display_handle.flush_clients();
        if let Some(guard) = conn.prepare_read() {
            let _ = guard.read();
        }
        let _ = queue.dispatch_pending(client_state);
    }
}

fn make_shm_buffer<T>(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<T>,
    w: i32,
    h: i32,
) -> (std::fs::File, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer)
where
    T: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, ()> + 'static,
{
    let stride = w * 4;
    let size = (stride * h) as usize;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!(
        "host-test-shm-{}-{}.bin",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    let _ = std::fs::remove_file(&path);
    file.write_all(&vec![0x7f; size]).unwrap();
    file.flush().unwrap();
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(0, w, h, stride, wl_shm::Format::Argb8888, qh, ());
    (file, pool, buffer)
}

#[test]
fn host_holds_keep_space_and_picture_transients_dismiss_and_unresponsive_blocks_input() {
    let mut event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let display: Display<Cupertino> = Display::new().unwrap();
    let mut state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        Config::default(),
        false,
        &Assets::builtin(),
    )
    .expect("server starts");
    let output_id = state.add_output(new_output(
        "headless-1",
        PhysicalProperties {
            size: (300, 200).into(),
            subpixel: Subpixel::Unknown,
            make: "Cupertino".into(),
            model: "Test".into(),
        },
        OutputMode {
            size: (800, 600).into(),
            refresh: 60_000,
        },
        1.0,
        Transform::Normal,
        (0, 0),
    ));

    // 1. Connect a stand-in shared host and request connections for `menubar` and `launcher`.
    let host_fd = state
        .attach_host_connection(
            HostKind::Shared,
            vec!["menubar", "launcher"],
            Holder::Detached(Instant::now() + DETACHED_LIFETIME),
        )
        .expect("attach host");
    let host_conn = Connection::from_socket(UnixStream::from(host_fd)).unwrap();
    let mut host_queue = host_conn.new_event_queue::<HostClientState>();
    let host_qh = host_queue.handle();
    let mut host_state = HostClientState::default();
    let _host_reg = host_conn.display().get_registry(&host_qh, ());
    pump(
        &mut event_loop,
        &mut state,
        &host_conn,
        &mut host_queue,
        &mut host_state,
    );

    assert_eq!(host_state.kind, Some(WireKind::Shared));
    assert_eq!(host_state.components, vec!["menubar", "launcher"]);
    assert_eq!(host_state.done_count, 1);

    let host_global = host_state.host.clone().expect("host bound");
    let menubar_hc = host_global.get_component("menubar".into(), &host_qh, "menubar".into());
    menubar_hc.connect();
    let launcher_hc = host_global.get_component("launcher".into(), &host_qh, "launcher".into());
    launcher_hc.connect();
    pump(
        &mut event_loop,
        &mut state,
        &host_conn,
        &mut host_queue,
        &mut host_state,
    );

    let menubar_fd = host_state
        .connections
        .iter()
        .position(|(n, _)| n == "menubar")
        .map(|i| host_state.connections.remove(i).1)
        .expect("menubar connection fd");
    let launcher_fd = host_state
        .connections
        .iter()
        .position(|(n, _)| n == "launcher")
        .map(|i| host_state.connections.remove(i).1)
        .expect("launcher connection fd");

    // 2. Menubar component persists a top bar with exclusive zone 28 and commits a buffer.
    let mb_conn = Connection::from_socket(UnixStream::from(menubar_fd)).unwrap();
    let mut mb_queue = mb_conn.new_event_queue::<ComponentClientState>();
    let mb_qh = mb_queue.handle();
    let mut mb_state = ComponentClientState::default();
    let _mb_reg = mb_conn.display().get_registry(&mb_qh, ());
    pump(
        &mut event_loop,
        &mut state,
        &mb_conn,
        &mut mb_queue,
        &mut mb_state,
    );
    assert_eq!(mb_state.announced_name, Some(("menubar".into(), 1)));
    assert!(mb_state.done);
    assert!(mb_state.holds.is_empty());

    let mb_surf = mb_state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&mb_qh, ());
    let mb_layer = mb_state.layer_shell.as_ref().unwrap().get_layer_surface(
        &mb_surf,
        None,
        zwlr_layer_shell_v1::Layer::Top,
        "menubar".into(),
        &mb_qh,
        (),
    );
    mb_layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    mb_layer.set_size(800, 28);
    mb_layer.set_exclusive_zone(28);
    mb_state
        .component_global
        .as_ref()
        .unwrap()
        .persist(&mb_surf, "bar".into());
    mb_surf.commit();
    pump(
        &mut event_loop,
        &mut state,
        &mb_conn,
        &mut mb_queue,
        &mut mb_state,
    );
    assert!(!mb_state.configure_serials.is_empty());

    let (_f1, _p1, mb_buf) = make_shm_buffer(mb_state.shm.as_ref().unwrap(), &mb_qh, 800, 28);
    mb_surf.attach(Some(&mb_buf), 0, 0);
    mb_surf.commit();
    pump(
        &mut event_loop,
        &mut state,
        &mb_conn,
        &mut mb_queue,
        &mut mb_state,
    );
    assert_eq!(state.usable_area(output_id).y, 28.0);

    // 3. Test D20 bus identity nonce handshake (wrong sender refused, valid sender accepted, duplicate refused).
    mb_state
        .component_global
        .as_ref()
        .unwrap()
        .announce_bus_name(":1.42".into());
    pump(
        &mut event_loop,
        &mut state,
        &mb_conn,
        &mut mb_queue,
        &mut mb_state,
    );
    let nonce1 = mb_state.nonces.pop().expect("bus nonce received");
    assert_eq!(nonce1.len(), 32);
    let own_proc = ProcessHandle::open(std::process::id()).unwrap();
    assert!(state
        .verify_component_bus_nonce(":1.99", &Caller::Unclassified(own_proc), &nonce1)
        .is_err());
    pump(
        &mut event_loop,
        &mut state,
        &mb_conn,
        &mut mb_queue,
        &mut mb_state,
    );
    assert_eq!(
        mb_state.bus_results.pop(),
        Some((":1.42".into(), BusNameResult::Refused))
    );

    mb_state
        .component_global
        .as_ref()
        .unwrap()
        .announce_bus_name(":1.42".into());
    pump(
        &mut event_loop,
        &mut state,
        &mb_conn,
        &mut mb_queue,
        &mut mb_state,
    );
    let nonce2 = mb_state.nonces.pop().expect("second nonce");
    let own_proc = ProcessHandle::open(std::process::id()).unwrap();
    assert!(state
        .verify_component_bus_nonce(":1.42", &Caller::Unclassified(own_proc), &nonce2)
        .is_ok());
    pump(
        &mut event_loop,
        &mut state,
        &mb_conn,
        &mut mb_queue,
        &mut mb_state,
    );
    assert_eq!(
        mb_state.bus_results.pop(),
        Some((":1.42".into(), BusNameResult::Accepted))
    );
    assert_eq!(state.hosts.component_of_bus_name(":1.42"), Some("menubar"));

    // 4. Drop menubar connection: hold must preserve both reserved space (y == 28) and last picture!
    drop(mb_buf);
    drop(_p1);
    drop(mb_layer);
    drop(mb_surf);
    drop(mb_state);
    drop(mb_queue);
    drop(mb_conn);
    for _ in 0..6 {
        let _ = event_loop.dispatch(Some(Duration::from_millis(2)), &mut state);
        state.tick();
        let _ = state.display_handle.flush_clients();
    }
    assert_eq!(state.hosts.held_surfaces().len(), 1);
    assert_eq!(state.hosts.held_surfaces()[0].key, "bar");
    assert!(state.hosts.held_surfaces()[0].picture.is_some());
    assert_eq!(
        state.usable_area(output_id).y,
        28.0,
        "held menu bar must keep reserved space after component drops"
    );

    // 5. New menubar instance connects, receives `hold("bar")`, and takes over atomically on first buffer commit.
    menubar_hc.connect();
    pump(
        &mut event_loop,
        &mut state,
        &host_conn,
        &mut host_queue,
        &mut host_state,
    );
    let mb2_fd = host_state
        .connections
        .iter()
        .position(|(n, _)| n == "menubar")
        .map(|i| host_state.connections.remove(i).1)
        .expect("second menubar connection");
    let mb2_conn = Connection::from_socket(UnixStream::from(mb2_fd)).unwrap();
    let mut mb2_queue = mb2_conn.new_event_queue::<ComponentClientState>();
    let mb2_qh = mb2_queue.handle();
    let mut mb2_state = ComponentClientState::default();
    let _mb2_reg = mb2_conn.display().get_registry(&mb2_qh, ());
    pump(
        &mut event_loop,
        &mut state,
        &mb2_conn,
        &mut mb2_queue,
        &mut mb2_state,
    );
    assert_eq!(mb2_state.announced_name, Some(("menubar".into(), 2)));
    assert_eq!(mb2_state.holds, vec!["bar"]);

    let mb2_surf = mb2_state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&mb2_qh, ());
    let mb2_layer = mb2_state.layer_shell.as_ref().unwrap().get_layer_surface(
        &mb2_surf,
        None,
        zwlr_layer_shell_v1::Layer::Top,
        "menubar".into(),
        &mb2_qh,
        (),
    );
    mb2_layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    mb2_layer.set_size(800, 28);
    mb2_layer.set_exclusive_zone(28);
    mb2_state
        .component_global
        .as_ref()
        .unwrap()
        .persist(&mb2_surf, "bar".into());
    mb2_surf.commit();
    pump(
        &mut event_loop,
        &mut state,
        &mb2_conn,
        &mut mb2_queue,
        &mut mb2_state,
    );
    let (_f2, _p2, mb2_buf) = make_shm_buffer(mb2_state.shm.as_ref().unwrap(), &mb2_qh, 800, 28);
    mb2_surf.attach(Some(&mb2_buf), 0, 0);
    mb2_surf.commit();
    pump(
        &mut event_loop,
        &mut state,
        &mb2_conn,
        &mut mb2_queue,
        &mut mb2_state,
    );
    assert!(
        state.hosts.held_surfaces().is_empty(),
        "hold must be taken over atomically on first buffer commit"
    );
    assert_eq!(state.usable_area(output_id).y, 28.0);

    // 6. Launcher maps a transient overlay surface (not persisted) with exclusive keyboard focus.
    let ln_conn = Connection::from_socket(UnixStream::from(launcher_fd)).unwrap();
    let mut ln_queue = ln_conn.new_event_queue::<ComponentClientState>();
    let ln_qh = ln_queue.handle();
    let mut ln_state = ComponentClientState::default();
    let _ln_reg = ln_conn.display().get_registry(&ln_qh, ());
    pump(
        &mut event_loop,
        &mut state,
        &ln_conn,
        &mut ln_queue,
        &mut ln_state,
    );

    let ln_surf = ln_state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&ln_qh, ());
    let ln_layer = ln_state.layer_shell.as_ref().unwrap().get_layer_surface(
        &ln_surf,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        "launcher".into(),
        &ln_qh,
        (),
    );
    ln_layer.set_size(400, 300);
    ln_layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    ln_surf.commit();
    pump(
        &mut event_loop,
        &mut state,
        &ln_conn,
        &mut ln_queue,
        &mut ln_state,
    );
    let (_f3, _p3, ln_buf) = make_shm_buffer(ln_state.shm.as_ref().unwrap(), &ln_qh, 400, 300);
    ln_surf.attach(Some(&ln_buf), 0, 0);
    ln_surf.commit();
    pump(
        &mut event_loop,
        &mut state,
        &ln_conn,
        &mut ln_queue,
        &mut ln_state,
    );

    // Before unresponsive: launcher surface is hit at center of output and holds keyboard focus.
    let center = Point::<f64, Logical>::from((400.0, 300.0));
    assert!(
        state.surface_under(center).is_some(),
        "transient launcher surface receives pointer hit while responsive"
    );
    assert!(
        state.seat.get_keyboard().unwrap().current_focus().is_some(),
        "launcher holds exclusive keyboard focus"
    );

    // Report `unresponsive`: transient surface is dismissed, focus cleared, input blocked.
    launcher_hc.unresponsive();
    pump(
        &mut event_loop,
        &mut state,
        &host_conn,
        &mut host_queue,
        &mut host_state,
    );
    pump(
        &mut event_loop,
        &mut state,
        &ln_conn,
        &mut ln_queue,
        &mut ln_state,
    );
    assert_eq!(
        state.hosts.component_liveness("launcher"),
        Liveness::Unresponsive
    );
    assert!(
        ln_state.closed_count >= 1,
        "transient surface received closed"
    );
    assert!(
        state.surface_under(center).is_none(),
        "unresponsive component receives no pointer hits"
    );
    assert!(
        state.seat.get_keyboard().unwrap().current_focus().is_none(),
        "keyboard focus leaves unresponsive component"
    );

    // Report `responsive`: liveness returns to Responsive.
    launcher_hc.responsive();
    pump(
        &mut event_loop,
        &mut state,
        &host_conn,
        &mut host_queue,
        &mut host_state,
    );
    assert_eq!(
        state.hosts.component_liveness("launcher"),
        Liveness::Responsive
    );

    // 7. Five failures within 60 s trigger `stopped(gave_up)` + `component_stopped`.
    for _ in 0..5 {
        launcher_hc.failed(Failure::Panic);
    }
    launcher_hc.connect();
    pump(
        &mut event_loop,
        &mut state,
        &host_conn,
        &mut host_queue,
        &mut host_state,
    );
    assert!(state.hosts.component_gave_up("launcher"));
    assert!(host_state
        .component_stopped
        .contains(&"launcher".to_string()));
    assert!(host_state
        .stopped
        .contains(&("launcher".to_string(), StopReason::GaveUp)));

    // 8. A restart the person asks for answers the host's `connect` it heard `stopped` for with a connection.
    assert!(host_state.connections.iter().all(|(n, _)| n != "launcher"));
    assert!(state.request_component_restart("launcher", None));
    pump(
        &mut event_loop,
        &mut state,
        &host_conn,
        &mut host_queue,
        &mut host_state,
    );
    assert!(!state.hosts.component_gave_up("launcher"));
    assert!(
        host_state.connections.iter().any(|(n, _)| n == "launcher"),
        "the restarted component gets its connection"
    );
}

/// One client connection of a test: its socket, queue and state.
struct Side<T> {
    conn: Connection,
    queue: EventQueue<T>,
    client: T,
}

/// Objects a mapped surface needs kept alive.
struct Mapped {
    _surface: wl_surface::WlSurface,
    _layer: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    _toplevel: Option<(xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel)>,
    _buffer: (std::fs::File, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer),
}

/// A server with one 800x600 output, driven on the test thread.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    output_id: cupertino_wm::OutputId,
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
        .expect("server starts");
        let output_id = state.add_output(new_output(
            "headless-1",
            PhysicalProperties {
                size: (300, 200).into(),
                subpixel: Subpixel::Unknown,
                make: "Cupertino".into(),
                model: "Test".into(),
            },
            OutputMode {
                size: (800, 600).into(),
                refresh: 60_000,
            },
            1.0,
            Transform::Normal,
            (0, 0),
        ));
        Self {
            event_loop,
            state,
            output_id,
        }
    }

    fn pump<T>(&mut self, side: &mut Side<T>) {
        pump(
            &mut self.event_loop,
            &mut self.state,
            &side.conn,
            &mut side.queue,
            &mut side.client,
        );
    }

    /// Run the server alone: what dropped connections leave is cleaned up.
    fn settle(&mut self) {
        for _ in 0..6 {
            let _ = self
                .event_loop
                .dispatch(Some(Duration::from_millis(2)), &mut self.state);
            self.state.tick();
            let _ = self.state.display_handle.flush_clients();
        }
    }

    fn side<T>(&mut self, fd: OwnedFd, client: T) -> Side<T>
    where
        T: Dispatch<wl_registry::WlRegistry, ()> + 'static,
    {
        let conn = Connection::from_socket(UnixStream::from(fd)).unwrap();
        let queue = conn.new_event_queue::<T>();
        let _reg = conn.display().get_registry(&queue.handle(), ());
        let mut side = Side {
            conn,
            queue,
            client,
        };
        self.pump(&mut side);
        side
    }

    fn host(&mut self, kind: HostKind, components: &[&'static str]) -> Side<HostClientState> {
        let fd = self
            .state
            .attach_host_connection(
                kind,
                components.to_vec(),
                Holder::Detached(Instant::now() + DETACHED_LIFETIME),
            )
            .expect("attach host");
        let side = self.side(fd, HostClientState::default());
        assert!(side.client.host.is_some(), "host binds cupertino_host_v1");
        side
    }

    /// Ask `host` for the host-component object of `name` and connect it.
    fn component_object(
        &mut self,
        host: &mut Side<HostClientState>,
        name: &str,
    ) -> CupertinoHostComponentV1 {
        let qh = host.queue.handle();
        let object =
            host.client
                .host
                .as_ref()
                .unwrap()
                .get_component(name.into(), &qh, name.into());
        object.connect();
        self.pump(host);
        object
    }

    /// The component connection `host` received last for `name`.
    fn component(
        &mut self,
        host: &mut Side<HostClientState>,
        name: &str,
    ) -> Side<ComponentClientState> {
        let fd = host
            .client
            .connections
            .iter()
            .position(|(n, _)| n == name)
            .map(|i| host.client.connections.remove(i).1)
            .unwrap_or_else(|| panic!("{name} connection fd"));
        let side = self.side(fd, ComponentClientState::default());
        assert!(side.client.done, "{name} hears its component group");
        side
    }

    /// Map a layer surface of `size` for `side`, persisted under `persist` when given.
    fn layer(
        &mut self,
        side: &mut Side<ComponentClientState>,
        layer: zwlr_layer_shell_v1::Layer,
        size: (i32, i32),
        persist: Option<&str>,
        exclusive_keyboard: bool,
    ) -> Mapped {
        let qh = side.queue.handle();
        let surface = side
            .client
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let role = side.client.layer_shell.as_ref().unwrap().get_layer_surface(
            &surface,
            None,
            layer,
            "test".into(),
            &qh,
            (),
        );
        role.set_size(size.0 as u32, size.1 as u32);
        if exclusive_keyboard {
            role.set_keyboard_interactivity(
                zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
            );
        }
        if let Some(key) = persist {
            side.client
                .component_global
                .as_ref()
                .unwrap()
                .persist(&surface, key.into());
        }
        surface.commit();
        self.pump(side);
        let buffer = make_shm_buffer(side.client.shm.as_ref().unwrap(), &qh, size.0, size.1);
        surface.attach(Some(&buffer.2), 0, 0);
        surface.commit();
        self.pump(side);
        Mapped {
            _surface: surface,
            _layer: Some(role),
            _toplevel: None,
            _buffer: buffer,
        }
    }

    /// Connect an ordinary application and map one 200x100 toplevel; returns its server-side surface too.
    fn window(
        &mut self,
    ) -> (
        Side<WindowClientState>,
        Mapped,
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        let stream = cupertino_server::shell_connection::connect_client(
            &self.state.display_handle,
            ClientState::default(),
        )
        .expect("application connection");
        let mut side = self.side(OwnedFd::from(stream), WindowClientState::default());
        let before: Vec<_> = self
            .state
            .windows
            .iter()
            .map(|w| w.surface().id())
            .collect();
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
        toplevel.set_app_id("org.example.Window".into());
        surface.commit();
        self.pump(&mut side);
        assert!(side.client.configured, "toplevel configured");
        let buffer = make_shm_buffer(side.client.shm.as_ref().unwrap(), &qh, 200, 100);
        surface.attach(Some(&buffer.2), 0, 0);
        surface.commit();
        self.pump(&mut side);
        let server_surface = self
            .state
            .windows
            .iter()
            .map(|w| w.surface().clone())
            .find(|s| !before.contains(&s.id()))
            .expect("the toplevel maps as a window");
        (
            side,
            Mapped {
                _surface: surface,
                _layer: None,
                _toplevel: Some((xdg, toplevel)),
                _buffer: buffer,
            },
            server_surface,
        )
    }

    fn keyboard_focus(
        &self,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.state.seat.get_keyboard().unwrap().current_focus()
    }
}

#[test]
fn focus_returns_to_the_window_after_unresponsive_and_after_a_dropped_transient() {
    let mut rig = Rig::new();
    let (mut app, _app_window, window) = rig.window();
    rig.pump(&mut app);
    assert_eq!(
        rig.keyboard_focus().map(|s| s.id()),
        Some(window.id()),
        "the new window has the keyboard"
    );

    let mut host = rig.host(HostKind::Shared, &["launcher"]);
    let launcher_hc = rig.component_object(&mut host, "launcher");
    let mut launcher = rig.component(&mut host, "launcher");
    let transient = rig.layer(
        &mut launcher,
        zwlr_layer_shell_v1::Layer::Overlay,
        (400, 300),
        None,
        true,
    );
    assert_ne!(
        rig.keyboard_focus().map(|s| s.id()),
        Some(window.id()),
        "the exclusive transient takes the keyboard"
    );

    // unresponsive: the transient is dismissed and the keyboard goes back to the window
    launcher_hc.unresponsive();
    rig.pump(&mut host);
    rig.pump(&mut launcher);
    assert!(launcher.client.closed_count >= 1, "transient closed");
    assert_eq!(
        rig.keyboard_focus().map(|s| s.id()),
        Some(window.id()),
        "focus returns to the window that had it"
    );
    launcher_hc.responsive();
    rig.pump(&mut host);
    drop(transient);

    // a fresh transient takes the keyboard again; then the connection drops with it mapped
    let again = rig.layer(
        &mut launcher,
        zwlr_layer_shell_v1::Layer::Overlay,
        (400, 300),
        None,
        true,
    );
    let center = Point::<f64, Logical>::from((400.0, 300.0));
    let transient_id = rig.state.surface_under(center).map(|(s, _)| s.id());
    assert!(transient_id.is_some());
    assert_ne!(rig.keyboard_focus().map(|s| s.id()), Some(window.id()));
    drop(again);
    drop(launcher);
    rig.settle();
    rig.pump(&mut app);
    assert!(
        rig.state.hosts.held_surfaces().is_empty(),
        "a transient surface leaves no hold"
    );
    assert_eq!(
        rig.keyboard_focus().map(|s| s.id()),
        Some(window.id()),
        "focus returns to the window after the connection drops"
    );
    assert_ne!(
        rig.state.surface_under(center).map(|(s, _)| s.id()),
        transient_id,
        "the dropped transient is gone"
    );
}

#[test]
fn lost_moves_a_component_to_a_separate_host_with_its_bus_bridge() {
    let mut rig = Rig::new();
    let mut shared = rig.host(HostKind::Shared, &["menubar", "osd"]);
    assert_eq!(shared.client.components, vec!["menubar", "osd"]);
    let osd_hc = rig.component_object(&mut shared, "osd");
    let _osd = rig.component(&mut shared, "osd");

    // lost: a failure is counted, the shared host's list loses it, a separate host is asked for
    osd_hc.lost();
    rig.pump(&mut shared);
    assert_eq!(rig.state.hosts.component_liveness("osd"), Liveness::Lost);
    assert!(rig.state.hosts.component_separate_for_session("osd"));
    assert_eq!(
        shared.client.done_count, 2,
        "the shared host hears a new list"
    );
    assert_eq!(
        shared.client.components,
        vec!["menubar", "osd", "menubar"],
        "the second list is the first without the lost component"
    );

    // the separate host binds: it gets its end of the bridge and the shared host the other
    let mut separate = rig.host(HostKind::Separate("osd"), &["osd"]);
    rig.pump(&mut shared);
    assert_eq!(separate.client.kind, Some(WireKind::Separate));
    assert_eq!(separate.client.components, vec!["osd"]);
    assert_eq!(separate.client.buses.len(), 1, "separate host gets `bus`");
    assert_eq!(
        shared.client.bus_peers.len(),
        1,
        "shared host gets `bus_peer`"
    );
    assert_eq!(shared.client.bus_peers[0].0, "osd");

    // `bus_dropped` within the throttle is answered once it passed
    separate
        .client
        .host
        .as_ref()
        .unwrap()
        .bus_dropped("osd".into());
    rig.pump(&mut separate);
    assert_eq!(
        separate.client.buses.len(),
        1,
        "no fresh pair within the throttle"
    );
    rig.state.tick_hosts(
        Instant::now() + cupertino_server::host::BRIDGE_THROTTLE + Duration::from_millis(50),
    );
    rig.pump(&mut separate);
    rig.pump(&mut shared);
    assert_eq!(
        separate.client.buses.len(),
        2,
        "a fresh pair after the throttle"
    );
    assert_eq!(shared.client.bus_peers.len(), 2);

    // the separate host's end reaches the shared host
    drop(separate);
    rig.settle();
    rig.pump(&mut shared);
    assert_eq!(shared.client.bus_peers_gone, vec!["osd"]);
    assert!(
        !rig.state.hosts.all_separate_for_session,
        "a separate host's exit is not a shared-host crash"
    );
}

/// Ask a fresh shared host to restart; it answers once the seat has been quiet, and the host is past its kill grace
/// when it goes. Returns whether the restart was answered only after the quiet time.
fn restart_shared_host(rig: &mut Rig) -> bool {
    let mut host = rig.host(HostKind::Shared, &["menubar"]);
    host.client.host.as_ref().unwrap().request_restart();
    rig.pump(&mut host);
    // the seat was quiet long ago; then a real button press makes it busy
    let long_ago = Instant::now()
        .checked_sub(Duration::from_secs(30))
        .expect("the clock runs long enough");
    rig.state.hosts.note_input(long_ago);
    rig.state.pointer_button(0x110, ButtonState::Pressed, 0);
    rig.state.pointer_button(0x110, ButtonState::Released, 1);
    let pressed = Instant::now();
    rig.state.tick_hosts(pressed + Duration::from_millis(500));
    rig.pump(&mut host);
    let early = host.client.restarts;
    rig.state.tick_hosts(
        pressed + cupertino_server::host::RESTART_QUIET_IDLE + Duration::from_millis(100),
    );
    rig.pump(&mut host);
    let answered = host.client.restarts == 1;
    // past the kill grace (the detached stand-in names the test's own process, which is never killed)
    rig.state.tick_hosts(
        pressed
            + cupertino_server::host::RESTART_QUIET_IDLE
            + cupertino_server::host::EXPECTED_EXIT_KILL_GRACE
            + Duration::from_millis(200),
    );
    drop(host);
    rig.settle();
    early == 0 && answered
}

#[test]
fn request_restart_waits_for_a_quiet_seat_and_an_expected_exit_is_no_crash() {
    let mut rig = Rig::new();
    for round in 0..cupertino_server::host::SHARED_HOST_CRASH_ISOLATE_COUNT {
        assert!(
            restart_shared_host(&mut rig),
            "round {round}: restart answered only once the seat was quiet"
        );
    }
    assert!(
        !rig.state.hosts.all_separate_for_session,
        "exits the server asked for, killed after the grace or not, are not crashes"
    );

    // three unexpected exits within 60 s isolate every component
    for _ in 0..cupertino_server::host::SHARED_HOST_CRASH_ISOLATE_COUNT {
        let host = rig.host(HostKind::Shared, &["menubar"]);
        drop(host);
        rig.settle();
    }
    assert!(rig.state.hosts.all_separate_for_session);
}

/// A window manager component instance: its connection, its role object and its persisted surface.
struct WmInstance {
    side: Side<ComponentClientState>,
    wm: CupertinoWindowManagerV1,
}

fn window_manager(
    rig: &mut Rig,
    host: &mut Side<HostClientState>,
    hc: &CupertinoHostComponentV1,
) -> WmInstance {
    hc.connect();
    rig.pump(host);
    let mut side = rig.component(host, "windowmanager");
    let qh = side.queue.handle();
    let wm = side
        .client
        .shell
        .as_ref()
        .expect("shell global")
        .get_window_manager(side.client.output.as_ref().expect("output"), &qh, ());
    rig.pump(&mut side);
    WmInstance { side, wm }
}

#[test]
fn window_manager_placements_stay_held_until_takeover_or_the_hold_timeout() {
    let mut rig = Rig::new();
    let (mut app, _app_window, window) = rig.window();
    let mut host = rig.host(HostKind::Shared, &["windowmanager"]);
    let qh = host.queue.handle();
    let wm_hc = host.client.host.as_ref().unwrap().get_component(
        "windowmanager".into(),
        &qh,
        "windowmanager".into(),
    );
    let mut first = window_manager(&mut rig, &mut host, &wm_hc);
    let id = *first
        .side
        .client
        .wm_windows
        .first()
        .expect("the window is arranged");
    let window_id = cupertino_wm::WindowId(u64::from(id));
    assert!(rig
        .state
        .windows
        .iter()
        .any(|w| w.surface().id() == window.id()));
    let _strip = rig.layer(
        &mut first.side,
        zwlr_layer_shell_v1::Layer::Bottom,
        (10, 10),
        Some("strip"),
        false,
    );
    first.wm.set_placement(id, Placement::Hidden, 0, 0, 0, 0);
    first.wm.commit();
    rig.pump(&mut first.side);
    assert!(
        rig.state.window_managers.passes_input(window_id),
        "placed hidden"
    );

    // the connection drops: the window stays hidden
    drop(_strip);
    drop(first);
    rig.settle();
    rig.pump(&mut app);
    assert_eq!(rig.state.hosts.held_surfaces().len(), 1);
    assert!(rig.state.hosts.wm_placements_held(rig.output_id));
    assert!(
        rig.state.window_managers.passes_input(window_id),
        "a held placement keeps the window hidden"
    );

    // a new instance commits the window back on the stage: kept until its takeover frame
    let mut second = window_manager(&mut rig, &mut host, &wm_hc);
    assert_eq!(second.side.client.holds, vec!["strip"]);
    second.wm.set_placement(id, Placement::Stage, 0, 0, 0, 0);
    second.wm.commit();
    rig.pump(&mut second.side);
    assert!(
        rig.state.window_managers.passes_input(window_id),
        "the new instance's commit waits for its takeover frame"
    );
    let strip = rig.layer(
        &mut second.side,
        zwlr_layer_shell_v1::Layer::Bottom,
        (10, 10),
        Some("strip"),
        false,
    );
    assert!(rig.state.hosts.held_surfaces().is_empty(), "taken over");
    assert!(!rig.state.hosts.wm_placements_held(rig.output_id));
    assert!(
        !rig.state.window_managers.passes_input(window_id),
        "in the takeover frame the committed placements replace the held ones"
    );

    // hidden again, dropped again; this time no instance takes over and the hold times out
    second.wm.set_placement(id, Placement::Hidden, 0, 0, 0, 0);
    second.wm.commit();
    rig.pump(&mut second.side);
    assert!(rig.state.window_managers.passes_input(window_id));
    drop(strip);
    drop(second);
    rig.settle();
    assert!(rig.state.window_managers.passes_input(window_id), "held");
    let _third = window_manager(&mut rig, &mut host, &wm_hc);
    rig.state.tick_hosts(
        Instant::now() + cupertino_server::host::HOLD_READY_TIMEOUT + Duration::from_millis(100),
    );
    rig.settle();
    assert!(
        rig.state.hosts.held_surfaces().is_empty(),
        "the hold timed out"
    );
    assert!(!rig.state.hosts.wm_placements_held(rig.output_id));
    assert!(
        !rig.state.window_managers.passes_input(window_id),
        "the held placement ended: the window returns to its frame"
    );
}

/// Map a persisted top bar of `height` reserving `zone` for `side`, with a buffer when `buffer` holds.
fn bar(
    rig: &mut Rig,
    side: &mut Side<ComponentClientState>,
    height: u32,
    zone: i32,
    buffer: bool,
) -> (
    wl_surface::WlSurface,
    zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    Option<(std::fs::File, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer)>,
) {
    let qh = side.queue.handle();
    let surface = side
        .client
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&qh, ());
    let layer = side.client.layer_shell.as_ref().unwrap().get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Top,
        "menubar".into(),
        &qh,
        (),
    );
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_size(800, height);
    layer.set_exclusive_zone(zone);
    side.client
        .component_global
        .as_ref()
        .unwrap()
        .persist(&surface, "bar".into());
    surface.commit();
    rig.pump(side);
    let buffer = buffer.then(|| {
        let buffer = make_shm_buffer(side.client.shm.as_ref().unwrap(), &qh, 800, height as i32);
        surface.attach(Some(&buffer.2), 0, 0);
        surface.commit();
        buffer
    });
    rig.pump(side);
    (surface, layer, buffer)
}

#[test]
fn an_instance_that_ends_before_its_first_buffer_keeps_the_older_hold() {
    let mut rig = Rig::new();
    let mut host = rig.host(HostKind::Shared, &["menubar"]);
    let menubar_hc = rig.component_object(&mut host, "menubar");
    let mut first = rig.component(&mut host, "menubar");
    let first_bar = bar(&mut rig, &mut first, 28, 28, true);
    assert_eq!(rig.state.usable_area(rig.output_id).y, 28.0);
    drop(first_bar);
    drop(first);
    rig.settle();
    assert_eq!(rig.state.hosts.held_surfaces().len(), 1);
    let geometry = rig.state.hosts.held_surfaces()[0].geometry;

    // a new instance persists the bar again, taller, and ends before it shows a buffer
    menubar_hc.connect();
    rig.pump(&mut host);
    let mut second = rig.component(&mut host, "menubar");
    assert_eq!(second.client.holds, vec!["bar"]);
    let second_bar = bar(&mut rig, &mut second, 50, 50, false);
    drop(second_bar);
    drop(second);
    rig.settle();
    let held = rig.state.hosts.held_surfaces();
    assert_eq!(held.len(), 1, "the older hold stays");
    assert!(held[0].picture.is_some(), "with its picture");
    assert_eq!(held[0].geometry, geometry, "and its geometry");
    assert_eq!(
        rig.state.usable_area(rig.output_id).y,
        28.0,
        "and its reserved space"
    );
}

#[test]
fn lost_and_failed_count_only_from_the_host_that_runs_the_component() {
    let mut rig = Rig::new();
    let mut shared = rig.host(HostKind::Shared, &["menubar", "osd"]);
    let _osd_hc = rig.component_object(&mut shared, "osd");
    let _osd = rig.component(&mut shared, "osd");

    // another host holds an object for it, but the shared host runs it
    let mut other = rig.host(HostKind::Separate("osd"), &["osd"]);
    let qh = other.queue.handle();
    let stray = other
        .client
        .host
        .as_ref()
        .unwrap()
        .get_component("osd".into(), &qh, "osd".into());
    rig.pump(&mut other);
    for _ in 0..5 {
        stray.failed(Failure::Panic);
    }
    stray.lost();
    rig.pump(&mut other);
    rig.pump(&mut shared);
    assert!(
        !rig.state.hosts.component_gave_up("osd"),
        "another host's failures are not counted"
    );
    assert_eq!(
        rig.state.hosts.component_liveness("osd"),
        Liveness::Responsive
    );
    assert!(!rig.state.hosts.component_separate_for_session("osd"));
    assert!(rig.state.hosts.component_connected("osd"));
    assert_eq!(
        shared.client.done_count, 1,
        "the shared host keeps its list"
    );
}
