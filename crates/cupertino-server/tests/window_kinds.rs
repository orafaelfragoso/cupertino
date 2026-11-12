//! Window kinds of Wayland dialogs against a headless compositor: a client maps a parent toplevel,
//! then a child with `xdg_toplevel.set_parent` and, for a modal one, xdg-dialog-v1 `set_modal`. The
//! model must pick the kind (sheet for a modal child, plain otherwise), place the child, play the
//! kind's open transform and, on close, its close transform. Tests skip (pass) when no EGL device is
//! available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_wm::{LifecycleState, Rect, WindowId, WindowKind};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
const PARENT: (i32, i32) = (600, 400);
const CHILD: (i32, i32) = (300, 200);

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

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Stop the clock at `t` and run the model there.
    fn at(&mut self, t: f64) {
        let time = t;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
    }

    fn kind(&self, w: WindowId) -> WindowKind {
        self.state.wm.window(w).unwrap().lifecycle.kind()
    }
}

// ---- client --------------------------------------------------------------------------------

struct ClientState {
    shm: wl_shm::WlShm,
}

/// Opaque white Argb8888 buffer of `size`.
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
    xdg_wm_dialog_v1::XdgWmDialogV1,
    xdg_dialog_v1::XdgDialogV1
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

/// The first configure of a toplevel is answered with a buffer of the size in the user data.
impl Dispatch<xdg_surface::XdgSurface, (wl_surface::WlSurface, (i32, i32))> for ClientState {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        data: &(wl_surface::WlSurface, (i32, i32)),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            let (surface, size) = data;
            surface.attach(Some(&solid_buffer(&state.shm, *size, qh)), 0, 0);
            surface.damage_buffer(0, 0, size.0, size.1);
            surface.commit();
        }
    }
}

/// A toplevel the client keeps alive until it destroys it.
struct Toplevel {
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    dialog: Option<xdg_dialog_v1::XdgDialogV1>,
}

impl Toplevel {
    fn destroy(self) {
        if let Some(d) = self.dialog {
            d.destroy();
        }
        self.toplevel.destroy();
        self.xdg.destroy();
        self.surface.destroy();
    }
}

struct Objects {
    globals: GlobalList,
    compositor: wl_compositor::WlCompositor,
    wm_base: xdg_wm_base::XdgWmBase,
    parent: Option<Toplevel>,
    child: Option<Toplevel>,
}

impl Objects {
    /// A toplevel of `size`, child of the parent when `parent` is set, modal through xdg-dialog when
    /// `modal` is set; both are sent before the first commit, as toolkits do.
    fn toplevel(
        &self,
        size: (i32, i32),
        parent: bool,
        modal: bool,
        qh: &QueueHandle<ClientState>,
    ) -> Toplevel {
        let surface = self.compositor.create_surface(qh, ());
        let xdg = self
            .wm_base
            .get_xdg_surface(&surface, qh, (surface.clone(), size));
        let toplevel = xdg.get_toplevel(qh, ());
        toplevel.set_app_id("org.example.dialogs".to_string());
        if parent {
            toplevel.set_parent(Some(
                &self.parent.as_ref().expect("the parent exists").toplevel,
            ));
        }
        let dialog = modal.then(|| {
            let manager: xdg_wm_dialog_v1::XdgWmDialogV1 =
                self.globals.bind(qh, 1..=1, ()).unwrap();
            let d = manager.get_xdg_dialog(&toplevel, qh, ());
            d.set_modal();
            d
        });
        surface.commit();
        Toplevel {
            surface,
            xdg,
            toplevel,
            dialog,
        }
    }
}

type Job = Box<dyn FnOnce(&mut Objects, &QueueHandle<ClientState>) + Send>;

/// A client thread running jobs between round trips; dropping it ends the thread.
struct Client {
    jobs: Sender<Job>,
    stop: Arc<AtomicBool>,
}

impl Client {
    fn spawn(socket: PathBuf) -> Client {
        let (jobs, rx) = channel::<Job>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            let conn = Connection::from_socket(
                UnixStream::connect(&socket).expect("connecting to the compositor"),
            )
            .unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let mut client = ClientState {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
            };
            let compositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base = globals.bind(&qh, 1..=5, ()).unwrap();
            let mut objects = Objects {
                globals,
                compositor,
                wm_base,
                parent: None,
                child: None,
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
        Client { jobs, stop }
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

/// A client whose parent toplevel is mapped and settled; returns the parent's id and frame.
fn parent_window(h: &mut Harness) -> (Client, WindowId, Rect) {
    let client = Client::spawn(h.socket_path());
    client.run(|o, qh| o.parent = Some(o.toplevel(PARENT, false, false, qh)));
    h.wait_until("the parent to be mapped", |s| s.wm.key_window().is_some());
    let t = h.state.now() + 5.0;
    h.at(t);
    let id = h.state.wm.key_window().unwrap();
    (client, id, h.state.wm.window(id).unwrap().geometry())
}

/// Map the child and stop the clock at the moment it maps (before its open transform advances).
fn map_child(h: &mut Harness, client: &Client, modal: bool) -> WindowId {
    let t = h.state.now();
    let time = t;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    client.run(move |o, qh| o.child = Some(o.toplevel(CHILD, true, modal, qh)));
    h.wait_until("the child to be mapped", |s| s.wm.windows().count() == 2);
    h.state
        .wm
        .windows()
        .map(|w| w.id)
        .max_by_key(|id| id.0)
        .unwrap()
}

#[test]
fn a_modal_child_maps_as_a_sheet_under_the_parent_title_bar_and_slides_in_and_out() {
    let Some(mut h) = Harness::new() else { return };
    let (client, parent, pf) = parent_window(&mut h);
    let child = map_child(&mut h, &client, true);
    let w = h.state.wm.window(child).unwrap();
    assert_eq!(h.kind(child), WindowKind::Sheet);
    assert_eq!(w.parent, Some(parent));
    assert!(w.modal);
    let rest = w.geometry();
    let titlebar = h.state.wm.config().snap.titlebar_height;
    assert_eq!(
        (rest.x, rest.y),
        (pf.x + ((pf.w - rest.w) / 2.0).round(), pf.y + titlebar),
        "hangs centred from the parent's title bar"
    );
    assert!(h.state.wm.is_blocked(parent));
    assert_eq!(
        h.state.wm.key_window(),
        Some(child),
        "the sheet takes the focus"
    );

    // open: 32 pt above its place and transparent, then in place after 0.26 s
    let t0 = h.state.now();
    let start = h.state.wm.presentation(child, t0).unwrap();
    assert_eq!((start.frame.y, start.opacity), (rest.y - 32.0, 0.0));
    h.at(t0 + 0.26);
    assert_eq!(
        h.state.wm.window(child).unwrap().state(),
        LifecycleState::Normal
    );
    assert_eq!(
        h.state.wm.presentation(child, t0 + 0.26).unwrap().frame,
        rest
    );

    // a click on the blocked parent focuses the sheet
    h.state
        .command(cupertino_wm::Command::FocusWindow { window: parent });
    assert_eq!(h.state.wm.key_window(), Some(child));

    // close: the compositor animates a snapshot of the sheet out with the sheet's close transform
    client.run(|o, _| o.child.take().unwrap().destroy());
    h.wait_until("the sheet to be destroyed", |s| {
        s.wm.window(child).is_none()
    });
    assert_eq!(
        h.state.wm.key_window(),
        Some(parent),
        "the parent gets the focus back"
    );
    let t1 = h.state.now();
    let ghost = h
        .state
        .fx
        .closing
        .iter()
        .find(|c| c.id == child)
        .expect("the sheet slides out as a snapshot");
    let (at_start, alpha_start) = ghost.placement(t1);
    let (half, alpha_half) = ghost.placement(t1 + 0.13);
    assert_eq!((at_start.y, alpha_start), (ghost.rect.y, 1.0));
    assert!(
        (half.y - (ghost.rect.y - 16.0)).abs() < 1e-9 && (alpha_half - 0.5).abs() < 1e-9,
        "half way: {half:?} at {alpha_half}"
    );
}

/// Under Reduce Motion an opening sheet is drawn at its place from the start and only fades in.
#[test]
fn under_reduce_motion_a_sheet_fades_in_at_its_place() {
    let Some(mut h) = Harness::new() else { return };
    let (client, _, _) = parent_window(&mut h);
    let child = map_child(&mut h, &client, true);
    let rest = h.state.wm.window(child).unwrap().geometry();
    let reduced = cupertino_defaults::accessibility::DisplayOptions {
        reduce_motion: true,
        ..h.state.display_options()
    };
    let t0 = h.state.now();
    let drawn = |h: &Harness, t: f64, options| {
        cupertino_server::render::scene::plan_for(
            &h.state.wm,
            h.state.entry(child).unwrap(),
            0.0,
            t,
            &h.state.fx,
            options,
        )
        .expect("the sheet is drawn")
        .1
    };
    let half = drawn(&h, t0 + 0.065, reduced);
    assert_eq!(half.frame, rest, "no slide");
    assert!(half.opacity > 0.0 && half.opacity < 1.0, "{}", half.opacity);
    let normal = drawn(&h, t0 + 0.065, h.state.display_options());
    assert!(normal.frame.y < rest.y, "the slide without Reduce Motion");
}

#[test]
fn a_parent_under_a_sheet_is_dimmed_as_the_sheet_shows_and_its_content_takes_no_clicks() {
    use cupertino_server::events::BTN_LEFT;
    use smithay::backend::input::ButtonState;
    let Some(mut h) = Harness::new() else { return };
    let (client, parent, pf) = parent_window(&mut h);
    let child = map_child(&mut h, &client, true);
    let t0 = h.state.now();
    assert_eq!(
        h.state.wm.sheet_dimming(parent, t0),
        0.0,
        "nothing dims before the sheet moves"
    );
    let half = h.state.wm.sheet_dimming(parent, t0 + 0.13);
    assert!(
        half > 0.0 && half < 1.0,
        "the dimming follows the sheet: {half}"
    );
    h.at(t0 + 0.26);
    assert_eq!(h.state.wm.sheet_dimming(parent, t0 + 0.26), 1.0);

    // the parent's content beside the sheet: the press is the server's and focuses the sheet
    h.state
        .pointer_moved_to((pf.x + 10.0, pf.bottom() - 10.0).into(), 0, None);
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    assert!(
        h.state.consumed_buttons.contains(&BTN_LEFT),
        "the blocked parent does not get the press"
    );
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    assert_eq!(h.state.wm.key_window(), Some(child));
    assert!(!h.state.wm.is_interacting());
}

#[test]
fn a_plain_child_centred_on_its_parent_grows_in_from_0_98_and_leaves_a_ghost_as_it_closes() {
    let Some(mut h) = Harness::new() else { return };
    let (client, parent, pf) = parent_window(&mut h);
    let child = map_child(&mut h, &client, false);
    let w = h.state.wm.window(child).unwrap();
    assert_eq!(h.kind(child), WindowKind::Plain);
    assert_eq!(w.parent, Some(parent));
    let rest = w.geometry();
    assert_eq!((rest.cx(), rest.cy()), (pf.cx(), pf.cy()));
    let t0 = h.state.now();
    let start = h.state.wm.presentation(child, t0).unwrap();
    assert!(
        (start.scale - 0.98).abs() < 1e-9,
        "it opens from 0.98: {}",
        start.scale
    );
    h.at(t0 + 0.16);
    let p = h.state.wm.presentation(child, t0 + 0.16).unwrap();
    assert_eq!((p.frame, p.opacity, p.scale), (rest, 1.0, 1.0));
    assert!(!h.state.wm.is_blocked(parent));

    client.run(|o, _| o.child.take().unwrap().destroy());
    h.wait_until("the child to be destroyed", |s| {
        s.wm.window(child).is_none()
    });
    assert!(
        h.state.fx.has_ghost(child),
        "a plain window fades and shrinks away as a ghost"
    );
    let t = h.state.now() + 5.0;
    h.at(t);
    assert!(
        !h.state.fx.has_ghost(child),
        "the ghost is gone after the close"
    );
}

#[test]
fn a_first_toplevel_is_centred_and_the_app_s_next_one_cascades_29_pt_right_and_down() {
    let Some(mut h) = Harness::new() else { return };
    let (client, _, first) = parent_window(&mut h);
    let output = h
        .state
        .wm
        .window(h.state.wm.key_window().unwrap())
        .unwrap()
        .output;
    let v = h.state.usable_area(output);
    let size = cupertino_wm::Size::new(first.w, first.h);
    assert_eq!(
        first,
        cupertino_server::policy::centred(v, size),
        "centred, a quarter of the vertical slack above"
    );
    client.run(|o, qh| o.child = Some(o.toplevel(PARENT, false, false, qh)));
    h.wait_until("the second toplevel to be mapped", |s| {
        s.wm.windows().count() == 2
    });
    let second = h
        .state
        .wm
        .windows()
        .map(|w| w.id)
        .max_by_key(|id| id.0)
        .unwrap();
    let next = h.state.wm.window(second).unwrap().geometry();
    assert_eq!(
        next,
        Rect::new(first.x + 29.0, first.y + 29.0, first.w, first.h)
    );
}
