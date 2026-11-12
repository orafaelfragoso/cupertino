//! Pointer button pairing: every `wl_pointer.button` release a client receives must follow the press it received, and a
//! press a client received must always be followed by its release. The compositor runs in-process on an offscreen EGL
//! device and real clients record the button events they get. Tests skip (pass) when no EGL device is available.

use crate::common;

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
use cupertino_wm::WindowId;
use smithay::backend::input::ButtonState;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
/// Height of the server-drawn title bar.
const TITLE_BAR: f64 = 32.0;
/// Linux evdev code of the secondary button.
const BTN_RIGHT: u32 = 0x111;

/// `(button, pressed)` in the order a client received them.
type ButtonLog = Arc<Mutex<Vec<(u32, bool)>>>;

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
        common::each_connection_a_process(&mut state);
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Run the compositor long enough for the clients to receive what it sent.
    fn settle(&mut self) {
        for _ in 0..20 {
            self.step();
        }
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Wait for `count` mapped windows and settle model time so no open animation is running.
    fn mapped(&mut self, count: usize) {
        self.wait_until("the windows to be mapped", |s| {
            s.wm.windows().count() == count
        });
        let t = self.state.now() + 5.0;
        let time = t;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        self.settle();
    }

    /// Move model time on by `seconds` and let the animations settle.
    fn advance(&mut self, seconds: f64) {
        let t = self.state.now() + seconds;
        let time = t;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        self.settle();
    }

    fn frame(&self, id: WindowId) -> cupertino_wm::Rect {
        self.state.wm.window(id).unwrap().geometry()
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.state.pointer_moved_to((x, y).into(), 0, None);
    }

    fn button(&mut self, button: u32, state: ButtonState) {
        self.state.pointer_button(button, state, 0);
    }

    /// First point of the window's content that no window above it covers.
    fn content_point(&self, id: WindowId) -> (f64, f64) {
        let f = self.frame(id);
        for dy in (TITLE_BAR as i32 + 4..f.h as i32 - 4).step_by(8) {
            for dx in (4..f.w as i32 - 4).step_by(8) {
                let p = (f.x + f64::from(dx), f.y + f64::from(dy));
                if self
                    .state
                    .window_surface_under(p.into())
                    .is_some_and(|(w, _, _)| w == id)
                {
                    return p;
                }
            }
        }
        panic!("window {id:?} has no uncovered content point");
    }
}

// ---- test client ---------------------------------------------------------------------------

struct ClientState {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    size: (i32, i32),
    attached: bool,
    log: ButtonLog,
    /// Ask for an interactive move on the first press, as a client-drawn title bar does.
    move_on_press: Option<(xdg_toplevel::XdgToplevel, wl_seat::WlSeat)>,
}

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
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1
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
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            serial,
            button,
            state: WEnum::Value(s),
            ..
        } = event
        {
            state
                .log
                .lock()
                .unwrap()
                .push((button, s == wl_pointer::ButtonState::Pressed));
            if let (wl_pointer::ButtonState::Pressed, Some((toplevel, seat))) =
                (s, state.move_on_press.take())
            {
                toplevel._move(&seat, serial);
            }
        }
    }
}

/// A server-decorated toplevel with a pointer; keeps the connection served until dropped.
struct TestClient {
    stop: Arc<AtomicBool>,
    log: ButtonLog,
}

impl TestClient {
    fn spawn(
        socket: PathBuf,
        app_id: &'static str,
        size: (i32, i32),
        move_on_press: bool,
    ) -> TestClient {
        let stop = Arc::new(AtomicBool::new(false));
        let log: ButtonLog = Arc::default();
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
            let _pointer = seat.get_pointer(&qh, ());
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            let manager: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let _decoration = manager.get_toplevel_decoration(&toplevel, &qh, ());
            _decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            surface.commit();
            let move_on_press = move_on_press.then(|| (toplevel.clone(), seat.clone()));
            let mut client = ClientState {
                shm,
                surface,
                size,
                attached: false,
                log: log2,
                move_on_press,
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

    fn log(&self) -> Vec<(u32, bool)> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A press the server consumes (title bar, server UI) is no client's press, so its release is no client's either.
#[test]
fn a_release_after_a_press_on_a_title_bar_never_reaches_the_client() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path(), "org.example.a", (400, 300), false);
    h.mapped(1);
    let id = h.state.wm.key_window().unwrap();
    let frame = h.frame(id);
    let (cx, cy) = h.content_point(id);

    // the secondary button on the title bar is consumed by the server; the pointer then enters the content
    h.move_to(frame.x + 250.0, frame.y + 10.0);
    h.button(BTN_RIGHT, ButtonState::Pressed);
    h.move_to(cx, cy);
    h.button(BTN_RIGHT, ButtonState::Released);
    h.settle();
    assert_eq!(
        client.log(),
        vec![],
        "the client saw a button event for a click the server consumed"
    );
}

#[test]
fn a_release_after_a_press_on_server_ui_never_reaches_the_window_mapped_under_the_pointer() {
    let Some(mut h) = Harness::new() else { return };
    h.state.toggle_overview(false);
    assert!(
        h.state.shell.overview_open(),
        "the overview opened on an empty desktop"
    );
    let center = (f64::from(OUTPUT.0) / 2.0, f64::from(OUTPUT.1) / 2.0);

    // the press lands on the overview over an empty desktop, which launches a window under the pointer
    h.move_to(center.0, center.1);
    h.button(BTN_LEFT, ButtonState::Pressed);
    let client = TestClient::spawn(h.socket_path(), "org.example.a", (400, 300), false);
    h.mapped(1);
    let id = h.state.wm.key_window().unwrap();
    let (cx, cy) = h.content_point(id);
    h.move_to(cx, cy);
    h.button(BTN_LEFT, ButtonState::Released);
    h.settle();
    assert_eq!(
        client.log(),
        vec![],
        "the window mapped after the press got a release without its press"
    );
}

/// A press a client receives stays with that client until its release, wherever the pointer goes.
#[test]
fn a_client_that_got_a_press_gets_its_release_after_the_pointer_moved_to_another_window() {
    let Some(mut h) = Harness::new() else { return };
    // a larger first window: two applications' first windows are both centred, so the second one
    // covers only the middle of the first
    let a = TestClient::spawn(h.socket_path(), "org.example.a", (500, 360), false);
    h.mapped(1);
    let first = h.state.wm.key_window().unwrap();
    let b = TestClient::spawn(h.socket_path(), "org.example.b", (300, 200), false);
    h.mapped(2);
    let second = h
        .state
        .wm
        .windows()
        .map(|w| w.id)
        .find(|w| *w != first)
        .unwrap();
    let (ax, ay) = h.content_point(first);
    let (bx, by) = h.content_point(second);
    assert_ne!(
        h.state
            .window_surface_under((ax, ay).into())
            .map(|(w, _, _)| w),
        h.state
            .window_surface_under((bx, by).into())
            .map(|(w, _, _)| w)
    );

    h.move_to(ax, ay);
    h.button(BTN_LEFT, ButtonState::Pressed);
    h.move_to(bx, by);
    h.button(BTN_LEFT, ButtonState::Released);
    h.settle();
    assert_eq!(a.log(), vec![(BTN_LEFT, true), (BTN_LEFT, false)]);
    assert_eq!(
        b.log(),
        vec![],
        "the second window saw a release of a press it never got"
    );
}

/// A click that raises and focuses a window still reaches it as one press and one release.
#[test]
fn a_click_that_focuses_a_background_window_reaches_it_as_a_press_and_a_release() {
    let Some(mut h) = Harness::new() else { return };
    // a larger first window: two applications' first windows are both centred, so the second one
    // covers only the middle of the first
    let a = TestClient::spawn(h.socket_path(), "org.example.a", (500, 360), false);
    h.mapped(1);
    let first = h.state.wm.key_window().unwrap();
    let b = TestClient::spawn(h.socket_path(), "org.example.b", (300, 200), false);
    h.mapped(2);
    assert_ne!(
        h.state.wm.key_window(),
        Some(first),
        "the second window is in front"
    );
    let (ax, ay) = h.content_point(first);

    h.move_to(ax, ay);
    h.button(BTN_LEFT, ButtonState::Pressed);
    h.button(BTN_LEFT, ButtonState::Released);
    h.settle();
    assert_eq!(
        h.state.wm.key_window(),
        Some(first),
        "the click focused the window"
    );
    assert_eq!(a.log(), vec![(BTN_LEFT, true), (BTN_LEFT, false)]);
    assert_eq!(b.log(), vec![]);
}

/// A client that asks for an interactive move after its press got the press, so it gets the release that ends the move.
#[test]
fn a_client_that_asked_for_a_move_after_its_press_still_gets_the_release() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path(), "org.example.a", (400, 300), true);
    h.mapped(1);
    let id = h.state.wm.key_window().unwrap();
    let (cx, cy) = h.content_point(id);

    h.move_to(cx, cy);
    h.button(BTN_LEFT, ButtonState::Pressed);
    h.settle();
    assert!(
        h.state.wm.is_interacting(),
        "the move request started a model move"
    );
    h.button(BTN_LEFT, ButtonState::Released);
    h.settle();
    assert!(!h.state.wm.is_interacting(), "the release ended the move");
    assert_eq!(client.log(), vec![(BTN_LEFT, true), (BTN_LEFT, false)]);
}

/// A press on the title bar with a few points of jitter is a click; past the 5 pt drag-start box it
/// becomes a move that catches up with the pointer.
#[test]
fn a_title_bar_press_moves_the_window_only_past_the_drag_threshold() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path(), "org.example.a", (400, 300), false);
    h.mapped(1);
    let id = h.state.wm.key_window().unwrap();
    let frame = h.frame(id);
    let (x, y) = (frame.x + 250.0, frame.y + 10.0);

    h.move_to(x, y);
    h.button(BTN_LEFT, ButtonState::Pressed);
    h.move_to(x + 2.0, y + 2.0);
    assert!(!h.state.wm.is_interacting(), "2 pt of jitter is not a drag");
    h.button(BTN_LEFT, ButtonState::Released);
    h.advance(1.0);
    assert_eq!(h.frame(id), frame, "the click left the window where it was");
    assert!(
        h.state.pending_drag.is_none(),
        "the release ended the press"
    );

    h.move_to(x, y);
    h.button(BTN_LEFT, ButtonState::Pressed);
    h.move_to(x + 5.0, y - 5.0);
    assert!(!h.state.wm.is_interacting(), "5 pt is still inside the box");
    h.move_to(x + 40.0, y + 30.0);
    assert!(
        h.state.wm.is_interacting(),
        "leaving the box starts the move"
    );
    h.button(BTN_LEFT, ButtonState::Released);
    h.advance(3.0);
    let moved = h.frame(id);
    assert!(
        (moved.x - frame.x - 40.0).abs() < 1.5 && (moved.y - frame.y - 30.0).abs() < 1.5,
        "moved {moved:?} from {frame:?}"
    );
    assert_eq!(
        client.log(),
        vec![],
        "the title bar presses were the server's"
    );
}
