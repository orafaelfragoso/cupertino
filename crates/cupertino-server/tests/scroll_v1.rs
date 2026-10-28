//! `cupertino_scroll_v1` end to end: a client binds the global for its `wl_pointer` and records the order of
//! `wl_pointer.axis`, `scroll_count` and `wl_pointer.frame`. The compositor runs in-process on an offscreen EGL
//! device and scrolls are injected as a touchpad or a wheel would send them. Tests skip (pass) when no EGL device is
//! available.

mod common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_protocols::client::scroll::cupertino_pointer_scroll_v1::{
    self, CupertinoPointerScrollV1,
};
use cupertino_protocols::client::scroll::cupertino_scroll_v1::CupertinoScrollV1;
use cupertino_server::backend::headless;
use cupertino_server::config::{Config, PointerSettings};
use cupertino_server::events::{AxisInput, PointerDevice};
use cupertino_server::Cupertino;
use cupertino_wm::WindowId;
use smithay::backend::input::{AxisRelativeDirection, AxisSource};
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
const TITLE_BAR: f64 = 28.0;
/// The key of the injected touchpad and wheel in the server's pointer devices.
const PAD: &str = "pad";
const WHEEL: &str = "wheel";
/// Touchpad counts per event that give well over 30 points per event after the acceleration.
const FAST: f64 = 100.0;
/// Time between the events of a touchpad, in microseconds.
const EVENT_USEC: u64 = 8_000;

/// What the client received, in order.
#[derive(Clone, Debug, PartialEq)]
enum Record {
    Axis(wl_pointer::Axis),
    Count(wl_pointer::Axis, u32),
    Frame,
}

type Log = Arc<Mutex<Vec<Record>>>;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .ok()?;
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => {
                let settings = PointerSettings::default();
                state
                    .pointer_devices
                    .insert(PAD.into(), PointerDevice::new(true, &settings));
                state
                    .pointer_devices
                    .insert(WHEEL.into(), PointerDevice::new(false, &settings));
                Some(Harness { state, event_loop })
            }
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
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
        let end = Instant::now() + Duration::from_secs(10);
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

    /// Wait for the mapped window and settle model time so no open animation is running.
    fn mapped(&mut self) {
        self.wait_until("the window to be mapped", |s| s.wm.windows().count() == 1);
        let t = self.state.now() + 5.0;
        let time = t;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        self.settle();
    }

    /// Put the pointer on the first point of the window's content that no window above it covers.
    fn point_at_content(&mut self, id: WindowId) {
        let f = self.state.wm.window(id).unwrap().geometry();
        for dy in (TITLE_BAR as i32 + 4..f.h as i32 - 4).step_by(8) {
            for dx in (4..f.w as i32 - 4).step_by(8) {
                let p = (f.x + f64::from(dx), f.y + f64::from(dy));
                if self
                    .state
                    .window_surface_under(p.into())
                    .is_some_and(|(w, _, _)| w == id)
                {
                    self.state.pointer_moved_to(p.into(), 0, None);
                    return;
                }
            }
        }
        panic!("window {id:?} has no uncovered content point");
    }

    /// One touchpad event of `counts` vertically at `usec`; `None` ends the gesture.
    fn pad(&mut self, counts: Option<f64>, usec: u64) {
        let amount = [None, Some(counts.unwrap_or(0.0))];
        self.state.pointer_axis(&axis_input(
            PAD,
            AxisSource::Finger,
            amount,
            [None, None],
            usec,
        ));
    }

    /// A flick of six fast events and the end of the gesture, from `start_usec`; returns the time of the end.
    fn flick(&mut self, start_usec: u64) -> u64 {
        for i in 0..6 {
            self.pad(Some(FAST), start_usec + i * EVENT_USEC);
        }
        let end = start_usec + 6 * EVENT_USEC;
        self.pad(None, end);
        end
    }
}

fn axis_input(
    device: &str,
    source: AxisSource,
    amount: [Option<f64>; 2],
    v120: [Option<f64>; 2],
    usec: u64,
) -> AxisInput {
    AxisInput {
        device: device.into(),
        source,
        usec,
        amount,
        v120,
        relative_direction: [AxisRelativeDirection::Identical; 2],
    }
}

// ---- test client ---------------------------------------------------------------------------

struct ClientState {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    size: (i32, i32),
    attached: bool,
    log: Log,
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
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    CupertinoScrollV1
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
        match event {
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                ..
            } => state.log.lock().unwrap().push(Record::Axis(axis)),
            wl_pointer::Event::Frame => state.log.lock().unwrap().push(Record::Frame),
            _ => {}
        }
    }
}

impl Dispatch<CupertinoPointerScrollV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoPointerScrollV1,
        event: cupertino_pointer_scroll_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let cupertino_pointer_scroll_v1::Event::ScrollCount {
            axis: WEnum::Value(axis),
            count,
        } = event
        else {
            panic!("unknown scroll_count axis")
        };
        state.log.lock().unwrap().push(Record::Count(axis, count));
    }
}

/// A server-decorated toplevel whose pointer has a scroll object; keeps the connection served until dropped.
struct TestClient {
    stop: Arc<AtomicBool>,
    log: Log,
}

impl TestClient {
    fn spawn(socket: PathBuf) -> TestClient {
        let stop = Arc::new(AtomicBool::new(false));
        let log: Log = Arc::default();
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
            let pointer = seat.get_pointer(&qh, ());
            let scroll: CupertinoScrollV1 = globals.bind(&qh, 1..=1, ()).unwrap();
            let _scroll = scroll.get_scroll(&pointer, &qh, ());
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.a".to_string());
            let manager: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let decoration = manager.get_toplevel_decoration(&toplevel, &qh, ());
            decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            surface.commit();
            let mut client = ClientState {
                shm,
                surface,
                size: (400, 300),
                attached: false,
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

    fn log(&self) -> Vec<Record> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// The counts of the `Count` records, each checked to sit between its axis event and the frame.
fn counts_in_frames(log: &[Record]) -> Vec<u32> {
    let mut counts = Vec::new();
    for frame in log.split(|r| *r == Record::Frame).filter(|f| !f.is_empty()) {
        if let [Record::Axis(wl_pointer::Axis::VerticalScroll), Record::Count(wl_pointer::Axis::VerticalScroll, count)] =
            frame
        {
            counts.push(*count);
        } else {
            assert!(
                frame.iter().all(|r| !matches!(r, Record::Count(..))),
                "a count outside axis, count, frame: {frame:?}"
            );
        }
    }
    counts
}

/// Each fast event of a touchpad flick tells the count after its axis event and before its frame; a flick within
/// the gap of the previous one raises the count, and the end of a gesture sends none.
#[test]
fn touchpad_scrolls_send_the_count_between_the_axis_event_and_the_frame() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.mapped();
    let id = h.state.wm.key_window().unwrap();
    h.point_at_content(id);
    h.settle();
    client.log.lock().unwrap().clear();

    let end = h.flick(1_000_000);
    h.flick(end + 100_000);
    h.settle();
    let counts = counts_in_frames(&client.log());
    // the first flick starts the series, the second begins at 2; coalescing may hold back events of either
    assert!(counts.contains(&1), "{counts:?}");
    assert_eq!(counts.last(), Some(&2), "{counts:?}");
    assert!(
        counts.windows(2).all(|w| w[0] <= w[1]) && counts.iter().all(|c| *c <= 2),
        "{counts:?}"
    );
}

/// A wheel has no gesture phases and so no count: its frames carry axis events only.
#[test]
fn wheel_scrolls_send_no_count() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.mapped();
    let id = h.state.wm.key_window().unwrap();
    h.point_at_content(id);
    h.settle();
    client.log.lock().unwrap().clear();

    h.state.pointer_axis(&axis_input(
        WHEEL,
        AxisSource::Wheel,
        [None, Some(15.0)],
        [None, Some(120.0)],
        1_000_000,
    ));
    h.settle();
    assert_eq!(
        client.log(),
        vec![
            Record::Axis(wl_pointer::Axis::VerticalScroll),
            Record::Frame
        ]
    );
}
