//! A compositor stand-in on the far end of a socket pair: the globals the shell client binds, run on a thread of its own
//! so the client's blocking calls can complete. Requests the client sends are recorded in order; events go out through
//! jobs that run on the server's thread.
#![allow(dead_code)]

use cupertino_protocols::server::control::cupertino_control_authority_v1::{
    self, CupertinoControlAuthorityV1,
};
use cupertino_protocols::server::control::cupertino_control_session_v1::{
    self, CupertinoControlSessionV1,
};
use cupertino_protocols::server::control::cupertino_control_v1::{self, CupertinoControlV1};
use cupertino_protocols::server::cupertino_dock_v1::{self, CupertinoDockV1};
use cupertino_protocols::server::cupertino_menu_bar_v1::{self, CupertinoMenuBarV1};
use cupertino_protocols::server::cupertino_session_v1::{self, CupertinoSessionV1};
use cupertino_protocols::server::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_protocols::server::cupertino_window_manager_v1::CupertinoWindowManagerV1;
use cupertino_shell_client::ShellEvent;
use cupertino_wayland_client::{Connection, Event, ExtensionEvent};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Once};
use std::thread::JoinHandle;
use std::time::Duration;
use wayland_protocols::ext::idle_notify::v1::server::ext_idle_notification_v1::{
    self, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::server::ext_idle_notifier_v1::{
    self, ExtIdleNotifierV1,
};
use wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::{
    self, ExtSessionLockManagerV1,
};
use wayland_protocols::ext::session_lock::v1::server::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::{
    self, ExtSessionLockV1,
};
use wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase;
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_output::{self, WlOutput};
use wayland_server::protocol::wl_region::WlRegion;
use wayland_server::protocol::wl_seat::{self, WlSeat};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::WlShmPool;
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
};

/// A request the shell client sent, in the order the server took it.
#[derive(Debug)]
pub enum Seen {
    LayerSurfaceCreated {
        namespace: String,
        layer: u32,
        on_output: bool,
    },
    Layer(zwlr_layer_surface_v1::Request),
    LockCreated,
    Lock(ext_session_lock_v1::Request),
    LockSurfaceCreated,
    IdleCreated {
        millis: u32,
    },
    Idle(ext_idle_notification_v1::Request),
    Session(cupertino_session_v1::Request),
    MenuBar(cupertino_menu_bar_v1::Request),
    Dock(cupertino_dock_v1::Request),
    ControlSession(cupertino_control_session_v1::Request),
    Authority(cupertino_control_authority_v1::Request),
    Control(cupertino_control_v1::Request),
}

/// Which globals the server advertises, each at the highest version it speaks.
#[derive(Clone, Debug)]
pub struct Offer {
    pub shell: Option<u32>,
    pub layer_shell: Option<u32>,
    pub lock: bool,
    pub idle: Option<u32>,
    pub control: Option<u32>,
    pub authority: Option<u32>,
}

impl Offer {
    pub fn all() -> Offer {
        Offer {
            shell: Some(31),
            layer_shell: Some(4),
            lock: true,
            idle: Some(2),
            control: Some(5),
            authority: Some(2),
        }
    }

    pub fn shell(version: u32) -> Offer {
        Offer {
            shell: Some(version),
            ..Offer::all()
        }
    }
}

/// The server's objects, which jobs send events on.
#[derive(Default)]
pub struct Server {
    log: Arc<Mutex<Vec<Seen>>>,
    pub layer_surfaces: Vec<ZwlrLayerSurfaceV1>,
    pub locks: Vec<ExtSessionLockV1>,
    pub lock_surfaces: Vec<ExtSessionLockSurfaceV1>,
    pub idle: Vec<ExtIdleNotificationV1>,
    pub sessions: Vec<CupertinoSessionV1>,
    pub menu_bars: Vec<CupertinoMenuBarV1>,
    pub docks: Vec<CupertinoDockV1>,
    pub control_sessions: Vec<CupertinoControlSessionV1>,
    pub authorities: Vec<CupertinoControlAuthorityV1>,
    pub outputs: Vec<WlOutput>,
    /// Sent on each session observer as it is created, as the compositor sends the state at connect.
    pub greet_session: Option<Greeting<CupertinoSessionV1>>,
    /// Sent on each menu bar as it is created: its first group.
    pub greet_menu_bar: Option<Greeting<CupertinoMenuBarV1>>,
    /// Sent on each dock as it is created: its first group.
    pub greet_dock: Option<Greeting<CupertinoDockV1>>,
}

/// What a new object is sent right away.
pub type Greeting<T> = Box<dyn Fn(&T) + Send>;

impl Server {
    fn see(&self, seen: Seen) {
        self.log.lock().expect("log").push(seen);
    }
}

struct Data;

impl ClientData for Data {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

type Job = Box<dyn FnOnce(&mut Server) + Send>;

/// The client's connection to the stand-in compositor.
pub struct Wire {
    pub conn: Connection,
    log: Arc<Mutex<Vec<Seen>>>,
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

/// `set_var` is sound here only because nextest runs every test in its own process, so no other thread reads the
/// environment while it is written. The `Once` keeps a plain `cargo test` of the same binary to a single write.
static HOME: Once = Once::new();

/// Points every directory the transport or the defaults could read at an empty one under the target directory.
fn private_home() {
    HOME.call_once(|| {
        let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("shell-client-home");
        std::fs::create_dir_all(&home).expect("home");
        for variable in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "XDG_RUNTIME_DIR",
        ] {
            std::env::set_var(variable, &home);
        }
        std::env::remove_var("XDG_DATA_DIRS");
        std::env::remove_var("XDG_CONFIG_DIRS");
    });
}

impl Wire {
    pub fn new(offer: Offer) -> Wire {
        private_home();
        let (client_end, server_end) = UnixStream::pair().expect("socket pair");
        let log = Arc::new(Mutex::new(Vec::new()));
        let (jobs, inbox) = channel::<Job>();
        let thread = {
            let log = log.clone();
            std::thread::spawn(move || serve(server_end, offer, log, inbox))
        };
        let conn = Connection::from_stream(client_end).expect("connection");
        Wire {
            conn,
            log,
            jobs: Some(jobs),
            thread: Some(thread),
        }
    }

    pub fn all() -> Wire {
        Wire::new(Offer::all())
    }

    /// Run `job` on the server's thread, and return what it returned once it ran.
    pub fn server<R: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Server) -> R + Send + 'static,
    ) -> R {
        let (answer, result) = channel();
        let job: Job = Box::new(move |server| {
            answer.send(job(server)).expect("answer");
        });
        self.jobs
            .as_ref()
            .expect("running")
            .send(job)
            .expect("server thread");
        result.recv().expect("job result")
    }

    /// The requests the server took since the last call.
    pub fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.log.lock().expect("log"))
    }

    /// Everything the server sent so far, as the transport's events: the server has handled each request sent before
    /// the call when it returns.
    pub fn events(&mut self) -> Vec<Event> {
        self.conn.roundtrip().expect("roundtrip");
        self.conn.wait(Some(Duration::ZERO)).expect("wait")
    }

    /// The shell events among [`Wire::events`], in order.
    pub fn shell_events(&mut self) -> Vec<ShellEvent> {
        shell_events(&self.events())
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            thread.join().expect("server thread");
        }
    }
}

/// An anonymous file holding exactly `bytes`, for a picture the server sends as a file descriptor.
pub fn pixel_file(bytes: &[u8]) -> std::fs::File {
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "pixels-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("pixel file");
    std::fs::remove_file(&path).expect("unlink");
    file.write_all(bytes).expect("write");
    file
}

/// The events of the shell client among `events`.
pub fn shell_events(events: &[Event]) -> Vec<ShellEvent> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Extension(e) => e.downcast_ref::<ShellEvent>().cloned(),
            _ => None,
        })
        .collect()
}

/// The extension events of `events` that are a `T`.
pub fn extension<T: Clone + 'static>(events: &[Event]) -> Vec<T> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Extension(e) => e.downcast_ref::<T>().cloned(),
            _ => None,
        })
        .collect()
}

/// The extension events of `events`, untouched.
pub fn extensions(events: &[Event]) -> Vec<&ExtensionEvent> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Extension(e) => Some(e),
            _ => None,
        })
        .collect()
}

fn serve(stream: UnixStream, offer: Offer, log: Arc<Mutex<Vec<Seen>>>, inbox: Receiver<Job>) {
    let mut display = Display::<Server>::new().expect("display");
    let mut handle = display.handle();
    handle.create_global::<Server, WlCompositor, ()>(6, ());
    handle.create_global::<Server, WlShm, ()>(1, ());
    handle.create_global::<Server, XdgWmBase, ()>(6, ());
    handle.create_global::<Server, WlSeat, ()>(7, ());
    {
        handle.create_global::<Server, WlOutput, ()>(4, ());
    }
    if let Some(version) = offer.shell {
        handle.create_global::<Server, CupertinoShellV1, ()>(version, ());
    }
    if let Some(version) = offer.layer_shell {
        handle.create_global::<Server, ZwlrLayerShellV1, ()>(version, ());
    }
    if offer.lock {
        handle.create_global::<Server, ExtSessionLockManagerV1, ()>(1, ());
    }
    if let Some(version) = offer.idle {
        handle.create_global::<Server, ExtIdleNotifierV1, ()>(version, ());
    }
    if let Some(version) = offer.control {
        handle.create_global::<Server, CupertinoControlV1, ()>(version, ());
    }
    if let Some(version) = offer.authority {
        handle.create_global::<Server, CupertinoControlAuthorityV1, ()>(version, ());
    }
    handle
        .insert_client(stream, Arc::new(Data))
        .expect("client");
    let mut server = Server {
        log,
        ..Server::default()
    };
    loop {
        match inbox.recv_timeout(Duration::from_millis(1)) {
            Ok(job) => job(&mut server),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        display.dispatch_clients(&mut server).expect("dispatch");
        display.flush_clients().expect("flush");
    }
}

macro_rules! plain_global {
    ($($interface:ty),*) => {$(
        impl GlobalDispatch<$interface, ()> for Server {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$interface>,
                _: &(),
                init: &mut DataInit<'_, Self>,
            ) {
                init.init(resource, ());
            }
        }
    )*};
}

plain_global!(
    WlCompositor,
    WlShm,
    XdgWmBase,
    ExtSessionLockManagerV1,
    ExtIdleNotifierV1,
    CupertinoControlV1,
    ZwlrLayerShellV1
);

macro_rules! ignore_requests {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, ()> for Server {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$interface,
                _: <$interface as Resource>::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
    )*};
}

ignore_requests!(
    XdgWmBase,
    WlRegion,
    WlShmPool,
    WlSeat,
    WlOutput,
    WlCallback,
    CupertinoWindowManagerV1,
    ExtSessionLockSurfaceV1
);

macro_rules! record_requests {
    ($($interface:ty => $variant:ident),*) => {$(
        impl Dispatch<$interface, ()> for Server {
            fn request(
                state: &mut Self,
                _: &Client,
                _: &$interface,
                request: <$interface as Resource>::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
                state.see(Seen::$variant(request));
            }
        }
    )*};
}

record_requests!(
    ZwlrLayerSurfaceV1 => Layer,
    ExtIdleNotificationV1 => Idle,
    CupertinoSessionV1 => Session,
    CupertinoMenuBarV1 => MenuBar,
    CupertinoDockV1 => Dock,
    CupertinoControlSessionV1 => ControlSession,
    CupertinoControlAuthorityV1 => Authority
);

impl GlobalDispatch<WlSeat, ()> for Server {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(resource, ());
        seat.capabilities(wl_seat::Capability::empty());
    }
}

impl GlobalDispatch<WlOutput, ()> for Server {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlOutput>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let output = init.init(resource, ());
        output.geometry(
            0,
            0,
            600,
            340,
            wl_output::Subpixel::Unknown,
            "Test".into(),
            "Output".into(),
            wl_output::Transform::Normal,
        );
        output.mode(wl_output::Mode::Current, 1920, 1080, 60_000);
        output.scale(1);
        output.name(format!("TEST-{}", state.outputs.len() + 1));
        output.done();
        state.outputs.push(output);
    }
}

impl Dispatch<WlCompositor, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                init.init(id, ());
            }
            wl_compositor::Request::CreateRegion { id } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlSurface,
        request: wl_surface::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_surface::Request::Frame { callback } = request {
            init.init(callback, ());
        }
    }
}

impl Dispatch<WlShm, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlShm,
        request: wl_shm::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, .. } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_layer_shell_v1::Request::GetLayerSurface {
            id,
            output,
            layer,
            namespace,
            ..
        } = request
        {
            let surface = init.init(id, ());
            state.layer_surfaces.push(surface);
            state.see(Seen::LayerSurfaceCreated {
                namespace,
                layer: layer.into(),
                on_output: output.is_some(),
            });
        }
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ExtSessionLockManagerV1,
        request: ext_session_lock_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_session_lock_manager_v1::Request::Lock { id } = request {
            let lock = init.init(id, ());
            state.locks.push(lock);
            state.see(Seen::LockCreated);
        }
    }
}

impl Dispatch<ExtSessionLockV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ExtSessionLockV1,
        request: ext_session_lock_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_session_lock_v1::Request::GetLockSurface { id, .. } => {
                let surface = init.init(id, ());
                state.lock_surfaces.push(surface);
                state.see(Seen::LockSurfaceCreated);
            }
            other => state.see(Seen::Lock(other)),
        }
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ExtIdleNotifierV1,
        request: ext_idle_notifier_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_idle_notifier_v1::Request::GetIdleNotification { id, timeout, .. } = request {
            let notification = init.init(id, ());
            state.idle.push(notification);
            state.see(Seen::IdleCreated { millis: timeout });
        }
    }
}

impl GlobalDispatch<CupertinoShellV1, ()> for Server {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoShellV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<CupertinoShellV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &CupertinoShellV1,
        request: cupertino_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_shell_v1::Request::GetMenuBar { id, .. } => {
                let bar = init.init(id, ());
                if let Some(greet) = &state.greet_menu_bar {
                    greet(&bar);
                }
                state.menu_bars.push(bar);
            }
            cupertino_shell_v1::Request::GetDock { id, .. } => {
                let dock = init.init(id, ());
                if let Some(greet) = &state.greet_dock {
                    greet(&dock);
                }
                state.docks.push(dock);
            }
            cupertino_shell_v1::Request::GetSession { id } => {
                let session = init.init(id, ());
                if let Some(greet) = &state.greet_session {
                    greet(&session);
                }
                state.sessions.push(session);
            }
            cupertino_shell_v1::Request::GetWindowManager { id, .. } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

impl GlobalDispatch<CupertinoControlAuthorityV1, ()> for Server {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoControlAuthorityV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let authority = init.init(resource, ());
        state.authorities.push(authority);
    }
}

impl Dispatch<CupertinoControlV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &CupertinoControlV1,
        request: cupertino_control_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_control_v1::Request::GetSession { id } => {
                let session = init.init(id, ());
                state.control_sessions.push(session);
            }
            other => state.see(Seen::Control(other)),
        }
    }
}
