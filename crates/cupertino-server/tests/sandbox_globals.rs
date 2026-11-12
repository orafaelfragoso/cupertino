//! The globals that read or change what other clients own are offered to clients outside a sandbox only: a client that
//! connected through a security context (`wp_security_context_v1`) does not see the clipboard managers, the display
//! configuration or the display power. While the session is locked, clipboard managers are offered no selection and
//! set none, and display configuration is closed. Tests skip (pass) when no EGL device is available.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::security_context::v1::client::{
    wp_security_context_manager_v1::WpSecurityContextManagerV1,
    wp_security_context_v1::WpSecurityContextV1,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::ZwlrOutputModeV1,
};

use crate::common;

const OUTPUT: (i32, i32) = (1000, 700);
const CLIPBOARD: &[u8] = "Grüße, clipboard".as_bytes();
const PRIMARY: &[u8] = b"primary \x00\xff bytes";

const DATA_CONTROL: [&str; 2] = [
    "zwlr_data_control_manager_v1",
    "ext_data_control_manager_v1",
];
const DISPLAY: [&str; 2] = ["zwlr_output_manager_v1", "zwlr_output_power_manager_v1"];
const SHELL_SURFACES: [&str; 2] = ["zwlr_layer_shell_v1", "ext_foreign_toplevel_list_v1"];
const SECURITY_CONTEXT: &str = "wp_security_context_manager_v1";

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
        // this program is an allowed clipboard manager: a sandboxed client of it is still refused
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

    /// Step the server until the client thread ends, and give back what it returned.
    fn finish<T>(&mut self, thread: JoinHandle<T>) -> T {
        let end = Instant::now() + common::HANG_GUARD;
        while !thread.is_finished() {
            assert!(Instant::now() < end, "timed out waiting for the client");
            self.step();
        }
        thread.join().unwrap()
    }
}

/// A client that does nothing with the events of the objects it is given but keep what the test looks at.
#[derive(Default)]
struct Seen {
    /// The bytes of the clipboard and of the primary selection a data control device was offered.
    clipboard: Option<Vec<u8>>,
    primary: Option<Vec<u8>>,
    /// How many times a selection that had been offered was withdrawn from a data control device.
    withdrawn: usize,
    /// The source this client set was cancelled.
    cancelled: bool,
    /// The output manager sent `finished`.
    finished: bool,
    /// The output manager announced a head.
    heads: usize,
}

struct Probe {
    seen: Arc<Mutex<Seen>>,
    offers: HashMap<u32, Vec<String>>,
    /// Read what the device is offered; the client that set the selections is offered them too, and cannot answer
    /// its own read from inside the event.
    reads: bool,
}

impl Probe {
    fn new(seen: Arc<Mutex<Seen>>) -> Probe {
        Probe {
            seen,
            offers: HashMap::new(),
            reads: true,
        }
    }
}

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Probe {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_seat::WlSeat,
    ZwlrDataControlManagerV1,
    WpSecurityContextManagerV1,
    WpSecurityContextV1,
    ZwlrOutputModeV1
);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
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

impl Dispatch<ZwlrDataControlDeviceV1, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                state.offers.insert(id.id().protocol_id(), Vec::new());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if !state.reads {
                    return;
                }
                let bytes = id.map(|offer| read_offer(conn, &offer, "text/plain"));
                let mut seen = state.seen.lock().unwrap();
                seen.withdrawn += usize::from(bytes.is_none() && seen.clipboard.is_some());
                seen.clipboard = bytes;
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id } => {
                if !state.reads {
                    return;
                }
                let bytes = id.map(|offer| read_offer(conn, &offer, "text/x-primary"));
                let mut seen = state.seen.lock().unwrap();
                seen.withdrawn += usize::from(bytes.is_none() && seen.primary.is_some());
                seen.primary = bytes;
            }
            _ => {}
        }
    }

    event_created_child!(Probe, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

fn read_offer(conn: &Connection, offer: &ZwlrDataControlOfferV1, mime: &str) -> Vec<u8> {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    offer.receive(mime.to_string(), write.as_fd());
    drop(write);
    conn.flush().unwrap();
    let mut data = Vec::new();
    std::fs::File::from(read).read_to_end(&mut data).unwrap();
    data
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for Probe {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            if let Some(mimes) = state.offers.get_mut(&offer.id().protocol_id()) {
                mimes.push(mime_type);
            }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_source_v1::Event::Cancelled => {
                state.seen.lock().unwrap().cancelled = true;
            }
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                let bytes: &[u8] = if mime_type == "text/plain" {
                    CLIPBOARD
                } else {
                    PRIMARY
                };
                let _ = std::fs::File::from(fd).write_all(bytes);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { .. } => state.seen.lock().unwrap().heads += 1,
            zwlr_output_manager_v1::Event::Finished => state.seen.lock().unwrap().finished = true,
            _ => {}
        }
    }

    event_created_child!(Probe, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputHeadV1,
        _: zwlr_output_head_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(Probe, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

fn names_of(stream: UnixStream) -> Vec<String> {
    let conn = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) =
        registry_queue_init::<Probe>(&conn).expect("the registry of the server");
    let mut probe = Probe::new(Arc::default());
    queue.roundtrip(&mut probe).unwrap();
    globals
        .contents()
        .with_list(|list| list.iter().map(|g| g.interface.clone()).collect())
}

fn connect(path: &PathBuf) -> UnixStream {
    UnixStream::connect(path).expect("the server's socket")
}

/// A client outside a sandbox asks the server for a security context and connects through it: the names of the globals
/// that connection sees.
fn sandboxed_names(socket: PathBuf, dir: PathBuf) -> Vec<String> {
    let listener_path = dir.join("sandbox.sock");
    let listener = UnixListener::bind(&listener_path).unwrap();
    let (close_here, close_there) = UnixStream::pair().unwrap();
    let conn = Connection::from_socket(connect(&socket)).unwrap();
    let (globals, mut queue) = registry_queue_init::<Probe>(&conn).unwrap();
    let qh = queue.handle();
    let mut probe = Probe::new(Arc::default());
    let manager: WpSecurityContextManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
    let context = manager.create_listener(listener.as_fd(), close_there.as_fd(), &qh, ());
    context.set_sandbox_engine("org.flatpak".into());
    context.set_app_id("org.example.App".into());
    context.commit();
    queue.roundtrip(&mut probe).unwrap();
    let names = names_of(connect(&listener_path));
    drop(close_here);
    names
}

fn has_all(names: &[String], wanted: &[&str]) -> bool {
    wanted.iter().all(|w| names.iter().any(|n| n == w))
}

fn has_none(names: &[String], unwanted: &[&str]) -> bool {
    unwanted.iter().all(|w| !names.iter().any(|n| n == w))
}

#[test]
fn a_plain_client_sees_the_three_globals_and_a_sandboxed_one_none_of_them() {
    let Some(mut h) = Harness::new() else { return };
    let dir = tempfile::tempdir().unwrap();

    let plain = std::thread::spawn({
        let stream = connect(&h.socket_path());
        move || names_of(stream)
    });
    let plain = h.finish(plain);
    assert!(
        has_all(&plain, &DATA_CONTROL)
            && has_all(&plain, &DISPLAY)
            && has_all(&plain, &SHELL_SURFACES),
        "a client of an allowed program outside a sandbox sees the clipboard managers and the display globals: {plain:?}"
    );
    assert!(has_all(&plain, &[SECURITY_CONTEXT]));

    let socket = h.socket_path();
    let path = dir.path().to_path_buf();
    let sandboxed = h.finish(std::thread::spawn(move || sandboxed_names(socket, path)));
    assert!(
        has_none(&sandboxed, &DATA_CONTROL)
            && has_none(&sandboxed, &DISPLAY)
            && has_none(&sandboxed, &SHELL_SURFACES),
        "a sandboxed client, even of an allowed program, sees neither the clipboard managers, the display globals nor the layer shell and the toplevel list: {sandboxed:?}"
    );
    assert!(
        has_none(&sandboxed, &[SECURITY_CONTEXT]),
        "a sandboxed client cannot make a security context of its own: {sandboxed:?}"
    );
    assert!(
        sandboxed.iter().any(|n| n == "wl_compositor"),
        "the sandboxed client still sees the ordinary globals: {sandboxed:?}"
    );
}

/// A data control client that stays until `stop`: it binds the manager, makes a device, optionally sets a selection
/// offering `text/plain`, and keeps dispatching.
fn data_control_client(
    stream: UnixStream,
    set: bool,
    seen: Arc<Mutex<Seen>>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<Probe>(&conn).unwrap();
        let qh = queue.handle();
        let mut probe = Probe::new(seen);
        probe.reads = !set;
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).unwrap();
        let manager: ZwlrDataControlManagerV1 = globals.bind(&qh, 1..=2, ()).unwrap();
        let device = manager.get_data_device(&seat, &qh, ());
        let source = set.then(|| {
            let source = manager.create_data_source(&qh, ());
            source.offer("text/plain".into());
            device.set_selection(Some(&source));
            let primary = manager.create_data_source(&qh, ());
            primary.offer("text/x-primary".into());
            device.set_primary_selection(Some(&primary));
            (source, primary)
        });
        while !stop.load(Ordering::SeqCst) {
            queue.roundtrip(&mut probe).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(source);
    })
}

#[test]
fn locking_leaves_the_selections_with_their_owner_and_a_set_while_locked_is_cancelled() {
    let Some(mut h) = Harness::new() else { return };
    let stop = Arc::new(AtomicBool::new(false));

    let owner_seen = Arc::new(Mutex::new(Seen::default()));
    let owner = data_control_client(
        connect(&h.socket_path()),
        true,
        owner_seen.clone(),
        stop.clone(),
    );
    let before_seen = Arc::new(Mutex::new(Seen::default()));
    let before = data_control_client(
        connect(&h.socket_path()),
        false,
        before_seen.clone(),
        stop.clone(),
    );
    h.wait_until("a clipboard manager to be offered both selections", |_| {
        let seen = before_seen.lock().unwrap();
        seen.clipboard.is_some() && seen.primary.is_some()
    });

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    for _ in 0..40 {
        h.step();
    }
    locker.unlock();
    h.wait_until("the unlock", |s| !s.session_lock.is_active());
    for _ in 0..40 {
        h.step();
    }

    let late_seen = Arc::new(Mutex::new(Seen::default()));
    let late = data_control_client(
        connect(&h.socket_path()),
        false,
        late_seen.clone(),
        stop.clone(),
    );
    h.wait_until(
        "a manager that binds after the unlock to be offered both",
        |_| {
            let seen = late_seen.lock().unwrap();
            seen.clipboard.is_some() && seen.primary.is_some()
        },
    );
    for seen in [&before_seen, &late_seen] {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.clipboard.as_deref(), Some(CLIPBOARD));
        assert_eq!(seen.primary.as_deref(), Some(PRIMARY));
    }
    assert_eq!(before_seen.lock().unwrap().withdrawn, 0);
    assert!(!owner_seen.lock().unwrap().cancelled);

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked again", |_| locker.locked());
    let setter_seen = Arc::new(Mutex::new(Seen::default()));
    let setter = data_control_client(
        connect(&h.socket_path()),
        true,
        setter_seen.clone(),
        stop.clone(),
    );
    h.wait_until("a selection set while locked to be cancelled", |_| {
        setter_seen.lock().unwrap().cancelled
    });

    stop.store(true, Ordering::SeqCst);
    for client in [owner, before, late, setter] {
        h.finish(client);
    }
}

#[test]
fn while_locked_output_management_is_finished_at_once() {
    let Some(mut h) = Harness::new() else { return };
    let stop = Arc::new(AtomicBool::new(false));
    let output_manager = |h: &Harness, seen: Arc<Mutex<Seen>>, stop: Arc<AtomicBool>| {
        let stream = connect(&h.socket_path());
        std::thread::spawn(move || {
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<Probe>(&conn).unwrap();
            let qh = queue.handle();
            let mut probe = Probe::new(seen);
            let _manager: ZwlrOutputManagerV1 = globals.bind(&qh, 1..=4, ()).unwrap();
            while !stop.load(Ordering::SeqCst) {
                queue.roundtrip(&mut probe).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };

    let open_seen = Arc::new(Mutex::new(Seen::default()));
    let open = output_manager(&h, open_seen.clone(), stop.clone());
    h.wait_until("an output manager to be told of a head", |_| {
        open_seen.lock().unwrap().heads > 0
    });
    assert!(!open_seen.lock().unwrap().finished);

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    h.wait_until("the manager of before the lock to be finished", |_| {
        open_seen.lock().unwrap().finished
    });

    let late_seen = Arc::new(Mutex::new(Seen::default()));
    let late = output_manager(&h, late_seen.clone(), stop.clone());
    h.wait_until("a manager that binds while locked to be finished", |_| {
        late_seen.lock().unwrap().finished
    });
    assert_eq!(
        late_seen.lock().unwrap().heads,
        0,
        "nothing about the displays is announced while locked"
    );

    stop.store(true, Ordering::SeqCst);
    h.finish(open);
    h.finish(late);
}
