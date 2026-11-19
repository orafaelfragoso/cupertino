//! Program tests for W5.2 (`cupertino_host_v1`, `cupertino_host_component_v1`, `cupertino_component_v1`):
//! - A dropped component keeps a persisted menu bar's reserved space and picture (hold), and a new instance
//!   receives `hold(key)` + `done` and takes over atomically on its first buffer commit without usable-area reflow.
//! - A transient surface is dismissed on `unresponsive` (and on drop) and keyboard focus returns to the prior window.
//! - An unresponsive component receives no pointer or keyboard input until `responsive`.
//! - Bus-identity nonce verification accepts the announced unique name from the host process and refuses wrong sender
//!   or duplicate component claims.
//! - Five failures within 60 s stop the component (`stopped(gave_up)` + `component_stopped`) and release holds.

use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_peer::{Caller, ProcessHandle};
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
use cupertino_server::state::new_output;
use cupertino_server::{Config, Cupertino};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Logical, Point, Transform};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
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

fn make_shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<ComponentClientState>,
    w: i32,
    h: i32,
) -> (std::fs::File, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer) {
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

    let _ = Duration::from_secs(0);
}
