//! The title bar style a first-party client chooses with `cupertino_app_effects_v1.set_titlebar_style`,
//! against a headless compositor: the unified window's surface fills the whole frame, its window
//! control buttons take the pointer over the surface and the rest of its top strip goes to the
//! client, while a standard window keeps the server's title bar above the surface. The clients ask
//! for server-side decorations the way the shell processes do. Tests skip (pass) when no EGL device
//! is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_decorations::{ButtonKind, Hit, TitlebarStyle as ServerStyle};
use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::{
    CupertinoAppEffectsV1, TitlebarStyle,
};
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::state::WindowTarget;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Point;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
const SIZE: (u32, u32) = (500, 300);
/// Centre of the close button of a 52 pt unified bar: the 16 pt cell centred in the bar.
const UNIFIED_CLOSE_Y: f64 = 26.0;

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
}

struct Window {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    /// The size of the latest toplevel configure, `SIZE` while the server leaves the size to the client.
    size: (i32, i32),
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Window {
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
        impl Dispatch<$t, ()> for Window {
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
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    CupertinoAppEffectsV1
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Window {
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Window {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            if width > 0 && height > 0 {
                state.size = (width, height);
            }
        }
    }
}

/// Every configure is answered with a white buffer of the configured size.
impl Dispatch<xdg_surface::XdgSurface, ()> for Window {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            let (w, h) = state.size;
            let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC)
                .unwrap();
            let mut file = std::fs::File::from(fd);
            file.write_all(&vec![255u8; (w * h * 4) as usize]).unwrap();
            let pool = state.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
            let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
            pool.destroy();
            state.surface.attach(Some(&buffer), 0, 0);
            state.surface.damage_buffer(0, 0, w, h);
            state.surface.commit();
        }
    }
}

/// A client with one server-decorated window of `style`, drawn white on every configure, until dropped.
struct App {
    stop: Arc<AtomicBool>,
}

impl App {
    fn spawn(socket: PathBuf, app_id: &str, style: TitlebarStyle) -> App {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let app_id = app_id.to_string();
        std::thread::spawn(move || {
            let conn = Connection::from_socket(
                UnixStream::connect(socket).expect("connecting to the compositor"),
            )
            .unwrap();
            let (globals, mut queue) = registry_queue_init::<Window>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let decorations: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let effects: CupertinoAppEffectsV1 = globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id);
            toplevel.set_title("Window".into());
            toplevel.set_min_size(100, 100);
            let decoration = decorations.get_toplevel_decoration(&toplevel, &qh, ());
            decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            if style == TitlebarStyle::Unified {
                effects.set_titlebar_style(&surface, style);
            }
            surface.commit();
            let mut window = Window {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                surface,
                size: (SIZE.0 as i32, SIZE.1 as i32),
            };
            while !flag.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut window).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        App { stop }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[test]
fn a_unified_window_fills_its_frame_and_only_its_buttons_take_the_pointer_over_the_surface() {
    let Some(mut h) = Harness::new() else { return };
    // the standard window first: the unified one maps above it and covers the points the test probes;
    // one application, so the unified window cascades 29 pt right and down and leaves the standard
    // title bar uncovered
    let _standard = App::spawn(
        h.socket_path(),
        "org.example.titlebars",
        TitlebarStyle::Standard,
    );
    h.wait_until("the standard window to map", |s| {
        s.windows.iter().any(|w| w.mapped)
    });
    let _unified = App::spawn(
        h.socket_path(),
        "org.example.titlebars",
        TitlebarStyle::Unified,
    );
    h.wait_until("both windows to map", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 2
    });
    // let the open transforms end, so the frames are where they will stay
    let t = h.state.now() + 5.0;
    let time = t;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();

    let entry_of = |style: ServerStyle| {
        h.state
            .windows
            .iter()
            .find(|w| w.titlebar == style)
            .expect("a window of the style")
    };
    let (unified, standard) = (
        entry_of(ServerStyle::Unified),
        entry_of(ServerStyle::Standard),
    );
    assert_eq!(
        unified.insets().vertical(),
        0.0,
        "the unified surface starts at the top edge"
    );
    assert_eq!(standard.insets().vertical(), 32.0);
    let (unified_id, standard_id) = (unified.id, standard.id);
    let frame_of = |id| h.state.wm.window(id).unwrap().geometry();
    assert_eq!(
        (frame_of(unified_id).w, frame_of(unified_id).h),
        (f64::from(SIZE.0), f64::from(SIZE.1))
    );
    assert_eq!(
        (frame_of(standard_id).w, frame_of(standard_id).h),
        (f64::from(SIZE.0), f64::from(SIZE.1) + 32.0)
    );

    let layout =
        h.state
            .decor
            .for_style(ServerStyle::Unified)
            .layout(cupertino_geometry::Size::new(
                f64::from(SIZE.0),
                f64::from(SIZE.1),
            ));
    let close_x = layout.button(ButtonKind::Close).center.x;
    let at = |id, x: f64, y: f64| {
        let f = frame_of(id);
        h.state.window_target_under(Point::from((f.x + x, f.y + y)))
    };
    match at(unified_id, close_x, UNIFIED_CLOSE_Y) {
        Some(WindowTarget::Decoration {
            id,
            hit: Hit::Button(ButtonKind::Close),
            ..
        }) => assert_eq!(id, unified_id),
        other => panic!("the close button of the unified bar takes the pointer: {other:?}"),
    }
    assert!(
        matches!(at(unified_id, 300.0, 10.0), Some(WindowTarget::Surface { id, .. }) if id == unified_id),
        "the rest of the unified strip belongs to the client's surface"
    );
    assert!(
        matches!(
            at(standard_id, 300.0, 10.0),
            Some(WindowTarget::Decoration {
                hit: Hit::Titlebar,
                ..
            })
        ),
        "the standard title bar is the server's"
    );
}
