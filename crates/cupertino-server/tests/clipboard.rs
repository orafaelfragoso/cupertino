//! Clipboard persistence with two real Wayland clients: one copies and exits, the other pastes from the
//! server's copy. The compositor runs in-process on an offscreen EGL device. Tests skip (pass) when no EGL
//! device is available.

use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::clipboard::CAPTURE_LIMIT;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::selection::data_device::current_data_device_selection_userdata;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};

use crate::common;

const OUTPUT: (i32, i32) = (1000, 700);
const TEXT: &[u8] = "Grüße, clipboard".as_bytes();
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
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
        // the clipboard managers below are clients of this program
        common::allow_data_control(&mut state);
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

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Give the keyboard to the window of `app_id` once it maps, as a click on it does: only a process's first window
    /// takes the focus by itself, and the test clients share one process.
    fn focus(&mut self, app_id: &str) {
        let window_of = |s: &Cupertino| {
            s.wm.windows()
                .find(|w| s.wm.app(w.app).is_some_and(|a| a.identity == app_id))
                .map(|w| w.id)
        };
        self.wait_until(&format!("{app_id} to map"), |s| window_of(s).is_some());
        let window = window_of(&self.state).unwrap();
        self.state.focus_window_by_input(window);
    }

    /// The types of the server's kept clipboard, `None` while a client (or nobody) owns it.
    fn kept(&self) -> Option<Vec<String>> {
        current_data_device_selection_userdata(&self.state.seat)
            .map(|snapshot| snapshot.mime_types())
    }
}

// ---- test clients --------------------------------------------------------------------------

/// What a client does with the clipboard.
#[derive(Clone)]
enum Role {
    /// Offer these types with their bytes once focused and `start` is set; clear the selection again when `clear`
    /// is set.
    Copy {
        offers: Vec<(String, Arc<Vec<u8>>)>,
        start: Arc<AtomicBool>,
        clear: Arc<AtomicBool>,
    },
    /// Record the offered types of every selection and read `want` from each offering it; read the last offer
    /// again each time `again` is set, after releasing its seat when `release` is set.
    Paste {
        want: Vec<String>,
        again: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
    },
}

#[derive(Default)]
struct Shared {
    /// The types of the current selection offer; `Some(vec![])` after a selection without an offer.
    offered: Option<Vec<String>>,
    /// Bytes read per type.
    pasted: Vec<(String, Vec<u8>)>,
    /// The source this client set was cancelled.
    cancelled: bool,
    /// The client had keyboard focus, so it holds a serial to set the selection with.
    focused: bool,
}

struct ClientState {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    attached: bool,
    seat: wl_seat::WlSeat,
    manager: wl_data_device_manager::WlDataDeviceManager,
    device: wl_data_device::WlDataDevice,
    role: Role,
    serial: Option<u32>,
    copied: bool,
    cleared: bool,
    offer: Option<wl_data_offer::WlDataOffer>,
    shared: Arc<Mutex<Shared>>,
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
                let (w, h) = (300, 200);
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

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Enter { serial, .. } = event {
            state.serial = Some(serial);
            state.shared.lock().unwrap().focused = true;
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
        if let wl_data_source::Event::Cancelled = event {
            state.shared.lock().unwrap().cancelled = true;
            return;
        }
        if let (wl_data_source::Event::Send { mime_type, fd }, Role::Copy { offers, .. }) =
            (event, &state.role)
        {
            let data = offers
                .iter()
                .find(|(m, _)| *m == mime_type)
                .map(|(_, d)| d.clone())
                .expect("a request for an offered type");
            // the reader may stop early (a type over the server's limit): a broken pipe is fine
            let _ = std::fs::File::from(fd).write_all(&data);
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
        let wl_data_device::Event::Selection { id } = event else {
            return;
        };
        let Some(offer) = id else {
            state.shared.lock().unwrap().offered = Some(Vec::new());
            return;
        };
        let types = offer.data::<OfferTypes>().unwrap().lock().unwrap().clone();
        state.shared.lock().unwrap().offered = Some(types);
        state.offer = Some(offer);
        state.paste(conn);
    }

    event_created_child!(ClientState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, OfferTypes::default()),
    ]);
}

impl ClientState {
    /// Read the wanted types the last offer has, as a paste does.
    fn paste(&mut self, conn: &Connection) {
        let (Role::Paste { want, .. }, Some(offer)) = (&self.role, &self.offer) else {
            return;
        };
        let types = offer.data::<OfferTypes>().unwrap().lock().unwrap().clone();
        let mut pasted = Vec::new();
        for mime in want.iter().filter(|m| types.contains(m)) {
            pasted.push((
                mime.clone(),
                receive(conn, |fd| offer.receive(mime.clone(), fd)),
            ));
        }
        self.shared.lock().unwrap().pasted.extend(pasted);
    }

    /// Copy once focused and started; clear when asked; paste again when asked.
    fn act(&mut self, conn: &Connection, qh: &QueueHandle<Self>) {
        if let Role::Paste { again, release, .. } = &self.role {
            if again.swap(false, Ordering::SeqCst) {
                if release.swap(false, Ordering::SeqCst) {
                    self.seat.release();
                }
                self.paste(conn);
            }
            return;
        }
        let (
            Role::Copy {
                offers,
                start,
                clear,
            },
            Some(serial),
        ) = (&self.role, self.serial)
        else {
            return;
        };
        if !self.copied && start.load(Ordering::SeqCst) {
            self.copied = true;
            let source = self.manager.create_data_source(qh, ());
            for (mime, _) in offers {
                source.offer(mime.clone());
            }
            self.device.set_selection(Some(&source), serial);
        } else if clear.load(Ordering::SeqCst) && !self.cleared {
            self.cleared = true;
            self.device.set_selection(None, serial);
        }
    }
}

/// A toplevel with a keyboard and a data device; serves its connection until dropped, then disconnects.
struct TestClient {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    shared: Arc<Mutex<Shared>>,
}

impl TestClient {
    fn spawn(socket: PathBuf, app_id: &'static str, role: Role) -> TestClient {
        let stop = Arc::new(AtomicBool::new(false));
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let (stop2, shared2) = (stop.clone(), shared.clone());
        let thread = std::thread::spawn(move || {
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
            let _keyboard = seat.get_keyboard(&qh, ());
            let device = manager.get_data_device(&seat, &qh, ());
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            surface.commit();
            let mut client = ClientState {
                shm,
                surface,
                attached: false,
                seat,
                manager,
                device,
                role,
                serial: None,
                copied: false,
                cleared: false,
                offer: None,
                shared: shared2,
            };
            while !stop2.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                client.act(&conn, &qh);
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        TestClient {
            stop,
            thread: Some(thread),
            shared,
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }

    /// Stop serving and close the connection, as an exiting client does. The compositor runs on the calling thread,
    /// so it must keep dispatching until the client thread is done: that thread may be inside a round trip, or writing
    /// a type to the compositor's pipe, and only the compositor can end either. Joining without dispatching deadlocks.
    fn exit(mut self, h: &mut Harness) {
        self.stop.store(true, Ordering::SeqCst);
        let thread = self.thread.take().unwrap();
        let end = Instant::now() + common::HANG_GUARD;
        while !thread.is_finished() {
            assert!(
                Instant::now() < end,
                "timed out waiting for the client thread to end"
            );
            h.step();
        }
        thread.join().unwrap();
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Read what `request` asks to have written into a new pipe, until the writer closes it.
fn receive(conn: &Connection, request: impl FnOnce(std::os::fd::BorrowedFd<'_>)) -> Vec<u8> {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    request(write.as_fd());
    drop(write);
    conn.flush().unwrap();
    let mut data = Vec::new();
    std::fs::File::from(read).read_to_end(&mut data).unwrap();
    data
}

/// A client that copies `offers` once focused, and the flag that makes it clear its selection.
fn copier(offers: &[(&str, &[u8])]) -> (Role, Arc<AtomicBool>) {
    let (role, start, clear) = held_copier(offers);
    start.store(true, Ordering::SeqCst);
    (role, clear)
}

/// A client that copies `offers` once focused and its start flag is set, with its start and clear flags.
fn held_copier(offers: &[(&str, &[u8])]) -> (Role, Arc<AtomicBool>, Arc<AtomicBool>) {
    let (start, clear) = (Arc::default(), Arc::default());
    let role = Role::Copy {
        offers: offers
            .iter()
            .map(|(m, d)| (m.to_string(), Arc::new(d.to_vec())))
            .collect(),
        start: Arc::clone(&start),
        clear: Arc::clone(&clear),
    };
    (role, start, clear)
}

fn paster(want: &[&str]) -> Role {
    repaster(want).0
}

/// A client that pastes `want` from every selection, and the flag that makes it read the last one again.
fn repaster(want: &[&str]) -> (Role, Arc<AtomicBool>) {
    let (role, again, _) = releasing_repaster(want);
    (role, again)
}

/// [`repaster`], with the flag that makes it release its seat before it reads again.
fn releasing_repaster(want: &[&str]) -> (Role, Arc<AtomicBool>, Arc<AtomicBool>) {
    let (again, release) = (Arc::default(), Arc::default());
    let role = Role::Paste {
        want: want.iter().map(|m| m.to_string()).collect(),
        again: Arc::clone(&again),
        release: Arc::clone(&release),
    };
    (role, again, release)
}

/// Copy text and an image in one client, let it exit, paste both in another: the server's copy serves the
/// paste, with the text under every UTF-8 name and the private type dropped.
#[test]
fn the_clipboard_outlives_the_client_that_copied() {
    let Some(mut h) = Harness::new() else { return };
    let (role, _) = copier(&[
        ("text/plain;charset=utf-8", TEXT),
        ("image/png", PNG),
        ("application/x-private", b"secret"),
    ]);
    let a = TestClient::spawn(h.socket_path(), "org.example.copy", role);
    h.wait_until("the copy to be read", |s| {
        s.clipboard.is_complete()
            && s.clipboard.captured_types() == ["text/plain;charset=utf-8", "image/png"]
    });
    assert_eq!(
        h.kept(),
        None,
        "the copying client still owns its selection"
    );
    a.exit(&mut h);
    h.wait_until("the server to keep the clipboard", |s| {
        current_data_device_selection_userdata(&s.seat).is_some()
    });
    assert_eq!(
        h.kept().unwrap(),
        [
            "text/plain;charset=utf-8",
            "UTF8_STRING",
            "text/plain",
            "TEXT",
            "image/png"
        ]
    );

    let b = TestClient::spawn(
        h.socket_path(),
        "org.example.paste",
        paster(&["text/plain", "UTF8_STRING", "image/png"]),
    );
    h.wait_until("the paste", |_| b.shared().pasted.len() == 3);
    let shared = b.shared();
    assert!(!shared
        .offered
        .as_ref()
        .unwrap()
        .iter()
        .any(|m| m == "application/x-private"));
    assert_eq!(
        shared.pasted,
        [
            ("text/plain".to_string(), TEXT.to_vec()),
            ("UTF8_STRING".to_string(), TEXT.to_vec()),
            ("image/png".to_string(), PNG.to_vec())
        ]
    );
}

/// A clipboard its client cleared before exiting stays empty.
#[test]
fn a_cleared_clipboard_is_not_kept() {
    let Some(mut h) = Harness::new() else { return };
    let (role, clear) = copier(&[("text/plain;charset=utf-8", TEXT)]);
    let a = TestClient::spawn(h.socket_path(), "org.example.copy", role);
    h.wait_until("the copy to be read", |s| {
        s.clipboard.captured_types() == ["text/plain;charset=utf-8"]
    });
    clear.store(true, Ordering::SeqCst);
    h.wait_until("the clear", |s| s.clipboard.captured_types().is_empty());
    a.exit(&mut h);
    let b = TestClient::spawn(
        h.socket_path(),
        "org.example.paste",
        paster(&["text/plain"]),
    );
    h.wait_until("the second client's selection", |_| {
        b.shared().offered.is_some()
    });
    for _ in 0..40 {
        h.step();
    }
    assert_eq!(h.kept(), None);
    assert_eq!(
        b.shared().offered.as_deref(),
        Some(&[][..]),
        "no selection is offered"
    );
}

/// Locking and unlocking the session leaves the clipboard with the client that copied it: its source is not cancelled
/// and the server takes no copy; when that client exits, the bytes are those it offered.
#[test]
fn the_clipboard_stays_with_its_owner_across_a_lock() {
    let Some(mut h) = Harness::new() else { return };
    let (role, _) = copier(&[("text/plain;charset=utf-8", TEXT)]);
    let a = TestClient::spawn(h.socket_path(), "org.example.copy", role);
    h.wait_until("the copy to be read", |s| {
        s.clipboard.is_complete() && s.clipboard.captured_types() == ["text/plain;charset=utf-8"]
    });

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the lock", |_| locker.locked());
    for _ in 0..40 {
        h.step();
    }
    locker.unlock();
    h.wait_until("the unlock", |s| !s.session_lock.is_active());
    for _ in 0..40 {
        h.step();
    }
    assert_eq!(
        h.kept(),
        None,
        "the copying client still owns its selection"
    );
    assert!(!a.shared().cancelled);

    a.exit(&mut h);
    h.wait_until("the server to keep the clipboard", |s| {
        current_data_device_selection_userdata(&s.seat).is_some()
    });
    let b = TestClient::spawn(
        h.socket_path(),
        "org.example.paste",
        paster(&["text/plain"]),
    );
    h.wait_until("the paste", |_| b.shared().pasted.len() == 1);
    assert_eq!(
        b.shared().pasted,
        [("text/plain".to_string(), TEXT.to_vec())]
    );
}

/// While the session is locked neither an application nor a clipboard manager (data control) can set the clipboard:
/// both are refused, the manager sees its source cancelled, and after the unlock a paste gets what was copied before.
#[test]
fn a_locked_session_refuses_new_selections_and_the_owner_keeps_the_clipboard() {
    let Some(mut h) = Harness::new() else { return };
    let (role, _) = copier(&[("text/plain;charset=utf-8", TEXT)]);
    let a = TestClient::spawn(h.socket_path(), "org.example.copy", role);
    h.wait_until("the copy to be read", |s| {
        s.clipboard.is_complete() && s.clipboard.captured_types() == ["text/plain;charset=utf-8"]
    });
    let (role, start, _) = held_copier(&[("text/plain;charset=utf-8", PNG)]);
    let b = TestClient::spawn(h.socket_path(), "org.example.later", role);
    h.focus("org.example.later");
    h.wait_until("the second client's focus", |_| b.shared().focused);

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the lock", |_| locker.locked());
    start.store(true, Ordering::SeqCst);
    let manager = ClipboardManager::spawn(h.socket_path(), Manage::Set);
    h.wait_until("the manager's source to be cancelled", |_| {
        manager.cancelled()
    });
    for _ in 0..40 {
        h.step();
    }
    assert_eq!(h.kept(), None, "the server did not replace the clipboard");
    assert!(
        !a.shared().cancelled,
        "the copying client still owns its selection"
    );

    locker.unlock();
    h.wait_until("the unlock", |s| !s.session_lock.is_active());
    let c = TestClient::spawn(
        h.socket_path(),
        "org.example.paste",
        paster(&["text/plain;charset=utf-8"]),
    );
    h.focus("org.example.paste");
    h.wait_until("the paste", |_| c.shared().pasted.len() == 1);
    assert_eq!(
        c.shared().pasted,
        [("text/plain;charset=utf-8".to_string(), TEXT.to_vec())]
    );
}

/// While the session is locked neither an application holding an offer nor a clipboard manager can read the
/// clipboard: each read gets an empty pipe. Before the lock and after the unlock both read what was copied.
#[test]
fn a_locked_session_refuses_reads_of_the_clipboard() {
    let Some(mut h) = Harness::new() else { return };
    let (role, _) = copier(&[("text/plain;charset=utf-8", TEXT)]);
    let a = TestClient::spawn(h.socket_path(), "org.example.copy", role);
    h.wait_until("the copy to be read", |s| {
        s.clipboard.is_complete() && s.clipboard.captured_types() == ["text/plain;charset=utf-8"]
    });
    let (role, again) = repaster(&["text/plain;charset=utf-8"]);
    let b = TestClient::spawn(h.socket_path(), "org.example.paste", role);
    h.focus("org.example.paste");
    h.wait_until("the paste", |_| b.shared().pasted.len() == 1);
    let manager = ClipboardManager::spawn(h.socket_path(), Manage::Read);
    assert_eq!(manager.read(&mut h), TEXT);

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the lock", |_| locker.locked());
    again.store(true, Ordering::SeqCst);
    h.wait_until("the paste while locked", |_| b.shared().pasted.len() == 2);
    assert_eq!(manager.read(&mut h), b"");

    locker.unlock();
    h.wait_until("the unlock", |s| !s.session_lock.is_active());
    assert_eq!(manager.read(&mut h), TEXT);
    again.store(true, Ordering::SeqCst);
    h.wait_until("the paste after the unlock", |_| {
        b.shared().pasted.len() >= 3
    });
    let pasted: Vec<Vec<u8>> = b.shared().pasted.iter().map(|(_, d)| d.clone()).collect();
    assert_eq!(pasted[..2], [TEXT.to_vec(), Vec::new()]);
    assert!(pasted[2..].iter().all(|d| d == TEXT));
    assert!(!a.shared().cancelled);
}

/// A client that released its seat still holds the offer of the selection; reading it while the session is locked
/// gets an empty pipe, as with the seat.
#[test]
fn a_reader_that_released_its_seat_is_refused_while_locked() {
    let Some(mut h) = Harness::new() else { return };
    let (role, _) = copier(&[("text/plain;charset=utf-8", TEXT)]);
    let _a = TestClient::spawn(h.socket_path(), "org.example.copy", role);
    h.wait_until("the copy to be read", |s| {
        s.clipboard.is_complete() && s.clipboard.captured_types() == ["text/plain;charset=utf-8"]
    });
    let (role, again, release) = releasing_repaster(&["text/plain;charset=utf-8"]);
    let b = TestClient::spawn(h.socket_path(), "org.example.paste", role);
    h.focus("org.example.paste");
    h.wait_until("the paste", |_| b.shared().pasted.len() == 1);

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the lock", |_| locker.locked());
    release.store(true, Ordering::SeqCst);
    again.store(true, Ordering::SeqCst);
    h.wait_until("the paste while locked", |_| b.shared().pasted.len() == 2);
    assert_eq!(b.shared().pasted[1].1, b"");
}

/// A clipboard manager clearing the clipboard while the session is locked (a password tool's timed clear) empties it
/// for good: the server's copy goes too, and nothing comes back at the unlock.
#[test]
fn a_clear_by_a_clipboard_manager_while_locked_empties_the_clipboard() {
    let Some(mut h) = Harness::new() else { return };
    let (role, _) = copier(&[("text/plain;charset=utf-8", TEXT)]);
    let a = TestClient::spawn(h.socket_path(), "org.example.copy", role);
    h.wait_until("the copy to be read", |s| {
        s.clipboard.is_complete() && s.clipboard.captured_types() == ["text/plain;charset=utf-8"]
    });
    a.exit(&mut h);
    h.wait_until("the server to keep the clipboard", |s| {
        current_data_device_selection_userdata(&s.seat).is_some()
    });

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the lock", |_| locker.locked());
    let _manager = ClipboardManager::spawn(h.socket_path(), Manage::Clear);
    h.wait_until("the clipboard to be cleared", |s| {
        current_data_device_selection_userdata(&s.seat).is_none()
    });
    for _ in 0..40 {
        h.step();
    }
    assert_eq!(
        h.kept(),
        None,
        "the cleared clipboard stays cleared while locked"
    );
    locker.unlock();
    h.wait_until("the unlock", |s| !s.session_lock.is_active());

    let b = TestClient::spawn(
        h.socket_path(),
        "org.example.paste",
        paster(&["text/plain"]),
    );
    h.wait_until("the second client's selection", |_| {
        b.shared().offered.is_some()
    });
    for _ in 0..40 {
        h.step();
    }
    assert_eq!(h.kept(), None);
    assert_eq!(
        b.shared().offered.as_deref(),
        Some(&[][..]),
        "no selection is offered"
    );
    assert!(b.shared().pasted.is_empty());
}

/// What a clipboard manager does once connected.
#[derive(Clone, Copy)]
enum Manage {
    /// Set a selection of its own.
    Set,
    /// Clear the selection.
    Clear,
    /// Read UTF-8 text from the selection offered last, each time asked.
    Read,
}

/// A data control client that sets, clears or reads the selection and stays until dropped.
struct ClipboardManager {
    stop: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    read: Arc<AtomicBool>,
    reads: Arc<Mutex<Vec<Vec<u8>>>>,
}

struct ManagerState {
    cancelled: Arc<AtomicBool>,
    offer: Option<ZwlrDataControlOfferV1>,
}

macro_rules! manager_noop {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for ManagerState {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
manager_noop!(
    wl_registry::WlRegistry,
    wl_seat::WlSeat,
    ZwlrDataControlManagerV1,
    ZwlrDataControlOfferV1
);

impl Dispatch<ZwlrDataControlDeviceV1, ()> for ManagerState {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_device_v1::Event::Selection { id } = event {
            state.offer = id;
        }
    }

    event_created_child!(ManagerState, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ManagerState {
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

impl Dispatch<ZwlrDataControlSourceV1, ()> for ManagerState {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_source_v1::Event::Cancelled = event {
            state.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

impl ClipboardManager {
    fn spawn(socket: PathBuf, manage: Manage) -> ClipboardManager {
        let manager = ClipboardManager {
            stop: Arc::default(),
            cancelled: Arc::default(),
            read: Arc::default(),
            reads: Arc::default(),
        };
        let (stop2, cancelled2) = (manager.stop.clone(), manager.cancelled.clone());
        let (read, reads) = (manager.read.clone(), manager.reads.clone());
        std::thread::spawn(move || {
            let conn = Connection::from_socket(UnixStream::connect(&socket).unwrap()).unwrap();
            let (globals, mut queue) = registry_queue_init::<ManagerState>(&conn).unwrap();
            let qh = queue.handle();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).unwrap();
            let manager: ZwlrDataControlManagerV1 = globals.bind(&qh, 1..=2, ()).unwrap();
            let device = manager.get_data_device(&seat, &qh, ());
            match manage {
                Manage::Set => {
                    let source = manager.create_data_source(&qh, ());
                    source.offer("text/plain".into());
                    device.set_selection(Some(&source));
                }
                Manage::Clear => device.set_selection(None),
                Manage::Read => {}
            }
            let mut state = ManagerState {
                cancelled: cancelled2,
                offer: None,
            };
            while !stop2.load(Ordering::SeqCst) && queue.roundtrip(&mut state).is_ok() {
                if let (true, Some(offer)) = (read.swap(false, Ordering::SeqCst), &state.offer) {
                    let data = receive(&conn, |fd| {
                        offer.receive("text/plain;charset=utf-8".into(), fd)
                    });
                    reads.lock().unwrap().push(data);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        manager
    }

    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Read the offered text once more, as a clipboard manager polling the clipboard does, and wait for the bytes.
    fn read(&self, h: &mut Harness) -> Vec<u8> {
        let before = self.reads.lock().unwrap().len();
        self.read.store(true, Ordering::SeqCst);
        h.wait_until("the clipboard manager's read", |_| {
            self.reads.lock().unwrap().len() > before
        });
        self.reads.lock().unwrap().last().unwrap().clone()
    }
}

impl Drop for ClipboardManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A type over the limit is not kept; the text beside it is.
#[test]
fn content_over_the_limit_is_dropped() {
    let Some(mut h) = Harness::new() else { return };
    let big = vec![7u8; CAPTURE_LIMIT + 1];
    let (role, _) = copier(&[("text/plain;charset=utf-8", TEXT), ("image/png", &big)]);
    let a = TestClient::spawn(h.socket_path(), "org.example.copy", role);
    h.wait_until("the copy to be read", |s| {
        s.clipboard.is_complete() && !s.clipboard.captured_types().is_empty()
    });
    assert_eq!(
        h.state.clipboard.captured_types(),
        ["text/plain;charset=utf-8"]
    );
    a.exit(&mut h);
    h.wait_until("the server to keep the clipboard", |s| {
        current_data_device_selection_userdata(&s.seat).is_some()
    });
    assert!(!h.kept().unwrap().iter().any(|m| m == "image/png"));
}
