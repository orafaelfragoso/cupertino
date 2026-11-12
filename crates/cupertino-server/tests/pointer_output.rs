//! Layer surfaces created without an output, against a headless compositor with two offscreen
//! displays: they go to the output under the pointer. The client creates its panel the way the
//! launcher does. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::desktop::layer_map_for_output;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const LEFT: (i32, i32) = (1000, 700);
const RIGHT: (i32, i32) = (800, 600);
const PANEL_NAMESPACE: &str = "cupertino-launcher";

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    /// A compositor with `headless-1` (`LEFT` pixels) and, right of it, `headless-2` (`RIGHT`), unscaled.
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("create test compositor");
        let displays = [
            HeadlessDisplay {
                name: "headless-1".into(),
                size: LEFT,
                physical_mm: (0, 0),
            },
            HeadlessDisplay {
                name: "headless-2".into(),
                size: RIGHT,
                physical_mm: (0, 0),
            },
        ];
        match headless::init_displays(&mut state, &displays, Some(1.0)) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Run the compositor until `received` delivers.
    fn wait_for<T>(&mut self, what: &str, received: &mpsc::Receiver<T>) -> T {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if let Ok(value) = received.try_recv() {
                return value;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Namespaces of the layer surfaces on the output named `name`.
    fn layers_on(&self, name: &str) -> Vec<String> {
        let entry = self
            .state
            .outputs
            .iter()
            .find(|o| o.output.name() == name)
            .expect("the output");
        layer_map_for_output(&entry.output)
            .layers()
            .map(|l| l.namespace().to_string())
            .collect()
    }
}

/// Where an output sits in the compositor's global space, in logical points (`zxdg_output_v1`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct OutputGeometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

struct Panel {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    /// Every output with its geometry; the index is the user data of its `zxdg_output_v1`.
    outputs: Vec<(wl_output::WlOutput, OutputGeometry)>,
    entered: Vec<wl_output::WlOutput>,
    report: mpsc::Sender<Vec<OutputGeometry>>,
    reported: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Panel {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Panel {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_output::WlOutput,
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zwlr_layer_shell_v1::ZwlrLayerShellV1
);

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for Panel {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let geometry = &mut state.outputs[*index].1;
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => (geometry.x, geometry.y) = (x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                (geometry.width, geometry.height) = (width as u32, height as u32)
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Panel {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { output } => state.entered.push(output),
            wl_surface::Event::Leave { output } => state.entered.retain(|o| *o != output),
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Panel {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        else {
            return;
        };
        layer.ack_configure(serial);
        let (w, h) = (width as i32, height as i32);
        let fd =
            rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        let mut file = std::fs::File::from(fd);
        file.write_all(&vec![255u8; (w * h * 4) as usize]).unwrap();
        let pool = state.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        state.surface.attach(Some(&buffer), 0, 0);
        state.surface.damage_buffer(0, 0, w, h);
        state.surface.commit();
        if !state.reported && (width, height) == (400, 300) {
            let geometries = state
                .entered
                .iter()
                .map(|wl| {
                    state
                        .outputs
                        .iter()
                        .find(|(o, _)| o == wl)
                        .map(|(_, g)| *g)
                        .filter(|g| g.width > 0 && g.height > 0)
                        .expect("a described output")
                })
                .collect();
            state.report.send(geometries).unwrap();
            state.reported = true;
        }
    }
}

/// Connect a client that creates a launcher-like panel without an output and reports the
/// geometry of the outputs the server put it on, once configured. It stays connected until the
/// compositor goes away.
fn spawn_panel_client(socket: PathBuf) -> mpsc::Receiver<Vec<OutputGeometry>> {
    let (report, received) = mpsc::channel();
    std::thread::spawn(move || {
        let conn = Connection::from_socket(
            UnixStream::connect(&socket).expect("connecting to the compositor"),
        )
        .unwrap();
        let (globals, mut queue) = registry_queue_init::<Panel>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
            globals.bind(&qh, 1..=4, ()).unwrap();
        let output_manager: zxdg_output_manager_v1::ZxdgOutputManagerV1 =
            globals.bind(&qh, 1..=3, ()).unwrap();
        let outputs: Vec<(wl_output::WlOutput, OutputGeometry)> =
            globals.contents().with_list(|list| {
                list.iter()
                    .filter(|g| g.interface == wl_output::WlOutput::interface().name)
                    .map(|g| {
                        let output = globals.registry().bind(g.name, g.version.min(4), &qh, ());
                        (output, OutputGeometry::default())
                    })
                    .collect()
            });
        let _xdg_outputs: Vec<zxdg_output_v1::ZxdgOutputV1> = outputs
            .iter()
            .enumerate()
            .map(|(i, (o, _))| output_manager.get_xdg_output(o, &qh, i))
            .collect();
        let surface = compositor.create_surface(&qh, ());
        let mut panel = Panel {
            shm: globals.bind(&qh, 1..=1, ()).unwrap(),
            surface: surface.clone(),
            outputs,
            entered: Vec::new(),
            report,
            reported: false,
        };
        // every output is described before the panel asks for a place
        queue.roundtrip(&mut panel).unwrap();
        let layer = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            PANEL_NAMESPACE.into(),
            &qh,
            (),
        );
        layer.set_size(400, 300);
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::Top);
        layer.set_margin(100, 0, 0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        surface.commit();
        while queue.roundtrip(&mut panel).is_ok() {
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    received
}

#[test]
fn a_panel_without_an_output_maps_on_the_output_under_the_pointer() {
    let Some(mut h) = Harness::new() else { return };
    h.state
        .pointer_moved_to((f64::from(LEFT.0) + 100.0, 200.0).into(), 0, None);
    let placed = spawn_panel_client(h.socket_path());
    let geometries = h.wait_for("the panel to be configured", &placed);
    assert_eq!(
        geometries,
        vec![OutputGeometry {
            x: LEFT.0,
            y: 0,
            width: RIGHT.0 as u32,
            height: RIGHT.1 as u32
        }],
        "the client is told the second output"
    );
    assert_eq!(h.layers_on("headless-2"), [PANEL_NAMESPACE]);
    assert!(h.layers_on("headless-1").is_empty());
}
