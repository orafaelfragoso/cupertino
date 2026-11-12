//! Which outputs a surface is told it is on, and the scale it is asked to draw at, against a headless compositor with
//! two offscreen displays: `headless-1` (1000 × 700 pixels at scale 1) and, right of it, `headless-2` (1600 × 1000
//! pixels at scale 2, so 800 × 500 points). The client binds `wl_compositor` version 6 and draws only at whole-number
//! scales: it follows `wl_surface.preferred_buffer_scale` and nothing else. Its window has a subsurface and a popup,
//! which follow the window. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::config::Config;
use cupertino_server::displays::surface_output::SCALE_SWITCH_MARGIN;
use cupertino_server::displays::HeadId;
use cupertino_server::events::BTN_LEFT;
use cupertino_server::state::PointerMode;
use cupertino_server::Cupertino;
use cupertino_wm::{Command, OutputId, Point as MPoint, Rect, WindowId};
use smithay::backend::input::ButtonState;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::{with_states, SurfaceAttributes};
use smithay::wayland::fractional_scale::with_fractional_scale;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const LEFT: (i32, i32) = (1000, 700);
const RIGHT: (i32, i32) = (1600, 1000);
const RIGHT_SCALE: f64 = 2.0;

// ---- the client ------------------------------------------------------------------------------------------------------

/// What one of the client's surfaces heard.
#[derive(Clone, Debug, PartialEq)]
enum Heard {
    Enter(String),
    Leave(String),
    Scale(i32),
    /// The global of the output went (`wl_registry.global_remove`), heard by the registry.
    Removed(String),
}

/// What the registry heard is logged under this name.
const REGISTRY: &str = "registry";

/// The client's surfaces by name: the window, its subsurface and its popup.
const WINDOW: &str = "window";
const SUBSURFACE: &str = "subsurface";
const POPUP: &str = "popup";
/// The cursor surface the pointer client sets, its drag icon and the second window of the client of two windows.
const CURSOR: &str = "cursor";
const ICON: &str = "icon";
const SECOND: &str = "second window";
/// The cursor surface's size in points and its hotspot.
const CURSOR_SIZE: (i32, i32) = (24, 24);
const HOTSPOT: (i32, i32) = (4, 4);
/// The drag icon's size in points.
const ICON_SIZE: (i32, i32) = (32, 32);

/// What a test client shows besides its window.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// A subsurface and a popup of the window.
    Popups,
    /// A cursor surface set on every pointer enter, and a drag with an icon started on the first button press.
    Pointer,
    /// A second window, as one connection shows many X11 windows.
    TwoWindows,
}

/// The pointer client's objects for the cursor and the drag.
struct PointerParts {
    compositor: wl_compositor::WlCompositor,
    manager: wl_data_device_manager::WlDataDeviceManager,
    device: wl_data_device::WlDataDevice,
    cursor: wl_surface::WlSurface,
    dragging: bool,
}

type Log = Arc<Mutex<Vec<(&'static str, Heard)>>>;

/// Layer surfaces a client is asked to open: each by name, with the name of the surface whose output it names (none:
/// it names no output).
type Opens = Arc<Mutex<Vec<(&'static str, Option<&'static str>)>>>;

/// One surface the client draws: its size in points and the whole-number scale it draws at.
struct Drawn {
    surface: wl_surface::WlSurface,
    size: (i32, i32),
    scale: i32,
    /// It can take a buffer: configured (a role with configures) and not drawn since.
    ready: bool,
    dirty: bool,
}

struct App {
    log: Log,
    shm: wl_shm::WlShm,
    surfaces: HashMap<&'static str, Drawn>,
    /// The outputs each surface is on, by what it heard.
    entered: HashMap<&'static str, Vec<wl_output::WlOutput>>,
    /// The outputs bound, by the name of their global.
    outputs: Vec<(u32, wl_output::WlOutput)>,
    pointer: Option<PointerParts>,
}

impl App {
    /// Draw every surface that is ready and has something new: a buffer of its size at its scale, `set_buffer_scale`.
    fn draw(&mut self, qh: &QueueHandle<App>) {
        for drawn in self.surfaces.values_mut() {
            if !(drawn.ready && drawn.dirty) {
                continue;
            }
            drawn.dirty = false;
            let (w, h) = (drawn.size.0 * drawn.scale, drawn.size.1 * drawn.scale);
            let buffer = buffer(&self.shm, (w, h), qh);
            drawn.surface.set_buffer_scale(drawn.scale);
            drawn.surface.attach(Some(&buffer), 0, 0);
            drawn.surface.damage_buffer(0, 0, w, h);
            drawn.surface.commit();
        }
    }
}

fn buffer(shm: &wl_shm::WlShm, size: (i32, i32), qh: &QueueHandle<App>) -> wl_buffer::WlBuffer {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    let row: Vec<u8> = [255, 0, 0, 255]
        .iter()
        .copied()
        .cycle()
        .take(w as usize * 4)
        .collect();
    for _ in 0..h {
        file.write_all(&row).unwrap();
    }
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(
        app: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::GlobalRemove { name } = event {
            if let Some((_, output)) = app.outputs.iter().find(|(n, _)| *n == name) {
                let output = output
                    .data::<Mutex<String>>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .clone();
                app.log
                    .lock()
                    .unwrap()
                    .push((REGISTRY, Heard::Removed(output)));
            }
        }
    }
}

macro_rules! ignore {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for App {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore!(
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface,
    xdg_positioner::XdgPositioner,
    xdg_popup::XdgPopup,
    wl_seat::WlSeat,
    wl_data_device_manager::WlDataDeviceManager,
    wl_data_source::WlDataSource,
    wl_data_offer::WlDataOffer
);

impl Dispatch<wl_data_device::WlDataDevice, ()> for App {
    fn event(
        _: &mut Self,
        _: &wl_data_device::WlDataDevice,
        _: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(App, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

/// The pointer client sets its cursor surface on every enter and starts a drag with its icon on its first press.
impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(parts) = app.pointer.as_mut() else {
            return;
        };
        match event {
            wl_pointer::Event::Enter { serial, .. } => {
                pointer.set_cursor(serial, Some(&parts.cursor), HOTSPOT.0, HOTSPOT.1);
            }
            wl_pointer::Event::Button {
                serial,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } if !parts.dragging => {
                parts.dragging = true;
                let source = parts.manager.create_data_source(qh, ());
                source.offer("text/plain".into());
                source.set_actions(wl_data_device_manager::DndAction::Copy);
                let icon = parts.compositor.create_surface(qh, ICON);
                let window = app.surfaces[WINDOW].surface.clone();
                parts
                    .device
                    .start_drag(Some(&source), &window, Some(&icon), serial);
                app.surfaces.insert(
                    ICON,
                    Drawn {
                        surface: icon,
                        size: ICON_SIZE,
                        scale: 1,
                        ready: true,
                        dirty: true,
                    },
                );
            }
            _ => {}
        }
    }
}

/// An output's name, from its `name` event.
impl Dispatch<wl_output::WlOutput, Mutex<String>> for App {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        name: &Mutex<String>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name: n } = event {
            *name.lock().unwrap() = n;
        }
    }
}

impl Dispatch<wl_surface::WlSurface, &'static str> for App {
    fn event(
        app: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        tag: &&'static str,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let name = |o: &wl_output::WlOutput| {
            o.data::<Mutex<String>>()
                .map(|n| n.lock().unwrap().clone())
                .unwrap_or_default()
        };
        let heard = match event {
            wl_surface::Event::Enter { output } => {
                app.entered.entry(tag).or_default().push(output.clone());
                Heard::Enter(name(&output))
            }
            wl_surface::Event::Leave { output } => {
                app.entered.entry(tag).or_default().retain(|o| *o != output);
                Heard::Leave(name(&output))
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                if let Some(drawn) = app.surfaces.get_mut(tag) {
                    drawn.scale = factor;
                    drawn.dirty = true;
                }
                Heard::Scale(factor)
            }
            _ => return,
        };
        app.log.lock().unwrap().push((tag, heard));
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
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

impl Dispatch<xdg_surface::XdgSurface, &'static str> for App {
    fn event(
        app: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        tag: &&'static str,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            if let Some(drawn) = app.surfaces.get_mut(tag) {
                drawn.ready = true;
                drawn.dirty = true;
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, &'static str> for App {
    fn event(
        app: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        tag: &&'static str,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            if let Some(drawn) = app.surfaces.get_mut(tag) {
                if width > 0 && height > 0 {
                    drawn.size = (width, height);
                }
            }
        }
    }
}

/// A client with a window and what its [`Kind`] adds, on a thread of its own; dropping it disconnects it.
struct Client {
    log: Log,
    stop: Arc<AtomicBool>,
    /// Layer surfaces to open.
    opens: Opens,
    /// The roundtrips the client's thread finished.
    rounds: common::Rounds,
}

impl Client {
    /// A client with a window, a subsurface of it and a popup of it.
    fn new(socket: PathBuf) -> Client {
        Client::spawn(socket, Kind::Popups)
    }

    fn spawn(socket: PathBuf, kind: Kind) -> Client {
        let log = Log::default();
        let stop = Arc::new(AtomicBool::new(false));
        let rounds = common::Rounds::default();
        let (log2, stop2, rounds2) = (log.clone(), stop.clone(), rounds.clone());
        std::thread::spawn(move || {
            let _ended = rounds2.ended_on_drop();
            let stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 6..=6, ()).unwrap();
            let subcompositor: wl_subcompositor::WlSubcompositor =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let outputs: Vec<(u32, wl_output::WlOutput)> = globals
                .contents()
                .clone_list()
                .into_iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| {
                    let output = globals
                        .registry()
                        .bind(g.name, 4, &qh, Mutex::new(String::new()));
                    (g.name, output)
                })
                .collect();
            let mut app = App {
                log: log2,
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                surfaces: HashMap::new(),
                entered: HashMap::new(),
                outputs,
                pointer: None,
            };
            let drawn = |surface, size, ready| Drawn {
                surface,
                size,
                scale: 1,
                ready,
                dirty: ready,
            };

            // the window, drawn once configured
            let window = compositor.create_surface(&qh, WINDOW);
            let window_xdg = base.get_xdg_surface(&window, &qh, WINDOW);
            let toplevel = window_xdg.get_toplevel(&qh, WINDOW);
            toplevel.set_app_id("org.example.surface-output".into());
            window.commit();
            app.surfaces
                .insert(WINDOW, drawn(window.clone(), (400, 300), false));
            while !app.surfaces[WINDOW].ready {
                queue.blocking_dispatch(&mut app).unwrap();
            }
            app.draw(&qh);
            queue.roundtrip(&mut app).unwrap();
            let mut keep: Vec<Box<dyn std::any::Any + Send>> = vec![Box::new(window_xdg.clone())];
            keep.push(Box::new(toplevel));
            match kind {
                Kind::Popups => {}
                Kind::Pointer => {
                    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
                    let _pointer = seat.get_pointer(&qh, ());
                    let manager: wl_data_device_manager::WlDataDeviceManager =
                        globals.bind(&qh, 3..=3, ()).unwrap();
                    let device = manager.get_data_device(&seat, &qh, ());
                    let cursor = compositor.create_surface(&qh, CURSOR);
                    app.surfaces
                        .insert(CURSOR, drawn(cursor.clone(), CURSOR_SIZE, true));
                    app.draw(&qh);
                    app.pointer = Some(PointerParts {
                        compositor: compositor.clone(),
                        manager,
                        device,
                        cursor,
                        dragging: false,
                    });
                    keep.push(Box::new(seat));
                }
                Kind::TwoWindows => {
                    let second = compositor.create_surface(&qh, SECOND);
                    let second_xdg = base.get_xdg_surface(&second, &qh, SECOND);
                    let second_toplevel = second_xdg.get_toplevel(&qh, SECOND);
                    second_toplevel.set_app_id("org.example.surface-output".into());
                    second.commit();
                    app.surfaces
                        .insert(SECOND, drawn(second.clone(), (400, 300), false));
                    keep.push(Box::new(second_xdg));
                    keep.push(Box::new(second_toplevel));
                }
            }
            if kind != Kind::Popups {
                while !stop2.load(Ordering::SeqCst) {
                    if queue.roundtrip(&mut app).is_err() {
                        break;
                    }
                    rounds2.tick();
                    app.draw(&qh);
                    std::thread::sleep(Duration::from_millis(2));
                }
                return;
            }

            // a subsurface in its top left corner, drawn at once; it shows with the window's next commit
            let sub = compositor.create_surface(&qh, SUBSURFACE);
            let subsurface = subcompositor.get_subsurface(&sub, &window, &qh, ());
            subsurface.set_position(10, 10);
            subsurface.set_desync();
            app.surfaces
                .insert(SUBSURFACE, drawn(sub.clone(), (50, 40), true));
            app.draw(&qh);
            window.commit();

            // a popup below its top edge, drawn once configured
            let popup = compositor.create_surface(&qh, POPUP);
            let popup_xdg = base.get_xdg_surface(&popup, &qh, POPUP);
            let positioner = base.create_positioner(&qh, ());
            positioner.set_size(120, 80);
            positioner.set_anchor_rect(20, 20, 1, 1);
            positioner.set_anchor(xdg_positioner::Anchor::BottomRight);
            positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
            let xdg_popup = popup_xdg.get_popup(Some(&window_xdg), &positioner, &qh, ());
            popup.commit();
            app.surfaces
                .insert(POPUP, drawn(popup.clone(), (120, 80), false));

            let _keep = (keep, subsurface, xdg_popup, popup_xdg);
            while !stop2.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut app).is_err() {
                    break;
                }
                rounds2.tick();
                app.draw(&qh);
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Client {
            log,
            stop,
            opens: Default::default(),
            rounds,
        }
    }

    /// A client of layer surfaces, as a shell panel is: it opens them when [`Client::open`] asks.
    fn layers(socket: PathBuf) -> Client {
        let log = Log::default();
        let stop = Arc::new(AtomicBool::new(false));
        let opens = Opens::default();
        let rounds = common::Rounds::default();
        let (log2, stop2, opens2, rounds2) =
            (log.clone(), stop.clone(), opens.clone(), rounds.clone());
        std::thread::spawn(move || {
            let _ended = rounds2.ended_on_drop();
            let stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 6..=6, ()).unwrap();
            let shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(&qh, 1..=4, ()).unwrap();
            let outputs: Vec<(u32, wl_output::WlOutput)> = globals
                .contents()
                .clone_list()
                .into_iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| {
                    let output = globals
                        .registry()
                        .bind(g.name, 4, &qh, Mutex::new(String::new()));
                    (g.name, output)
                })
                .collect();
            let mut app = App {
                log: log2,
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                surfaces: HashMap::new(),
                entered: HashMap::new(),
                outputs,
                pointer: None,
            };
            let mut layers = Vec::new();
            while !stop2.load(Ordering::SeqCst) {
                let wanted: Vec<_> = opens2.lock().unwrap().drain(..).collect();
                for (tag, beside) in wanted {
                    let output = beside.and_then(|b| app.entered.get(b)?.first().cloned());
                    let surface = compositor.create_surface(&qh, tag);
                    let layer = shell.get_layer_surface(
                        &surface,
                        output.as_ref(),
                        zwlr_layer_shell_v1::Layer::Top,
                        "surface-output-test".into(),
                        &qh,
                        tag,
                    );
                    layer.set_size(100, 100);
                    layer.set_anchor(
                        zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Right,
                    );
                    surface.commit();
                    app.surfaces.insert(
                        tag,
                        Drawn {
                            surface,
                            size: (100, 100),
                            scale: 1,
                            ready: false,
                            dirty: false,
                        },
                    );
                    layers.push(layer);
                }
                if queue.roundtrip(&mut app).is_err() {
                    break;
                }
                rounds2.tick();
                app.draw(&qh);
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Client {
            log,
            stop,
            opens,
            rounds,
        }
    }

    /// Open the layer surface `tag`, on the output the surface `beside` is on, or naming no output.
    fn open(&self, tag: &'static str, beside: Option<&'static str>) {
        self.opens.lock().unwrap().push((tag, beside));
    }

    /// What `surface` heard, in order.
    fn heard(&self, surface: &str) -> Vec<Heard> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|(s, _)| *s == surface)
            .map(|(_, h)| h.clone())
            .collect()
    }

    /// The outputs `surface` is on now, by what it heard.
    fn on(&self, surface: &str) -> Vec<String> {
        let mut on = Vec::new();
        for h in self.heard(surface) {
            match h {
                Heard::Enter(o) if !on.contains(&o) => on.push(o),
                Heard::Leave(o) => on.retain(|x| *x != o),
                _ => {}
            }
        }
        on
    }

    /// The outputs `surface` is on now, by name.
    fn on_sorted(&self, surface: &str) -> Vec<String> {
        let mut on = self.on(surface);
        on.sort();
        on
    }

    /// The whole-number scale `surface` was last asked for; 1 until asked.
    fn scale(&self, surface: &str) -> i32 {
        self.heard(surface)
            .into_iter()
            .filter_map(|h| match h {
                Heard::Scale(s) => Some(s),
                _ => None,
            })
            .next_back()
            .unwrap_or(1)
    }

    /// The whole-number scales `surface` was asked for, in order.
    fn scales(&self, surface: &str) -> Vec<i32> {
        self.heard(surface)
            .into_iter()
            .filter_map(|h| match h {
                Heard::Scale(s) => Some(s),
                _ => None,
            })
            .collect()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, &'static str> for App {
    fn event(
        app: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        tag: &&'static str,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            layer.ack_configure(serial);
            if let Some(drawn) = app.surfaces.get_mut(tag) {
                drawn.ready = true;
                drawn.dirty = true;
            }
        }
    }
}

// ---- the compositor --------------------------------------------------------------------------------------------------

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        let display = |name: &str, size| HeadlessDisplay {
            name: name.into(),
            size,
            physical_mm: (0, 0),
        };
        if let Err(e) =
            headless::init_displays(&mut state, &[display("headless-1", LEFT)], Some(1.0))
        {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        headless::add_display(&mut state, &display("headless-2", RIGHT), Some(RIGHT_SCALE))
            .unwrap();
        Some(Harness { state, event_loop })
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    /// Run the compositor until `cond` holds of it and the client.
    fn wait(
        &mut self,
        what: &str,
        client: &Client,
        mut cond: impl FnMut(&Cupertino, &Client) -> bool,
    ) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state, client) {
                return;
            }
        }
        panic!(
            "timed out waiting for {what}; the client heard {:?}",
            client.log.lock().unwrap()
        );
    }

    /// Wait for `client`'s [`common::Rounds::barrier`]: it has every event sent before, so a test can say what it did
    /// not hear.
    fn sync(&mut self, client: &Client) {
        let done = client.rounds.barrier();
        self.wait("the client's roundtrips", client, move |_, _| done());
    }

    /// Let every animation finish.
    fn settle(&mut self) {
        let time = self.state.now() + 5.0;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        self.step();
    }

    fn socket(&self) -> PathBuf {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join(self.state.socket_name.as_ref().unwrap())
    }

    fn output(&self, name: &str) -> OutputId {
        self.state
            .outputs
            .iter()
            .find(|o| o.output.name() == name)
            .unwrap()
            .id
    }

    fn head(&self, name: &str) -> HeadId {
        self.state
            .displays
            .heads
            .iter()
            .find(|h| h.info.connector == name)
            .unwrap()
            .id
    }

    /// Map the client's window, with its subsurface and popup on it.
    fn map(&mut self) -> (Client, WindowId) {
        let client = Client::new(self.socket());
        self.wait(
            "the window, its subsurface and its popup",
            &client,
            |s, c| {
                s.windows.iter().any(|e| e.mapped)
                    && [WINDOW, SUBSURFACE, POPUP]
                        .iter()
                        .all(|t| !c.on(t).is_empty())
            },
        );
        self.settle();
        let window = self.state.windows[0].id;
        (client, window)
    }

    /// Map a client of `kind` and wait until its first window is on a display.
    fn map_kind(&mut self, kind: Kind) -> Client {
        let client = Client::spawn(self.socket(), kind);
        self.wait("the window on a display", &client, |s, c| {
            s.windows.iter().any(|e| e.mapped) && !c.on(WINDOW).is_empty()
        });
        self.settle();
        client
    }

    fn put(&mut self, window: WindowId, output: &str, frame: Rect) {
        let output = self.output(output);
        self.state.command(Command::MoveWindowToOutput {
            window,
            output,
            frame,
        });
        self.settle();
    }

    fn move_pointer(&mut self, x: f64, y: f64) {
        let time = (self.state.now() * 1000.0) as u32;
        self.state.pointer_moved_to((x, y).into(), time, None);
    }

    /// Drag `window` by its title bar from `from` to `to` and let go.
    fn drag(&mut self, window: WindowId, from: (f64, f64), to: (f64, f64)) {
        self.move_pointer(from.0, from.1);
        self.state.begin_model_interaction(Command::BeginMove {
            window,
            pointer: MPoint::new(from.0, from.1),
        });
        for i in 1..=10 {
            let t = f64::from(i) / 10.0;
            self.move_pointer(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            self.step();
        }
        self.release();
    }

    fn release(&mut self) {
        self.state.command(Command::PointerRelease);
        self.state.pointer_mode = PointerMode::Normal;
        self.settle();
    }

    fn frame(&self, window: WindowId) -> Rect {
        self.state.wm.window(window).unwrap().geometry()
    }

    /// The buffer scale of the window's last commit, and the size its buffer covers in points.
    fn drawn(&self) -> (i32, Option<(i32, i32)>) {
        let surface = self.state.windows[0].surface().clone();
        let scale = with_states(&surface, |s| {
            s.cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer_scale
        });
        let size = smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |s| {
            s.buffer_size().map(|b| (b.w, b.h))
        })
        .flatten();
        (scale, size)
    }

    /// The exact scale the window's surface was told.
    fn fractional(&self) -> Option<f64> {
        let surface = self.state.windows[0].surface().clone();
        with_states(&surface, |s| {
            with_fractional_scale(s, |fs| fs.preferred_scale())
        })
    }
}

/// Item 1: a window, its subsurface and its popup hear `enter` and `leave` as the window moves to the other display
/// and as that display is unplugged, with the whole-number scale of the display they are on, and the client, which
/// draws at whole-number scales only, draws at 2 on the display of scale 2.
#[test]
fn a_window_is_told_the_outputs_it_is_on_and_their_scale_as_it_moves_and_as_they_go() {
    let Some(mut h) = Harness::new() else { return };
    let (client, window) = h.map();
    for t in [WINDOW, SUBSURFACE, POPUP] {
        assert_eq!(client.on(t), ["headless-1"], "{t}");
        assert_eq!(
            client.scale(t),
            1,
            "{t}: drawn at 1 on the display of scale 1"
        );
    }
    assert_eq!(h.drawn().0, 1);

    // onto the display of scale 2: everything leaves the first and enters the second, and is asked to draw at 2
    h.put(window, "headless-2", Rect::new(1100.0, 100.0, 400.0, 300.0));
    h.wait("the move to be heard", &client, |_, c| {
        [WINDOW, SUBSURFACE, POPUP]
            .iter()
            .all(|t| c.on(t) == ["headless-2"] && c.scale(t) == 2)
    });
    h.sync(&client);
    for t in [WINDOW, SUBSURFACE, POPUP] {
        let heard = client.heard(t);
        let leave = heard
            .iter()
            .position(|e| *e == Heard::Leave("headless-1".into()));
        let enter = heard
            .iter()
            .position(|e| *e == Heard::Enter("headless-2".into()));
        assert!(leave.is_some() && enter.is_some(), "{t}: {heard:?}");
        assert_eq!(client.scales(t), [2], "{t}: one new scale");
    }
    // the client that draws at whole-number scales only now draws at 2, twice the window's points
    let deadline = Instant::now() + common::HANG_GUARD;
    while h.drawn().0 != 2 {
        assert!(
            Instant::now() < deadline,
            "the window was not drawn at 2: {:?}",
            h.drawn()
        );
        h.step();
    }
    // its buffer has twice the pixels of the points it covers, the window's whole surface
    let points = h.drawn().1.expect("a buffer");
    let geometry = h.state.windows[0].window.geometry().size;
    assert_eq!(
        points,
        (geometry.w, geometry.h),
        "the buffer covers the window"
    );
    assert_eq!(h.fractional(), Some(2.0), "the exact scale is sent as well");

    // the display of scale 2 goes: back on the first, at 1
    let head = h.head("headless-2");
    headless::remove_display(&mut h.state, head).unwrap();
    h.settle();
    h.wait("the unplug to be heard", &client, |_, c| {
        [WINDOW, SUBSURFACE, POPUP]
            .iter()
            .all(|t| c.on(t) == ["headless-1"] && c.scale(t) == 1)
    });
    h.wait("the global of the display to go", &client, |_, c| {
        c.heard(REGISTRY) == [Heard::Removed("headless-2".into())]
    });
    h.sync(&client);
    let log = client.log.lock().unwrap().clone();
    let removed = log
        .iter()
        .position(|(_, e)| *e == Heard::Removed("headless-2".into()))
        .unwrap();
    for t in [WINDOW, SUBSURFACE, POPUP] {
        let leave = log
            .iter()
            .position(|(s, e)| *s == t && *e == Heard::Leave("headless-2".into()));
        assert!(
            leave.is_some_and(|l| l < removed),
            "{t}: the display that went is left while its global is there: {log:?}"
        );
        assert_eq!(client.scales(t), [2, 1], "{t}");
    }
    assert_eq!(h.fractional(), Some(1.0));
}

/// A window across the seam is on both displays; one that is minimized is on none, and on its display again when it
/// comes back.
#[test]
fn a_window_across_the_seam_is_on_both_displays_and_a_minimized_one_on_none() {
    let Some(mut h) = Harness::new() else { return };
    let (client, window) = h.map();
    h.put(window, "headless-1", Rect::new(300.0, 100.0, 400.0, 300.0));
    h.drag(window, (500.0, 110.0), (950.0, 110.0));
    let frame = h.frame(window);
    assert!(frame.x < 1000.0 && frame.right() > 1000.0, "{frame:?}");
    h.wait("both displays", &client, |_, c| {
        c.on(WINDOW) == ["headless-1", "headless-2"] && c.on(POPUP).len() == 2
    });
    // most of it is on the first display, which so gives the scale
    assert_eq!(
        h.state.window_scale_output(window),
        Some(h.output("headless-1"))
    );
    assert_eq!(client.scale(WINDOW), 1);

    h.state.command(Command::Minimize {
        window,
        target: None,
    });
    h.settle();
    h.wait("no display", &client, |_, c| {
        [WINDOW, SUBSURFACE].iter().all(|t| c.on(t).is_empty())
    });
    h.state.command(Command::Restore { window });
    h.settle();
    h.wait("both displays again", &client, |_, c| {
        c.on(WINDOW).len() == 2
    });
}

/// A display's scale changes under a window: the exact scale follows it and the whole-number one is the exact one
/// rounded up, sent only when it changes.
#[test]
fn a_scale_change_sends_the_exact_scale_and_the_whole_number_one_rounded_up() {
    let Some(mut h) = Harness::new() else { return };
    let (client, window) = h.map();
    h.put(window, "headless-2", Rect::new(1100.0, 100.0, 400.0, 300.0));
    h.wait("scale 2", &client, |_, c| c.scale(WINDOW) == 2);
    for (scale, whole) in [(1.5, 2), (2.5, 3), (1.0, 1)] {
        let head = h.head("headless-2");
        let mut state = h.state.displays.head(head).unwrap().state;
        state.scale = scale;
        h.state.configure_heads(&[(head, state)], false).unwrap();
        h.state.heads_changed();
        h.settle();
        h.wait("the new scale", &client, |_, c| c.scale(WINDOW) == whole);
        h.sync(&client);
        assert_eq!(h.fractional(), Some(scale), "{scale}: exact");
        assert_eq!(client.on(WINDOW), ["headless-2"], "{scale}");
    }
    assert_eq!(client.scales(WINDOW), [2, 3, 1], "only changes are sent");
}

/// Item 2: a window dragged back and forth across the seam changes scale once per crossing, past the middle by the
/// margin; let go with a little more of it on the other display than within the margin, it takes that display's scale
/// when the drag ends.
#[test]
fn a_window_dragged_across_the_seam_changes_scale_once_per_crossing() {
    let Some(mut h) = Harness::new() else { return };
    let (client, window) = h.map();
    // the popup would follow the window; this is about the window
    h.put(window, "headless-1", Rect::new(300.0, 100.0, 400.0, 300.0));
    let (left, right) = (h.output("headless-1"), h.output("headless-2"));
    let grab = (500.0, 110.0);
    h.move_pointer(grab.0, grab.1);
    h.state.begin_model_interaction(Command::BeginMove {
        window,
        pointer: MPoint::new(grab.0, grab.1),
    });
    let there: Vec<f64> = (0..=80).map(|i| grab.0 + f64::from(i) * 10.0).collect();
    let back: Vec<f64> = there.iter().rev().copied().collect();
    let mut current = h.state.window_scale_output(window);
    assert_eq!(current, Some(left));
    let mut switches = Vec::new();
    for xs in [&there, &back, &there, &back] {
        for &x in xs {
            h.move_pointer(x, grab.1);
            h.step();
            let now = h.state.window_scale_output(window);
            if now != current {
                switches.push((h.frame(window), now));
                current = now;
            }
        }
    }
    let rects: Vec<Rect> = [left, right]
        .iter()
        .map(|o| h.state.output_entry(*o).unwrap().rect)
        .collect();
    let overlap = |f: Rect, r: Rect| f.intersection(&r).map_or(0.0, |i| i.w * i.h);
    assert_eq!(switches.len(), 4, "once per crossing: {switches:?}");
    for (i, (frame, to)) in switches.iter().enumerate() {
        let (from_rect, to_rect) = if i % 2 == 0 {
            (rects[0], rects[1])
        } else {
            (rects[1], rects[0])
        };
        assert_eq!(*to, Some(if i % 2 == 0 { right } else { left }));
        assert!(
            overlap(*frame, to_rect) - overlap(*frame, from_rect)
                > SCALE_SWITCH_MARGIN * frame.w * frame.h,
            "{i}: {frame:?} switched within the margin"
        );
    }
    h.wait("the client to hear every switch", &client, |_, c| {
        c.scales(WINDOW).len() == 4
    });
    h.sync(&client);
    assert_eq!(client.scales(WINDOW), [2, 1, 2, 1]);

    // a little over half on the second display, within the margin: the first holds until the drag ends
    let mut x = grab.0;
    loop {
        x += 2.0;
        h.move_pointer(x, grab.1);
        h.step();
        let f = h.frame(window);
        if overlap(f, rects[1]) > overlap(f, rects[0]) {
            break;
        }
        assert!(x < 2000.0, "the window never reached the second display");
    }
    let f = h.frame(window);
    assert!(overlap(f, rects[1]) - overlap(f, rects[0]) <= SCALE_SWITCH_MARGIN * f.w * f.h);
    assert_eq!(h.state.window_scale_output(window), Some(left));
    h.release();
    let f = h.frame(window);
    let expected = if overlap(f, rects[1]) > overlap(f, rects[0]) {
        right
    } else {
        left
    };
    assert_eq!(h.state.window_scale_output(window), Some(expected), "{f:?}");
}

/// Item 3, the server's side: a panel opens two layer surfaces naming no output, which go to the active output and are
/// told it; the person moves to the other display, so it becomes the active output; a surface the panel opens again
/// naming the output its other surface was told goes beside it, while one naming no output goes to the new active
/// output.
#[test]
fn a_layer_surface_is_told_the_active_output_it_opened_on_and_one_naming_it_opens_beside_it() {
    let Some(mut h) = Harness::new() else { return };
    h.move_pointer(500.0, 300.0);
    h.step();
    assert_eq!(h.state.active_output(), Some(h.output("headless-1")));
    let panel = Client::layers(h.socket());
    panel.open("catcher", None);
    panel.open("card", None);
    h.wait("both on the active output", &panel, |_, c| {
        c.on("catcher") == ["headless-1"] && c.on("card") == ["headless-1"]
    });

    // the pointer goes to the other display, which becomes the active output
    h.move_pointer(1400.0, 300.0);
    h.step();
    assert_eq!(h.state.active_output(), Some(h.output("headless-2")));
    panel.open("catcher again", Some("card"));
    panel.open("elsewhere", None);
    h.wait("the new surfaces placed", &panel, |_, c| {
        !c.on("catcher again").is_empty() && !c.on("elsewhere").is_empty()
    });
    assert_eq!(panel.on("catcher again"), ["headless-1"], "beside the card");
    assert_eq!(
        panel.on("elsewhere"),
        ["headless-2"],
        "the active output changed"
    );
    assert_eq!(
        panel.scale("elsewhere"),
        2,
        "a layer surface draws at its output's scale"
    );
    assert_eq!(panel.scale("catcher again"), 1);

    // the display of the surface opened elsewhere goes: the surface is told it left it before the display's global goes
    let head = h.head("headless-2");
    headless::remove_display(&mut h.state, head).unwrap();
    h.wait("the global of the display to go", &panel, |_, c| {
        c.heard(REGISTRY) == [Heard::Removed("headless-2".into())]
    });
    let log = panel.log.lock().unwrap().clone();
    let at = |surface: &str, e: Heard| log.iter().position(|(s, h)| *s == surface && *h == e);
    let removed = at(REGISTRY, Heard::Removed("headless-2".into())).unwrap();
    assert!(
        at("elsewhere", Heard::Leave("headless-2".into())).is_some_and(|l| l < removed),
        "{log:?}"
    );
    assert_eq!(panel.on("catcher again"), ["headless-1"]);
}

/// The client's cursor surface is on every display its image overlaps and draws at the scale of the display under the
/// pointer; once the pointer leaves the client, so that its cursor is no longer shown, the surface is on none.
#[test]
fn a_cursor_surface_is_on_the_displays_it_overlaps_at_the_scale_under_the_pointer() {
    let Some(mut h) = Harness::new() else { return };
    let client = h.map_kind(Kind::Pointer);
    let window = h.state.windows[0].id;
    h.put(window, "headless-2", Rect::new(1100.0, 100.0, 400.0, 300.0));
    h.move_pointer(1300.0, 250.0);
    h.wait("the cursor on the second display", &client, |_, c| {
        c.on(CURSOR) == ["headless-2"] && c.scale(CURSOR) == 2
    });
    // a window across the seam takes input on the display the window model has it on, the first
    h.put(window, "headless-1", Rect::new(300.0, 100.0, 400.0, 300.0));
    h.drag(window, (500.0, 110.0), (950.0, 110.0));
    let frame = h.frame(window);
    assert!(frame.x < 980.0 && frame.right() > 1000.0, "{frame:?}");
    // the image, from 4 points left of the pointer, reaches over the seam: on both, at the scale under the pointer
    h.move_pointer(995.0, 250.0);
    h.wait("the cursor on both displays", &client, |_, c| {
        c.on_sorted(CURSOR) == ["headless-1", "headless-2"] && c.scale(CURSOR) == 1
    });
    // over the desktop the server's cursor shows: the client's surface is on no display
    h.move_pointer(300.0, 600.0);
    h.wait("the cursor on no display", &client, |_, c| {
        c.on(CURSOR).is_empty()
    });
}

/// A drag icon is on every display it overlaps while the drag runs, at the scale under the pointer, and on none once
/// the drag ended.
#[test]
fn a_drag_icon_is_on_the_displays_it_overlaps_while_the_drag_runs() {
    let Some(mut h) = Harness::new() else { return };
    let client = h.map_kind(Kind::Pointer);
    let window = h.state.windows[0].id;
    h.put(window, "headless-1", Rect::new(300.0, 100.0, 400.0, 300.0));
    h.move_pointer(500.0, 250.0);
    h.step();
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    h.wait("the icon on the first display", &client, |s, c| {
        s.drag_icon.is_some() && c.on(ICON) == ["headless-1"]
    });
    // the icon hangs from the pointer, 32 points wide: over the seam it is on both
    h.move_pointer(990.0, 250.0);
    h.wait("the icon on both displays", &client, |_, c| {
        c.on_sorted(ICON) == ["headless-1", "headless-2"] && c.scale(ICON) == 1
    });
    h.move_pointer(1300.0, 250.0);
    h.wait("the icon on the second display", &client, |_, c| {
        c.on(ICON) == ["headless-2"] && c.scale(ICON) == 2
    });
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    h.wait("the icon on no display", &client, |s, c| {
        s.drag_icon.is_none() && c.on(ICON).is_empty()
    });
}

/// One connection that shows several windows, as the X11 bridge shows every X11 window: each window is told its own
/// display and scale.
#[test]
fn each_window_of_one_client_is_told_its_own_display() {
    let Some(mut h) = Harness::new() else { return };
    let client = Client::spawn(h.socket(), Kind::TwoWindows);
    h.wait("both windows on a display", &client, |s, c| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
            && !c.on(WINDOW).is_empty()
            && !c.on(SECOND).is_empty()
    });
    h.settle();
    let (first, second) = (h.state.windows[0].id, h.state.windows[1].id);
    h.put(first, "headless-1", Rect::new(100.0, 100.0, 400.0, 300.0));
    h.put(second, "headless-2", Rect::new(1100.0, 100.0, 400.0, 300.0));
    h.wait("each on its display", &client, |_, c| {
        c.on(WINDOW) == ["headless-1"]
            && c.scale(WINDOW) == 1
            && c.on(SECOND) == ["headless-2"]
            && c.scale(SECOND) == 2
    });
}
