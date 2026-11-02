//! Drag and drop between two real Wayland clients: one starts a drag of text from a button press in its window, the
//! pointer moves onto the other's window and is released there, and the other reads the dropped text. The compositor
//! runs in-process on an offscreen EGL device. Tests skip (pass) when no EGL device is available.

use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::events::BTN_LEFT;
use cupertino_server::Cupertino;
use smithay::backend::input::ButtonState;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

mod common;

const OUTPUT: (i32, i32) = (1000, 700);
const TEXT: &[u8] = "Grüße, drag and drop".as_bytes();
const MIME: &str = "text/plain;charset=utf-8";
const SOURCE_SIZE: (i32, i32) = (600, 120);
const TARGET_SIZE: (i32, i32) = (120, 500);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
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

    fn run_for(&mut self, duration: Duration) {
        let end = Instant::now() + duration;
        self.wait_until("the time to pass", |_| Instant::now() >= end);
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
        self.run_for(Duration::from_millis(60));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// Starts a drag of [`TEXT`] on a button press.
    Source,
    /// Accepts [`MIME`] for a copy and reads it on the drop.
    Target,
}

#[derive(Debug, Default, PartialEq)]
struct Log {
    /// Source: what the target said it accepts, the action chosen, the drop and the finish, in order.
    source: Vec<String>,
    /// Target: the types offered on enter, then the drop's bytes.
    offered: Vec<String>,
    dropped: Option<Vec<u8>>,
}

struct ClientState {
    role: Role,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    size: (i32, i32),
    attached: bool,
    manager: wl_data_device_manager::WlDataDeviceManager,
    device: wl_data_device::WlDataDevice,
    offer: Option<wl_data_offer::WlDataOffer>,
    dragging: bool,
    log: Arc<Mutex<Log>>,
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
                let fd =
                    rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC)
                        .unwrap();
                let mut file = std::fs::File::from(fd);
                file.write_all(&vec![128u8; (w * h * 4) as usize]).unwrap();
                let pool = state.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
                let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
                pool.destroy();
                state.surface.attach(Some(&buffer), 0, 0);
                state.surface.damage_buffer(0, 0, w, h);
                state.surface.commit();
            }
        }
    }
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
        let wl_pointer::Event::Button {
            serial,
            state: WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        else {
            return;
        };
        if state.role == Role::Source && !state.dragging {
            state.dragging = true;
            let source = state.manager.create_data_source(qh, ());
            source.offer(MIME.into());
            source.set_actions(wl_data_device_manager::DndAction::Copy);
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
                std::fs::File::from(fd).write_all(TEXT).unwrap();
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

/// A toplevel of `size` with a pointer and a data device; serves its connection until dropped.
struct TestClient {
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Log>>,
}

impl TestClient {
    fn spawn(socket: PathBuf, role: Role, size: (i32, i32)) -> TestClient {
        let stop = Arc::new(AtomicBool::new(false));
        let log: Arc<Mutex<Log>> = Arc::default();
        let (stop2, log2) = (stop.clone(), log.clone());
        std::thread::spawn(move || {
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
            let device = manager.get_data_device(&seat, &qh, ());
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(match role {
                Role::Source => "org.example.drag-source".into(),
                Role::Target => "org.example.drop-target".into(),
            });
            surface.commit();
            let mut client = ClientState {
                role,
                shm,
                surface,
                size,
                attached: false,
                manager,
                device,
                offer: None,
                dragging: false,
                log: log2,
            };
            while !stop2.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        TestClient { stop, log }
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
    // the wide source and the tall target cross, so each has an end the other cannot cover, whichever is on top
    let target = TestClient::spawn(h.socket_path(), Role::Target, TARGET_SIZE);
    h.wait_until("the target window", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 1
    });
    let source = TestClient::spawn(h.socket_path(), Role::Source, SOURCE_SIZE);
    h.wait_until("the source window", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 2
    });
    h.run_for(Duration::from_millis(100));
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
    h.run_for(Duration::from_millis(100));
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
    (source, target)
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
