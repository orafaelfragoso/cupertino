//! Program tests for W5.3 (`cupertino_transaction_manager_v1`, `cupertino_transaction_v1`, `cupertino_hole_v1`):
//! - Two participants (`dock` + `menu_bar`) start on the same presented frame timestamp.
//! - A late participant joins in step at the original start timestamp after the deadline passed.
//! - A gesture-bound transaction (`overview`) tracks progress and settles with the release velocity (and supports
//!   reverse gestures).
//! - Window/workspace holes (`get_hole`) stack claims in commit order, fall back on destroy, and emit `source_closed`.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_protocols::client::host::cupertino_component_v1::{self, CupertinoComponentV1};
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1};
use cupertino_protocols::client::layers::cupertino_layer_v1::{self, CupertinoLayerV1};
use cupertino_protocols::client::layers::cupertino_layers_v1::{self, CupertinoLayersV1};
use cupertino_protocols::client::transaction::cupertino_hole_v1::{
    self, CupertinoHoleV1, Source as HoleWireSource,
};
use cupertino_protocols::client::transaction::cupertino_transaction_manager_v1::{
    self, CupertinoTransactionManagerV1,
};
use cupertino_protocols::client::transaction::cupertino_transaction_v1::{
    self, CupertinoTransactionV1, Gesture, Outcome,
};
use cupertino_server::authorization::{Holder, DETACHED_LIFETIME};
use cupertino_server::host::HostKind;
use cupertino_server::state::{insert_client, new_output, ClientState};
use cupertino_server::{Config, Cupertino};
use cupertino_wm::{OutputId, WindowId};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::layer_map_for_output;
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

#[derive(Default)]
struct HostClient {
    host: Option<CupertinoHostV1>,
    connections: Vec<(String, std::os::fd::OwnedFd)>,
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

#[derive(Default)]
struct CompClient {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    /// Frame callbacks answered.
    frames: usize,
    output: Option<wl_output::WlOutput>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    layers: Option<CupertinoLayersV1>,
    component_global: Option<CupertinoComponentV1>,
    tx_manager: Option<CupertinoTransactionManagerV1>,
    instance: Option<u32>,
    started: Vec<(u32, u32, u32)>,
    gesture_began: Vec<Gesture>,
    gesture_ended: Vec<Outcome>,
    finished: Vec<Outcome>,
    source_closed: usize,
    /// The buffers the server released, in the order it did.
    released: Vec<wayland_client::backend::ObjectId>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for CompClient {
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
                "wl_output" => {
                    state.output =
                        Some(reg.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ()));
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
                "cupertino_layers_v1" => {
                    state.layers =
                        Some(reg.bind::<CupertinoLayersV1, _, _>(name, version.min(6), qh, ()));
                }
                "cupertino_component_v1" => {
                    state.component_global =
                        Some(reg.bind::<CupertinoComponentV1, _, _>(name, version.min(1), qh, ()));
                }
                "cupertino_transaction_manager_v1" => {
                    state.tx_manager = Some(reg.bind::<CupertinoTransactionManagerV1, _, _>(
                        name,
                        version.min(1),
                        qh,
                        (),
                    ));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for CompClient {
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

impl Dispatch<wl_output::WlOutput, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(CompClient: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(CompClient: ignore wl_shm_pool::WlShmPool);

impl Dispatch<wl_buffer::WlBuffer, ()> for CompClient {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.released.push(buffer.id());
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for CompClient {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frames += 1;
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for CompClient {
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

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for CompClient {
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

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for CompClient {
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

impl Dispatch<CupertinoLayersV1, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &CupertinoLayersV1,
        _: cupertino_layers_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoLayerV1, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &CupertinoLayerV1,
        _: cupertino_layer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoComponentV1, ()> for CompClient {
    fn event(
        state: &mut Self,
        _: &CupertinoComponentV1,
        event: cupertino_component_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_component_v1::Event::Component { instance, .. } = event {
            state.instance = Some(instance);
        }
    }
}

impl Dispatch<CupertinoTransactionManagerV1, ()> for CompClient {
    fn event(
        _: &mut Self,
        _: &CupertinoTransactionManagerV1,
        _: cupertino_transaction_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoTransactionV1, ()> for CompClient {
    fn event(
        state: &mut Self,
        _: &CupertinoTransactionV1,
        event: cupertino_transaction_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_transaction_v1::Event::Started {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => state.started.push((tv_sec_hi, tv_sec_lo, tv_nsec)),
            cupertino_transaction_v1::Event::GestureBegan {
                gesture: WEnum::Value(g),
            } => state.gesture_began.push(g),
            cupertino_transaction_v1::Event::GestureEnded {
                outcome: WEnum::Value(o),
            } => state.gesture_ended.push(o),
            cupertino_transaction_v1::Event::Finished {
                outcome: WEnum::Value(o),
            } => state.finished.push(o),
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHoleV1, ()> for CompClient {
    fn event(
        state: &mut Self,
        _: &CupertinoHoleV1,
        event: cupertino_hole_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_hole_v1::Event::SourceClosed = event {
            state.source_closed += 1;
        }
    }
}

struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    output_id: OutputId,
    /// Drawn by the headless backend into an offscreen texture, rather than frames prepared without drawing.
    headless: bool,
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
        let output = new_output(
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
        );
        let output_id = state.add_output(output);
        Self {
            event_loop,
            state,
            output_id,
            headless: false,
        }
    }

    /// A compositor drawing one offscreen output of `size` pixels at scale 1 with the headless backend.
    fn headless(size: (i32, i32)) -> Self {
        // the software rasteriser, as the program tests run the headless server
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
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
        cupertino_server::backend::headless::init(&mut state, size, 1.0)
            .expect("an offscreen renderer");
        let output_id = state.outputs[0].id;
        Self {
            event_loop,
            state,
            output_id,
            headless: true,
        }
    }

    /// The output as it shows now, without the pointer.
    fn image(&mut self) -> image::RgbaImage {
        let mut backend = self.state.backend.take().unwrap();
        let image = self
            .state
            .draw_output_image(backend.renderer(), 0, false, false)
            .unwrap();
        self.state.backend = Some(backend);
        image
    }

    /// One turn of the loop: dispatch, advance the model and prepare a frame of the output, as a backend does (the
    /// headless one draws it).
    fn pump(&mut self) {
        let _ = self
            .event_loop
            .dispatch(Some(Duration::from_millis(2)), &mut self.state);
        self.state.tick();
        if self.headless {
            cupertino_server::backend::headless::render(&mut self.state);
        } else {
            let idx = self
                .state
                .outputs
                .iter()
                .position(|o| o.id == self.output_id)
                .unwrap();
            self.state.transactions_frame(idx);
        }
        let _ = self.state.display_handle.flush_clients();
    }

    fn connect_host(
        &mut self,
        components: &[&'static str],
    ) -> (Connection, EventQueue<HostClient>, HostClient) {
        let host_fd = self
            .state
            .attach_host_connection(
                HostKind::Shared,
                components.to_vec(),
                Holder::Detached(Instant::now() + DETACHED_LIFETIME),
            )
            .unwrap();
        let conn = Connection::from_socket(UnixStream::from(host_fd)).unwrap();
        let mut queue: EventQueue<HostClient> = conn.new_event_queue();
        let qh = queue.handle();
        let mut client = HostClient::default();
        let _reg = conn.display().get_registry(&qh, ());
        for _ in 0..8 {
            let _ = conn.flush();
            self.pump();
            let _ = queue.dispatch_pending(&mut client);
            if let Some(guard) = conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = queue.dispatch_pending(&mut client);
            if client.host.is_some() {
                break;
            }
        }
        (conn, queue, client)
    }

    fn spawn_component(
        &mut self,
        host_conn: &Connection,
        host_queue: &mut EventQueue<HostClient>,
        host_client: &mut HostClient,
        name: &str,
    ) -> (
        CupertinoHostComponentV1,
        Connection,
        EventQueue<CompClient>,
        CompClient,
    ) {
        let qh = host_queue.handle();
        let handle =
            host_client
                .host
                .as_ref()
                .unwrap()
                .get_component(name.into(), &qh, name.to_string());
        handle.connect();
        for _ in 0..8 {
            let _ = host_conn.flush();
            self.pump();
            if let Some(guard) = host_conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = host_queue.dispatch_pending(host_client);
            if host_client.connections.iter().any(|(n, _)| n == name) {
                break;
            }
        }
        let pos = host_client
            .connections
            .iter()
            .position(|(n, _)| n == name)
            .expect("host should receive component connection fd");
        let (_, fd) = host_client.connections.remove(pos);
        let stream: UnixStream = fd.into();
        let conn = Connection::from_socket(stream).unwrap();
        let mut queue: EventQueue<CompClient> = conn.new_event_queue();
        let cqh = queue.handle();
        let mut client = CompClient::default();
        let _reg = conn.display().get_registry(&cqh, ());
        for _ in 0..10 {
            let _ = conn.flush();
            self.pump();
            if let Some(guard) = conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = queue.dispatch_pending(&mut client);
            if client.tx_manager.is_some()
                && client.output.is_some()
                && client.instance.is_some()
                && client.layer_shell.is_some()
            {
                break;
            }
        }
        assert!(
            client.tx_manager.is_some(),
            "{name} should bind transaction manager"
        );
        assert!(client.output.is_some(), "{name} should bind wl_output");
        (handle, conn, queue, client)
    }

    fn pump_comp(
        &mut self,
        conn: &Connection,
        queue: &mut EventQueue<CompClient>,
        state: &mut CompClient,
    ) {
        let _ = conn.flush();
        self.pump();
        if let Some(guard) = conn.prepare_read() {
            let _ = guard.read();
        }
        let _ = queue.dispatch_pending(state);
    }

    fn create_layer_surface(
        &mut self,
        conn: &Connection,
        queue: &mut EventQueue<CompClient>,
        state: &mut CompClient,
        namespace: &str,
    ) -> (
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    ) {
        let qh = queue.handle();
        let surf = state.compositor.as_ref().unwrap().create_surface(&qh, ());
        let layer = state.layer_shell.as_ref().unwrap().get_layer_surface(
            &surf,
            state.output.as_ref(),
            zwlr_layer_shell_v1::Layer::Top,
            namespace.into(),
            &qh,
            (),
        );
        layer.set_size(200, 40);
        surf.commit();
        for _ in 0..4 {
            self.pump_comp(conn, queue, state);
        }
        (surf, layer)
    }
}

/// An ordinary client with one window.
#[derive(Default)]
struct WinClient {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    configured: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for WinClient {
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

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WinClient {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for WinClient {
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

wayland_client::delegate_noop!(WinClient: ignore xdg_toplevel::XdgToplevel);
wayland_client::delegate_noop!(WinClient: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(WinClient: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(WinClient: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(WinClient: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(WinClient: ignore wl_buffer::WlBuffer);

/// One colour in `wl_shm` ARGB8888 memory order (blue, green, red, alpha): an opaque magenta, which neither the
/// wallpaper nor a window's frame shows.
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
/// How many commits drawn on frame callbacks a test keeps in an armed part: well past a component's two or three buffers.
const KEPT_FRAMES: usize = 12;
/// How many commits of a new buffer each a test keeps in an armed part without waiting for frame callbacks: more than a
/// component's commits of a long gesture would pile up in kept states of their own.
const KEPT_BUFFERS: usize = 100;
/// A clear pixel.
const CLEAR: [u8; 4] = [0, 0, 0, 0];
/// Model seconds a test steps its fixed clock by for a closing window's fade to have ended: well past the closing
/// animation's length.
const PAST_CLOSING_FADE: f64 = 5.0;

/// A `width` × `height` buffer of one colour `pixel` (ARGB8888 memory order).
fn shm_buffer<D>(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<D>,
    (width, height): (i32, i32),
    pixel: [u8; 4],
) -> wl_buffer::WlBuffer
where
    D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, ()> + 'static,
{
    let len = width * height * 4;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    let bytes: Vec<u8> = pixel.iter().copied().cycle().take(len as usize).collect();
    file.write_all(&bytes).unwrap();
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

/// How many pixels of `image` inside `rect` (`x`, `y`, `width`, `height` in pixels) show the magenta of a window's
/// content, its picture faded part of the way included.
fn magenta_in(image: &image::RgbaImage, rect: (u32, u32, u32, u32)) -> usize {
    let (x0, y0, w, h) = rect;
    let mut n = 0;
    for y in y0..(y0 + h).min(image.height()) {
        for x in x0..(x0 + w).min(image.width()) {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            if r > 120 && b > 120 && g < 60 {
                n += 1;
            }
        }
    }
    n
}

impl Rig {
    /// Dispatch one loop turn of the compositor and of client `conn`.
    fn pump_with<D: 'static>(
        &mut self,
        conn: &Connection,
        queue: &mut EventQueue<D>,
        state: &mut D,
    ) {
        let _ = conn.flush();
        self.pump();
        if let Some(guard) = conn.prepare_read() {
            let _ = guard.read();
        }
        let _ = queue.dispatch_pending(state);
    }

    /// An ordinary client showing a `size` window of magenta content, mapped; returns its window's id too.
    fn magenta_window(
        &mut self,
        size: (i32, i32),
    ) -> (
        Connection,
        EventQueue<WinClient>,
        WinClient,
        wl_surface::WlSurface,
        xdg_toplevel::XdgToplevel,
        WindowId,
    ) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        insert_client(&self.state.display_handle, ours, ClientState::default()).unwrap();
        let conn = Connection::from_socket(theirs).unwrap();
        let mut queue: EventQueue<WinClient> = conn.new_event_queue();
        let qh = queue.handle();
        let mut client = WinClient::default();
        let _reg = conn.display().get_registry(&qh, ());
        for _ in 0..6 {
            self.pump_with(&conn, &mut queue, &mut client);
        }
        let surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg = client
            .wm_base
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title("hole source".into());
        toplevel.set_app_id("org.cupertino.test.hole".into());
        surface.commit();
        for _ in 0..20 {
            self.pump_with(&conn, &mut queue, &mut client);
            if client.configured {
                break;
            }
        }
        assert!(client.configured, "the window is configured");
        let buffer = shm_buffer(client.shm.as_ref().unwrap(), &qh, size, MAGENTA);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        for _ in 0..10 {
            self.pump_with(&conn, &mut queue, &mut client);
        }
        let id = self
            .state
            .windows
            .iter()
            .find(|e| e.mapped)
            .map(|e| e.id)
            .expect("the window is mapped");
        (conn, queue, client, surface, toplevel, id)
    }
}

#[test]
fn two_participants_start_on_same_presented_frame() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "menubar"]);
    let (_hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");

    let (dock_surf, _dock_layer) = rig.create_layer_surface(&dconn, &mut dq, &mut dst, "dock");
    let (bar_surf, _bar_layer) = rig.create_layer_surface(&bconn, &mut bq, &mut bst, "menubar");

    let dock_inst = dst.instance.unwrap();
    let dqh = dq.handle();
    let bqh = bq.handle();

    // Dock creates transaction serial 1 expecting menubar and stages its surface.
    let tx_dock =
        dst.tx_manager
            .as_ref()
            .unwrap()
            .create(1, dst.output.as_ref().unwrap(), &dqh, ());
    tx_dock.expect("menubar".into());
    tx_dock.stage(&dock_surf);
    tx_dock.submit();
    rig.pump_comp(&dconn, &mut dq, &mut dst);

    // Dock has not committed its staged surface yet, and menubar hasn't joined -> not started.
    assert!(dst.started.is_empty());

    // Menu bar joins and stages its surface, commits it, and submits.
    let tx_bar = bst.tx_manager.as_ref().unwrap().join(
        "dock".into(),
        dock_inst,
        1,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    tx_bar.stage(&bar_surf);
    tx_bar.submit();
    bar_surf.commit();
    rig.pump_comp(&bconn, &mut bq, &mut bst);

    // Still waiting for dock's staged surface commit!
    assert!(dst.started.is_empty());
    assert!(bst.started.is_empty());

    // Dock commits its staged surface -> both start on the exact same presented frame timestamp.
    dock_surf.commit();
    for _ in 0..6 {
        rig.pump_comp(&dconn, &mut dq, &mut dst);
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }

    assert_eq!(dst.started.len(), 1, "dock should receive started once");
    assert_eq!(bst.started.len(), 1, "menubar should receive started once");
    assert_eq!(
        dst.started[0], bst.started[0],
        "both participants must start on the exact same presented frame timestamp"
    );
}

#[test]
fn late_participant_joins_in_step() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "menubar"]);
    let (_hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");

    let dock_inst = dst.instance.unwrap();
    let dqh = dq.handle();
    let bqh = bq.handle();

    let tx_dock =
        dst.tx_manager
            .as_ref()
            .unwrap()
            .create(7, dst.output.as_ref().unwrap(), &dqh, ());
    tx_dock.expect("menubar".into());
    tx_dock.set_deadline(20);
    tx_dock.submit();
    rig.pump_comp(&dconn, &mut dq, &mut dst);
    assert!(
        dst.started.is_empty(),
        "must wait for expected menubar before deadline"
    );

    // Advance past the 20 ms deadline.
    std::thread::sleep(Duration::from_millis(30));
    for _ in 0..4 {
        rig.pump_comp(&dconn, &mut dq, &mut dst);
    }
    assert_eq!(
        dst.started.len(),
        1,
        "dock should start when deadline passes"
    );
    let orig_ts = dst.started[0];

    // Advance wall clock slightly so a fresh timestamp would differ, then menubar joins late.
    std::thread::sleep(Duration::from_millis(15));
    let tx_bar = bst.tx_manager.as_ref().unwrap().join(
        "dock".into(),
        dock_inst,
        7,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    tx_bar.submit();
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(bst.started.len(), 1, "late joiner should receive started");
    assert_eq!(
        bst.started[0], orig_ts,
        "late participant must receive the transaction's original start timestamp to join in step"
    );
}

#[test]
fn gesture_bound_transaction_settles_with_velocity() {
    let sample_after_40ms =
        |release_velocity: f64| -> (f64, Vec<Gesture>, Vec<Outcome>, Vec<Outcome>) {
            let mut rig = Rig::new();
            rig.state.fixed_time = Some(10.0);
            let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
            let (_hov, oconn, mut oq, mut ost) =
                rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
            let oqh = oq.handle();

            let tx =
                ost.tx_manager
                    .as_ref()
                    .unwrap()
                    .create(1, ost.output.as_ref().unwrap(), &oqh, ());
            tx.bind_gesture(Gesture::SwipeUp, 0.45, 1.0);
            tx.bind_reverse_gesture(Gesture::SwipeDown);
            tx.submit();
            for _ in 0..4 {
                rig.pump_comp(&oconn, &mut oq, &mut ost);
            }

            // Begin gesture -> update to 0.35 -> release toward completion with `release_velocity`.
            assert!(rig.state.transaction_gesture_begin(
            rig.output_id,
            cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
        ));
            assert!(rig.state.transaction_gesture_update(rig.output_id, 0.35));
            assert!(rig
                .state
                .transaction_gesture_end(rig.output_id, true, release_velocity));
            rig.pump_comp(&oconn, &mut oq, &mut ost);

            // Sample 40 ms into the spring settle.
            let p_40ms = rig
                .state
                .transactions
                .gesture_progress_on(rig.output_id, 10.04)
                .expect("gesture should still be settling at +40 ms");

            // Advance well past settle duration -> finished(Completed).
            rig.state.fixed_time = Some(13.0);
            for _ in 0..4 {
                rig.pump_comp(&oconn, &mut oq, &mut ost);
            }
            (p_40ms, ost.gesture_began, ost.gesture_ended, ost.finished)
        };

    let (slow_p, began_s, ended_s, fin_s) = sample_after_40ms(0.0);
    let (fast_p, began_f, ended_f, fin_f) = sample_after_40ms(6.0);

    assert_eq!(began_s, vec![Gesture::SwipeUp]);
    assert_eq!(ended_s, vec![Outcome::Completed]);
    assert_eq!(fin_s, vec![Outcome::Completed]);
    assert_eq!(began_f, vec![Gesture::SwipeUp]);
    assert_eq!(ended_f, vec![Outcome::Completed]);
    assert_eq!(fin_f, vec![Outcome::Completed]);
    assert!(
        fast_p > slow_p + 0.05,
        "higher release velocity must carry the spring further toward target (fast={fast_p}, slow={slow_p})"
    );
}

#[test]
fn holes_stack_claims_and_notify_source_closed() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (parent_surf, _parent_layer) =
        rig.create_layer_surface(&oconn, &mut oq, &mut ost, "overview");

    let oqh = oq.handle();
    let child1 = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
    let child2 = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
    let _layer1 = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child1, &parent_surf, &oqh, ());
    let _layer2 = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child2, &parent_surf, &oqh, ());
    let hole1 = ost.tx_manager.as_ref().unwrap().get_hole(&child1, &oqh, ());
    let hole2 = ost.tx_manager.as_ref().unwrap().get_hole(&child2, &oqh, ());

    hole1.set_source(HoleWireSource::Window, 42);
    hole1.set_claim(1);
    child1.commit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }

    let wid = WindowId(42);
    assert!(rig.state.transactions.is_window_claimed(wid));
    let first_id = rig.state.transactions.active_claim_for(wid).unwrap().id;

    // Second hole claims the same window -> becomes top of stack.
    hole2.set_source(HoleWireSource::Window, 42);
    hole2.set_claim(1);
    child2.commit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let second_id = rig.state.transactions.active_claim_for(wid).unwrap().id;
    assert_ne!(first_id, second_id);

    // Destroying the top hole falls back to hole1.
    hole2.destroy();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        rig.state.transactions.active_claim_for(wid).unwrap().id,
        first_id
    );

    // Window closes -> claim clears and hole1 receives source_closed.
    rig.state.transactions.notify_window_closed(wid);
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert!(!rig.state.transactions.is_window_claimed(wid));
    assert_eq!(ost.source_closed, 1);

    // Keep test timestamps alive cleanly.
    let _ = Instant::now();
}

/// What a component keeps of a surface with a hole: its objects, alive while it holds them.
struct HoleHost {
    _parent: wl_surface::WlSurface,
    _shell_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    _buffer: wl_buffer::WlBuffer,
    _child: wl_surface::WlSurface,
    _layer: CupertinoLayerV1,
    _hole: CupertinoHoleV1,
}

impl Rig {
    /// A layer shell surface of the component on `conn` at the output's top left, 160 × 120 points, clear, with one layer
    /// over all of it that is a hole claiming window `window`; persisted (`cupertino_component_v1.persist`) with
    /// `persist`.
    fn hole_host(
        &mut self,
        conn: &Connection,
        queue: &mut EventQueue<CompClient>,
        state: &mut CompClient,
        window: WindowId,
        persist: bool,
    ) -> HoleHost {
        let qh = queue.handle();
        let parent = state.compositor.as_ref().unwrap().create_surface(&qh, ());
        let shell_surface = state.layer_shell.as_ref().unwrap().get_layer_surface(
            &parent,
            state.output.as_ref(),
            zwlr_layer_shell_v1::Layer::Top,
            "hole-host".into(),
            &qh,
            (),
        );
        shell_surface
            .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
        shell_surface.set_size(160, 120);
        if persist {
            state
                .component_global
                .as_ref()
                .unwrap()
                .persist(&parent, "holes".into());
        }
        parent.commit();
        for _ in 0..4 {
            self.pump_comp(conn, queue, state);
        }
        let buffer = shm_buffer(state.shm.as_ref().unwrap(), &qh, (160, 120), CLEAR);
        parent.attach(Some(&buffer), 0, 0);
        parent.damage_buffer(0, 0, 160, 120);
        let child = state.compositor.as_ref().unwrap().create_surface(&qh, ());
        let layer = state
            .layers
            .as_ref()
            .unwrap()
            .get_layer(&child, &parent, &qh, ());
        layer.set_value(cupertino_layer_v1::Property::Width, 160.0);
        layer.set_value(cupertino_layer_v1::Property::Height, 120.0);
        let hole = state.tx_manager.as_ref().unwrap().get_hole(&child, &qh, ());
        hole.set_source(HoleWireSource::Window, window.0 as u32);
        hole.set_claim(1);
        child.commit();
        parent.commit();
        for _ in 0..6 {
            self.pump_comp(conn, queue, state);
        }
        HoleHost {
            _parent: parent,
            _shell_surface: shell_surface,
            _buffer: buffer,
            _child: child,
            _layer: layer,
            _hole: hole,
        }
    }
}

/// `(tv_sec_hi, tv_sec_lo, tv_nsec)` of a presentation-clock time, as `started` carries it.
fn wire_time(t: Duration) -> (u32, u32, u32) {
    let secs = t.as_secs();
    ((secs >> 32) as u32, secs as u32, t.subsec_nanos())
}

#[test]
fn staged_commits_are_kept_until_the_part_applies() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "menubar"]);
    let (_hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");
    let (dock_surf, _dock_layer) = rig.create_layer_surface(&dconn, &mut dq, &mut dst, "dock");
    let server_dock = layer_map_for_output(&rig.state.outputs[0].output)
        .layers()
        .find(|l| l.namespace() == "dock")
        .map(|l| l.wl_surface().clone())
        .expect("the dock's layer shell surface");
    let layers_of = |s: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface| {
        cupertino_server::layers_v1::with_parent(s, |layers| layers.len())
    };
    let dock_inst = dst.instance.unwrap();
    let dqh = dq.handle();
    let bqh = bq.handle();

    let tx = dst
        .tx_manager
        .as_ref()
        .unwrap()
        .create(3, dst.output.as_ref().unwrap(), &dqh, ());
    tx.expect("menubar".into());
    tx.set_deadline(100);
    tx.stage(&dock_surf);
    // a layer made while its parent is staged, the parent's commit with a frame callback
    let child = dst.compositor.as_ref().unwrap().create_surface(&dqh, ());
    let _layer = dst
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &dock_surf, &dqh, ());
    let _callback = dock_surf.frame(&dqh, ());
    dock_surf.commit();
    tx.submit();
    for _ in 0..3 {
        rig.pump_comp(&dconn, &mut dq, &mut dst);
    }
    assert!(dst.started.is_empty(), "the menu bar is expected");
    assert_eq!(
        layers_of(&server_dock),
        0,
        "the staged surface's commit is kept: its new layer is not there yet"
    );
    assert!(
        dst.frames >= 1,
        "a kept commit's frame callback is answered with the output's next frame"
    );

    // the menu bar joins and submits: the kept commit applies on the start frame
    let tx_bar = bst.tx_manager.as_ref().unwrap().join(
        "dock".into(),
        dock_inst,
        3,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    tx_bar.submit();
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
        rig.pump_comp(&dconn, &mut dq, &mut dst);
    }
    assert_eq!(dst.started.len(), 1, "the transaction started");
    assert_eq!(
        layers_of(&server_dock),
        1,
        "the kept commit applied with the part, its layer with it"
    );

    // after the part applied, the surface commits as usual
    let child2 = dst.compositor.as_ref().unwrap().create_surface(&dqh, ());
    let _layer2 = dst
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child2, &dock_surf, &dqh, ());
    dock_surf.commit();
    for _ in 0..2 {
        rig.pump_comp(&dconn, &mut dq, &mut dst);
    }
    assert_eq!(
        layers_of(&server_dock),
        2,
        "an unstaged commit applies at once"
    );
}

#[test]
fn participants_start_at_the_presentation_time_of_their_start_frame() {
    let mut rig = Rig::new();
    // a model clock that stands still, and a display whose last vblank came 5 ms before it
    rig.state.fixed_time = Some(20.0);
    let refresh = rig.state.refresh_of(0);
    let vblank = Duration::from_secs_f64(19.995);
    rig.state.outputs[0].scheduler.vblank(vblank, refresh);
    let presented = vblank + refresh;

    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "menubar"]);
    let (_hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");
    let dock_inst = dst.instance.unwrap();
    let (dqh, bqh) = (dq.handle(), bq.handle());
    let tx = dst
        .tx_manager
        .as_ref()
        .unwrap()
        .create(4, dst.output.as_ref().unwrap(), &dqh, ());
    tx.expect("menubar".into());
    tx.set_deadline(100);
    tx.submit();
    rig.pump_comp(&dconn, &mut dq, &mut dst);
    let tx_bar = bst.tx_manager.as_ref().unwrap().join(
        "dock".into(),
        dock_inst,
        4,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    tx_bar.submit();
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
        rig.pump_comp(&dconn, &mut dq, &mut dst);
    }
    assert_eq!(dst.started.len(), 1);
    assert_eq!(bst.started.len(), 1);
    assert_eq!(
        dst.started[0],
        wire_time(presented),
        "the start time is the presentation time of the frame the transaction starts on"
    );
    assert_eq!(
        bst.started[0], dst.started[0],
        "one start time for every participant"
    );
    assert_ne!(
        dst.started[0],
        wire_time(rig.state.frame_now()),
        "not the time of the loop turn that found it ready"
    );
}

#[test]
fn a_claimed_window_shows_in_its_hole_alone_and_fades_out_there() {
    let mut rig = Rig::headless((480, 300));
    let (wconn, mut wq, mut wst, wsurf, toplevel, wid) = rig.magenta_window((100, 80));
    let frame = rig.state.wm.window(wid).unwrap().geometry();
    let own = (
        frame.x.max(0.0) as u32,
        frame.y.max(0.0) as u32,
        frame.w as u32,
        frame.h as u32,
    );
    let hole_rect = (0, 0, 160, 120);
    assert!(
        own.0 >= hole_rect.2 || own.1 >= hole_rect.3,
        "the window's own frame {own:?} lies apart from the hole"
    );
    let image = rig.image();
    assert!(
        magenta_in(&image, own) > 1000,
        "the window shows at its own frame"
    );

    // the overview's surface at the output's top left, a hole layer over all of it claiming the window
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let _host = rig.hole_host(&oconn, &mut oq, &mut ost, wid, false);
    let image = rig.image();
    assert_eq!(
        magenta_in(&image, own),
        0,
        "a claimed window is not drawn at its own frame"
    );
    assert!(
        magenta_in(&image, hole_rect) > 1000,
        "the claimed window is drawn in its hole"
    );

    // the window closes: its last picture fades out in the hole alone, then the hole draws nothing. The model clock
    // stands still from here, so however long the loop turns take the fade is still under way when it is looked at.
    rig.state.fixed_time = Some(rig.state.now());
    rig.state.fx.cfg.closing = true;
    toplevel.destroy();
    wsurf.destroy();
    for _ in 0..3 {
        rig.pump_with(&wconn, &mut wq, &mut wst);
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(ost.source_closed, 1, "the hole hears its source closed");
    let image = rig.image();
    assert_eq!(
        magenta_in(&image, own),
        0,
        "the closing window does not fade out at its own frame"
    );
    assert!(
        magenta_in(&image, hole_rect) > 0,
        "its last picture fades out in the hole"
    );
    rig.state.fixed_time = rig.state.fixed_time.map(|t| t + PAST_CLOSING_FADE);
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let image = rig.image();
    assert_eq!(
        magenta_in(&image, hole_rect),
        0,
        "once faded out the hole draws nothing"
    );
}

#[test]
fn a_held_surface_keeps_its_hole_drawing_live() {
    let mut rig = Rig::headless((480, 300));
    let (_wconn, _wq, _wst, _wsurf, _toplevel, wid) = rig.magenta_window((100, 80));
    let frame = rig.state.wm.window(wid).unwrap().geometry();
    let own = (
        frame.x.max(0.0) as u32,
        frame.y.max(0.0) as u32,
        frame.w as u32,
        frame.h as u32,
    );
    let hole_rect = (0, 0, 160, 120);
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let host = rig.hole_host(&oconn, &mut oq, &mut ost, wid, true);
    let image = rig.image();
    assert!(
        magenta_in(&image, hole_rect) > 1000,
        "the window is drawn in its hole"
    );

    // frames of the output show the hole where it lies
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(5));
        rig.pump();
    }
    // the overview's connection ends: the server holds its persisted surface, the hole and its claim stay
    drop(host);
    drop(ost);
    drop(oq);
    drop(oconn);
    for _ in 0..6 {
        rig.pump();
    }
    assert_eq!(
        rig.state.hosts.held_surfaces().len(),
        1,
        "the surface is held"
    );
    assert!(
        rig.state.transactions.is_window_claimed(wid),
        "the held hole keeps its claim"
    );
    let image = rig.image();
    assert_eq!(magenta_in(&image, own), 0, "the window stays claimed");
    assert!(
        magenta_in(&image, hole_rect) > 1000,
        "the held hole goes on drawing the window live"
    );
}

/// The server's surface of the component's layer shell surface of namespace `namespace`.
fn server_layer_surface(
    rig: &Rig,
    namespace: &str,
) -> smithay::reexports::wayland_server::protocol::wl_surface::WlSurface {
    layer_map_for_output(&rig.state.outputs[0].output)
        .layers()
        .find(|l| l.namespace() == namespace)
        .map(|l| l.wl_surface().clone())
        .expect("the component's layer shell surface")
}

/// How many `cupertino_layers_v1` layers the server's surface `parent` has applied.
fn layers_of(
    parent: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> usize {
    cupertino_server::layers_v1::with_parent(parent, |layers| layers.len())
}

/// The dock creates transaction `serial` expecting the menu bar, which joins and stages its surface with a commit that
/// makes a layer and submits: the commit is kept. Returns the dock's transaction, the menu bar's and its layer's objects.
fn menu_bar_part_kept(
    rig: &mut Rig,
    serial: u32,
    (dconn, dq, dst): (&Connection, &mut EventQueue<CompClient>, &mut CompClient),
    (bconn, bq, bst): (&Connection, &mut EventQueue<CompClient>, &mut CompClient),
    bar_surf: &wl_surface::WlSurface,
) -> (
    CupertinoTransactionV1,
    CupertinoTransactionV1,
    wl_surface::WlSurface,
    CupertinoLayerV1,
) {
    let (dqh, bqh) = (dq.handle(), bq.handle());
    let tx =
        dst.tx_manager
            .as_ref()
            .unwrap()
            .create(serial, dst.output.as_ref().unwrap(), &dqh, ());
    tx.expect("menubar".into());
    tx.set_deadline(100);
    rig.pump_comp(dconn, dq, dst);
    let tx_bar = bst.tx_manager.as_ref().unwrap().join(
        "dock".into(),
        dst.instance.unwrap(),
        serial,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    tx_bar.stage(bar_surf);
    let child = bst.compositor.as_ref().unwrap().create_surface(&bqh, ());
    let layer = bst
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, bar_surf, &bqh, ());
    bar_surf.commit();
    tx_bar.submit();
    for _ in 0..3 {
        rig.pump_comp(bconn, bq, bst);
    }
    (tx, tx_bar, child, layer)
}

#[test]
fn a_transaction_whose_creator_withdraws_before_submit_ends_cancelled() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "menubar"]);
    let (_hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");
    let (bar_surf, _bar_layer) = rig.create_layer_surface(&bconn, &mut bq, &mut bst, "bar");
    let server_bar = server_layer_surface(&rig, "bar");
    let (tx, _tx_bar, _child, _layer) = menu_bar_part_kept(
        &mut rig,
        8,
        (&dconn, &mut dq, &mut dst),
        (&bconn, &mut bq, &mut bst),
        &bar_surf,
    );
    assert_eq!(layers_of(&server_bar), 0, "the joiner's commit is kept");
    assert!(bst.finished.is_empty());

    // the creator destroys its part before it submitted: nothing would start the transaction any more
    tx.destroy();
    for _ in 0..3 {
        rig.pump_comp(&dconn, &mut dq, &mut dst);
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(
        bst.finished,
        vec![Outcome::Cancelled],
        "the joiner hears the transaction ended as cancelled"
    );
    assert!(bst.started.is_empty(), "it never started");
    assert_eq!(
        layers_of(&server_bar),
        1,
        "the joiner's kept commit applies at once, as an ordinary commit"
    );
    // the joiner's surface is no longer staged: it commits as usual
    let bqh = bq.handle();
    let child2 = bst.compositor.as_ref().unwrap().create_surface(&bqh, ());
    let _layer2 = bst
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child2, &bar_surf, &bqh, ());
    bar_surf.commit();
    for _ in 0..2 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(layers_of(&server_bar), 2, "no commit of it is kept");
}

#[test]
fn a_transaction_whose_creator_disconnects_before_it_starts_ends_cancelled() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "menubar"]);
    let (hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");
    let (bar_surf, _bar_layer) = rig.create_layer_surface(&bconn, &mut bq, &mut bst, "bar");
    let server_bar = server_layer_surface(&rig, "bar");
    let (tx, _tx_bar, _child, _layer) = menu_bar_part_kept(
        &mut rig,
        9,
        (&dconn, &mut dq, &mut dst),
        (&bconn, &mut bq, &mut bst),
        &bar_surf,
    );
    // the creator submitted too, but its connection ends before the menu bar's part and the deadline came
    tx.submit();
    let _ = dconn.flush();
    drop(tx);
    drop(dst);
    drop(dq);
    drop(dconn);
    drop(hdock);
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(
        bst.finished,
        vec![Outcome::Cancelled],
        "the joiner hears the transaction ended as cancelled"
    );
    assert!(bst.started.is_empty(), "it never started");
    assert_eq!(
        layers_of(&server_bar),
        1,
        "the joiner's kept commit applies at once"
    );
}

/// The overview's surface of namespace `namespace`, staged in a transaction armed for a swipe up, with one layer made
/// while it is staged. Returns the transaction and the layer's objects.
fn armed_overview_part(
    rig: &mut Rig,
    (oconn, oq, ost): (&Connection, &mut EventQueue<CompClient>, &mut CompClient),
    surf: &wl_surface::WlSurface,
) -> (
    CupertinoTransactionV1,
    wl_surface::WlSurface,
    CupertinoLayerV1,
) {
    let oqh = oq.handle();
    let tx = ost
        .tx_manager
        .as_ref()
        .unwrap()
        .create(1, ost.output.as_ref().unwrap(), &oqh, ());
    tx.bind_gesture(Gesture::SwipeUp, 0.45, 1.0);
    tx.stage(surf);
    let child = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
    let layer = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, surf, &oqh, ());
    surf.commit();
    tx.submit();
    for _ in 0..4 {
        rig.pump_comp(oconn, oq, ost);
    }
    (tx, child, layer)
}

#[test]
fn an_armed_part_answers_the_frame_callbacks_of_its_kept_commits_every_frame() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let server_strip = server_layer_surface(&rig, "strip");
    let (_tx, _child, _layer) = armed_overview_part(&mut rig, (&oconn, &mut oq, &mut ost), &surf);
    let oqh = oq.handle();
    // the component draws on its frame callbacks while the transaction waits for its gesture, for long
    let mut callbacks = Vec::new();
    for drawn in 1..=KEPT_FRAMES {
        callbacks.push(surf.frame(&oqh, ()));
        surf.commit();
        for _ in 0..2 {
            rig.pump_comp(&oconn, &mut oq, &mut ost);
        }
        assert_eq!(
            ost.frames, drawn,
            "a kept commit's frame callback is answered with the output's next frame"
        );
    }
    assert_eq!(layers_of(&server_strip), 0, "every commit is kept");

    // the gesture applies the part
    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(layers_of(&server_strip), 1, "the kept commits applied");
    assert_eq!(
        ost.frames, KEPT_FRAMES,
        "every frame callback was answered once"
    );
}

#[test]
fn an_armed_part_merges_later_commits_into_its_kept_one() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let server_strip = server_layer_surface(&rig, "strip");
    let (_tx, _child, _layer) = armed_overview_part(&mut rig, (&oconn, &mut oq, &mut ost), &surf);
    let oqh = oq.handle();
    // a component that commits a new buffer each time without waiting for its frame callbacks, many times
    let size = |i: usize| (100 + i as i32, 40);
    let mut buffers = Vec::new();
    for i in 0..KEPT_BUFFERS {
        let buffer = shm_buffer(ost.shm.as_ref().unwrap(), &oqh, size(i), GREEN);
        surf.attach(Some(&buffer), 0, 0);
        surf.damage_buffer(0, 0, size(i).0, size(i).1);
        surf.commit();
        buffers.push(buffer);
        if i % 8 == 0 {
            rig.pump_comp(&oconn, &mut oq, &mut ost);
        }
    }
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(layers_of(&server_strip), 0, "every commit is kept");
    let (latest, replaced) = buffers.split_last().unwrap();
    for buffer in replaced {
        assert!(
            ost.released.contains(&buffer.id()),
            "a buffer a later kept commit replaced is released at once"
        );
    }
    assert!(
        !ost.released.contains(&latest.id()),
        "the newest buffer is kept for the part"
    );
    assert_eq!(
        ost.released.len(),
        KEPT_BUFFERS - 1,
        "each replaced buffer is released once"
    );

    // the surface is still in the part: the gesture applies the newest state
    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        layers_of(&server_strip),
        1,
        "the layer made in the first kept commit applies with the newest one"
    );
    let shown = with_renderer_surface_state(&server_strip, |s| s.buffer_size()).flatten();
    let (w, h) = size(KEPT_BUFFERS - 1);
    assert_eq!(
        shown,
        Some((w, h).into()),
        "the part's start shows the newest buffer"
    );
    assert!(
        !ost.released.contains(&latest.id()),
        "the newest buffer is the surface's"
    );
}

#[test]
fn a_hole_layer_merged_into_its_kept_commit_applies_the_newest_hole_state() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let (_tx, child, _layer) = armed_overview_part(&mut rig, (&oconn, &mut oq, &mut ost), &surf);
    let oqh = oq.handle();
    let hole = ost.tx_manager.as_ref().unwrap().get_hole(&child, &oqh, ());
    // the hole layer's kept commit claims one window, and the commits merged into it another
    let (first, newest, after) = (WindowId(41), WindowId(42), WindowId(43));
    hole.set_source(HoleWireSource::Window, first.0 as u32);
    hole.set_claim(1);
    child.commit();
    for _ in 0..KEPT_FRAMES {
        hole.set_source(HoleWireSource::Window, newest.0 as u32);
        child.commit();
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert!(
        !rig.state.transactions.is_window_claimed(first)
            && !rig.state.transactions.is_window_claimed(newest),
        "the hole layer's commits are kept"
    );

    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert!(
        rig.state.transactions.is_window_claimed(newest),
        "the part applies the newest hole state"
    );
    assert!(
        !rig.state.transactions.is_window_claimed(first),
        "the hole state a merged commit replaced never applies"
    );

    // after the part, the hole layer's next commit applies its own hole state at once
    hole.set_source(HoleWireSource::Window, after.0 as u32);
    child.commit();
    for _ in 0..2 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert!(
        rig.state.transactions.is_window_claimed(after)
            && !rig.state.transactions.is_window_claimed(newest),
        "an unstaged commit applies its own hole state"
    );
}

/// One colour in `wl_shm` ARGB8888 memory order: an opaque green, which neither the wallpaper nor a window shows.
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// How many pixels of `image` inside `rect` (`x`, `y`, `width`, `height` in pixels) show a layer's green.
fn green_in(image: &image::RgbaImage, rect: (u32, u32, u32, u32)) -> usize {
    let (x0, y0, w, h) = rect;
    let mut n = 0;
    for y in y0..(y0 + h).min(image.height()) {
        for x in x0..(x0 + w).min(image.width()) {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            if g > 180 && r < 60 && b < 60 {
                n += 1;
            }
        }
    }
    n
}

/// The largest difference of any channel between two images of the same output inside `rect` (`x`, `y`, `width`,
/// `height` in pixels).
fn largest_difference(
    a: &image::RgbaImage,
    b: &image::RgbaImage,
    rect: (u32, u32, u32, u32),
) -> u8 {
    let (x0, y0, w, h) = rect;
    let mut most = 0;
    for y in y0..(y0 + h).min(a.height()) {
        for x in x0..(x0 + w).min(a.width()) {
            let (p, q) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
            for c in 0..4 {
                most = most.max(p[c].abs_diff(q[c]));
            }
        }
    }
    most
}

/// The largest difference of a channel a held surface's picture may show from the live surface's: the rounding of its
/// content drawn into a texture and back.
const HELD_PICTURE_TOLERANCE: u8 = 4;

/// The kinds of the output's held and live layer content and backdrops, front to back, a run of one kind counted once.
fn content_and_glass_order(rig: &mut Rig) -> Vec<cupertino_scene::SceneKind> {
    use cupertino_scene::SceneKind;
    let mut backend = rig.state.backend.take().unwrap();
    let composed = rig
        .state
        .compose_output(backend.renderer(), 0, false)
        .expect("the render resources are loaded");
    rig.state.backend = Some(backend);
    let mut kinds: Vec<SceneKind> = composed
        .frame
        .items()
        .iter()
        .map(|(_, item)| item.id.kind)
        .filter(|kind| matches!(kind, SceneKind::LayerContent | SceneKind::Backdrop))
        .collect();
    kinds.dedup();
    kinds
}

/// Switch the appearance and let the model clock step past any fade it starts.
fn switch_appearance(rig: &mut Rig, appearance: cupertino_theme::Appearance) {
    rig.state.set_appearance(appearance);
    for _ in 0..4 {
        rig.state.fixed_time = Some(rig.state.now() + 1.0);
        rig.pump();
    }
}

/// The output as it shows in the light appearance and then in the dark one; the appearance is light again after it.
fn images_in_both_appearances(rig: &mut Rig) -> [image::RgbaImage; 2] {
    use cupertino_theme::Appearance;
    let images = [Appearance::Light, Appearance::Dark].map(|appearance| {
        switch_appearance(rig, appearance);
        rig.image()
    });
    switch_appearance(rig, Appearance::Light);
    images
}

#[test]
fn a_held_surface_draws_its_layer_glass_over_its_own_content_in_the_frame_s_appearance() {
    let mut rig = Rig::headless((480, 300));
    rig.state.fixed_time = Some(rig.state.now());
    let surface_rect = (0, 0, 160, 120);
    let glass_rect = (40, 30, 80, 60);
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    // a persisted surface at the output's top left drawing green all over, with one clear layer of glass over the
    // middle of it
    let qh = oq.handle();
    let parent = ost.compositor.as_ref().unwrap().create_surface(&qh, ());
    let shell_surface = ost.layer_shell.as_ref().unwrap().get_layer_surface(
        &parent,
        ost.output.as_ref(),
        zwlr_layer_shell_v1::Layer::Top,
        "strip".into(),
        &qh,
        (),
    );
    shell_surface
        .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
    shell_surface.set_size(160, 120);
    ost.component_global
        .as_ref()
        .unwrap()
        .persist(&parent, "strip".into());
    parent.commit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let green = shm_buffer(ost.shm.as_ref().unwrap(), &qh, (160, 120), GREEN);
    parent.attach(Some(&green), 0, 0);
    parent.damage_buffer(0, 0, 160, 120);
    let child = ost.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &parent, &qh, ());
    let (x, y, w, h) = glass_rect;
    for (property, value) in [
        (cupertino_layer_v1::Property::X, x),
        (cupertino_layer_v1::Property::Y, y),
        (cupertino_layer_v1::Property::Width, w),
        (cupertino_layer_v1::Property::Height, h),
    ] {
        layer.set_value(property, f64::from(value));
    }
    // regular glass (cupertino_backdrop_v1.glass_style), at its size
    layer.set_glass(0, 1.0);
    let clear = shm_buffer(ost.shm.as_ref().unwrap(), &qh, (w as i32, h as i32), CLEAR);
    child.attach(Some(&clear), 0, 0);
    child.damage_buffer(0, 0, w as i32, h as i32);
    child.commit();
    parent.commit();
    for _ in 0..6 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let live_order = content_and_glass_order(&mut rig);
    let live = images_in_both_appearances(&mut rig);
    let inside = (x + 8, y + 8, w - 16, h - 16);
    assert_eq!(
        green_in(&live[0], inside),
        0,
        "the live layer's glass lies over the surface's own content"
    );
    assert!(
        largest_difference(&live[0], &live[1], inside) > 16,
        "the live glass follows the appearance"
    );

    // the surface's layer role ends, then its connection: the server holds it
    shell_surface.destroy();
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    drop((layer, child, green, clear, parent));
    drop(ost);
    drop(oq);
    drop(oconn);
    for _ in 0..6 {
        rig.pump();
    }
    assert_eq!(
        rig.state.hosts.held_surfaces().len(),
        1,
        "the surface is held"
    );
    assert_eq!(
        content_and_glass_order(&mut rig),
        live_order,
        "the held surface draws its content and its layer's glass in the live order"
    );
    // each slice of the picture covers what it draws alone: the layer's content in front of its glass, the surface's own
    // behind it
    let slices = held_slices(&rig);
    assert_eq!(slices.len(), 2, "the picture is cut once, at the glass");
    for (slice, (bounds, what)) in slices.iter().zip([
        (glass_rect, "the layer's content"),
        (surface_rect, "the surface's own content"),
    ]) {
        let (rect, size) = slice.expect("the slice draws something");
        assert_eq!(
            (rect.w as i32, rect.h as i32),
            size,
            "the texture of {what} has a pixel per point of what it covers"
        );
        assert!(
            covers_within_margin(rect, bounds, surface_rect),
            "the slice of {what} covers its bounds {bounds:?} alone, not {rect:?}"
        );
    }
    // the appearance changes during the hold: the held glass follows it as the live glass did
    let held = images_in_both_appearances(&mut rig);
    for (appearance, (live, held)) in ["light", "dark"].into_iter().zip(live.iter().zip(&held)) {
        assert!(
            largest_difference(live, held, surface_rect) <= HELD_PICTURE_TOLERANCE,
            "the held surface shows what the live one showed in the {appearance} appearance, its layer's glass over its \
             own content"
        );
    }
}

/// Device pixels a held slice may reach past the bounds of what it draws on every side: the renderer's rounding and
/// antialiasing of their edges.
const HELD_SLICE_MARGIN: f64 = 1.0;

/// Of each slice of the picture of the output's one held surface, front to back, what it covers (logical points from
/// the surface's top-left corner) and the size of its texture in pixels; `None` for an empty slice.
fn held_slices(rig: &Rig) -> Vec<Option<(cupertino_wm::Rect, (i32, i32))>> {
    use smithay::backend::renderer::Texture;
    let held = &rig.state.hosts.held_surfaces()[0];
    let picture = held
        .picture
        .as_ref()
        .expect("the held surface has a picture");
    let cupertino_server::host::HeldContent::Texture { slices, .. } = &picture.content else {
        panic!("the renderer drew the held picture");
    };
    let snapshots = &rig.state.render.as_ref().unwrap().snapshots;
    slices
        .iter()
        .map(|slice| {
            slice.as_ref().map(|slice| {
                let (texture, _lease) = snapshots
                    .lease(slice.pixels.id())
                    .expect("the slice's texture is kept");
                let size = texture.size();
                (slice.rect, (size.w, size.h))
            })
        })
        .collect()
}

/// Whether `rect` covers `bounds` (`x`, `y`, `width`, `height`) and reaches past them by no more than
/// [`HELD_SLICE_MARGIN`], inside `within`.
fn covers_within_margin(
    rect: cupertino_wm::Rect,
    bounds: (u32, u32, u32, u32),
    within: (u32, u32, u32, u32),
) -> bool {
    let f = |(x, y, w, h): (u32, u32, u32, u32)| [x, y, x + w, y + h].map(f64::from);
    let ([x0, y0, x1, y1], [l, t, r, b]) = (f(bounds), f(within));
    let outer = [
        (x0 - HELD_SLICE_MARGIN).max(l),
        (y0 - HELD_SLICE_MARGIN).max(t),
        (x1 + HELD_SLICE_MARGIN).min(r),
        (y1 + HELD_SLICE_MARGIN).min(b),
    ];
    let got = [rect.x, rect.y, rect.right(), rect.bottom()];
    let covers = got[0] <= x0 && got[1] <= y0 && got[2] >= x1 && got[3] >= y1;
    let within_margin =
        got[0] >= outer[0] && got[1] >= outer[1] && got[2] <= outer[2] && got[3] <= outer[3];
    covers && within_margin
}

#[test]
fn a_held_surface_shows_its_layers_and_gives_way_as_the_live_one() {
    let mut rig = Rig::headless((480, 300));
    let rect = (0, 0, 160, 120);
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    // a persisted surface at the output's top left, clear, with one layer over all of it drawing green
    let qh = oq.handle();
    let parent = ost.compositor.as_ref().unwrap().create_surface(&qh, ());
    let shell_surface = ost.layer_shell.as_ref().unwrap().get_layer_surface(
        &parent,
        ost.output.as_ref(),
        zwlr_layer_shell_v1::Layer::Top,
        "strip".into(),
        &qh,
        (),
    );
    shell_surface
        .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
    shell_surface.set_size(160, 120);
    ost.component_global
        .as_ref()
        .unwrap()
        .persist(&parent, "strip".into());
    parent.commit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let clear = shm_buffer(ost.shm.as_ref().unwrap(), &qh, (160, 120), CLEAR);
    parent.attach(Some(&clear), 0, 0);
    parent.damage_buffer(0, 0, 160, 120);
    let child = ost.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &parent, &qh, ());
    layer.set_value(cupertino_layer_v1::Property::Width, 160.0);
    layer.set_value(cupertino_layer_v1::Property::Height, 120.0);
    let green = shm_buffer(ost.shm.as_ref().unwrap(), &qh, (160, 120), GREEN);
    child.attach(Some(&green), 0, 0);
    child.damage_buffer(0, 0, 160, 120);
    child.commit();
    parent.commit();
    for _ in 0..6 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let image = rig.image();
    assert!(
        green_in(&image, rect) > 1000,
        "the live surface shows its layer"
    );

    // the surface's layer role ends, then its connection: the server holds it, and the held picture shows what the live
    // surface showed, its layer's content with it
    shell_surface.destroy();
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    drop((layer, child, green, clear, parent));
    drop(ost);
    drop(oq);
    drop(oconn);
    for _ in 0..6 {
        rig.pump();
    }
    assert_eq!(
        rig.state.hosts.held_surfaces().len(),
        1,
        "the surface is held"
    );
    let image = rig.image();
    assert!(
        green_in(&image, rect) > 1000,
        "the held picture keeps the layer's content"
    );

    // a full-screen window covers the output: the held surface gives way, as the live one does
    let (wconn, mut wq, mut wst, _wsurf, _toplevel, wid) = rig.magenta_window((100, 80));
    rig.state.command(cupertino_wm::Command::Fullscreen {
        window: wid,
        output: None,
        requested_by: cupertino_wm::Requester::User,
    });
    // the model clock steps past the move into full screen
    for _ in 0..10 {
        rig.state.fixed_time = Some(rig.state.now() + 1.0);
        rig.pump_with(&wconn, &mut wq, &mut wst);
    }
    assert!(
        rig.state.wm.fullscreen_front(rig.output_id).is_some(),
        "the window covers the output"
    );
    let image = rig.image();
    assert_eq!(
        green_in(&image, rect),
        0,
        "a held surface that gives way is not drawn over a full-screen window"
    );
}

/// The kinds of everything the output draws, front to back, a run of one kind counted once.
fn scene_order(rig: &mut Rig) -> Vec<cupertino_scene::SceneKind> {
    let mut backend = rig.state.backend.take().unwrap();
    let composed = rig
        .state
        .compose_output(backend.renderer(), 0, false)
        .expect("the render resources are loaded");
    rig.state.backend = Some(backend);
    let mut kinds: Vec<_> = composed
        .frame
        .items()
        .iter()
        .map(|(_, item)| item.id.kind)
        .collect();
    kinds.dedup();
    kinds
}

#[test]
fn a_held_surface_draws_its_hole_between_its_layer_s_content_and_glass_as_live() {
    let mut rig = Rig::headless((480, 300));
    rig.state.fixed_time = Some(rig.state.now());
    let (_wconn, _wq, _wst, _wsurf, _toplevel, wid) = rig.magenta_window((100, 80));
    let surface_rect = (0, 0, 160, 120);
    let hole_rect = (20, 20, 120, 80);
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    // a persisted surface at the output's top left drawing green all over, with one layer over the middle of it that is
    // a hole claiming the window and has clear glass of its own
    let qh = oq.handle();
    let parent = ost.compositor.as_ref().unwrap().create_surface(&qh, ());
    let shell_surface = ost.layer_shell.as_ref().unwrap().get_layer_surface(
        &parent,
        ost.output.as_ref(),
        zwlr_layer_shell_v1::Layer::Top,
        "strip".into(),
        &qh,
        (),
    );
    shell_surface
        .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
    shell_surface.set_size(160, 120);
    ost.component_global
        .as_ref()
        .unwrap()
        .persist(&parent, "strip".into());
    parent.commit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let green = shm_buffer(ost.shm.as_ref().unwrap(), &qh, (160, 120), GREEN);
    parent.attach(Some(&green), 0, 0);
    parent.damage_buffer(0, 0, 160, 120);
    let child = ost.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &parent, &qh, ());
    let (x, y, w, h) = hole_rect;
    for (property, value) in [
        (cupertino_layer_v1::Property::X, x),
        (cupertino_layer_v1::Property::Y, y),
        (cupertino_layer_v1::Property::Width, w),
        (cupertino_layer_v1::Property::Height, h),
    ] {
        layer.set_value(property, f64::from(value));
    }
    // regular glass (cupertino_backdrop_v1.glass_style), at its size
    layer.set_glass(0, 1.0);
    let hole = ost.tx_manager.as_ref().unwrap().get_hole(&child, &qh, ());
    hole.set_source(HoleWireSource::Window, wid.0 as u32);
    hole.set_claim(1);
    let clear = shm_buffer(ost.shm.as_ref().unwrap(), &qh, (w as i32, h as i32), CLEAR);
    child.attach(Some(&clear), 0, 0);
    child.damage_buffer(0, 0, w as i32, h as i32);
    child.commit();
    parent.commit();
    for _ in 0..6 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    // frames of the output show the hole where it lies
    for _ in 0..4 {
        rig.pump();
    }
    let live_order = scene_order(&mut rig);
    let live = rig.image();
    let inside = (x + 8, y + 8, w - 16, h - 16);
    let area = (inside.2 * inside.3) as usize;
    assert!(
        green_in(&live, inside) < area / 4,
        "live, the window drawn in the hole covers the surface's own content, under the hole's glass"
    );

    // the surface's layer role ends, then its connection: the server holds it, the hole and its claim with it
    shell_surface.destroy();
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    drop((hole, layer, child, green, clear, parent));
    drop(ost);
    drop(oq);
    drop(oconn);
    for _ in 0..6 {
        rig.pump();
    }
    assert_eq!(
        rig.state.hosts.held_surfaces().len(),
        1,
        "the surface is held"
    );
    assert!(
        rig.state.transactions.is_window_claimed(wid),
        "the held hole keeps its claim"
    );
    assert_eq!(
        scene_order(&mut rig),
        live_order,
        "the held surface draws its layer's content, its hole and its glass in the live order"
    );
    let held = rig.image();
    assert!(
        largest_difference(&live, &held, surface_rect) <= HELD_PICTURE_TOLERANCE,
        "the held surface shows what the live one showed: the window in the hole under the hole's glass, over the \
         surface's own content"
    );
}

/// An opaque white pixel.
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn a_held_surface_screens_its_vibrant_layer_over_what_lies_behind_it_as_live() {
    use cupertino_theme::Appearance;
    let mut rig = Rig::headless((480, 300));
    rig.state.fixed_time = Some(rig.state.now());
    // over a dark face a vibrant layer's coverage is screened into what lies behind it
    switch_appearance(&mut rig, Appearance::Dark);
    let surface_rect = (0, 0, 160, 120);
    let layer_rect = (40, 30, 80, 60);
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    // a persisted clear surface at the output's top left, the wallpaper showing through it, with one vibrant layer over
    // the middle of it covering all of its bounds
    let qh = oq.handle();
    let parent = ost.compositor.as_ref().unwrap().create_surface(&qh, ());
    let shell_surface = ost.layer_shell.as_ref().unwrap().get_layer_surface(
        &parent,
        ost.output.as_ref(),
        zwlr_layer_shell_v1::Layer::Top,
        "strip".into(),
        &qh,
        (),
    );
    shell_surface
        .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
    shell_surface.set_size(160, 120);
    ost.component_global
        .as_ref()
        .unwrap()
        .persist(&parent, "strip".into());
    parent.commit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let clear = shm_buffer(ost.shm.as_ref().unwrap(), &qh, (160, 120), CLEAR);
    parent.attach(Some(&clear), 0, 0);
    parent.damage_buffer(0, 0, 160, 120);
    let child = ost.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &parent, &qh, ());
    let (x, y, w, h) = layer_rect;
    for (property, value) in [
        (cupertino_layer_v1::Property::X, x),
        (cupertino_layer_v1::Property::Y, y),
        (cupertino_layer_v1::Property::Width, w),
        (cupertino_layer_v1::Property::Height, h),
    ] {
        layer.set_value(property, f64::from(value));
    }
    layer.set_blend(cupertino_layer_v1::Blend::Vibrant);
    let white = shm_buffer(ost.shm.as_ref().unwrap(), &qh, (w as i32, h as i32), WHITE);
    child.attach(Some(&white), 0, 0);
    child.damage_buffer(0, 0, w as i32, h as i32);
    child.commit();
    parent.commit();
    for _ in 0..6 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let live = rig.image();
    let inside = (x + 8, y + 8, w - 16, h - 16);
    let beside = (x + w + 8, y + 8, w - 16, h - 16);
    let [lit, plain] = [inside, beside].map(|(px, py, ..)| live.get_pixel(px, py).0);
    assert!(
        lit[..3].iter().zip(&plain[..3]).all(|(l, p)| l > p),
        "the live vibrant layer lifts the wallpaper behind it: {lit:?} over {plain:?}"
    );
    assert!(
        lit[..3].iter().any(|&c| c < 250),
        "the lift is screened in, short of white: {lit:?}"
    );

    // the surface's layer role ends, then its connection: the server holds it
    shell_surface.destroy();
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    drop((layer, child, white, clear, parent));
    drop(ost);
    drop(oq);
    drop(oconn);
    for _ in 0..6 {
        rig.pump();
    }
    assert_eq!(
        rig.state.hosts.held_surfaces().len(),
        1,
        "the surface is held"
    );
    let held = rig.image();
    let most = largest_difference(&live, &held, surface_rect);
    assert!(
        most <= HELD_PICTURE_TOLERANCE,
        "the held vibrant layer is screened over the wallpaper as the live one was, not added to it (largest \
         difference {most})"
    );
    switch_appearance(&mut rig, Appearance::Light);
}
