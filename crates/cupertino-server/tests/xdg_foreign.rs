//! xdg-foreign-unstable-v2 against a headless compositor: one client exports its toplevel and passes the handle on, and
//! another imports it and makes it the parent of its own toplevel, as a portal dialog does for the window that asked for
//! it; then `cupertino-portal` itself, whose file chooser opens as a child of the window its request names. Tests skip
//! (pass) when no EGL device is available.

mod common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_wm::WindowId;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::foreign::zv2::client::{
    zxdg_exported_v2, zxdg_exporter_v2, zxdg_imported_v2, zxdg_importer_v2,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);

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

    fn client(&self) -> Client {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        Client::spawn(PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap()))
    }

    /// The window of the toplevel titled `app_id`, once it is mapped.
    fn window(&mut self, app_id: &str) -> WindowId {
        self.wait_until(&format!("{app_id} to be mapped"), |s| {
            s.wm.windows().any(|w| w.title == app_id)
        });
        self.state
            .wm
            .windows()
            .find(|w| w.title == app_id)
            .unwrap()
            .id
    }

    fn parent(&self, window: WindowId) -> Option<WindowId> {
        self.state.wm.window(window).unwrap().parent
    }
}

// ---- client --------------------------------------------------------------------------------

/// What the client was told.
#[derive(Default)]
struct Seen {
    handles: Vec<String>,
    /// How many imported objects the server destroyed.
    destroyed: usize,
}

struct ClientState {
    shm: wl_shm::WlShm,
    seen: Arc<Mutex<Seen>>,
}

fn solid_buffer(
    shm: &wl_shm::WlShm,
    size: (i32, i32),
    qh: &QueueHandle<ClientState>,
) -> wl_buffer::WlBuffer {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![255u8; w as usize * h as usize * 4])
        .unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
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
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    xdg_toplevel::XdgToplevel,
    zxdg_exporter_v2::ZxdgExporterV2,
    zxdg_importer_v2::ZxdgImporterV2
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

impl Dispatch<xdg_surface::XdgSurface, wl_surface::WlSurface> for ClientState {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            surface.attach(Some(&solid_buffer(&state.shm, (300, 200), qh)), 0, 0);
            surface.damage_buffer(0, 0, 300, 200);
            surface.commit();
        }
    }
}

impl Dispatch<zxdg_exported_v2::ZxdgExportedV2, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zxdg_exported_v2::ZxdgExportedV2,
        event: zxdg_exported_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_exported_v2::Event::Handle { handle } = event {
            state.seen.lock().unwrap().handles.push(handle);
        }
    }
}

impl Dispatch<zxdg_imported_v2::ZxdgImportedV2, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zxdg_imported_v2::ZxdgImportedV2,
        event: zxdg_imported_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_imported_v2::Event::Destroyed = event {
            state.seen.lock().unwrap().destroyed += 1;
        }
    }
}

struct Objects {
    globals: GlobalList,
    compositor: wl_compositor::WlCompositor,
    wm_base: xdg_wm_base::XdgWmBase,
    surface: Option<wl_surface::WlSurface>,
    exported: Vec<zxdg_exported_v2::ZxdgExportedV2>,
}

impl Objects {
    /// Map a toplevel of `app_id`, made a child of the toplevel `handle` names (imported) before its first commit.
    fn toplevel(&mut self, app_id: &str, handle: Option<String>, qh: &QueueHandle<ClientState>) {
        let surface = self.compositor.create_surface(qh, ());
        let xdg = self.wm_base.get_xdg_surface(&surface, qh, surface.clone());
        let toplevel = xdg.get_toplevel(qh, ());
        toplevel.set_title(app_id.to_string());
        if let Some(handle) = handle {
            let importer: zxdg_importer_v2::ZxdgImporterV2 =
                self.globals.bind(qh, 1..=1, ()).unwrap();
            importer
                .import_toplevel(handle, qh, ())
                .set_parent_of(&surface);
        }
        surface.commit();
        self.surface = Some(surface);
    }

    fn export(&mut self, qh: &QueueHandle<ClientState>) {
        let exporter: zxdg_exporter_v2::ZxdgExporterV2 = self.globals.bind(qh, 1..=1, ()).unwrap();
        let surface = self.surface.as_ref().expect("a toplevel to export");
        self.exported
            .push(exporter.export_toplevel(surface, qh, ()));
    }
}

type Job = Box<dyn FnOnce(&mut Objects, &QueueHandle<ClientState>) + Send>;

/// A client thread running jobs between round trips; dropping it ends the thread.
struct Client {
    jobs: Sender<Job>,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Seen>>,
}

impl Client {
    fn spawn(socket: PathBuf) -> Client {
        let (jobs, rx) = channel::<Job>();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (stop2, seen2) = (stop.clone(), seen.clone());
        std::thread::spawn(move || {
            let conn = Connection::from_socket(UnixStream::connect(&socket).unwrap()).unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let mut client = ClientState {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                seen: seen2,
            };
            let compositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base = globals.bind(&qh, 1..=5, ()).unwrap();
            let mut objects = Objects {
                globals,
                compositor,
                wm_base,
                surface: None,
                exported: Vec::new(),
            };
            while !stop2.load(Ordering::SeqCst) {
                while let Ok(job) = rx.try_recv() {
                    job(&mut objects, &qh);
                }
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Client { jobs, stop, seen }
    }

    fn run(&self, job: impl FnOnce(&mut Objects, &QueueHandle<ClientState>) + Send + 'static) {
        self.jobs.send(Box::new(job)).unwrap();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A client whose toplevel `app_id` is mapped and exported: the client, its window and the handle.
fn exporter(h: &mut Harness, app_id: &'static str) -> (Client, WindowId, String) {
    let client = h.client();
    client.run(move |o, qh| o.toplevel(app_id, None, qh));
    let window = h.window(app_id);
    client.run(|o, qh| o.export(qh));
    let seen = client.seen.clone();
    h.wait_until("the handle", |_| !seen.lock().unwrap().handles.is_empty());
    let handle = client.seen.lock().unwrap().handles[0].clone();
    (client, window, handle)
}

#[test]
fn an_imported_handle_makes_the_exported_window_the_parent() {
    let Some(mut h) = Harness::new() else { return };
    let (_app, app, handle) = exporter(&mut h, "org.example.App");
    let portal = h.client();
    portal.run(move |o, qh| o.toplevel("org.example.Chooser", Some(handle), qh));
    let dialog = h.window("org.example.Chooser");
    assert_eq!(h.parent(dialog), Some(app));
    assert_eq!(portal.seen.lock().unwrap().destroyed, 0);
}

#[test]
fn an_unknown_handle_is_destroyed_at_once_and_sets_no_parent() {
    let Some(mut h) = Harness::new() else { return };
    let (_app, _, handle) = exporter(&mut h, "org.example.App");
    let portal = h.client();
    let guessed = format!("{handle}x");
    portal.run(move |o, qh| o.toplevel("org.example.Chooser", Some(guessed), qh));
    let dialog = h.window("org.example.Chooser");
    let seen = portal.seen.clone();
    h.wait_until("the import to be destroyed", |_| {
        seen.lock().unwrap().destroyed == 1
    });
    assert_eq!(h.parent(dialog), None);
}

#[test]
fn a_withdrawn_export_ends_the_parent_and_cannot_be_imported_again() {
    let Some(mut h) = Harness::new() else { return };
    let (app_client, app, handle) = exporter(&mut h, "org.example.App");
    let portal = h.client();
    let first = handle.clone();
    portal.run(move |o, qh| o.toplevel("org.example.Chooser", Some(first), qh));
    let dialog = h.window("org.example.Chooser");
    assert_eq!(h.parent(dialog), Some(app));

    app_client.run(|o, _| o.exported.remove(0).destroy());
    h.wait_until("the parent to end", |s| {
        s.wm.window(dialog).unwrap().parent.is_none()
    });

    portal.run(move |o, qh| o.toplevel("org.example.Second", Some(handle), qh));
    let second = h.window("org.example.Second");
    let seen = portal.seen.clone();
    h.wait_until("the late import to be destroyed", |_| {
        seen.lock().unwrap().destroyed == 1
    });
    assert_eq!(h.parent(second), None);
}

#[test]
fn a_closed_exported_window_leaves_its_imported_child_without_a_parent() {
    let Some(mut h) = Harness::new() else { return };
    let (app_client, _, handle) = exporter(&mut h, "org.example.App");
    let portal = h.client();
    portal.run(move |o, qh| o.toplevel("org.example.Chooser", Some(handle), qh));
    let dialog = h.window("org.example.Chooser");

    drop(app_client);
    h.wait_until("the child to lose its parent", |s| {
        s.wm.windows().count() == 1 && s.wm.window(dialog).unwrap().parent.is_none()
    });
}

#[test]
fn each_export_has_a_handle_of_its_own_that_names_no_window() {
    let Some(mut h) = Harness::new() else { return };
    let (app_client, _, first) = exporter(&mut h, "org.example.App");
    app_client.run(|o, qh| o.export(qh));
    let seen = app_client.seen.clone();
    h.wait_until("the second handle", |_| {
        seen.lock().unwrap().handles.len() == 2
    });
    let second = app_client.seen.lock().unwrap().handles[1].clone();
    assert_ne!(first, second);
    for handle in [&first, &second] {
        assert_eq!(handle.len(), 32, "{handle}");
        assert!(
            handle.chars().all(|c| c.is_ascii_alphanumeric()),
            "{handle}"
        );
    }
}

// ---- cupertino-portal ------------------------------------------------------------------------------------------------

/// The portal artifact prepared before the test runner starts.
fn portal_binary() -> PathBuf {
    common::fixtures::artifact("cupertino-portal", "bin", "cupertino-portal")
}

/// The portal process on the harness's display and `bus`, killed on drop, and its home folder.
struct Portal {
    child: Child,
    _home: tempfile::TempDir,
}

impl Portal {
    fn start(h: &Harness, bus: &cupertino_test_bus::PrivateBus) -> Portal {
        let home = tempfile::tempdir().unwrap();
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        let child = common::spawn::cleared(portal_binary(), home.path())
            .env("XDG_RUNTIME_DIR", runtime)
            .env("WAYLAND_DISPLAY", h.state.socket_name.as_ref().unwrap())
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .spawn()
            .unwrap();
        Portal { child, _home: home }
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Ask the portal for a file chooser titled `title` for the window `parent_window`, as the frontend does; the call waits
/// for the dialog's answer, so it runs on a thread of its own and is never answered here.
fn open_file(frontend: &zbus::blocking::Connection, title: &'static str, parent_window: String) {
    let frontend = frontend.clone();
    std::thread::spawn(move || {
        let handle = zbus::zvariant::ObjectPath::try_from(format!(
            "/org/freedesktop/portal/desktop/request/1_9/{}",
            title.to_lowercase().replace(' ', "_")
        ))
        .unwrap();
        let options: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> =
            Default::default();
        let _ = frontend.call_method(
            Some("org.freedesktop.impl.portal.desktop.cupertino"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.impl.portal.FileChooser"),
            "OpenFile",
            &(handle, "org.example.App", parent_window, title, options),
        );
    });
}

#[test]
fn the_portal_opens_its_file_chooser_as_a_child_of_the_window_that_asked_and_alone_for_any_other() {
    let Some(mut h) = Harness::new() else { return };
    let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
        eprintln!("skipped: dbus-daemon is not available");
        return;
    };
    let (_app, app, handle) = exporter(&mut h, "org.example.App");
    let frontend = bus.connect();
    frontend
        .request_name("org.freedesktop.portal.Desktop")
        .unwrap();
    let _portal = Portal::start(&h, &bus);
    let dbus = zbus::blocking::fdo::DBusProxy::new(&frontend).unwrap();
    let name: zbus::names::BusName<'_> = "org.freedesktop.impl.portal.desktop.cupertino"
        .try_into()
        .unwrap();
    h.wait_until("the portal on the bus", |_| {
        dbus.name_has_owner(name.clone()).unwrap()
    });

    open_file(&frontend, "Open Attachment", format!("wayland:{handle}"));
    let dialog = h.window("Open Attachment");
    assert_eq!(
        h.parent(dialog),
        Some(app),
        "the exported window is the parent"
    );

    // a handle the server does not know, and an X11 window, leave the chooser on its own, and it opens all the same
    for (title, parent_window) in [
        ("Open Unknown", format!("wayland:{handle}x")),
        ("Open From X11", "x11:3a00004".to_string()),
    ] {
        open_file(&frontend, title, parent_window);
        let dialog = h.window(title);
        assert_eq!(h.parent(dialog), None, "{title}");
    }
    assert_eq!(
        h.parent(dialog),
        Some(app),
        "the first chooser keeps its parent"
    );
}
