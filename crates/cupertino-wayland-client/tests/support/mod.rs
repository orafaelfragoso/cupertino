//! A minimal in-process Wayland server on one end of a socket pair, for the transport's client on the other.
//!
//! It offers the globals a client needs (compositor, shm, xdg shell, a seat with a pointer) and the Cupertino
//! protocols at versions the test picks. Every request of a Cupertino protocol is recorded as text in
//! [`Harness::requests`]; [`Harness::run`] runs a closure on the server's thread to send events.
#![allow(dead_code)]

use cupertino_app_protocols::semantics::{ActionReply, Snapshot, MAX_FRAME};
use cupertino_app_protocols::server::app_effects::{
    cupertino_app_effects_v1::{self, CupertinoAppEffectsV1},
    cupertino_app_menu_v1::{self, CupertinoAppMenuV1},
    cupertino_appearance_v1::{self, CupertinoAppearanceV1},
    cupertino_backdrop_v1::{self, CupertinoBackdropV1},
};
use cupertino_app_protocols::server::layers::{
    cupertino_layer_v1::{self, CupertinoLayerV1},
    cupertino_layers_v1::{self, CupertinoLayersV1},
};
use cupertino_app_protocols::server::scroll::{
    cupertino_pointer_scroll_v1::{self, CupertinoPointerScrollV1},
    cupertino_scroll_v1::{self, CupertinoScrollV1},
};
use cupertino_app_protocols::server::semantics::{
    cupertino_semantic_surface_v1::{self, CupertinoSemanticSurfaceV1},
    cupertino_semantics_v1::{self, CupertinoSemanticsV1},
};
use cupertino_wayland_client::app::AppEvent;
use cupertino_wayland_client::{Connection, Decorations, Event, Toplevel, ToplevelConfig};
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use wayland_protocols::wp::fractional_scale::v1::server::{
    wp_fractional_scale_manager_v1::{self, WpFractionalScaleManagerV1},
    wp_fractional_scale_v1::WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::server::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::{self, WpViewporter},
};
use wayland_protocols::xdg::shell::server::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::XdgToplevel,
    xdg_wm_base::{self, XdgWmBase},
};
use wayland_server::backend::ObjectId;
use wayland_server::protocol::{
    wl_buffer::WlBuffer,
    wl_callback::WlCallback,
    wl_compositor::{self, WlCompositor},
    wl_pointer::WlPointer,
    wl_region::WlRegion,
    wl_seat::{self, Capability, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::{self, WlShmPool},
    wl_subcompositor::{self, WlSubcompositor},
    wl_subsurface::WlSubsurface,
    wl_surface::{self, WlSurface},
};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

/// The versions of the Cupertino protocols the server offers; `None` leaves a global out.
#[derive(Clone, Copy)]
pub struct Offer {
    pub layers: Option<u32>,
    pub app_effects: Option<u32>,
    pub scroll: Option<u32>,
    pub semantics: Option<u32>,
    /// Whether the window shell (`xdg_wm_base`) is offered; a client cannot connect without it.
    pub shell: bool,
    /// Whether the server offers `abgr8888` shared memory and releases each buffer once a later commit of its surface
    /// replaces it, as a compositor showing the frames does; otherwise it keeps every buffer it was given.
    pub releases: bool,
    /// Whether the viewporter and fractional scaling are offered, so surfaces have viewports; each source rectangle
    /// a viewport is given is one of [`Harness::presents`].
    pub scaling: bool,
    /// Whether `wl_subcompositor` is offered.
    pub subsurfaces: bool,
    /// The whole scale the server prefers for every surface it makes (`wl_surface.preferred_buffer_scale`); `None`
    /// prefers none.
    pub scale: Option<i32>,
}

impl Offer {
    /// Every protocol at its newest version.
    pub const ALL: Offer = Offer {
        layers: Some(6),
        app_effects: Some(21),
        scroll: Some(1),
        semantics: Some(2),
        shell: true,
        releases: false,
        scaling: false,
        subsurfaces: false,
        scale: None,
    };

    /// Only the standard protocols.
    pub const NONE: Offer = Offer {
        layers: None,
        app_effects: None,
        scroll: None,
        semantics: None,
        shell: true,
        releases: false,
        scaling: false,
        subsurfaces: false,
        scale: None,
    };
}

/// A window of the connection, configured by the server.
pub fn window(conn: &mut Connection) -> Toplevel {
    let window = conn
        .toplevel(&ToplevelConfig {
            app_id: "org.example.test".into(),
            title: "Test".into(),
            min_size: None,
            max_size: None,
            decorations: Decorations::Client,
            foreign_parent: None,
        })
        .expect("create a window");
    let mut configured = false;
    for _ in 0..100 {
        configured |= conn
            .wait(Some(Duration::from_millis(50)))
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Configure { .. }));
        if configured {
            return window;
        }
    }
    panic!("the server did not configure the window");
}

/// Wait for the connection until `done` holds for the Cupertino events received, and return them with the
/// transport's other events.
pub fn wait_app_events(
    conn: &mut Connection,
    done: impl Fn(&[AppEvent]) -> bool,
) -> (Vec<AppEvent>, Vec<Event>) {
    let (mut app, mut other) = (Vec::new(), Vec::new());
    for _ in 0..100 {
        for event in conn.wait(Some(Duration::from_millis(50))).unwrap() {
            match event {
                Event::Extension(e) => match e.downcast_ref::<AppEvent>() {
                    Some(app_event) => app.push(app_event.clone()),
                    None => other.push(Event::Extension(e)),
                },
                event => other.push(event),
            }
        }
        if done(&app) {
            return (app, other);
        }
    }
    panic!("the events did not arrive within 5 s; got {app:?}");
}

/// The snapshots a semantic surface published, each with the epoch it was sent for.
pub type Publications = Arc<Mutex<Vec<(u64, Result<Snapshot, String>)>>>;

/// What the server holds between dispatches.
#[derive(Default)]
pub struct Server {
    requests: Arc<Mutex<Vec<String>>>,
    /// The configure acknowledgements, attaches and commits of the windows, as text, in the order received.
    presents: Arc<Mutex<Vec<String>>>,
    /// The commits that showed a buffer, each as the role of its surface, in the order received.
    commits: Arc<Mutex<Vec<String>>>,
    /// The role of each surface given one: `window`, `subsurface` or `layer`.
    roles: HashMap<ObjectId, &'static str>,
    publications: Publications,
    replies: Arc<Mutex<Vec<String>>>,
    pub layers: Vec<CupertinoLayerV1>,
    pub backdrops: Vec<CupertinoBackdropV1>,
    pub appearances: Vec<CupertinoAppearanceV1>,
    pub menus: Vec<CupertinoAppMenuV1>,
    pub scrolls: Vec<CupertinoPointerScrollV1>,
    pub semantic_surfaces: Vec<CupertinoSemanticSurfaceV1>,
    /// The windows, in creation order, with the `xdg_surface` each configures through.
    pub toplevels: Vec<(XdgToplevel, XdgSurface)>,
    serial: u32,
    /// Whether a buffer replaced on its surface is released ([`Offer::releases`]).
    releases: bool,
    /// The scale preferred for every surface ([`Offer::scale`]).
    scale: Option<i32>,
    /// The buffer each surface attached since its last commit, and the one it shows.
    pending: HashMap<ObjectId, Option<WlBuffer>>,
    shown: HashMap<ObjectId, WlBuffer>,
}

impl Server {
    fn record(&self, request: impl std::fmt::Debug) {
        self.requests.lock().unwrap().push(format!("{request:?}"));
    }

    fn note(&self, text: &str) {
        self.requests.lock().unwrap().push(text.to_owned());
    }

    fn present(&self, text: &str) {
        self.presents.lock().unwrap().push(text.to_owned());
    }

    /// Configure window `index` again, returning the serial.
    pub fn reconfigure(&mut self, index: usize) -> u32 {
        let serial = self.next_serial();
        let (toplevel, surface) = &self.toplevels[index];
        toplevel.configure(0, 0, Vec::new());
        surface.configure(serial);
        serial
    }

    /// Configure window `index` again at `width` × `height` points, returning the serial.
    pub fn resize(&mut self, index: usize, width: i32, height: i32) -> u32 {
        let serial = self.next_serial();
        let (toplevel, surface) = &self.toplevels[index];
        toplevel.configure(width, height, Vec::new());
        surface.configure(serial);
        serial
    }

    fn next_serial(&mut self) -> u32 {
        self.serial += 1;
        self.serial
    }
}

type Job = Box<dyn FnOnce(&mut Server) + Send>;

/// The server's thread and what it saw.
pub struct Harness {
    requests: Arc<Mutex<Vec<String>>>,
    presents: Arc<Mutex<Vec<String>>>,
    commits: Arc<Mutex<Vec<String>>>,
    pub publications: Publications,
    pub replies: Arc<Mutex<Vec<String>>>,
    jobs: Sender<Job>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Harness {
    /// Start a server offering `offer` and connect the transport to it.
    pub fn start(offer: Offer) -> (Harness, Connection) {
        let (harness, connection) = Harness::try_start(offer);
        (harness, connection.expect("connect to the test server"))
    }

    /// Start a server offering `offer` and try to connect the transport to it.
    pub fn try_start(
        offer: Offer,
    ) -> (Harness, Result<Connection, cupertino_wayland_client::Error>) {
        let (client_end, server_end) = UnixStream::pair().unwrap();
        let server = Server {
            releases: offer.releases,
            scale: offer.scale,
            ..Server::default()
        };
        let requests = server.requests.clone();
        let presents = server.presents.clone();
        let commits = server.commits.clone();
        let publications = server.publications.clone();
        let replies = server.replies.clone();
        let (jobs, inbox) = channel::<Job>();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || serve(server, server_end, offer, inbox, flag));
        let connection = Connection::from_stream(client_end);
        (
            Harness {
                requests,
                presents,
                commits,
                publications,
                replies,
                jobs,
                stop,
                thread: Some(thread),
            },
            connection,
        )
    }

    /// Stop the server and close its end of the connection.
    pub fn hang_up(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            join_strictly(thread);
        }
    }

    /// Run `job` on the server's thread, where it can send events through the resources it holds.
    pub fn run(&self, job: impl FnOnce(&mut Server) + Send + 'static) {
        self.jobs.send(Box::new(job)).unwrap();
    }

    /// The configure acknowledgements, attaches and commits of the windows received so far, as text, in order.
    pub fn presents(&self) -> Vec<String> {
        self.presents.lock().unwrap().clone()
    }

    /// The commits that showed a buffer received so far, each as the role of its surface (`window`, `subsurface`,
    /// `layer`, or `surface` for one without these), in order.
    pub fn commits(&self) -> Vec<String> {
        self.commits.lock().unwrap().clone()
    }

    /// The requests of the Cupertino protocols received so far, as text.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// Wait until `done` holds for the requests received, and return them.
    pub fn wait_until(&self, what: &str, done: impl Fn(&[String]) -> bool) -> Vec<String> {
        for _ in 0..500 {
            let seen = self.requests();
            if done(&seen) {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "{what} did not happen within 5 s; saw {:?}",
            self.requests()
        );
    }

    /// Wait until `count` requests contain `needle`, and return all requests received.
    pub fn wait_for(&self, needle: &str, count: usize) -> Vec<String> {
        self.wait_until(needle, |seen| {
            seen.iter().filter(|r| r.contains(needle)).count() >= count
        })
    }
}

/// Joins the server thread and re-raises its panic, so a failing job shows its own message and not a timeout. A panic
/// already unwinding the test is left alone: a second one would abort.
fn join_strictly(thread: JoinHandle<()>) {
    if let Err(panic) = thread.join() {
        if !std::thread::panicking() {
            std::panic::resume_unwind(panic);
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            join_strictly(thread);
        }
    }
}

fn serve(
    mut server: Server,
    stream: UnixStream,
    offer: Offer,
    inbox: Receiver<Job>,
    stop: Arc<AtomicBool>,
) {
    let mut display = wayland_server::Display::<Server>::new().unwrap();
    let mut handle = display.handle();
    handle.create_global::<Server, WlCompositor, ()>(6, ());
    handle.create_global::<Server, WlShm, bool>(1, offer.releases);
    if offer.shell {
        handle.create_global::<Server, XdgWmBase, ()>(6, ());
    }
    handle.create_global::<Server, WlSeat, ()>(7, ());
    if offer.subsurfaces {
        handle.create_global::<Server, WlSubcompositor, ()>(1, ());
    }
    if offer.scaling {
        handle.create_global::<Server, WpViewporter, ()>(1, ());
        handle.create_global::<Server, WpFractionalScaleManagerV1, ()>(1, ());
    }
    if let Some(v) = offer.layers {
        handle.create_global::<Server, CupertinoLayersV1, ()>(v, ());
    }
    if let Some(v) = offer.app_effects {
        handle.create_global::<Server, CupertinoAppEffectsV1, ()>(v, ());
    }
    if let Some(v) = offer.scroll {
        handle.create_global::<Server, CupertinoScrollV1, ()>(v, ());
    }
    if let Some(v) = offer.semantics {
        handle.create_global::<Server, CupertinoSemanticsV1, ()>(v, ());
    }
    handle
        .insert_client(stream, Arc::new(Quiet))
        .expect("insert the test client");
    let poll = display.backend().poll_fd().try_clone_to_owned().unwrap();
    while !stop.load(Ordering::SeqCst) {
        while let Ok(job) = inbox.try_recv() {
            job(&mut server);
        }
        let _ = display.dispatch_clients(&mut server);
        let _ = display.flush_clients();
        let mut fds = [rustix::event::PollFd::new(
            &poll,
            rustix::event::PollFlags::IN,
        )];
        let wait = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 5_000_000,
        };
        let _ = rustix::event::poll(&mut fds, Some(&wait));
    }
}

struct Quiet;
impl wayland_server::backend::ClientData for Quiet {}

macro_rules! global {
    ($($interface:ty),*) => {$(
        impl GlobalDispatch<$interface, ()> for Server {
            fn bind(
                _: &mut Server,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$interface>,
                _: &(),
                init: &mut DataInit<'_, Server>,
            ) {
                init.init(resource, ());
            }
        }
    )*};
}
global!(
    WlCompositor,
    WlSubcompositor,
    XdgWmBase,
    CupertinoLayersV1,
    CupertinoAppEffectsV1,
    CupertinoScrollV1,
    CupertinoSemanticsV1,
    WpViewporter,
    WpFractionalScaleManagerV1
);

impl Dispatch<WpViewporter, ()> for Server {
    fn request(
        _: &mut Server,
        _: &Client,
        _: &WpViewporter,
        request: wp_viewporter::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let wp_viewporter::Request::GetViewport { id, .. } = request {
            init.init(id, ());
        }
    }
}

/// A viewport's source rectangle is part of the presents a test reads back.
impl Dispatch<WpViewport, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &WpViewport,
        request: wp_viewport::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Server>,
    ) {
        if let wp_viewport::Request::SetSource {
            x,
            y,
            width,
            height,
        } = request
        {
            server.present(&format!("SetSource {x} {y} {width} {height}"));
        }
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for Server {
    fn request(
        _: &mut Server,
        _: &Client,
        _: &WpFractionalScaleManagerV1,
        request: wp_fractional_scale_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let wp_fractional_scale_manager_v1::Request::GetFractionalScale { id, .. } = request {
            init.init(id, ());
        }
    }
}

/// Shared memory, `abgr8888` listed when the server shows frames ([`Offer::releases`]).
impl GlobalDispatch<WlShm, bool> for Server {
    fn bind(
        _: &mut Server,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlShm>,
        abgr: &bool,
        init: &mut DataInit<'_, Server>,
    ) {
        let shm = init.init(resource, ());
        if *abgr {
            shm.format(wl_shm::Format::Abgr8888);
        }
    }
}

impl GlobalDispatch<WlSeat, ()> for Server {
    fn bind(
        _: &mut Server,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Server>,
    ) {
        let seat = init.init(resource, ());
        seat.capabilities(Capability::Pointer);
    }
}

macro_rules! ignore {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, ()> for Server {
            fn request(
                _: &mut Server,
                _: &Client,
                _: &$interface,
                _: <$interface as Resource>::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Server>,
            ) {
            }
        }
    )*};
}
ignore!(
    WlRegion,
    WlBuffer,
    WlPointer,
    XdgToplevel,
    WlCallback,
    WpFractionalScaleV1,
    WlSubsurface
);

impl Dispatch<WlSubcompositor, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &WlSubcompositor,
        request: wl_subcompositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let wl_subcompositor::Request::GetSubsurface { id, surface, .. } = request {
            init.init(id, ());
            server.roles.insert(surface.id(), "subsurface");
        }
    }
}

impl Dispatch<WlSurface, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        surface: &WlSurface,
        request: wl_surface::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            wl_surface::Request::Frame { callback } => {
                init.init(callback, ());
            }
            // the damage a client reports is part of what a test can read back
            damage @ wl_surface::Request::DamageBuffer { .. } => server.record(damage),
            wl_surface::Request::Attach { buffer, .. } => {
                server.present("Attach");
                server.pending.insert(surface.id(), buffer);
            }
            wl_surface::Request::Commit => {
                server.present("Commit");
                // a later buffer replaces the one shown, which goes back to the client
                if let Some(Some(buffer)) = server.pending.remove(&surface.id()) {
                    let role = server.roles.get(&surface.id()).copied();
                    server
                        .commits
                        .lock()
                        .unwrap()
                        .push(role.unwrap_or("surface").to_owned());
                    if let Some(old) = server.shown.insert(surface.id(), buffer.clone()) {
                        if server.releases && old != buffer {
                            old.release();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlCompositor, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                let surface = init.init(id, ());
                if let Some(scale) = server.scale {
                    surface.preferred_buffer_scale(scale);
                }
            }
            wl_compositor::Request::CreateRegion { id } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlShm, ()> for Server {
    fn request(
        _: &mut Server,
        _: &Client,
        _: &WlShm,
        request: wl_shm::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let wl_shm::Request::CreatePool { id, .. } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<WlShmPool, ()> for Server {
    fn request(
        _: &mut Server,
        _: &Client,
        _: &WlShmPool,
        request: wl_shm_pool::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let wl_shm_pool::Request::CreateBuffer { id, .. } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<WlSeat, ()> for Server {
    fn request(
        _: &mut Server,
        _: &Client,
        _: &WlSeat,
        request: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let wl_seat::Request::GetPointer { id } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &XdgWmBase,
        request: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let xdg_wm_base::Request::GetXdgSurface { id, surface } = request {
            init.init(id, ());
            server.roles.insert(surface.id(), "window");
        }
    }
}

impl Dispatch<XdgSurface, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        surface: &XdgSurface,
        request: xdg_surface::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let xdg_surface::Request::AckConfigure { serial } = request {
            server.present(&format!("AckConfigure {serial}"));
            return;
        }
        if let xdg_surface::Request::GetToplevel { id } = request {
            let toplevel = init.init(id, ());
            toplevel.configure(0, 0, Vec::new());
            surface.configure(server.next_serial());
            server.toplevels.push((toplevel, surface.clone()));
        }
    }
}

impl Dispatch<CupertinoLayersV1, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &CupertinoLayersV1,
        request: cupertino_layers_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            cupertino_layers_v1::Request::GetLayer { id, surface, .. } => {
                server.roles.insert(surface.id(), "layer");
                let layer = init.init(id, ());
                server.layers.push(layer);
                server.note("get_layer");
            }
            cupertino_layers_v1::Request::GetPresentation { id, .. } => {
                let layer = init.init(id, ());
                server.layers.push(layer);
                server.note("get_presentation");
            }
            other => server.record(other),
        }
    }
}

impl Dispatch<CupertinoAppEffectsV1, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &CupertinoAppEffectsV1,
        request: cupertino_app_effects_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            cupertino_app_effects_v1::Request::GetBackdrop { id, .. } => {
                let backdrop = init.init(id, ());
                server.backdrops.push(backdrop);
                server.note("get_backdrop");
            }
            cupertino_app_effects_v1::Request::GetAppearance { id } => {
                let appearance = init.init(id, ());
                server.appearances.push(appearance);
                server.note("get_appearance");
            }
            cupertino_app_effects_v1::Request::GetMenu { id, .. } => {
                let menu = init.init(id, ());
                server.menus.push(menu);
                server.note("get_menu");
            }
            other => server.record(other),
        }
    }
}

macro_rules! recorded {
    ($($interface:ty => $module:ident),*) => {$(
        impl Dispatch<$interface, ()> for Server {
            fn request(
                server: &mut Server,
                _: &Client,
                _: &$interface,
                request: $module::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Server>,
            ) {
                server.record(request);
            }
        }
    )*};
}
recorded!(
    CupertinoLayerV1 => cupertino_layer_v1,
    CupertinoBackdropV1 => cupertino_backdrop_v1,
    CupertinoAppearanceV1 => cupertino_appearance_v1,
    CupertinoAppMenuV1 => cupertino_app_menu_v1,
    CupertinoPointerScrollV1 => cupertino_pointer_scroll_v1
);

impl Dispatch<CupertinoScrollV1, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &CupertinoScrollV1,
        request: cupertino_scroll_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            cupertino_scroll_v1::Request::GetScroll { id, .. } => {
                let scroll = init.init(id, ());
                server.scrolls.push(scroll);
                server.note("get_scroll");
            }
            other => server.record(other),
        }
    }
}

impl Dispatch<CupertinoSemanticsV1, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &CupertinoSemanticsV1,
        request: cupertino_semantics_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            cupertino_semantics_v1::Request::GetSurface { id, .. } => {
                let surface = init.init(id, ());
                server.semantic_surfaces.push(surface);
                server.note("get_semantic_surface");
            }
            other => server.record(other),
        }
    }
}

impl Dispatch<CupertinoSemanticSurfaceV1, ()> for Server {
    fn request(
        server: &mut Server,
        _: &Client,
        _: &CupertinoSemanticSurfaceV1,
        request: cupertino_semantic_surface_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Server>,
    ) {
        use cupertino_semantic_surface_v1::Request;
        match request {
            Request::Publish {
                epoch_hi,
                epoch_lo,
                payload,
                length,
            } => {
                let epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                let read = cupertino_app_protocols::semantics::read::<Snapshot>(
                    payload, length, MAX_FRAME,
                )
                .map_err(|e| e.to_string());
                server.publications.lock().unwrap().push((epoch, read));
                server.note("publish");
            }
            Request::Unavailable { epoch_hi, epoch_lo } => {
                let epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo);
                server.note(&format!("unavailable {epoch}"));
            }
            Request::Complete { payload, length } => {
                let reply = cupertino_app_protocols::semantics::read::<ActionReply>(
                    payload, length, MAX_FRAME,
                )
                .map(|r| format!("{r:?}"))
                .unwrap_or_else(|e| e.to_string());
                server.replies.lock().unwrap().push(reply);
                server.note("complete");
            }
            other => server.record(other),
        }
    }
}
