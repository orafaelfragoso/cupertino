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
    /// The serials of the layer animations the server answered `replaced`, in the order it did.
    layer_replaced: Vec<u32>,
    /// What the server answered `replaced` with, in the order it did: the serial, the value and the velocity.
    layer_replaced_with: Vec<(u32, f64, f64)>,
    /// The serials of the layer animations the server answered `finished`, in the order it did.
    layer_finished: Vec<u32>,
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
        state: &mut Self,
        _: &CupertinoLayerV1,
        event: cupertino_layer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_layer_v1::Event::Replaced {
                serial,
                value,
                velocity,
            } => {
                state.layer_replaced.push(serial);
                state.layer_replaced_with.push((serial, value, velocity));
            }
            cupertino_layer_v1::Event::Finished { serial } => state.layer_finished.push(serial),
            _ => {}
        }
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

/// Commits of an armed part that each animate the same layer property: more than a client may have animations.
const ANIMATED_COMMITS: u32 = 1000;

#[test]
fn an_armed_part_keeps_the_newest_animation_of_a_layer_property_however_often_it_commits() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let server_strip = server_layer_surface(&rig, "strip");
    let (_tx, _child, layer) = armed_overview_part(&mut rig, (&oconn, &mut oq, &mut ost), &surf);
    // a component that moves a layer with each commit while its part waits for the gesture
    for serial in 0..ANIMATED_COMMITS {
        layer.animate_timed(
            serial,
            cupertino_layer_v1::Property::X,
            f64::from(serial),
            200,
            0.25,
            0.1,
            0.25,
            1.0,
        );
        surf.commit();
        if serial % 16 == 0 {
            rig.pump_comp(&oconn, &mut oq, &mut ost);
        }
    }
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert!(
        oconn.protocol_error().is_none(),
        "the kept animations stay under the client's limit"
    );
    assert_eq!(layers_of(&server_strip), 0, "every commit is kept");
    let newest = ANIMATED_COMMITS - 1;
    assert_eq!(
        ost.layer_replaced,
        (0..newest).collect::<Vec<_>>(),
        "each animation a later one superseded before it applied is answered replaced, once"
    );

    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let target =
        cupertino_server::layers_v1::with_parent(&server_strip, |layers| layers[0].target(0));
    assert_eq!(
        target,
        f64::from(newest),
        "the part applies the newest animation"
    );
    assert!(
        !ost.layer_replaced.contains(&newest),
        "the newest animation runs"
    );
}

#[test]
fn an_armed_part_applies_its_merged_layer_requests_as_one_commit_after_another() {
    use cupertino_layer_v1::Property;
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let server_strip = server_layer_surface(&rig, "strip");
    let (_tx, _child, layer) = armed_overview_part(&mut rig, (&oconn, &mut oq, &mut ost), &surf);
    let timed = |serial: u32, property: Property, to: f64| {
        layer.animate_timed(serial, property, to, 200, 0.25, 0.1, 0.25, 1.0);
    };
    // a value, an animation from it, and a later value that replaces the animation
    layer.set_value(Property::Opacity, 0.5);
    surf.commit();
    timed(7, Property::Opacity, 1.0);
    surf.commit();
    layer.set_value(Property::Opacity, 0.25);
    surf.commit();
    // an animation a later cancel stops
    timed(8, Property::Y, 30.0);
    surf.commit();
    layer.cancel(Property::Y);
    surf.commit();
    // a value, then an animation from it
    layer.set_value(Property::X, 10.0);
    surf.commit();
    timed(9, Property::X, 40.0);
    surf.commit();
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(layers_of(&server_strip), 0, "every commit is kept");
    assert_eq!(
        ost.layer_replaced,
        vec![7, 8],
        "the animations a later value or cancel replaced are answered replaced"
    );

    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let targets = cupertino_server::layers_v1::with_parent(&server_strip, |layers| {
        [4, 1, 0].map(|prop| layers[0].target(prop))
    });
    assert_eq!(
        targets,
        [0.25, 0.0, 40.0],
        "the opacity rests at its newest value, y where it was, and x animates to its newest target"
    );
    assert_eq!(ost.layer_replaced, vec![7, 8], "the kept animation runs");
}

/// Commits of an armed part that each damage a pixel of their own: far more than a kept state holds rectangles.
const DAMAGED_COMMITS: i32 = 1000;

#[test]
fn an_armed_part_keeps_the_damage_of_its_merged_commits_bounded() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let server_strip = server_layer_surface(&rig, "strip");
    let before = with_renderer_surface_state(&server_strip, |s| s.current_commit())
        .expect("the surface's renderer state");
    let (_tx, _child, _layer) = armed_overview_part(&mut rig, (&oconn, &mut oq, &mut ost), &surf);
    let oqh = oq.handle();
    let buffer = shm_buffer(ost.shm.as_ref().unwrap(), &oqh, (200, 40), GREEN);
    surf.attach(Some(&buffer), 0, 0);
    let pixel = |i: i32| (i % 200, i / 200 * 8);
    for i in 0..DAMAGED_COMMITS {
        let (x, y) = pixel(i);
        surf.damage_buffer(x, y, 1, 1);
        surf.commit();
        if i % 16 == 0 {
            rig.pump_comp(&oconn, &mut oq, &mut ost);
        }
    }
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let damage =
        with_renderer_surface_state(&server_strip, |s| s.damage_since(Some(before)).to_vec())
            .expect("the damage of the part's commit");
    assert!(
        !damage.iter().any(|r| r.contains((0, 39))),
        "the damage is the commits', not the whole buffer"
    );
    assert!(
        !damage.is_empty() && damage.len() <= smithay::wayland::compositor::MAX_COALESCED_DAMAGE,
        "the merged commits keep at most a few rectangles, not one each: {}",
        damage.len()
    );
    for i in 0..DAMAGED_COMMITS {
        assert!(
            damage.iter().any(|r| r.contains(pixel(i))),
            "the kept damage covers every damaged pixel"
        );
    }
}

#[test]
fn a_transaction_cancelled_after_its_part_applied_leaves_its_surface_to_a_newer_one() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let server_strip = server_layer_surface(&rig, "strip");
    let (first, _child, _layer) = armed_overview_part(&mut rig, (&oconn, &mut oq, &mut ost), &surf);
    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(layers_of(&server_strip), 1, "the first part applied");

    // the overview stages the surface again, in a transaction armed for another gesture, with a commit it keeps
    let oqh = oq.handle();
    let newer = ost
        .tx_manager
        .as_ref()
        .unwrap()
        .create(2, ost.output.as_ref().unwrap(), &oqh, ());
    newer.bind_gesture(Gesture::Pinch, 0.45, 1.0);
    newer.stage(&surf);
    let child = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
    let _layer2 = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &surf, &oqh, ());
    surf.commit();
    newer.submit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        layers_of(&server_strip),
        1,
        "the newer part keeps its commit"
    );

    // the first transaction's creator cancels it once its part applied
    first.cancel();
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        ost.finished,
        vec![Outcome::Cancelled],
        "the first one ends cancelled"
    );
    assert_eq!(
        layers_of(&server_strip),
        1,
        "the surface stays staged in the newer transaction, its commit kept"
    );
    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::Pinch
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        layers_of(&server_strip),
        2,
        "the newer part applies its commit with its gesture"
    );
}

#[test]
fn a_value_or_cancel_answers_the_animations_still_waiting_to_start_replaced() {
    use cupertino_layer_v1::Property;
    let mut rig = Rig::new();
    // a model clock that stands still, and no frame that shows a commit: an applied animation waits to start
    rig.state.fixed_time = Some(30.0);
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let server_strip = server_layer_surface(&rig, "strip");
    let oqh = oq.handle();
    let child = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
    let layer = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &surf, &oqh, ());
    surf.commit();
    for _ in 0..2 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(layers_of(&server_strip), 1);
    let timed = |serial: u32, to: f64| {
        layer.animate_timed(serial, Property::X, to, 200, 0.25, 0.1, 0.25, 1.0);
    };

    // an animation that waits to start, then a value of its property
    timed(3, 40.0);
    surf.commit();
    rig.pump_comp(&oconn, &mut oq, &mut ost);
    let waiting = |rig: &Rig| {
        cupertino_server::layers_v1::with_parent(&server_strip, |layers| {
            layers[0].animating(rig.state.now())
        })
    };
    assert!(waiting(&rig), "the animation waits for a frame to show it");
    layer.set_value(Property::X, 10.0);
    surf.commit();
    for _ in 0..2 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        ost.layer_replaced_with,
        vec![(3, 0.0, 0.0)],
        "the value replaces the waiting animation, answered with what was shown"
    );

    // another, then a cancel
    timed(4, 80.0);
    surf.commit();
    rig.pump_comp(&oconn, &mut oq, &mut ost);
    assert!(waiting(&rig), "the animation waits for a frame to show it");
    layer.cancel(Property::X);
    surf.commit();
    for _ in 0..2 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        ost.layer_replaced_with,
        vec![(3, 0.0, 0.0), (4, 10.0, 0.0)],
        "the cancel replaces the waiting animation, answered with what was shown"
    );
    assert!(!waiting(&rig), "nothing waits or runs any more");
    let target =
        cupertino_server::layers_v1::with_parent(&server_strip, |layers| layers[0].target(0));
    assert_eq!(target, 10.0, "x rests where it was shown");
}

/// `surf`'s component creates transaction `serial` and stages `surf` in it with a commit that makes a layer, a commit
/// the transaction keeps until it applies. With `armed` (the overview) it is armed for a pinch, otherwise it is not
/// submitted yet. Returns the transaction and the layer's objects.
fn newer_part_kept(
    rig: &mut Rig,
    serial: u32,
    armed: bool,
    (conn, q, st): (&Connection, &mut EventQueue<CompClient>, &mut CompClient),
    surf: &wl_surface::WlSurface,
) -> (
    CupertinoTransactionV1,
    wl_surface::WlSurface,
    CupertinoLayerV1,
) {
    let qh = q.handle();
    let tx = st
        .tx_manager
        .as_ref()
        .unwrap()
        .create(serial, st.output.as_ref().unwrap(), &qh, ());
    if armed {
        tx.bind_gesture(Gesture::Pinch, 0.45, 1.0);
    }
    tx.stage(surf);
    let child = st.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer = st.layers.as_ref().unwrap().get_layer(&child, surf, &qh, ());
    surf.commit();
    if armed {
        tx.submit();
    }
    for _ in 0..2 {
        rig.pump_comp(conn, q, st);
    }
    (tx, child, layer)
}

#[test]
fn a_joiner_that_cancels_after_its_part_started_leaves_its_surface_to_a_newer_transaction() {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["dock", "overview"]);
    let (_hdock, dconn, mut dq, mut dst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "dock");
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (strip, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let server_strip = server_layer_surface(&rig, "strip");
    let (dqh, oqh) = (dq.handle(), oq.handle());

    // the dock's transaction, which the overview joins with its surface staged and a commit that makes a layer
    let tx = dst
        .tx_manager
        .as_ref()
        .unwrap()
        .create(1, dst.output.as_ref().unwrap(), &dqh, ());
    tx.expect("overview".into());
    tx.set_deadline(100);
    rig.pump_comp(&dconn, &mut dq, &mut dst);
    let joined = ost.tx_manager.as_ref().unwrap().join(
        "dock".into(),
        dst.instance.unwrap(),
        1,
        ost.output.as_ref().unwrap(),
        &oqh,
        (),
    );
    joined.stage(&strip);
    let child = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
    let _layer = ost
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &strip, &oqh, ());
    strip.commit();
    joined.submit();
    tx.submit();
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
        rig.pump_comp(&dconn, &mut dq, &mut dst);
    }
    assert_eq!(ost.started.len(), 1, "the transaction started");
    assert_eq!(layers_of(&server_strip), 1, "the overview's part applied");

    // the overview stages its surface again, in a transaction armed for a pinch, and withdraws from the first one
    let (_newer, _child2, _layer2) =
        newer_part_kept(&mut rig, 2, true, (&oconn, &mut oq, &mut ost), &strip);
    assert_eq!(
        layers_of(&server_strip),
        1,
        "the newer part keeps its commit"
    );
    joined.cancel();
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        layers_of(&server_strip),
        1,
        "the surface stays staged in the newer transaction, its commit kept"
    );
    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::Pinch
    ));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(
        layers_of(&server_strip),
        2,
        "the newer part applies its commit with its gesture"
    );
}

#[test]
fn a_joiner_that_cancels_after_its_part_applied_with_a_gesture_leaves_its_surface_to_a_newer_transaction(
) {
    let mut rig = Rig::new();
    let (hconn, mut hq, mut hst) = rig.connect_host(&["overview", "menubar"]);
    let (_hov, oconn, mut oq, mut ost) = rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");
    let (strip, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
    let (bar_surf, _bar_shell) = rig.create_layer_surface(&bconn, &mut bq, &mut bst, "bar");
    let server_bar = server_layer_surface(&rig, "bar");
    let (_armed, _child, _layer) =
        armed_overview_part(&mut rig, (&oconn, &mut oq, &mut ost), &strip);

    // the menu bar joins the armed transaction with its surface staged and a commit that makes a layer
    let bqh = bq.handle();
    let joined = bst.tx_manager.as_ref().unwrap().join(
        "overview".into(),
        ost.instance.unwrap(),
        1,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    joined.stage(&bar_surf);
    let child = bst.compositor.as_ref().unwrap().create_surface(&bqh, ());
    let _layer = bst
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &bar_surf, &bqh, ());
    bar_surf.commit();
    joined.submit();
    for _ in 0..3 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(layers_of(&server_bar), 0, "the joiner's commit is kept");
    assert!(rig.state.transaction_gesture_begin(
        rig.output_id,
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
    ));
    for _ in 0..3 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(
        layers_of(&server_bar),
        1,
        "the joiner's part applied with the gesture"
    );

    // the menu bar stages its surface again, in a transaction of its own, and withdraws from the gesture's
    let (newer, _child2, _layer2) =
        newer_part_kept(&mut rig, 2, false, (&bconn, &mut bq, &mut bst), &bar_surf);
    assert_eq!(layers_of(&server_bar), 1, "the newer part keeps its commit");
    joined.cancel();
    for _ in 0..2 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(
        layers_of(&server_bar),
        1,
        "the surface stays staged in the newer transaction, its commit kept"
    );
    newer.submit();
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert_eq!(
        layers_of(&server_bar),
        2,
        "the newer part applies its commit when it starts"
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

/// How far the gesture-bound part moves its layer along x, in points, and the animation's serial.
const GESTURE_TRAVEL: f64 = 600.0;
const MOVE_SERIAL: u32 = 1;
/// The serial of the part's fade in.
const FADE_SERIAL: u32 = 2;
/// The milliseconds of the layer's own motion, the longest the layer protocol takes: on its own clock the layer would
/// move a tenth of the way in a second, so only the gesture's progress takes it far.
const SLOW_MS: u32 = 10_000;
/// The model time the gesture tests start at, in seconds.
const GESTURE_T0: f64 = 10.0;

/// The model's clock stands at `t` (seconds): what the layer protocol starts and shows counts from it.
fn at_time(rig: &mut Rig, t: f64) {
    rig.state.fixed_time = Some(t);
}

/// A server with an overview component whose transaction 1 is armed for the swipe up (response 0.45 s, critically
/// damped), and with `reverse` for the swipe down back, its strip staged with a layer that moves [`GESTURE_TRAVEL`]
/// points along x and fades in from 0 over [`SLOW_MS`]; on a fixed clock at [`GESTURE_T0`].
struct GestureRig {
    rig: Rig,
    tx: CupertinoTransactionV1,
    /// The server's strip surface, the layer's parent.
    strip: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    oconn: Connection,
    oq: EventQueue<CompClient>,
    ost: CompClient,
    /// The host, which starts the other components.
    host: (Connection, EventQueue<HostClient>, HostClient),
    _layer: CupertinoLayerV1,
}

impl GestureRig {
    fn new(reduce_motion: bool, reverse: bool) -> Self {
        use cupertino_layer_v1::Property;
        let mut rig = Rig::new();
        at_time(&mut rig, GESTURE_T0);
        rig.state.set_display_option(
            cupertino_defaults::accessibility::DisplayOption::ReduceMotion,
            reduce_motion,
        );
        let (hconn, mut hq, mut hst) = rig.connect_host(&["overview", "menubar"]);
        let (_hov, oconn, mut oq, mut ost) =
            rig.spawn_component(&hconn, &mut hq, &mut hst, "overview");
        let (surf, _shell) = rig.create_layer_surface(&oconn, &mut oq, &mut ost, "strip");
        let strip = server_layer_surface(&rig, "strip");
        let oqh = oq.handle();
        let tx = ost
            .tx_manager
            .as_ref()
            .unwrap()
            .create(1, ost.output.as_ref().unwrap(), &oqh, ());
        tx.bind_gesture(Gesture::SwipeUp, 0.45, 1.0);
        if reverse {
            tx.bind_reverse_gesture(Gesture::SwipeDown);
        }
        tx.stage(&surf);
        let child = ost.compositor.as_ref().unwrap().create_surface(&oqh, ());
        let layer = ost
            .layers
            .as_ref()
            .unwrap()
            .get_layer(&child, &surf, &oqh, ());
        layer.animate_timed(
            MOVE_SERIAL,
            Property::X,
            GESTURE_TRAVEL,
            SLOW_MS,
            0.0,
            0.0,
            1.0,
            1.0,
        );
        layer.set_value(Property::Opacity, 0.0);
        layer.animate_timed(
            FADE_SERIAL,
            Property::Opacity,
            1.0,
            SLOW_MS,
            0.0,
            0.0,
            1.0,
            1.0,
        );
        surf.commit();
        tx.submit();
        for _ in 0..4 {
            rig.pump_comp(&oconn, &mut oq, &mut ost);
        }
        assert_eq!(layers_of(&strip), 0, "the armed part's commits are kept");
        Self {
            rig,
            tx,
            strip,
            oconn,
            oq,
            ost,
            host: (hconn, hq, hst),
            _layer: layer,
        }
    }

    /// Turn the loop `n` times with the overview's connection.
    fn pump(&mut self, n: usize) {
        for _ in 0..n {
            self.rig.pump_comp(&self.oconn, &mut self.oq, &mut self.ost);
        }
    }

    /// The strip's layer's x and opacity as a frame built at `now` shows them.
    fn shown(&self, now: f64) -> (f64, f64) {
        shown_layer(&self.rig, &self.strip, now)
    }

    /// The fingers lift with `velocity` at the clock's time, and the transaction comes to rest by then + 3 s.
    fn release(&mut self, completed: bool, velocity: f64) -> f64 {
        let output = self.rig.output_id;
        assert!(self
            .rig
            .state
            .transaction_gesture_end(output, completed, velocity));
        let later = self.rig.state.now() + 3.0;
        at_time(&mut self.rig, later);
        self.pump(4);
        later
    }
}

/// A gesture-bound overview part whose layer moves and fades in ([`GestureRig`]), without a reverse binding; returns the
/// rig, the transaction and layer objects, the server's surface and the component's connection, queue and client.
fn gesture_moved_layer(
    reduce_motion: bool,
) -> (
    Rig,
    CupertinoTransactionV1,
    CupertinoLayerV1,
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    (Connection, EventQueue<CompClient>, CompClient),
) {
    let g = GestureRig::new(reduce_motion, false);
    (g.rig, g.tx, g._layer, g.strip, (g.oconn, g.oq, g.ost))
}

/// The layer's x and opacity as a frame built at `now` shows them.
fn shown_layer(
    rig: &Rig,
    strip: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    now: f64,
) -> (f64, f64) {
    let reduce_motion = rig.state.display_options().reduce_motion;
    let drawn = cupertino_server::layers_v1::present(strip, now, reduce_motion);
    assert_eq!(drawn.len(), 1, "the part's layer shows");
    (drawn[0].x, drawn[0].opacity)
}

fn swipe_up() -> cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture {
    cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeUp
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn a_bound_gesture_s_progress_moves_the_layers_its_part_animates_while_the_fingers_move() {
    let (mut rig, _tx, _layer, strip, (oconn, mut oq, mut ost)) = gesture_moved_layer(false);
    assert!(rig
        .state
        .transaction_gesture_begin(rig.output_id, swipe_up()));
    assert!(rig.state.transaction_gesture_update(rig.output_id, 0.5));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert_eq!(ost.gesture_began, vec![Gesture::SwipeUp]);
    let (x, opacity) = shown_layer(&rig, &strip, GESTURE_T0);
    assert!(
        close(x, GESTURE_TRAVEL / 2.0) && close(opacity, 0.5),
        "at progress 0.5 every tracked property is half way: x {x}, opacity {opacity}"
    );

    // a second later the fingers went back a little: the progress, not the layer's clock, places it
    at_time(&mut rig, GESTURE_T0 + 1.0);
    assert!(rig.state.transaction_gesture_update(rig.output_id, 0.25));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let (x, opacity) = shown_layer(&rig, &strip, GESTURE_T0 + 1.0);
    assert!(
        close(x, GESTURE_TRAVEL / 4.0) && close(opacity, 0.25),
        "start + progress x (target - start): x {x}, opacity {opacity}"
    );
    let target = cupertino_server::layers_v1::with_parent(&strip, |layers| layers[0].target(0));
    assert_eq!(
        target, GESTURE_TRAVEL,
        "the client hit-tests against the target"
    );
    assert!(
        ost.layer_finished.is_empty() && ost.layer_replaced.is_empty(),
        "the tracked animations run"
    );
    assert!(
        !rig.state.layers.settled(GESTURE_T0 + 1.0),
        "what shows is not the targets while the gesture moves the layer"
    );
}

#[test]
fn a_bound_gesture_released_past_its_threshold_settles_its_layers_to_their_targets_with_its_velocity(
) {
    // the layer's x 40 ms after a release at progress 0.6 with `velocity`, and what shows and was heard at rest
    let release = |velocity: f64| {
        let (mut rig, _tx, _layer, strip, (oconn, mut oq, mut ost)) = gesture_moved_layer(false);
        assert!(rig
            .state
            .transaction_gesture_begin(rig.output_id, swipe_up()));
        assert!(rig.state.transaction_gesture_update(rig.output_id, 0.6));
        for _ in 0..3 {
            rig.pump_comp(&oconn, &mut oq, &mut ost);
        }
        assert!(rig
            .state
            .transaction_gesture_end(rig.output_id, true, velocity));
        rig.pump_comp(&oconn, &mut oq, &mut ost);
        let soon = GESTURE_T0 + 0.04;
        let progress = rig
            .state
            .transactions
            .gesture_progress_on(rig.output_id, soon)
            .expect("the progress settles");
        let (x, _) = shown_layer(&rig, &strip, soon);
        assert!(
            close(x, progress * GESTURE_TRAVEL),
            "the layer follows the settling progress: x {x}, progress {progress}"
        );
        assert!(ost.finished.is_empty(), "not at rest yet");

        let deadline = rig
            .state
            .transaction_deadline()
            .expect("the settle's end is a deadline");
        assert!(
            deadline > GESTURE_T0 && deadline < GESTURE_T0 + 3.0,
            "the settle ends at {deadline}"
        );
        at_time(&mut rig, GESTURE_T0 + 3.0);
        for _ in 0..4 {
            rig.pump_comp(&oconn, &mut oq, &mut ost);
        }
        let (rest, opacity) = shown_layer(&rig, &strip, GESTURE_T0 + 3.0);
        (x, rest, opacity, ost)
    };
    let (slow, rest, opacity, ost) = release(0.0);
    let (fast, ..) = release(6.0);
    assert!(
        fast > slow + 0.05 * GESTURE_TRAVEL,
        "the gesture's velocity carries the layer further along (fast {fast}, slow {slow})"
    );
    assert!(
        close(rest, GESTURE_TRAVEL) && close(opacity, 1.0),
        "at rest every property is at its target: x {rest}, opacity {opacity}"
    );
    assert_eq!(ost.gesture_ended, vec![Outcome::Completed]);
    assert_eq!(ost.finished, vec![Outcome::Completed]);
    let mut finished = ost.layer_finished.clone();
    finished.sort_unstable();
    assert_eq!(
        finished,
        vec![MOVE_SERIAL, FADE_SERIAL],
        "each animation finished"
    );
    assert!(ost.layer_replaced.is_empty(), "no animation was replaced");
}

#[test]
fn a_bound_gesture_released_short_of_its_threshold_settles_its_layers_back_to_their_start() {
    let (mut rig, _tx, _layer, strip, (oconn, mut oq, mut ost)) = gesture_moved_layer(false);
    assert!(rig
        .state
        .transaction_gesture_begin(rig.output_id, swipe_up()));
    assert!(rig.state.transaction_gesture_update(rig.output_id, 0.3));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    assert!(close(
        shown_layer(&rig, &strip, GESTURE_T0).0,
        0.3 * GESTURE_TRAVEL
    ));
    assert!(rig
        .state
        .transaction_gesture_end(rig.output_id, false, -1.0));
    at_time(&mut rig, GESTURE_T0 + 3.0);
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let (x, opacity) = shown_layer(&rig, &strip, GESTURE_T0 + 3.0);
    assert!(
        close(x, 0.0) && close(opacity, 0.0),
        "reverted, the layer is back at its start: x {x}, opacity {opacity}"
    );
    assert_eq!(ost.gesture_ended, vec![Outcome::Reverted]);
    assert_eq!(ost.finished, vec![Outcome::Reverted]);
    let mut replaced = ost.layer_replaced_with.clone();
    replaced.sort_by_key(|r| r.0);
    assert_eq!(
        replaced,
        vec![(MOVE_SERIAL, 0.0, 0.0), (FADE_SERIAL, 0.0, 0.0)],
        "each animation is answered replaced at its start"
    );
    let targets = cupertino_server::layers_v1::with_parent(&strip, |layers| {
        [0, 4].map(|prop| layers[0].target(prop))
    });
    assert_eq!(
        targets,
        [0.0, 0.0],
        "the client hit-tests against the start again"
    );
    assert!(
        rig.state.layers.settled(GESTURE_T0 + 3.0),
        "what shows is the targets"
    );
    // the rest of the gesture's events are the transaction's, and the next swipe goes to the dock
    assert!(!rig
        .state
        .transaction_gesture_begin(rig.output_id, swipe_up()));
}

#[test]
fn under_reduced_motion_a_bound_gesture_s_progress_drives_opacity_alone() {
    let (mut rig, _tx, _layer, strip, (oconn, mut oq, mut ost)) = gesture_moved_layer(true);
    assert!(rig
        .state
        .transaction_gesture_begin(rig.output_id, swipe_up()));
    assert!(rig.state.transaction_gesture_update(rig.output_id, 0.5));
    for _ in 0..3 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let (x, opacity) = shown_layer(&rig, &strip, GESTURE_T0);
    assert!(
        close(x, 0.0) && close(opacity, 0.5),
        "the layer fades with the progress and stays where it started: x {x}, opacity {opacity}"
    );
    assert!(rig.state.transaction_gesture_end(rig.output_id, true, 0.0));
    rig.pump_comp(&oconn, &mut oq, &mut ost);
    let (x, _) = shown_layer(&rig, &strip, GESTURE_T0 + 0.04);
    assert!(
        close(x, 0.0),
        "the layer stays at its start while the progress settles: x {x}"
    );
    at_time(&mut rig, GESTURE_T0 + 3.0);
    for _ in 0..4 {
        rig.pump_comp(&oconn, &mut oq, &mut ost);
    }
    let (x, opacity) = shown_layer(&rig, &strip, GESTURE_T0 + 3.0);
    assert!(
        close(x, GESTURE_TRAVEL) && close(opacity, 1.0),
        "at rest at 1 the spatial property takes its target: x {x}, opacity {opacity}"
    );
    assert_eq!(ost.finished, vec![Outcome::Completed]);
}

#[test]
fn a_reverse_gesture_drives_a_completed_transaction_s_layers_back_to_where_the_first_gesture_began()
{
    let swipe_down =
        cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture::SwipeDown;
    let mut g = GestureRig::new(false, true);
    let output = g.rig.output_id;
    assert!(g.rig.state.transaction_gesture_begin(output, swipe_up()));
    assert!(g.rig.state.transaction_gesture_update(output, 0.8));
    g.pump(3);
    let rest = g.release(true, 0.0);
    assert_eq!(g.ost.finished, vec![Outcome::Completed]);
    assert!(close(g.shown(rest).0, GESTURE_TRAVEL), "at rest completed");
    let mut finished = g.ost.layer_finished.clone();
    finished.sort_unstable();
    assert_eq!(
        finished,
        vec![MOVE_SERIAL, FADE_SERIAL],
        "at rest at 1 the animations finished"
    );
    assert!(
        g.rig.state.layers.settled(rest),
        "what shows at rest is the targets"
    );

    // a reverse gesture half way down: the properties are half way back
    assert!(g.rig.state.transaction_gesture_begin(output, swipe_down));
    assert!(g.rig.state.transaction_gesture_update(output, 0.5));
    g.pump(3);
    let (x, opacity) = g.shown(rest);
    assert!(
        close(x, GESTURE_TRAVEL / 2.0) && close(opacity, 0.5),
        "the reverse gesture drives the same properties back: x {x}, opacity {opacity}"
    );
    // released short, it settles to the rest completed again and stays for the reverse gesture
    let rest = g.release(false, 0.0);
    assert_eq!(
        g.ost.gesture_ended,
        vec![Outcome::Completed, Outcome::Completed]
    );
    assert_eq!(g.ost.finished, vec![Outcome::Completed, Outcome::Completed]);
    assert!(
        close(g.shown(rest).0, GESTURE_TRAVEL),
        "back at rest completed"
    );

    // a reverse gesture that goes through takes the layer back to its start, and the transaction is over
    assert!(g.rig.state.transaction_gesture_begin(output, swipe_down));
    assert!(g.rig.state.transaction_gesture_update(output, 0.9));
    g.pump(3);
    let rest = g.release(true, 0.0);
    assert_eq!(g.ost.gesture_ended.last(), Some(&Outcome::Reverted));
    assert_eq!(g.ost.finished.last(), Some(&Outcome::Reverted));
    let (x, opacity) = g.shown(rest);
    assert!(
        close(x, 0.0) && close(opacity, 0.0),
        "back where the first gesture began: x {x}, opacity {opacity}"
    );
    let targets = cupertino_server::layers_v1::with_parent(&g.strip, |layers| {
        [0, 4].map(|prop| layers[0].target(prop))
    });
    assert_eq!(targets, [0.0, 0.0], "the properties rest at their start");
    assert!(
        !g.rig.state.transaction_gesture_begin(output, swipe_down),
        "the reverse binding ended with the transaction"
    );
}

#[test]
fn a_bound_gesture_cancelled_under_way_settles_back_and_keeps_the_rest_of_the_gesture() {
    let mut g = GestureRig::new(false, false);
    let output = g.rig.output_id;
    assert!(g.rig.state.transaction_gesture_begin(output, swipe_up()));
    assert!(g.rig.state.transaction_gesture_update(output, 0.5));
    g.pump(3);
    g.tx.cancel();
    g.pump(2);
    assert!(g.ost.finished.is_empty(), "the progress settles back first");
    let later = GESTURE_T0 + 3.0;
    at_time(&mut g.rig, later);
    g.pump(4);
    assert_eq!(g.ost.finished, vec![Outcome::Cancelled]);
    let (x, opacity) = g.shown(later);
    assert!(
        close(x, 0.0) && close(opacity, 0.0),
        "the layer is back at its start: x {x}, opacity {opacity}"
    );
    let mut replaced = g.ost.layer_replaced_with.clone();
    replaced.sort_by_key(|r| r.0);
    assert_eq!(
        replaced,
        vec![(MOVE_SERIAL, 0.0, 0.0), (FADE_SERIAL, 0.0, 0.0)],
        "each animation is answered replaced at its start"
    );
    // the fingers go on and lift past the threshold: the gesture stays the cancelled transaction's, not the dock's
    assert!(g.rig.state.transaction_gesture_update(output, 0.9));
    assert!(g.rig.state.transaction_gesture_end(output, true, 2.0));
    g.pump(3);
    assert!(close(g.shown(later).0, 0.0), "nothing moves the layer");
    assert!(
        g.ost.gesture_ended.is_empty(),
        "the transaction hears nothing more"
    );
}

#[test]
fn a_part_ready_while_a_bound_gesture_is_under_way_joins_at_the_progress_shown() {
    use cupertino_layer_v1::Property;
    let mut g = GestureRig::new(false, false);
    let output = g.rig.output_id;
    let (hconn, hq, hst) = &mut g.host;
    let (_hbar, bconn, mut bq, mut bst) = g.rig.spawn_component(hconn, hq, hst, "menubar");
    let (bar_surf, _bar_shell) = g.rig.create_layer_surface(&bconn, &mut bq, &mut bst, "bar");
    let server_bar = server_layer_surface(&g.rig, "bar");
    assert!(g.rig.state.transaction_gesture_begin(output, swipe_up()));
    assert!(g.rig.state.transaction_gesture_update(output, 0.5));
    g.pump(3);

    // the menu bar joins under way with a layer it moves 200 points
    let bqh = bq.handle();
    let joined = bst.tx_manager.as_ref().unwrap().join(
        "overview".into(),
        g.ost.instance.unwrap(),
        1,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    joined.stage(&bar_surf);
    let child = bst.compositor.as_ref().unwrap().create_surface(&bqh, ());
    let layer = bst
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &bar_surf, &bqh, ());
    layer.animate_timed(7, Property::X, 200.0, SLOW_MS, 0.0, 0.0, 1.0, 1.0);
    bar_surf.commit();
    joined.submit();
    for _ in 0..4 {
        g.rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    let (bar_x, _) = shown_layer(&g.rig, &server_bar, GESTURE_T0);
    assert!(
        close(bar_x, 100.0),
        "the late part shows at the progress reached: x {bar_x}"
    );

    let rest = g.release(true, 0.0);
    for _ in 0..2 {
        g.rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert!(
        close(shown_layer(&g.rig, &server_bar, rest).0, 200.0),
        "the late part is at its target"
    );
    assert_eq!(bst.finished, vec![Outcome::Completed]);
    assert_eq!(bst.layer_finished, vec![7]);
}

#[test]
fn a_bound_gesture_whose_creator_destroys_its_object_under_way_settles_back_to_its_start() {
    let mut g = GestureRig::new(false, false);
    let output = g.rig.output_id;
    assert!(g.rig.state.transaction_gesture_begin(output, swipe_up()));
    assert!(g.rig.state.transaction_gesture_update(output, 0.6));
    g.pump(3);
    g.tx.destroy();
    g.pump(2);
    let later = GESTURE_T0 + 3.0;
    at_time(&mut g.rig, later);
    g.pump(4);
    let (x, opacity) = g.shown(later);
    assert!(
        close(x, 0.0) && close(opacity, 0.0),
        "the binding ended with the object: the layer is back at its start: x {x}, opacity {opacity}"
    );
    let mut replaced = g.ost.layer_replaced_with.clone();
    replaced.sort_by_key(|r| r.0);
    assert_eq!(
        replaced,
        vec![(MOVE_SERIAL, 0.0, 0.0), (FADE_SERIAL, 0.0, 0.0)]
    );
    assert!(
        g.ost.finished.is_empty(),
        "the destroyed object hears nothing"
    );
    assert!(
        g.rig.state.transaction_gesture_end(output, true, 2.0),
        "the rest of the gesture is the transaction's"
    );
    assert!(
        !g.rig.state.transaction_gesture_begin(output, swipe_up()),
        "the binding ended"
    );
}

#[test]
fn a_bound_gesture_whose_creator_s_connection_ends_under_way_settles_a_joiner_s_layers_back() {
    use cupertino_layer_v1::Property;
    let g = GestureRig::new(false, false);
    let GestureRig {
        mut rig,
        oconn,
        oq,
        ost,
        host: (hconn, mut hq, mut hst),
        tx,
        _layer,
        ..
    } = g;
    let output = rig.output_id;
    let (_hbar, bconn, mut bq, mut bst) = rig.spawn_component(&hconn, &mut hq, &mut hst, "menubar");
    let (bar_surf, _bar_shell) = rig.create_layer_surface(&bconn, &mut bq, &mut bst, "bar");
    let server_bar = server_layer_surface(&rig, "bar");
    // the menu bar's part, ready before the gesture, moves its layer 200 points
    let bqh = bq.handle();
    let joined = bst.tx_manager.as_ref().unwrap().join(
        "overview".into(),
        ost.instance.unwrap(),
        1,
        bst.output.as_ref().unwrap(),
        &bqh,
        (),
    );
    joined.stage(&bar_surf);
    let child = bst.compositor.as_ref().unwrap().create_surface(&bqh, ());
    let layer = bst
        .layers
        .as_ref()
        .unwrap()
        .get_layer(&child, &bar_surf, &bqh, ());
    layer.animate_timed(7, Property::X, 200.0, SLOW_MS, 0.0, 0.0, 1.0, 1.0);
    bar_surf.commit();
    joined.submit();
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert!(rig.state.transaction_gesture_begin(output, swipe_up()));
    assert!(rig.state.transaction_gesture_update(output, 0.5));
    for _ in 0..3 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    assert!(close(shown_layer(&rig, &server_bar, GESTURE_T0).0, 100.0));

    // the overview's connection ends with the fingers on the touchpad
    drop((tx, _layer, ost, oq, oconn));
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    let later = GESTURE_T0 + 3.0;
    at_time(&mut rig, later);
    for _ in 0..4 {
        rig.pump_comp(&bconn, &mut bq, &mut bst);
    }
    let bar_x = shown_layer(&rig, &server_bar, later).0;
    assert!(
        close(bar_x, 0.0),
        "the joiner's layer settled back to its start: x {bar_x}"
    );
    assert_eq!(bst.finished, vec![Outcome::Cancelled]);
    assert_eq!(bst.layer_replaced_with, vec![(7, 0.0, 0.0)]);
    assert!(
        rig.state.transaction_gesture_end(output, true, 2.0),
        "the rest of the gesture is the transaction's"
    );
}
