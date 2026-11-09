//! The popup grab policy against a raw Wayland client: a popup with a subsurface holds the grab when its subsurface is
//! pressed, and a press outside the popup chain dismisses it and never reaches the toplevel. Tests skip (pass) when no
//! EGL device is available.

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
use smithay::backend::input::ButtonState;
use smithay::desktop::PopupManager;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};

const OUTPUT: (i32, i32) = (1000, 700);
const WINDOW: (i32, i32) = (400, 300);
/// The popup in the window's coordinates, and the subsurface in the popup's.
const POPUP: (i32, i32, i32, i32) = (50, 50, 120, 120);
const SUBSURFACE: (i32, i32, i32, i32) = (20, 20, 40, 40);

#[derive(Default)]
struct Log {
    window_drawn: bool,
    popup_ready: bool,
    popup_done: bool,
    subsurface: Option<wayland_client::backend::ObjectId>,
    /// Button events: the surface that had the pointer and whether it was a press.
    buttons: Vec<(wayland_client::backend::ObjectId, bool)>,
}

struct Client {
    log: Arc<Mutex<Log>>,
    shm: wl_shm::WlShm,
    window: wl_surface::WlSurface,
    window_xdg: xdg_surface::XdgSurface,
    popup_xdg: Option<xdg_surface::XdgSurface>,
    popup_surface: Option<wl_surface::WlSurface>,
    focus: Option<wayland_client::backend::ObjectId>,
    press_serial: Option<u32>,
    buffers: Vec<wl_buffer::WlBuffer>,
}

fn draw(
    shm: &wl_shm::WlShm,
    surface: &wl_surface::WlSurface,
    size: (i32, i32),
    qh: &QueueHandle<Client>,
) -> wl_buffer::WlBuffer {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("popup-grab", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![255u8; (w * h * 4) as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w, h);
    surface.commit();
    buffer
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
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    xdg_positioner::XdgPositioner,
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
            if *surface == state.window_xdg && !state.log.lock().unwrap().window_drawn {
                let buffer = draw(&state.shm, &state.window, WINDOW, qh);
                state.buffers.push(buffer);
                state.log.lock().unwrap().window_drawn = true;
            }
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for Client {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            state.log.lock().unwrap().popup_done = true;
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
        match event {
            wl_pointer::Event::Enter { surface, .. } => state.focus = Some(surface.id()),
            wl_pointer::Event::Leave { .. } => state.focus = None,
            wl_pointer::Event::Button {
                serial,
                state: WEnum::Value(button),
                ..
            } => {
                let pressed = button == wl_pointer::ButtonState::Pressed;
                if pressed && state.press_serial.is_none() {
                    state.press_serial = Some(serial);
                }
                if let Some(focus) = state.focus.clone() {
                    state.log.lock().unwrap().buttons.push((focus, pressed));
                }
            }
            _ => {}
        }
    }
}

struct TestClient {
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Log>>,
    rounds: common::Rounds,
}

impl TestClient {
    fn spawn(socket: PathBuf) -> TestClient {
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Log::default()));
        let rounds = common::Rounds::default();
        let (stop2, log2, rounds2) = (stop.clone(), log.clone(), rounds.clone());
        std::thread::spawn(move || {
            let _ended = rounds2.ended_on_drop();
            let conn = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
            let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let subcompositor: wl_subcompositor::WlSubcompositor =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=5, ()).unwrap();
            let window = compositor.create_surface(&qh, ());
            let window_xdg = wm_base.get_xdg_surface(&window, &qh, ());
            let toplevel = window_xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.popup-grab".into());
            window.commit();
            let mut client = Client {
                log: log2.clone(),
                shm,
                window,
                window_xdg: window_xdg.clone(),
                popup_xdg: None,
                popup_surface: None,
                focus: None,
                press_serial: None,
                buffers: Vec::new(),
            };
            while !stop2.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                rounds2.tick();
                if let (Some(serial), None) = (client.press_serial, &client.popup_surface) {
                    let (x, y, w, h) = POPUP;
                    let positioner = wm_base.create_positioner(&qh, ());
                    positioner.set_size(w, h);
                    positioner.set_anchor_rect(x, y, 1, 1);
                    positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
                    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
                    let surface = compositor.create_surface(&qh, ());
                    let popup_xdg = wm_base.get_xdg_surface(&surface, &qh, ());
                    let popup = popup_xdg.get_popup(Some(&window_xdg), &positioner, &qh, ());
                    popup.grab(&seat, serial);
                    surface.commit();
                    queue.roundtrip(&mut client).unwrap();
                    let buffer = draw(&client.shm, &surface, (w, h), &qh);
                    client.buffers.push(buffer);
                    let (sx, sy, sw, sh) = SUBSURFACE;
                    let child = compositor.create_surface(&qh, ());
                    let subsurface = subcompositor.get_subsurface(&child, &surface, &qh, ());
                    subsurface.set_position(sx, sy);
                    subsurface.set_desync();
                    let buffer = draw(&client.shm, &child, (sw, sh), &qh);
                    client.buffers.push(buffer);
                    surface.commit();
                    {
                        let mut log = log2.lock().unwrap();
                        log.subsurface = Some(child.id());
                        log.popup_ready = true;
                    }
                    client.popup_xdg = Some(popup_xdg);
                    client.popup_surface = Some(surface);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        TestClient { stop, log, rounds }
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

    /// Wait for `client`'s [`common::Rounds::barrier`]: it has every event sent before, and the server handled its
    /// answers.
    fn sync(&mut self, client: &TestClient) {
        let done = client.rounds.barrier();
        self.wait_until("the client's roundtrips", move |_| done());
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// A press and release at `at`, each step heard by `client` before the next.
    fn click(&mut self, client: &TestClient, at: (f64, f64)) {
        self.state.pointer_moved_to(at.into(), 0, None);
        self.sync(client);
        self.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
        self.sync(client);
        self.state
            .pointer_button(BTN_LEFT, ButtonState::Released, 0);
        self.sync(client);
    }
}

#[test]
fn a_press_on_a_popup_subsurface_keeps_the_grab_and_an_outside_press_ends_it() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let window = h.state.windows.iter().find(|w| w.mapped).unwrap();
    let frame = h.state.wm.window(window.id).unwrap().geometry();
    h.sync(&client);

    // the press that gives the client the serial its popup grab needs
    h.click(&client, (frame.x + 300.0, frame.y + 220.0));
    h.wait_until("a drawn popup", |s| {
        let window = s.windows.iter().find(|w| w.mapped).unwrap();
        client.log.lock().unwrap().popup_ready
            && PopupManager::popups_for_surface(window.surface()).count() == 1
    });
    h.sync(&client);
    assert!(h.state.popup_grab.is_some(), "the popup holds a grab");
    let before = client.log.lock().unwrap().buttons.len();

    let (px, py, ..) = POPUP;
    let (sx, sy, sw, sh) = SUBSURFACE;
    h.click(
        &client,
        (
            frame.x + f64::from(px + sx + sw / 2),
            frame.y + f64::from(py + sy + sh / 2),
        ),
    );
    {
        let log = client.log.lock().unwrap();
        let sub = log.subsurface.clone().unwrap();
        assert_eq!(
            log.buttons[before..],
            [(sub.clone(), true), (sub, false)],
            "the subsurface of the popup got the press and the release"
        );
        assert!(!log.popup_done, "a press on popup content keeps the grab");
    }
    assert!(h.state.popup_grab.is_some());

    let before = client.log.lock().unwrap().buttons.len();
    h.click(&client, (frame.x + 350.0, frame.y + 250.0));
    let done = client.log.clone();
    h.wait_until("the popup to be dismissed", move |_| {
        done.lock().unwrap().popup_done
    });
    // a button event the toplevel got would have arrived by a roundtrip after the click
    h.sync(&client);
    let log = client.log.lock().unwrap();
    assert!(log.popup_done, "a press outside the popup dismisses it");
    assert_eq!(
        log.buttons.len(),
        before,
        "and the toplevel under it never sees the press"
    );
    assert!(h.state.popup_grab.is_none());
}

#[test]
fn a_session_lock_ends_the_popup_grab_and_the_first_press_reaches_the_lock_surface() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let window = h.state.windows.iter().find(|w| w.mapped).unwrap();
    let frame = h.state.wm.window(window.id).unwrap().geometry();
    h.sync(&client);
    h.click(&client, (frame.x + 300.0, frame.y + 220.0));
    h.wait_until("a drawn popup", |s| {
        let window = s.windows.iter().find(|w| w.mapped).unwrap();
        client.log.lock().unwrap().popup_ready
            && PopupManager::popups_for_surface(window.surface()).count() == 1
    });
    h.sync(&client);
    assert!(h.state.popup_grab.is_some(), "the popup holds a grab");

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    let done = client.log.clone();
    h.wait_until("the lock to dismiss the popup", move |_| {
        done.lock().unwrap().popup_done
    });
    assert!(h.state.popup_grab.is_none());

    // the first press after the lock is the lock surface's, not a dismissal's
    h.click(
        &client,
        (f64::from(OUTPUT.0) / 2.0, f64::from(OUTPUT.1) / 2.0),
    );
    h.wait_until("the lock surface to get the press", |_| {
        locker.presses() >= 1
    });
    assert_eq!(locker.presses(), 1, "the lock surface got the press");
}

#[test]
fn a_window_that_takes_the_focus_ends_the_popup_grab_of_another_application() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let window = h.state.windows.iter().find(|w| w.mapped).unwrap();
    let frame = h.state.wm.window(window.id).unwrap().geometry();
    h.sync(&client);
    h.click(&client, (frame.x + 300.0, frame.y + 220.0));
    h.wait_until("a drawn popup", |s| {
        let window = s.windows.iter().find(|w| w.mapped).unwrap();
        client.log.lock().unwrap().popup_ready
            && PopupManager::popups_for_surface(window.surface()).count() == 1
    });
    h.sync(&client);
    assert!(h.state.popup_grab.is_some(), "the popup holds a grab");

    // a window the person asked for (a token made by the server) comes up and takes the keyboard
    let token = std::sync::Arc::new(std::sync::Mutex::new(Some(
        h.state.server_activation_token(),
    )));
    let _launched = common::Client::toplevel_after(
        h.socket_path(),
        "org.example.launched",
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        token,
    );
    h.wait_until("the second window to map", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 2
    });
    let done = client.log.clone();
    h.wait_until(
        "the menu to be dismissed, not left open without its keys",
        move |_| done.lock().unwrap().popup_done,
    );
    assert!(h.state.popup_grab.is_none());
}

/// The display a window shows a menu on goes away: the menu is dismissed and its grab ends, and the window comes onto
/// the remaining display without it.
#[test]
fn a_display_that_goes_dismisses_the_menus_of_its_windows() {
    let Some(mut h) = Harness::new() else { return };
    let second = headless::HeadlessDisplay {
        name: "headless-2".into(),
        size: (800, 600),
        physical_mm: (0, 0),
    };
    let head = headless::add_display(&mut h.state, &second, Some(1.0)).unwrap();
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let id = h.state.windows.iter().find(|w| w.mapped).unwrap().id;
    let out2 = h
        .state
        .outputs
        .iter()
        .find(|o| o.output.name() == "headless-2")
        .unwrap()
        .id;
    h.state.command(cupertino_wm::Command::MoveWindowToOutput {
        window: id,
        output: out2,
        frame: cupertino_wm::Rect::new(1100.0, 100.0, 400.0, 300.0),
    });
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let frame = h.state.wm.window(id).unwrap().geometry();
    h.sync(&client);
    h.click(&client, (frame.x + 300.0, frame.y + 220.0));
    h.wait_until("a drawn popup", |s| {
        client.log.lock().unwrap().popup_ready && s.popup_grab.is_some()
    });

    headless::remove_display(&mut h.state, head).unwrap();
    let done = client.log.clone();
    h.wait_until("the menu to be dismissed", move |_| {
        done.lock().unwrap().popup_done
    });
    assert!(h.state.popup_grab.is_none());
    let left = h.state.outputs[0].id;
    assert_eq!(h.state.wm.window(id).unwrap().output, left);
}
