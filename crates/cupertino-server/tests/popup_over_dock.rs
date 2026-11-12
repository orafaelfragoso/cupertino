//! A window's popup that opens low on the screen lies over the dock: it is composed above the top layer-shell
//! surfaces, its material reads the dock below it, and a click where the two overlap reaches the popup. One client maps
//! a bar on the top layer at the bottom edge (as the dock does) and a window, then opens a popup with a `Menu`
//! backdrop that reaches over the bar. Only the key window's popups are raised so: a second window that comes in front
//! keeps the first window's popup below it. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::CupertinoBackdropV1;
use cupertino_scene::{SceneKind, SceneLayer};
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::render::scene::compose_scene;
use cupertino_server::Cupertino;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::{layer_map_for_output, PopupManager, WindowSurfaceType};
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::reexports::wayland_server::Resource;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const OUTPUT: (i32, i32) = (1000, 700);
/// The bar at the bottom edge, centred: it spans x 200..800, y 620..700.
const DOCK: (u32, u32) = (600, 80);
/// The bar's colour, opaque red (ARGB8888 bytes in memory order: blue, green, red, alpha).
const DOCK_BYTES: [u8; 4] = [0, 0, 255, 255];
const WINDOW: (i32, i32) = (400, 300);
/// The popup, at the top left of the window's geometry. Wherever the window is placed in the area above the bar, the
/// popup reaches over the bar at `OVERLAP`.
const POPUP: (i32, i32) = (900, 690);
/// A point of the output inside the bar, and inside the popup once it is open.
const OVERLAP: (f64, f64) = (650.0, 660.0);

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
            self.step(1);
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn step(&mut self, n: usize) {
        for _ in 0..n {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
        }
    }

    /// Let every animation (a window's appearance) settle.
    fn settle(&mut self) {
        let t = self.state.now() + 5.0;
        common::set_time(&mut self.state, &mut self.event_loop, t);
        self.state.tick();
        self.step(20);
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// The namespace of the layer surface the pointer would focus at `at`, or "popup" for the window's popup.
    fn under(&self, at: (f64, f64)) -> Option<String> {
        let (surface, _) = self.state.surface_under(at.into())?;
        let map = layer_map_for_output(&self.state.outputs[0].output);
        if let Some(layer) = map.layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL) {
            return Some(layer.namespace().to_string());
        }
        if self.state.popups.find_popup(&surface).is_some() {
            return Some("popup".into());
        }
        Some("window".into())
    }

    fn popup_shown(&self) -> bool {
        self.state.windows.iter().any(|w| {
            w.window.toplevel().is_some_and(|t| {
                PopupManager::popups_for_surface(t.wl_surface())
                    .any(|(p, _)| p.wl_surface().is_alive())
            })
        })
    }

    /// The composed frame, front to back: each item's layer and kind.
    fn composed(&mut self) -> Vec<(SceneLayer, SceneKind)> {
        let mut backend = self.state.backend.take().unwrap();
        let items = self
            .state
            .with_scene(backend.renderer(), 0, |r, mut p| {
                compose_scene(r, &mut p)
                    .frame
                    .items()
                    .iter()
                    .map(|(layer, item)| (*layer, item.id.kind))
                    .collect()
            })
            .expect("render resources");
        self.state.backend = Some(backend);
        items
    }

    fn screenshot(&mut self) -> image::RgbaImage {
        let path = std::env::temp_dir().join(format!(
            "cupertino-test-{}-popup-over-dock.png",
            std::process::id()
        ));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }
}

/// Which of the client's xdg surfaces a configure is for.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    Window,
    Popup,
    Front,
}

struct Client {
    shm: wl_shm::WlShm,
    window: wl_surface::WlSurface,
    size: (i32, i32),
    popup: Option<wl_surface::WlSurface>,
    /// The second window, mapped in front of the first once the popup is open.
    front: Option<wl_surface::WlSurface>,
}

impl Client {
    /// Attach a buffer filled with `pixel` (ARGB8888 bytes in memory order) and commit it.
    fn draw(
        &self,
        surface: &wl_surface::WlSurface,
        (w, h): (i32, i32),
        pixel: [u8; 4],
        qh: &QueueHandle<Self>,
    ) {
        let fd =
            rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        let mut file = std::fs::File::from(fd);
        file.write_all(&pixel.repeat((w * h) as usize)).unwrap();
        let pool = self.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
    }
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
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    wl_output::WlOutput,
    xdg_positioner::XdgPositioner,
    xdg_popup::XdgPopup,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    CupertinoAppEffectsV1,
    CupertinoBackdropV1
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
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

/// The window is drawn white and the popup transparent (its material shows) on every configure.
impl Dispatch<xdg_surface::XdgSurface, Role> for Client {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        role: &Role,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        xdg.ack_configure(serial);
        match role {
            Role::Window => state.draw(&state.window, state.size, [255; 4], qh),
            Role::Popup => {
                if let Some(surface) = &state.popup {
                    state.draw(surface, POPUP, [0; 4], qh);
                }
            }
            Role::Front => {
                if let Some(surface) = &state.front {
                    state.draw(surface, WINDOW, [255; 4], qh);
                }
            }
        }
    }
}

/// The bar is drawn in its colour at the configured size.
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, wl_surface::WlSurface> for Client {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            layer.ack_configure(serial);
            state.draw(surface, (width as i32, height as i32), DOCK_BYTES, qh);
        }
    }
}

/// The client: the bar and the window at once, the popup when `open` is set, a second window when `front` is set.
struct App {
    open: Arc<AtomicBool>,
    front: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl App {
    fn spawn(socket: PathBuf) -> App {
        let open = Arc::new(AtomicBool::new(false));
        let front = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (opening, fronting, stopping) = (open.clone(), front.clone(), stop.clone());
        std::thread::spawn(move || {
            let conn = Connection::from_socket(
                UnixStream::connect(socket).expect("connecting to the compositor"),
            )
            .unwrap();
            let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(&qh, 1..=4, ()).unwrap();
            let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
            let effects: CupertinoAppEffectsV1 = globals.bind(&qh, 1..=1, ()).unwrap();

            let bar = compositor.create_surface(&qh, ());
            let bar_layer = layer_shell.get_layer_surface(
                &bar,
                Some(&output),
                zwlr_layer_shell_v1::Layer::Top,
                "dock".into(),
                &qh,
                bar.clone(),
            );
            bar_layer.set_size(DOCK.0, DOCK.1);
            bar_layer.set_anchor(zwlr_layer_surface_v1::Anchor::Bottom);
            bar_layer.set_exclusive_zone(DOCK.1 as i32);
            bar.commit();

            let window = compositor.create_surface(&qh, ());
            let window_xdg = wm_base.get_xdg_surface(&window, &qh, Role::Window);
            let toplevel = window_xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.popup-over-dock".into());
            toplevel.set_title("Window".into());
            window.commit();
            let mut client = Client {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                window,
                size: WINDOW,
                popup: None,
                front: None,
            };
            let mut front_toplevel: Option<xdg_toplevel::XdgToplevel> = None;
            let mut backdrop: Option<CupertinoBackdropV1> = None;
            while !stopping.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                if opening.load(Ordering::SeqCst) && client.popup.is_none() {
                    let (w, h) = POPUP;
                    let positioner = wm_base.create_positioner(&qh, ());
                    positioner.set_size(w, h);
                    positioner.set_anchor_rect(0, 0, 1, 1);
                    positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
                    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
                    let surface = compositor.create_surface(&qh, ());
                    let popup_xdg = wm_base.get_xdg_surface(&surface, &qh, Role::Popup);
                    let _popup = popup_xdg.get_popup(Some(&window_xdg), &positioner, &qh, ());
                    positioner.destroy();
                    let b: CupertinoBackdropV1 = effects.get_backdrop(&surface, &qh, ());
                    b.set_material("Menu".into());
                    b.set_shape(0, 0, w, h, 10.0);
                    surface.commit();
                    client.popup = Some(surface);
                    backdrop = Some(b);
                }
                if fronting.load(Ordering::SeqCst) && client.front.is_none() {
                    let surface = compositor.create_surface(&qh, ());
                    let xdg = wm_base.get_xdg_surface(&surface, &qh, Role::Front);
                    let toplevel = xdg.get_toplevel(&qh, ());
                    toplevel.set_app_id("org.example.popup-over-dock".into());
                    toplevel.set_title("Front".into());
                    surface.commit();
                    client.front = Some(surface);
                    front_toplevel = Some(toplevel);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if let Some(b) = backdrop {
                b.destroy();
            }
            drop(front_toplevel);
        });
        App { open, front, stop }
    }

    fn open_popup(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// The bar and the window mapped and settled, the popup not open yet.
fn desktop() -> Option<(Harness, App)> {
    let mut h = Harness::new()?;
    h.state.cursor_status = CursorImageStatus::Hidden;
    let app = App::spawn(h.socket_path());
    h.wait_until("the bar and the window to map", |s| {
        let bar = layer_map_for_output(&s.outputs[0].output)
            .layers()
            .any(|l| !l.bbox().is_empty());
        bar && s.windows.iter().any(|w| w.mapped)
    });
    h.settle();
    Some((h, app))
}

fn open_popup(h: &mut Harness, app: &App) {
    app.open_popup();
    h.wait_until("the popup to show", |s| {
        s.windows.iter().any(|w| {
            w.window.toplevel().is_some_and(|t| {
                PopupManager::popups_for_surface(t.wl_surface()).any(|(p, _)| {
                    with_renderer_surface_state(p.wl_surface(), |r| r.buffer().is_some())
                        .unwrap_or(false)
                })
            })
        })
    });
    h.settle();
    assert!(h.popup_shown());
}

#[test]
fn a_window_popup_is_composed_above_the_dock_with_its_material() {
    let Some((mut h, app)) = desktop() else {
        return;
    };
    open_popup(&mut h, &app);
    let items = h.composed();
    let first = |what: &dyn Fn(&(SceneLayer, SceneKind)) -> bool| items.iter().position(what);
    let popup = first(&|(_, k)| *k == SceneKind::PopupContent).expect("the popup is composed");
    let dock = first(&|(l, _)| *l == SceneLayer::LayerTop).expect("the bar is composed");
    assert_eq!(items[popup].0, SceneLayer::Popups, "{items:?}");
    assert!(
        popup < dock,
        "front to back, the popup comes before the top layer: {items:?}"
    );
    // the popup's outline and its material, with what the material samples, lie over the bar too
    let outline =
        first(&|(_, k)| *k == SceneKind::PopupOutline).expect("the popup's outline is composed");
    assert_eq!(items[outline].0, SceneLayer::Popups, "{items:?}");
    assert!(outline < dock, "{items:?}");
    let material = items[popup..]
        .iter()
        .position(|(l, k)| *l == SceneLayer::Popups && *k == SceneKind::Backdrop)
        .map(|i| i + popup)
        .expect("the popup's material is composed with it");
    assert!(material < dock, "{items:?}");
    // the windows stay below the bar
    let window = first(&|(_, k)| *k == SceneKind::WindowContent).expect("the window is composed");
    assert_eq!(items[window].0, SceneLayer::Windows);
    assert!(dock < window, "{items:?}");
}

#[test]
fn a_window_popup_over_the_dock_shows_its_material_over_the_dock() {
    let Some((mut h, app)) = desktop() else {
        return;
    };
    let (x, y) = (OVERLAP.0 as u32, OVERLAP.1 as u32);
    let bar = h.screenshot().get_pixel(x, y).0;
    assert_eq!(bar, [255, 0, 0, 255], "the bar shows at the point at first");
    open_popup(&mut h, &app);
    let over = h.screenshot().get_pixel(x, y).0;
    assert_ne!(
        over[..3],
        bar[..3],
        "the popup's material covers the bar where they overlap"
    );
    assert!(
        over[0] > over[1].saturating_add(16) && over[0] > over[2].saturating_add(16),
        "the material reads the red bar below it: {over:?}"
    );
}

#[test]
fn a_click_where_a_window_popup_overlaps_the_dock_reaches_the_popup() {
    let Some((mut h, app)) = desktop() else {
        return;
    };
    assert_eq!(
        h.under(OVERLAP).as_deref(),
        Some("dock"),
        "the bar takes the pointer at the point before the popup opens"
    );
    open_popup(&mut h, &app);
    assert_eq!(
        h.under(OVERLAP).as_deref(),
        Some("popup"),
        "the popup drawn over the bar takes the pointer there"
    );
    let target = h.state.window_target_under(OVERLAP.into());
    assert!(
        matches!(
            target,
            Some(cupertino_server::state::WindowTarget::Surface { .. })
        ),
        "the popup is the window's target there"
    );
    // a point of the bar left of the popup (which starts at the window's left edge) still reaches the bar
    let id = h.state.wm.key_window().expect("a key window");
    let left = h.state.wm.window(id).unwrap().geometry().x;
    if left > 210.0 {
        assert_eq!(h.under((205.0, 690.0)).as_deref(), Some("dock"));
    }
}

#[test]
fn a_window_in_front_of_the_popups_window_covers_its_popup_and_takes_the_pointer() {
    let Some((mut h, app)) = desktop() else {
        return;
    };
    open_popup(&mut h, &app);
    let back = h.state.wm.key_window().expect("a key window");
    app.front.store(true, Ordering::SeqCst);
    h.wait_until("the second window to map and take the keyboard", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 2
            && s.wm.key_window().is_some_and(|k| k != back)
    });
    h.settle();
    assert!(h.popup_shown(), "the popup is still open");
    let front = h.state.wm.key_window().unwrap();

    // front to back: the second window, then the first window's popup with its window, all below the bar
    let items = h.composed();
    let popup = items
        .iter()
        .position(|(_, k)| *k == SceneKind::PopupContent)
        .expect("the popup is composed");
    assert_eq!(items[popup].0, SceneLayer::Windows, "{items:?}");
    assert!(
        !items.iter().any(|(l, _)| *l == SceneLayer::Popups),
        "nothing of the window behind is raised: {items:?}"
    );
    let contents: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, (_, k))| *k == SceneKind::WindowContent)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(contents.len(), 2, "{items:?}");
    assert!(
        contents[0] < popup && popup < contents[1],
        "the popup lies between the window in front and its own window: {items:?}"
    );

    // the middle of the window in front, which the popup of the window behind reaches, belongs to the window in front
    let frame = h.state.wm.window(front).unwrap().geometry();
    let middle = (frame.x + frame.w / 2.0, frame.y + frame.h / 2.0);
    let (id, ..) = h
        .state
        .window_surface_under(middle.into())
        .expect("a window there");
    assert_eq!(id, front, "the window in front takes the pointer");
    assert_eq!(h.under(middle).as_deref(), Some("window"));
    assert!(h.state.window_popup_under(middle.into()).is_none());

    // the popup's window back in front and key: the popup is raised again and covers the other window
    h.state
        .command(cupertino_wm::Command::FocusWindow { window: back });
    h.settle();
    assert_eq!(h.state.wm.key_window(), Some(back));
    assert_eq!(
        h.under(middle).as_deref(),
        Some("popup"),
        "the popup reaches the middle of the other window"
    );
    let items = h.composed();
    assert!(
        items
            .iter()
            .any(|(l, k)| *l == SceneLayer::Popups && *k == SceneKind::PopupContent),
        "{items:?}"
    );
}
