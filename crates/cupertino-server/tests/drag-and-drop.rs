//! Drag and drop between two real Wayland clients: one starts a drag of text from a button press in its window, the
//! pointer moves onto the other's window and is released there, and the other reads the dropped text. A drag with an
//! icon surface shows it with the pointer at its hotspot until the drag ends. The compositor runs in-process on an
//! offscreen EGL device. Tests skip (pass) when no EGL device is available.

use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::drag_return::{Back, DRAG_FADE_DURATION, DRAG_RETURN_DURATION};
use cupertino_server::events::BTN_LEFT;
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::backend::renderer::element::Element;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::{Display, Resource};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

mod common;

const OUTPUT: (i32, i32) = (1000, 700);
const TEXT: &[u8] = "Grüße, drag and drop".as_bytes();
const MIME: &str = "text/plain;charset=utf-8";
/// The type files are dragged in.
const URI_LIST: &str = "text/uri-list";
const SOURCE_SIZE: (i32, i32) = (600, 120);
const TARGET_SIZE: (i32, i32) = (120, 500);
/// The size of a source of files: taller than the file manager's window, so part of it is never under that window.
const FILE_SOURCE_SIZE: (i32, i32) = (120, 640);
/// The drag icon's size and the point of it under the pointer, in points: a size no other element of the scene has.
const ICON_SIZE: (i32, i32) = (37, 23);
const ICON_HOTSPOT: (i32, i32) = (9, 5);
/// The keycode of Escape (evdev 1, plus the 8 of the X keycodes).
const ESCAPE: u32 = 1 + 8;
/// The most pixels the returning icon of a drag is drawn wider and taller than it is, and from before its origin.
const NEAR: i32 = 3;

/// Whether `drawn`, an origin of the returning icon, is where an icon at `at` is drawn from.
fn drawn_at(drawn: (i32, i32), at: (f64, f64)) -> bool {
    let before = |d: i32, a: f64| (0..=NEAR).contains(&(a.round() as i32 - d));
    before(drawn.0, at.0) && before(drawn.1, at.1)
}

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    /// The roundtrips of each test client, for [`Harness::sync`].
    rounds: Vec<common::Rounds>,
}

impl Harness {
    fn new() -> Option<Harness> {
        Harness::with_scale(1.0)
    }

    fn with_scale(scale: f64) -> Option<Harness> {
        Harness::running(scale, Vec::new())
    }

    /// The compositor at `scale` supervising `processes`.
    fn running(scale: f64, processes: Vec<ShellProcess>) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let config = Config {
            shell_processes: processes,
            ..Config::default()
        };
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        match headless::init(&mut state, OUTPUT, scale) {
            Ok(()) => {
                start_shell_supervision(&state, &event_loop).expect("the supervision timer");
                Some(Harness {
                    state,
                    event_loop,
                    rounds: Vec::new(),
                })
            }
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Wait for every test client's [`common::Rounds::barrier`]: each has every event sent before, and the server
    /// handled their answers.
    fn sync(&mut self) {
        let barriers: Vec<_> = self.rounds.iter().map(common::Rounds::barrier).collect();
        self.wait_until("the clients' roundtrips", move |_| {
            barriers.iter().all(|b| b())
        });
    }

    /// One turn of the event loop.
    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// The frames of the mapped windows, the wide source's first, then the tall target's.
    fn frames(&self) -> Vec<(f64, f64, f64, f64)> {
        let mut frames: Vec<_> = self
            .state
            .windows
            .iter()
            .filter(|w| w.mapped)
            .filter_map(|w| self.state.wm.window(w.id))
            .map(|w| {
                let g = w.geometry();
                (g.x, g.y, g.w, g.h)
            })
            .collect();
        frames.sort_by(|a, b| (b.2 / b.3).total_cmp(&(a.2 / a.3)));
        frames
    }

    fn move_to(&mut self, at: (f64, f64)) {
        self.state.pointer_moved_to(at.into(), 0, None);
        self.sync();
    }

    /// The physical rectangle (x, y, width, height) of each element drawn of `size` points on the output at `scale`.
    fn drawn_of_size(&mut self, size: (i32, i32), scale: f64) -> Vec<(i32, i32, i32, i32)> {
        let mut backend = self.state.backend.take().unwrap();
        let elements = self.state.build_output_elements(backend.renderer(), 0);
        self.state.backend = Some(backend);
        let physical = (
            (f64::from(size.0) * scale).round() as i32,
            (f64::from(size.1) * scale).round() as i32,
        );
        elements
            .iter()
            .map(|e| e.geometry(smithay::utils::Scale::from(scale)))
            .filter(|g| (g.size.w, g.size.h) == physical)
            .map(|g| (g.loc.x, g.loc.y, g.size.w, g.size.h))
            .collect()
    }

    /// [`Harness::drawn_of_size`] at scale 1 for the returning icon of a drag, which lies on fractional points and is
    /// drawn with a margin of up to [`NEAR`] pixels: its origin is up to that much before the icon's.
    fn drawn_near_size(&mut self, size: (i32, i32)) -> Vec<(i32, i32, i32, i32)> {
        let mut backend = self.state.backend.take().unwrap();
        let elements = self.state.build_output_elements(backend.renderer(), 0);
        self.state.backend = Some(backend);
        elements
            .iter()
            .map(|e| e.geometry(smithay::utils::Scale::from(1.0)))
            .filter(|g| (0..=NEAR).contains(&(g.size.w - size.0)))
            .filter(|g| (0..=NEAR).contains(&(g.size.h - size.1)))
            .map(|g| (g.loc.x, g.loc.y, g.size.w, g.size.h))
            .collect()
    }

    /// Press Escape and let it go, and let the clients hear it.
    fn escape(&mut self) {
        for state in [KeyState::Pressed, KeyState::Released] {
            self.state
                .keyboard_event_now(Keycode::new(ESCAPE), state, 0);
        }
        self.sync();
    }

    /// Stops the compositor's clock where it is, so an animation moves only as [`Harness::advance`] moves it, however
    /// long the test waits for its clients.
    fn freeze(&mut self) {
        self.state.fixed_time = Some(self.state.now());
    }

    /// Moves the stopped clock on by `seconds`, and lets the compositor see it.
    fn advance(&mut self, seconds: f64) {
        let now = self.state.fixed_time.expect("the clock is stopped");
        self.state.fixed_time = Some(now + seconds);
        self.state.tick();
    }

    /// Where the icon of a drag that ended without a drop goes, while it shows.
    fn icon_going(&self) -> Option<Back> {
        self.state.fx.drag_return.as_ref().map(|r| r.back)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// Starts a drag of [`TEXT`] on a button press.
    Source,
    /// Starts a drag of [`TEXT`] on the first button press with an icon of [`ICON_SIZE`] drawn at this buffer scale,
    /// the pointer at [`ICON_HOTSPOT`].
    IconSource(i32),
    /// Accepts [`MIME`] for a copy and reads it on the drop.
    Target,
    /// Starts a drag of its payload as [`URI_LIST`], copy or move, on a button press.
    FileSource,
}

#[derive(Debug, Default, PartialEq)]
struct Log {
    /// Source: what the target said it accepts, the action chosen, the drop and the finish, in order.
    source: Vec<String>,
    /// Target: the types offered on enter, then the drop's bytes.
    offered: Vec<String>,
    dropped: Option<Vec<u8>>,
    /// Pointer events on the client's window: enter, leave and the buttons pressed and released, in order.
    pointer: Vec<String>,
    /// The protocol id of the icon surface the client gave `start_drag`.
    icon: Option<u32>,
    /// What the data device was told of drags over the client's window: enter, leave and drop, in order.
    device: Vec<String>,
    /// The keys the client was sent, as `key CODE pressed` or `key CODE released` (evdev codes).
    keys: Vec<String>,
}

struct ClientState {
    role: Role,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    size: (i32, i32),
    attached: bool,
    manager: wl_data_device_manager::WlDataDeviceManager,
    device: wl_data_device::WlDataDevice,
    offer: Option<wl_data_offer::WlDataOffer>,
    dragging: bool,
    /// What a source sends.
    payload: Vec<u8>,
    log: Arc<Mutex<Log>>,
    /// The window's roles, destroyed with its surface when the test closes the window.
    window: Option<(xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel)>,
}

type OfferTypes = Mutex<Vec<String>>;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ClientState {
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
        impl Dispatch<$t, ()> for ClientState {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    wl_seat::WlSeat,
    xdg_toplevel::XdgToplevel,
    wl_data_device_manager::WlDataDeviceManager
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for ClientState {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for ClientState {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            if !state.attached {
                state.attached = true;
                let (w, h) = state.size;
                let buffer = shm_buffer(&state.shm, qh, (w, h), 128);
                state.surface.attach(Some(&buffer), 0, 0);
                state.surface.damage_buffer(0, 0, w, h);
                state.surface.commit();
            }
        }
    }
}

/// An shm buffer of `size` pixels, every byte `value`.
fn shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<ClientState>,
    (w, h): (i32, i32),
    value: u8,
) -> wl_buffer::WlBuffer {
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![value; (w * h * 4) as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
}

impl Dispatch<wl_pointer::WlPointer, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let entry = match &event {
            wl_pointer::Event::Enter { .. } => "enter",
            wl_pointer::Event::Leave { .. } => "leave",
            wl_pointer::Event::Button {
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => "press",
            wl_pointer::Event::Button { .. } => "release",
            _ => return,
        };
        state.log.lock().unwrap().pointer.push(entry.into());
        let wl_pointer::Event::Button {
            serial,
            state: WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        else {
            return;
        };
        if let (Role::IconSource(scale), false) = (state.role, state.dragging) {
            state.dragging = true;
            let source = state.manager.create_data_source(qh, ());
            source.offer(MIME.into());
            source.set_actions(wl_data_device_manager::DndAction::Copy);
            let icon = state.compositor.create_surface(qh, ());
            state
                .device
                .start_drag(Some(&source), &state.surface, Some(&icon), serial);
            let (w, h) = (ICON_SIZE.0 * scale, ICON_SIZE.1 * scale);
            let buffer = shm_buffer(&state.shm, qh, (w, h), 200);
            let (hx, hy) = ICON_HOTSPOT;
            icon.set_buffer_scale(scale);
            if icon.version() >= 5 {
                icon.offset(-hx, -hy);
                icon.attach(Some(&buffer), 0, 0);
            } else {
                icon.attach(Some(&buffer), -hx, -hy);
            }
            icon.damage_buffer(0, 0, w, h);
            icon.commit();
            state.log.lock().unwrap().icon = Some(icon.id().protocol_id());
            return;
        }
        if state.role == Role::Source && !state.dragging {
            state.dragging = true;
            let source = state.manager.create_data_source(qh, ());
            source.offer(MIME.into());
            source.set_actions(wl_data_device_manager::DndAction::Copy);
            state
                .device
                .start_drag(Some(&source), &state.surface, None, serial);
        }
        if state.role == Role::FileSource && !state.dragging {
            state.dragging = true;
            let source = state.manager.create_data_source(qh, ());
            source.offer(URI_LIST.into());
            source.set_actions(
                wl_data_device_manager::DndAction::Copy | wl_data_device_manager::DndAction::Move,
            );
            state
                .device
                .start_drag(Some(&source), &state.surface, None, serial);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let entry = match event {
            wl_data_source::Event::Target { mime_type } => {
                format!("target {}", mime_type.as_deref().unwrap_or("none"))
            }
            wl_data_source::Event::Action {
                dnd_action: WEnum::Value(action),
            } => format!("action {action:?}"),
            wl_data_source::Event::Send { mime_type, fd } => {
                std::fs::File::from(fd).write_all(&state.payload).unwrap();
                format!("send {mime_type}")
            }
            wl_data_source::Event::DndDropPerformed => "drop performed".into(),
            wl_data_source::Event::DndFinished => "finished".into(),
            wl_data_source::Event::Cancelled => "cancelled".into(),
            _ => return,
        };
        let mut log = state.log.lock().unwrap();
        if log.source.last() != Some(&entry) {
            log.source.push(entry);
        }
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, OfferTypes> for ClientState {
    fn event(
        _: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        types: &OfferTypes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            types.lock().unwrap().push(mime_type);
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let entry = match &event {
            wl_data_device::Event::Enter { .. } => Some("enter"),
            wl_data_device::Event::Leave => Some("leave"),
            wl_data_device::Event::Drop => Some("drop"),
            _ => None,
        };
        if let Some(entry) = entry {
            state.log.lock().unwrap().device.push(entry.into());
        }
        if state.role != Role::Target {
            return;
        }
        match event {
            wl_data_device::Event::Enter {
                serial,
                id: Some(offer),
                ..
            } => {
                let types = offer.data::<OfferTypes>().unwrap().lock().unwrap().clone();
                offer.accept(serial, Some(MIME.into()));
                offer.set_actions(
                    wl_data_device_manager::DndAction::Copy,
                    wl_data_device_manager::DndAction::Copy,
                );
                state.log.lock().unwrap().offered = types;
                state.offer = Some(offer);
            }
            wl_data_device::Event::Drop => {
                let offer = state.offer.take().expect("a drop after an enter");
                let (read, write) =
                    rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
                offer.receive(MIME.into(), write.as_fd());
                drop(write);
                conn.flush().unwrap();
                let mut data = Vec::new();
                std::fs::File::from(read).read_to_end(&mut data).unwrap();
                offer.finish();
                offer.destroy();
                state.log.lock().unwrap().dropped = Some(data);
            }
            _ => {}
        }
    }

    event_created_child!(ClientState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, OfferTypes::default()),
    ]);
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: WEnum::Value(pressed),
            ..
        } = event
        {
            let pressed = match pressed {
                wl_keyboard::KeyState::Pressed => "pressed",
                _ => "released",
            };
            state
                .log
                .lock()
                .unwrap()
                .keys
                .push(format!("key {key} {pressed}"));
        }
    }
}

/// A toplevel of `size` with a pointer, a keyboard and a data device; serves its connection until dropped.
struct TestClient {
    stop: Arc<AtomicBool>,
    /// Set to close the window: the client destroys its toplevel and its surface, and keeps its drag going.
    close: Arc<AtomicBool>,
    log: Arc<Mutex<Log>>,
}

impl TestClient {
    fn spawn(h: &mut Harness, role: Role, size: (i32, i32)) -> TestClient {
        TestClient::sending(h, role, size, TEXT.to_vec())
    }

    /// A client whose drag sends `payload`.
    fn sending(h: &mut Harness, role: Role, size: (i32, i32), payload: Vec<u8>) -> TestClient {
        let socket = h.socket_path();
        let stop = Arc::new(AtomicBool::new(false));
        let close = Arc::new(AtomicBool::new(false));
        let log: Arc<Mutex<Log>> = Arc::default();
        let rounds = common::Rounds::default();
        h.rounds.push(rounds.clone());
        let (stop2, close2, log2) = (stop.clone(), close.clone(), log.clone());
        std::thread::spawn(move || {
            let _ended = rounds.ended_on_drop();
            let conn = Connection::from_socket(
                UnixStream::connect(&socket).expect("connecting to the compositor"),
            )
            .unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let manager: wl_data_device_manager::WlDataDeviceManager =
                globals.bind(&qh, 3..=3, ()).unwrap();
            let _pointer = seat.get_pointer(&qh, ());
            let _keyboard = seat.get_keyboard(&qh, ());
            let device = manager.get_data_device(&seat, &qh, ());
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(match role {
                Role::Source | Role::IconSource(_) | Role::FileSource => {
                    "org.example.drag-source".into()
                }
                Role::Target => "org.example.drop-target".into(),
            });
            surface.commit();
            let mut client = ClientState {
                role,
                compositor,
                shm,
                surface,
                size,
                attached: false,
                manager,
                device,
                offer: None,
                dragging: false,
                payload,
                log: log2,
                window: Some((xdg, toplevel)),
            };
            while !stop2.load(Ordering::SeqCst) {
                if close2.load(Ordering::SeqCst) {
                    if let Some((xdg, toplevel)) = client.window.take() {
                        toplevel.destroy();
                        xdg.destroy();
                        client.surface.destroy();
                    }
                }
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                rounds.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        TestClient { stop, close, log }
    }

    /// Close the client's window, keeping the connection and any drag it started.
    fn close_window(&self) {
        self.close.store(true, Ordering::SeqCst);
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().unwrap()
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A drag of text from the source's window held over the target's window, which chose to copy it.
fn drag_held_over_the_target(h: &mut Harness) -> (TestClient, TestClient) {
    let (source, target, _) = drag_as_held_over_the_target(h, Role::Source);
    (source, target)
}

/// [`drag_held_over_the_target`] by a source of `role`, with the point the drag started at.
fn drag_as_held_over_the_target(
    h: &mut Harness,
    role: Role,
) -> (TestClient, TestClient, (f64, f64)) {
    // the wide source and the tall target cross, so each has an end the other cannot cover, whichever is on top
    let target = TestClient::spawn(h, Role::Target, TARGET_SIZE);
    h.wait_until("the target window", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 1
    });
    let source = TestClient::spawn(h, role, SOURCE_SIZE);
    h.wait_until("the source window", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 2
    });
    h.sync();
    let frames = h.frames();
    let (sx, sy, _, sh) = frames[0];
    let (tx, ty, tw, th) = frames[1];
    let start = (sx + 30.0, sy + sh / 2.0);
    let end = (tx + tw / 2.0, ty + th - 30.0);
    assert!(
        !(tx..tx + tw).contains(&start.0) && !(sy..sy + sh).contains(&end.1),
        "the windows must cross: source {:?}, target {:?}",
        frames[0],
        frames[1]
    );

    h.move_to(start);
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    h.sync();
    for step in 1..=10 {
        let t = f64::from(step) / 10.0;
        h.move_to((
            start.0 + (end.0 - start.0) * t,
            start.1 + (end.1 - start.1) * t,
        ));
    }
    h.wait_until("the target to choose a copy", |_| {
        source
            .log()
            .source
            .iter()
            .any(|e| e == "action DndAction(Copy)")
    });
    (source, target, start)
}

/// A press in one client's window starts a drag of text; moving onto another client's window and releasing there
/// drops it: the target was offered the text, read it whole, and the source saw the copy performed and finished.
#[test]
fn text_dragged_from_one_client_drops_into_another() {
    let Some(mut h) = Harness::new() else { return };
    let (source, target) = drag_held_over_the_target(&mut h);
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    h.wait_until("the drop to finish", |_| {
        source.log().source.last().map(String::as_str) == Some("finished")
    });

    assert_eq!(target.log().offered, [MIME]);
    assert_eq!(target.log().dropped.as_deref(), Some(TEXT));
    assert_eq!(
        source.log().source,
        [
            "action DndAction(Copy)".to_string(),
            "drop performed".to_string(),
            format!("send {MIME}"),
            "finished".to_string(),
        ]
    );
}

/// The session locks while a drag is held over another client's window: the lock ends the drag, and the target, which
/// had accepted the text, is not given it.
#[test]
fn a_drag_held_when_the_session_locks_gives_the_target_nothing() {
    let Some(mut h) = Harness::new() else { return };
    let (_source, target) = drag_held_over_the_target(&mut h);
    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the lock", |_| locker.locked());
    h.wait_until("the target to read the drop", |_| {
        target.log().dropped.is_some()
    });
    assert_eq!(target.log().dropped.as_deref(), Some(&b""[..]));
}

/// A drag that names an icon surface: the compositor takes that surface as the drag's icon, with the pointer at the
/// hotspot the client set with the surface's offset, and draws it there, at the output's scale, as the pointer moves.
#[test]
fn a_drag_icon_is_the_clients_surface_drawn_with_the_pointer_at_its_hotspot() {
    for scale in [1, 2] {
        let Some(mut h) = Harness::with_scale(f64::from(scale)) else {
            return;
        };
        let source = TestClient::spawn(&mut h, Role::IconSource(scale), SOURCE_SIZE);
        h.wait_until("the source window", |s| {
            s.windows.iter().filter(|w| w.mapped).count() == 1
        });
        h.sync();
        let (sx, sy, sw, sh) = h.frames()[0];
        let start = (sx + 40.0, sy + sh / 2.0);
        h.move_to(start);
        h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
        h.wait_until("the drag icon", |s| s.drag_icon.is_some());
        h.sync();
        let at = (sx + sw / 2.0, sy + sh / 2.0 + 7.0);
        h.move_to(at);

        let (icon, offset) = h.state.drag_icon.clone().unwrap();
        assert_eq!(
            Some(icon.id().protocol_id()),
            source.log().icon,
            "at {scale}x"
        );
        assert_eq!(
            (offset.x, offset.y),
            (-ICON_HOTSPOT.0, -ICON_HOTSPOT.1),
            "at {scale}x"
        );
        let s = f64::from(scale);
        let expected = (
            ((at.0 - f64::from(ICON_HOTSPOT.0)) * s).round() as i32,
            ((at.1 - f64::from(ICON_HOTSPOT.1)) * s).round() as i32,
            ICON_SIZE.0 * scale,
            ICON_SIZE.1 * scale,
        );
        assert_eq!(h.drawn_of_size(ICON_SIZE, s), [expected], "at {scale}x");
    }
}

/// A drop released where nothing accepts it (the source's own window, which takes no drops) is refused: the source
/// hears the cancel, and the icon, no longer the client's surface, flies from where it was let go back to where the
/// drag started and is gone once [`DRAG_RETURN_DURATION`] has passed; the window gets the pointer back and the next
/// press reaches it.
#[test]
fn a_refused_drop_flies_the_icon_back_and_gives_the_pointer_back() {
    let Some(mut h) = Harness::new() else { return };
    let source = TestClient::spawn(&mut h, Role::IconSource(1), SOURCE_SIZE);
    h.wait_until("the source window", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 1
    });
    h.sync();
    let (sx, sy, _, sh) = h.frames()[0];
    let start = (sx + 40.0, sy + sh / 2.0);
    h.move_to(start);
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    h.wait_until("the drag icon", |s| s.drag_icon.is_some());
    h.move_to((start.0 + 60.0, start.1));
    h.wait_until("the pointer to leave the source for the drag", |_| {
        source.log().pointer.last().map(String::as_str) == Some("leave")
    });
    assert_eq!(h.drawn_of_size(ICON_SIZE, 1.0).len(), 1);

    h.freeze();
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    h.wait_until("the cancel", |_| {
        source.log().source.last().map(String::as_str) == Some("cancelled")
    });
    assert!(
        h.state.drag_icon.is_none(),
        "no longer the client's surface"
    );
    let (hx, hy) = (f64::from(ICON_HOTSPOT.0), f64::from(ICON_HOTSPOT.1));
    assert_eq!(h.icon_going(), Some(Back::To(start.0 - hx, start.1 - hy)));
    let first = h.drawn_near_size(ICON_SIZE);
    assert_eq!(first.len(), 1, "the icon still shows: {first:?}");
    assert!(
        drawn_at(
            (first[0].0, first[0].1),
            (start.0 + 60.0 - hx, start.1 - hy)
        ),
        "where it was let go: {first:?}"
    );
    h.advance(DRAG_RETURN_DURATION / 4.0);
    let half = h.drawn_near_size(ICON_SIZE);
    assert_eq!(half.len(), 1, "the icon still shows: {half:?}");
    assert!(
        half[0].0 < first[0].0 && half[0].0 >= (start.0 - hx).round() as i32 - NEAR,
        "on its way back: {half:?} after {first:?}"
    );
    assert!(
        (0..=NEAR).contains(&((start.1 - hy).round() as i32 - half[0].1)),
        "level: {half:?}"
    );
    h.advance(DRAG_RETURN_DURATION);
    assert!(h.drawn_near_size(ICON_SIZE).is_empty(), "back and gone");
    assert_eq!(h.icon_going(), None);
    h.move_to((start.0 + 70.0, start.1));
    h.wait_until("the pointer back on the source", |_| {
        source.log().pointer.last().map(String::as_str) == Some("enter")
    });
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    h.wait_until("the next click", |_| {
        source
            .log()
            .pointer
            .ends_with(&["press".into(), "release".into()])
    });
    assert!(
        h.state.drag_icon.is_none(),
        "a press after a drag starts none"
    );
}

/// Escape while a drag is held over a target that chose to take it: the drag ends without a drop. The source hears
/// it cancelled, the target is left and never dropped on, the key reaches no client, and the icon flies back to where
/// the drag started. The window under the pointer gets the pointer back at once, and the release that follows drops
/// nothing.
#[test]
fn escape_cancels_a_drag_held_over_a_target_and_flies_the_icon_back() {
    let Some(mut h) = Harness::new() else { return };
    let (source, target, start) = drag_as_held_over_the_target(&mut h, Role::IconSource(1));
    assert!(h.state.drag_icon.is_some());
    assert_eq!(
        target.log().device.last().map(String::as_str),
        Some("enter")
    );
    h.freeze();
    h.escape();
    h.wait_until("the cancel", |_| {
        source.log().source.last().map(String::as_str) == Some("cancelled")
    });
    h.wait_until("the target left", |_| {
        target.log().device.last().map(String::as_str) == Some("leave")
    });
    let (hx, hy) = (f64::from(ICON_HOTSPOT.0), f64::from(ICON_HOTSPOT.1));
    assert_eq!(h.icon_going(), Some(Back::To(start.0 - hx, start.1 - hy)));
    assert_eq!(
        h.drawn_near_size(ICON_SIZE).len(),
        1,
        "the icon still shows"
    );
    h.wait_until("the pointer back on the target", |_| {
        target.log().pointer.last().map(String::as_str) == Some("enter")
    });
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    // the clients heard all the release led to before they are asked what they did not hear
    h.sync();
    h.advance(DRAG_RETURN_DURATION);
    assert!(h.drawn_near_size(ICON_SIZE).is_empty(), "back and gone");
    assert_eq!(h.icon_going(), None);
    assert_eq!(target.log().dropped, None, "the target got no drop");
    assert!(
        !target.log().device.iter().any(|e| e == "drop"),
        "{:?}",
        target.log().device
    );
    assert!(
        !source.log().source.iter().any(|e| e == "drop performed"),
        "{:?}",
        source.log().source
    );
    assert!(h.state.drag_icon.is_none());
    let escape = format!("key {}", ESCAPE - 8);
    for client in [&source, &target] {
        let keys = client.log().keys.clone();
        assert!(!keys.iter().any(|k| k.starts_with(&escape)), "{keys:?}");
    }
    // with no drag, Escape is the focused client's again: the source's, where the drag was pressed
    h.escape();
    h.wait_until("Escape to reach the source", |_| {
        source
            .log()
            .keys
            .ends_with(&[format!("{escape} pressed"), format!("{escape} released")])
    });
}

/// The window a drag started in closes while the drag is held, and the drag is released where nothing takes it: the
/// icon has nowhere to fly back to, so it fades out where it was let go, and is gone once [`DRAG_FADE_DURATION`] has
/// passed.
#[test]
fn a_refused_drop_whose_window_closed_fades_the_icon_where_it_was_let_go() {
    let Some(mut h) = Harness::new() else { return };
    let source = TestClient::spawn(&mut h, Role::IconSource(1), SOURCE_SIZE);
    h.wait_until("the source window", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 1
    });
    h.sync();
    let (sx, sy, _, sh) = h.frames()[0];
    let start = (sx + 40.0, sy + sh / 2.0);
    h.move_to(start);
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    h.wait_until("the drag icon", |s| s.drag_icon.is_some());
    // below the window, where nothing is
    let end = (start.0 + 60.0, sy + sh + 80.0);
    h.move_to(end);
    source.close_window();
    h.wait_until("the window to close", |s| {
        s.windows.iter().all(|w| !w.mapped)
    });
    h.freeze();
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    h.wait_until("the cancel", |_| {
        source.log().source.last().map(String::as_str) == Some("cancelled")
    });
    assert_eq!(h.icon_going(), Some(Back::Fade));
    let (hx, hy) = (f64::from(ICON_HOTSPOT.0), f64::from(ICON_HOTSPOT.1));
    let at = (end.0 - hx, end.1 - hy);
    let first = h.drawn_near_size(ICON_SIZE);
    assert_eq!(first.len(), 1, "the icon still shows: {first:?}");
    assert!(
        drawn_at((first[0].0, first[0].1), at),
        "where it was let go: {first:?}, {at:?}"
    );
    h.advance(DRAG_FADE_DURATION / 2.0);
    let half = h.drawn_near_size(ICON_SIZE);
    assert_eq!(half.len(), 1, "still fading: {half:?}");
    assert_eq!(half[0], first[0], "it does not move");
    let now = h.state.now();
    let alpha = h.state.fx.drag_return.as_ref().unwrap().placement(now).1;
    assert!((alpha - 0.5).abs() < 1e-6, "half faded: {alpha}");
    h.advance(DRAG_FADE_DURATION / 2.0);
    assert!(h.drawn_near_size(ICON_SIZE).is_empty(), "faded out");
    assert_eq!(h.icon_going(), None);
}

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The file manager, built before the test runner starts, with `home` as its home folder and its configuration and
/// data directories inside it, so that its sidebar does not depend on the host: `data_dirs` stands for
/// `$XDG_DATA_DIRS`. Its session bus is the test's own `bus`: Files serves one process per session bus name, and
/// another test's Files on a shared bus would take its launch.
fn files_process(
    home: &std::path::Path,
    data_dirs: &std::path::Path,
    bus: &PrivateBus,
) -> ShellProcess {
    let env = |name: &str, value: &std::path::Path| format!("{name}={}", value.display());
    let binary = common::fixtures::artifact("cupertino-files", "bin", "cupertino-files");
    ShellProcess {
        name: "cupertino-files".into(),
        command: vec![
            "env".into(),
            "CUPERTINO_ASSET_SEARCH_PATH=".into(),
            format!("DBUS_SESSION_BUS_ADDRESS={}", bus.address),
            env("HOME", home),
            env("XDG_CONFIG_HOME", &home.join(".config")),
            env("XDG_DATA_HOME", &home.join(".local/share")),
            env("XDG_DATA_DIRS", data_dirs),
            binary.display().to_string(),
        ],
        roles: Vec::new(),
    }
}

/// A file dragged from another client into the file manager's window, a CupertinoKit window, and released over the
/// folder it shows moves there: the window took the URI list for a move, read it and finished the drop, and by then
/// the file had left its folder for the one shown.
#[test]
fn a_file_dragged_from_another_client_into_the_file_manager_moves_into_its_folder() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("cupertino-drop-into-files-{}", std::process::id())),
    );
    let home = scratch.0.join("home");
    let outside = scratch.0.join("outside");
    let data_dirs = scratch.0.join("share");
    for dir in [&home, &outside, &data_dirs.join("applications")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let dragged = outside.join("dropped.txt");
    std::fs::write(&dragged, b"dropped").unwrap();
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let Some(mut h) = Harness::running(1.0, vec![files_process(&home, &data_dirs, &bus)]) else {
        return;
    };
    h.wait_until("the file manager's window", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 1
    });
    let uri = format!("file://{}\r\n", dragged.display());
    let source = TestClient::sending(&mut h, Role::FileSource, FILE_SOURCE_SIZE, uri.into_bytes());
    h.wait_until("the source window", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 2
    });
    h.sync();
    // the wide file manager first, then the tall source
    let frames = h.frames();
    let (fx, fy, fw, fh) = frames[0];
    let (sx, sy, sw, sh) = frames[1];
    // the part of the source below the file manager, and the empty folder area low on the trailing side
    let below = (fy + fh, (sy + sh).min(f64::from(OUTPUT.1)));
    let start = (sx + sw / 2.0, (below.0 + below.1) / 2.0);
    let end = (fx + fw - 80.0, fy + fh - 60.0);
    let inside = |(x, y, w, h): (f64, f64, f64, f64), at: (f64, f64)| {
        (x..x + w).contains(&at.0) && (y..y + h).contains(&at.1)
    };
    assert!(
        below.1 - below.0 > 20.0 && !inside(frames[1], end),
        "each window must have a part the other leaves uncovered: files {:?}, source {:?}",
        frames[0],
        frames[1]
    );

    h.move_to(start);
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    h.sync();
    for step in 1..=10 {
        let t = f64::from(step) / 10.0;
        h.move_to((
            start.0 + (end.0 - start.0) * t,
            start.1 + (end.1 - start.1) * t,
        ));
    }
    let end_at = Instant::now() + common::HANG_GUARD;
    let moved = |source: &TestClient| {
        let log = source.log();
        log.source.iter().any(|e| e == "action DndAction(Move)")
    };
    while !moved(&source) && Instant::now() < end_at {
        h.step();
    }
    assert!(
        moved(&source),
        "the file manager took no move: {:?}, files at {:?}, source at {:?}",
        source.log(),
        frames[0],
        frames[1]
    );
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    h.wait_until("the drop to finish", |_| {
        source.log().source.last().map(String::as_str) == Some("finished")
    });
    // what the pointer crossed on the way may have taken or refused the type: the steps from the move on count
    let steps: Vec<String> = source
        .log()
        .source
        .iter()
        .skip_while(|e| *e != "action DndAction(Move)")
        .filter(|e| !e.starts_with("target "))
        .cloned()
        .collect();
    assert_eq!(
        steps,
        [
            "action DndAction(Move)".to_string(),
            "drop performed".to_string(),
            format!("send {URI_LIST}"),
            "finished".to_string(),
        ]
    );
    assert!(!dragged.exists(), "the file left its folder");
    assert_eq!(
        std::fs::read(home.join("dropped.txt")).unwrap(),
        b"dropped",
        "the file is in the folder the window shows"
    );
}
