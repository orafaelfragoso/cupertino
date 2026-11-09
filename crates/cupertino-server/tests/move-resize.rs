//! A client may start a move or resize of its window only with the serial of a pointer press that is still held on it,
//! not with that of a key or focus change that came after the press. Tests skip (pass) when no EGL device is available.

mod common;

use std::io::Write;
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
use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
const WINDOW: (i32, i32) = (400, 300);
/// The key typed while the press is held: A, as an xkb keycode.
const KEY_A: u32 = 38;

#[derive(Clone, Copy, Debug)]
enum Ask {
    Move(u32),
    Resize(u32),
}

#[derive(Default)]
struct Log {
    /// Serial of the last button press, and whether that button is still down.
    press: Option<(u32, bool)>,
    /// Serial of the last key event.
    key: Option<u32>,
    drawn: bool,
}

struct Client {
    log: Arc<Mutex<Log>>,
    shm: wl_shm::WlShm,
    window: wl_surface::WlSurface,
    window_xdg: xdg_surface::XdgSurface,
    buffers: Vec<wl_buffer::WlBuffer>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
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
        impl Dispatch<$t, ()> for Client {
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
    xdg_toplevel::XdgToplevel
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
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
            if *surface == state.window_xdg && !state.log.lock().unwrap().drawn {
                let (w, h) = WINDOW;
                let fd = rustix::fs::memfd_create("move-resize", rustix::fs::MemfdFlags::CLOEXEC)
                    .unwrap();
                let mut file = std::fs::File::from(fd);
                file.write_all(&vec![200u8; (w * h * 4) as usize]).unwrap();
                let pool = state.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
                let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
                pool.destroy();
                state.window.attach(Some(&buffer), 0, 0);
                state.window.damage_buffer(0, 0, w, h);
                state.window.commit();
                state.buffers.push(buffer);
                state.log.lock().unwrap().drawn = true;
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Client {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
            }
            if caps.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            serial,
            state: WEnum::Value(button),
            ..
        } = event
        {
            let down = button == wl_pointer::ButtonState::Pressed;
            let mut log = state.log.lock().unwrap();
            if down {
                log.press = Some((serial, true));
            } else if let Some((s, _)) = log.press {
                log.press = Some((s, false));
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key { serial, .. } = event {
            state.log.lock().unwrap().key = Some(serial);
        }
    }
}

struct TestClient {
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Log>>,
    asks: Arc<Mutex<Vec<Ask>>>,
}

impl TestClient {
    fn spawn(socket: PathBuf) -> TestClient {
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Log::default()));
        let asks: Arc<Mutex<Vec<Ask>>> = Arc::default();
        let (stop2, log2, asks2) = (stop.clone(), log.clone(), asks.clone());
        std::thread::spawn(move || {
            let conn = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
            let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=5, ()).unwrap();
            let window = compositor.create_surface(&qh, ());
            let window_xdg = wm_base.get_xdg_surface(&window, &qh, ());
            let toplevel = window_xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.move-resize".into());
            window.commit();
            let mut client = Client {
                log: log2,
                shm,
                window,
                window_xdg,
                buffers: Vec::new(),
            };
            while !stop2.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                for ask in asks2.lock().unwrap().drain(..) {
                    match ask {
                        Ask::Move(serial) => toplevel._move(&seat, serial),
                        Ask::Resize(serial) => {
                            toplevel.resize(&seat, serial, xdg_toplevel::ResizeEdge::BottomRight)
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        TestClient { stop, log, asks }
    }

    fn ask(&self, ask: Ask) {
        self.asks.lock().unwrap().push(ask);
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

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

    fn run_for(&mut self, duration: Duration) {
        let end = Instant::now() + duration;
        self.wait_until("the time to pass", |_| Instant::now() >= end);
    }

    fn socket_path(&self) -> PathBuf {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(self.state.socket_name.as_ref().unwrap())
    }

    /// Press the button over the window's content and move the pointer there first; the press stays down.
    fn press(&mut self, client: &TestClient) -> u32 {
        let window = self.state.windows.iter().find(|e| e.mapped).unwrap().id;
        let frame = self.state.wm.window(window).unwrap().geometry();
        self.state
            .pointer_moved_to((frame.x + 120.0, frame.y + 150.0).into(), 0, None);
        self.run_for(Duration::from_millis(60));
        self.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
        self.wait_until("the press to reach the client", |_| {
            client
                .log
                .lock()
                .unwrap()
                .press
                .is_some_and(|(_, down)| down)
        });
        client.log.lock().unwrap().press.unwrap().0
    }

    fn release(&mut self) {
        self.state
            .pointer_button(BTN_LEFT, ButtonState::Released, 0);
        self.run_for(Duration::from_millis(60));
    }
}

fn mapped_client(h: &mut Harness) -> TestClient {
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the window to map", |s| s.windows.iter().any(|e| e.mapped));
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    h.wait_until("the window to draw", |_| client.log.lock().unwrap().drawn);
    h.run_for(Duration::from_millis(100));
    client
}

#[test]
fn a_move_and_a_resize_with_the_serial_of_a_held_press_start() {
    let Some(mut h) = Harness::new() else { return };
    let client = mapped_client(&mut h);
    let serial = h.press(&client);
    client.ask(Ask::Move(serial));
    h.wait_until("the move to start", |s| s.wm.is_interacting());
    h.release();
    assert!(!h.state.wm.is_interacting(), "the release ends it");

    let serial = h.press(&client);
    client.ask(Ask::Resize(serial));
    h.wait_until("the resize to start", |s| s.wm.is_interacting());
    h.release();
    assert!(!h.state.wm.is_interacting());
}

#[test]
fn a_stale_released_or_made_up_serial_starts_nothing() {
    let Some(mut h) = Harness::new() else { return };
    let client = mapped_client(&mut h);
    // a serial no press ever had
    client.ask(Ask::Move(1));
    client.ask(Ask::Resize(4242));
    h.run_for(Duration::from_millis(300));
    assert!(!h.state.wm.is_interacting(), "no press at all");

    // the serial of a press that has been released
    let serial = h.press(&client);
    h.release();
    client.ask(Ask::Move(serial));
    client.ask(Ask::Resize(serial));
    h.run_for(Duration::from_millis(300));
    assert!(
        !h.state.wm.is_interacting(),
        "the button is up: nothing follows the pointer"
    );

    // a held press, but another serial
    let serial = h.press(&client);
    client.ask(Ask::Move(serial.wrapping_add(1000)));
    h.run_for(Duration::from_millis(300));
    assert!(
        !h.state.wm.is_interacting(),
        "not the serial of the held press"
    );
    h.release();
}

#[test]
fn a_move_with_the_serial_of_a_key_after_the_held_press_starts_nothing_and_the_press_serial_does() {
    let Some(mut h) = Harness::new() else { return };
    let client = mapped_client(&mut h);
    let press = h.press(&client);
    // the pointer moves on with the button down and a key goes down and up, as before a toolbar drag starts a move
    let window = h.state.windows.iter().find(|e| e.mapped).unwrap().id;
    let frame = h.state.wm.window(window).unwrap().geometry();
    h.state
        .pointer_moved_to((frame.x + 140.0, frame.y + 160.0).into(), 0, None);
    let key = Keycode::new(KEY_A);
    h.state.keyboard_event_now(key, KeyState::Pressed, 0);
    h.state.keyboard_event_now(key, KeyState::Released, 0);
    h.wait_until("the key to reach the client", |_| {
        client.log.lock().unwrap().key.is_some()
    });
    let later = client.log.lock().unwrap().key.unwrap();
    assert_ne!(later, press);

    client.ask(Ask::Move(later));
    client.ask(Ask::Resize(later));
    h.run_for(Duration::from_millis(300));
    assert!(
        !h.state.wm.is_interacting(),
        "the serial of a key after the press is stale"
    );

    client.ask(Ask::Move(press));
    h.wait_until("the move to start", |s| s.wm.is_interacting());
    h.release();
    assert!(!h.state.wm.is_interacting(), "the release ends it");
}
