//! A menu never stays open without its keys: when another surface takes the keyboard, the popup grab of the menu ends
//! and the menu gets `popup_done`. Each menu belongs to a window or to a layer surface placed as the shell process places
//! its panel (the dock's bar at the bottom edge, the menu bar along the top, the launcher's panel on the overlay layer,
//! which holds the keyboard), opened with a grab after a press on it. The keyboard is then taken by each kind of surface
//! that can take it: another window (the focus policy), an exclusive overlay surface (the launcher, Control Center), an
//! exclusive top layer surface (Focus Dock, the menu bar engaged by its shortcut), the lock screen, the authentication
//! prompt, an on-demand layer surface (by a press), or nothing (the key window goes). A submenu opened from a menu with
//! a grab of its own belongs to the same chain: focus moving within it keeps both open, and a surface outside it closes
//! both, each told once. The tests need an offscreen EGL device.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::desktop::Desktop;
use cupertino_server::authorization::ShellRole;
use cupertino_server::events::BTN_LEFT;
use cupertino_server::shell_connection::shell_process_connection;
use cupertino_server::Cupertino;
use cupertino_wm::Command;
use smithay::backend::input::ButtonState;
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
use smithay::utils::SERIAL_COUNTER;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// The size of a menu.
const MENU: (i32, i32) = (160, 120);
/// The size of a submenu, which opens beside its menu.
const SUBMENU: (i32, i32) = (120, 80);
/// The size of a window that owns a menu.
const WINDOW: (i32, i32) = (400, 300);

/// A surface with a menu: a window, or a shell panel's layer surface; where it lies, the roles of its process and where
/// a press opens the menu, in output coordinates.
struct Panel {
    /// A window of an application on the public socket, rather than a layer surface.
    window: bool,
    namespace: &'static str,
    roles: &'static [ShellRole],
    layer: Layer,
    anchor: Anchor,
    size: (u32, u32),
    keyboard: KeyboardInteractivity,
    /// Where the menu hangs from, panel-local: it opens above this point for a panel at the bottom edge, below it
    /// otherwise.
    opener: (i32, i32),
    upward: bool,
    /// Where the press goes; `None` for a window, pressed at its centre wherever it was placed.
    press: Option<(f64, f64)>,
}

/// The dock's bar: on the top layer at the bottom edge of the 1000 x 700 output, centred, without the keyboard.
const DOCK: Panel = Panel {
    window: false,
    namespace: "dock",
    roles: &[ShellRole::Session, ShellRole::Dock],
    layer: Layer::Top,
    anchor: Anchor::Bottom,
    size: (600, 80),
    keyboard: KeyboardInteractivity::None,
    opener: (300, 0),
    upward: true,
    press: Some((500.0, 660.0)),
};

/// The menu bar: on the top layer along the top edge, without the keyboard.
const MENU_BAR: Panel = Panel {
    window: false,
    namespace: "menu-bar",
    roles: &[ShellRole::Session, ShellRole::MenuBar],
    layer: Layer::Top,
    anchor: Anchor::Top.union(Anchor::Left).union(Anchor::Right),
    size: (0, 24),
    keyboard: KeyboardInteractivity::None,
    opener: (40, 24),
    upward: false,
    press: Some((40.0, 12.0)),
};

/// The launcher's panel, whose View Options button opens a menu: on the overlay layer at the top, centred, holding the
/// keyboard.
const LAUNCHER: Panel = Panel {
    window: false,
    namespace: "launcher",
    roles: &[ShellRole::Session],
    layer: Layer::Overlay,
    anchor: Anchor::Top,
    size: (600, 400),
    keyboard: KeyboardInteractivity::Exclusive,
    opener: (560, 40),
    upward: false,
    press: Some((760.0, 20.0)),
};

/// A window of an application, whose menu opens below the press.
const APP_WINDOW: Panel = Panel {
    window: true,
    namespace: "",
    roles: &[],
    layer: Layer::Top,
    anchor: Anchor::empty(),
    size: (0, 0),
    keyboard: KeyboardInteractivity::None,
    opener: (WINDOW.0 / 2, WINDOW.1 / 2),
    upward: false,
    press: None,
};

/// What a panel's client saw: whether the menu and the submenu drew themselves, and how many `popup_done` each got.
#[derive(Default)]
struct Seen {
    menu_drawn: bool,
    submenu_drawn: bool,
    menu_done: usize,
    submenu_done: usize,
    /// Each keyboard enter (`true`) and leave the client got, on the panel or window, the menu or the submenu.
    keys: Vec<(&'static str, bool)>,
}

struct State {
    seen: Arc<Mutex<Seen>>,
    /// The surface of the window, for a window's client.
    window: Option<wl_surface::WlSurface>,
    window_drawn: bool,
    /// The surfaces of the menu and the submenu, once they were made.
    menu: Option<wl_surface::WlSurface>,
    submenu: Option<wl_surface::WlSurface>,
    shm: wl_shm::WlShm,
    /// The serials of the presses the client got, in order: the first opens the menu, the second (on the menu) the
    /// submenu.
    presses: Vec<u32>,
    buffers: Vec<wl_buffer::WlBuffer>,
}

fn draw(
    state: &mut State,
    surface: &wl_surface::WlSurface,
    size: (i32, i32),
    qh: &QueueHandle<State>,
) {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("menu", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![255u8; (w * h * 4) as usize]).unwrap();
    let pool = state.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w, h);
    surface.commit();
    state.buffers.push(buffer);
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
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
        impl Dispatch<$t, ()> for State {
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
    xdg_positioner::XdgPositioner,
    xdg_toplevel::XdgToplevel,
    zwlr_layer_shell_v1::ZwlrLayerShellV1
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
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

/// The panel: it draws itself at each configure, at the size it was given.
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, wl_surface::WlSurface> for State {
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
            draw(state, surface, (width as i32, height as i32), qh);
        }
    }
}

/// The window, the menu and the submenu: each draws itself at its first configure.
impl Dispatch<xdg_surface::XdgSurface, wl_surface::WlSurface> for State {
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
            if state.window.as_ref() == Some(surface) {
                if !state.window_drawn {
                    draw(state, surface, WINDOW, qh);
                    state.window_drawn = true;
                }
            } else if state.submenu.as_ref() == Some(surface) {
                if !state.seen.lock().unwrap().submenu_drawn {
                    draw(state, surface, SUBMENU, qh);
                    state.seen.lock().unwrap().submenu_drawn = true;
                }
            } else if !state.seen.lock().unwrap().menu_drawn {
                draw(state, surface, MENU, qh);
                state.seen.lock().unwrap().menu_drawn = true;
            }
        }
    }
}

/// A popup, `true` for the submenu: it counts the `popup_done` it gets.
impl Dispatch<xdg_popup::XdgPopup, bool> for State {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        submenu: &bool,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            let mut seen = state.seen.lock().unwrap();
            if *submenu {
                seen.submenu_done += 1;
            } else {
                seen.menu_done += 1;
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
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
            if caps.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}

/// The keyboard: it notes on which of its surfaces each enter and leave was.
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let (surface, entered) = match event {
            wl_keyboard::Event::Enter { surface, .. } => (surface, true),
            wl_keyboard::Event::Leave { surface, .. } => (surface, false),
            _ => return,
        };
        let name = if state.submenu.as_ref() == Some(&surface) {
            "submenu"
        } else if state.menu.as_ref() == Some(&surface) {
            "menu"
        } else {
            "panel"
        };
        state.seen.lock().unwrap().keys.push((name, entered));
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            serial,
            state: WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        {
            state.presses.push(serial);
        }
    }
}

/// A panel's client on a thread of its own: it maps the panel, the first press on it opens the menu with a popup grab
/// on that press, and a second press (on the menu) opens a submenu beside it with a grab of its own, or, once told to
/// ([`PanelClient::rest_on_menu`]), a submenu opens as a pointer resting on its row opens it: with a grab on the press
/// that opened the menu. It disconnects when dropped.
struct PanelClient {
    seen: Arc<Mutex<Seen>>,
    stop: Arc<AtomicBool>,
    rested: Arc<AtomicBool>,
    rounds: common::Rounds,
}

impl PanelClient {
    fn spawn(stream: UnixStream, panel: &'static Panel) -> PanelClient {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let rested = Arc::new(AtomicBool::new(false));
        let rested2 = rested.clone();
        let rounds = common::Rounds::default();
        let (seen2, stop2, rounds2) = (seen.clone(), stop.clone(), rounds.clone());
        std::thread::spawn(move || {
            let _ended = rounds2.ended_on_drop();
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<State>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=5, ()).unwrap();
            let layers: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(&qh, 1..=4, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            // the menu's parent: the window's xdg surface, or the panel's layer surface
            let (parent, layer, _toplevel) = if panel.window {
                let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
                let toplevel = xdg.get_toplevel(&qh, ());
                toplevel.set_app_id("org.example.menu-window".into());
                (Some(xdg), None, Some(toplevel))
            } else {
                let layer = layers.get_layer_surface(
                    &surface,
                    None,
                    panel.layer,
                    panel.namespace.into(),
                    &qh,
                    surface.clone(),
                );
                layer.set_size(panel.size.0, panel.size.1);
                layer.set_anchor(panel.anchor);
                layer.set_keyboard_interactivity(panel.keyboard);
                (None, Some(layer), None)
            };
            surface.commit();
            let mut state = State {
                seen: seen2,
                window: panel.window.then(|| surface.clone()),
                window_drawn: false,
                menu: None,
                submenu: None,
                shm,
                presses: Vec::new(),
                buffers: Vec::new(),
            };
            let mut menu = None;
            let mut submenu = None;
            while !stop2.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut state).is_err() {
                    return;
                }
                rounds2.tick();
                if let (Some(&serial), None) = (state.presses.first(), &menu) {
                    let positioner = wm_base.create_positioner(&qh, ());
                    positioner.set_size(MENU.0, MENU.1);
                    positioner.set_anchor_rect(panel.opener.0, panel.opener.1, 1, 1);
                    let (anchor, gravity) = if panel.upward {
                        (xdg_positioner::Anchor::Top, xdg_positioner::Gravity::Top)
                    } else {
                        (
                            xdg_positioner::Anchor::Bottom,
                            xdg_positioner::Gravity::Bottom,
                        )
                    };
                    positioner.set_anchor(anchor);
                    positioner.set_gravity(gravity);
                    let menu_surface = compositor.create_surface(&qh, ());
                    let xdg = wm_base.get_xdg_surface(&menu_surface, &qh, menu_surface.clone());
                    let popup = xdg.get_popup(parent.as_ref(), &positioner, &qh, false);
                    if let Some(layer) = &layer {
                        layer.get_popup(&popup);
                    }
                    popup.grab(&seat, serial);
                    menu_surface.commit();
                    state.menu = Some(menu_surface.clone());
                    menu = Some((menu_surface, xdg, popup));
                }
                let rest = rested2
                    .load(Ordering::SeqCst)
                    .then(|| state.presses.first())
                    .flatten();
                if let (Some(&serial), Some((_, menu_xdg, _)), None) =
                    (state.presses.get(1).or(rest), &menu, &submenu)
                {
                    let positioner = wm_base.create_positioner(&qh, ());
                    positioner.set_size(SUBMENU.0, SUBMENU.1);
                    positioner.set_anchor_rect(MENU.0 - 1, 10, 1, 1);
                    positioner.set_anchor(xdg_positioner::Anchor::Right);
                    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
                    let surface = compositor.create_surface(&qh, ());
                    let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
                    let popup = xdg.get_popup(Some(menu_xdg), &positioner, &qh, true);
                    popup.grab(&seat, serial);
                    surface.commit();
                    state.submenu = Some(surface.clone());
                    submenu = Some((surface, xdg, popup));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        PanelClient {
            seen,
            stop,
            rested,
            rounds,
        }
    }

    fn menu_drawn(&self) -> bool {
        self.seen.lock().unwrap().menu_drawn
    }

    fn submenu_drawn(&self) -> bool {
        self.seen.lock().unwrap().submenu_drawn
    }

    fn popup_done(&self) -> bool {
        self.seen.lock().unwrap().menu_done > 0
    }

    /// The pointer rests on the menu's row of a submenu: the submenu opens with a grab on the press that opened the menu.
    fn rest_on_menu(&self) {
        self.rested.store(true, Ordering::SeqCst);
    }

    /// The keyboard enters and leaves the client got, in order.
    fn keys(&self) -> Vec<(&'static str, bool)> {
        self.seen.lock().unwrap().keys.clone()
    }

    /// The `popup_done` the menu and the submenu got.
    fn done_counts(&self) -> (usize, usize) {
        let seen = self.seen.lock().unwrap();
        (seen.menu_done, seen.submenu_done)
    }
}

impl Drop for PanelClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn wait_until(d: &mut Desktop, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
    let end = Instant::now() + common::HANG_GUARD;
    while !cond(&d.state) {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        d.pump();
    }
}

fn keyboard_focus(s: &Cupertino) -> Option<ServerSurface> {
    s.seat.get_keyboard().unwrap().current_focus()
}

/// The namespace of the layer surface `surface` is, if it is one.
fn namespace(s: &Cupertino, surface: &ServerSurface) -> Option<String> {
    s.outputs.iter().find_map(|o| {
        layer_map_for_output(&o.output)
            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .map(|l| l.namespace().to_string())
    })
}

/// Whether a layer surface named `name` is mapped with a buffer.
fn mapped(s: &Cupertino, name: &str) -> bool {
    s.outputs.iter().any(|o| {
        layer_map_for_output(&o.output).layers().any(|l| {
            l.namespace() == name
                && smithay::backend::renderer::utils::with_renderer_surface_state(
                    l.wl_surface(),
                    |state| state.buffer().is_some(),
                )
                .unwrap_or(false)
        })
    })
}

/// The centre of the first mapped window, in output coordinates.
fn window_centre(s: &Cupertino) -> (f64, f64) {
    let window = s
        .windows
        .iter()
        .find(|w| w.mapped)
        .expect("a mapped window");
    let frame = s.wm.window(window.id).unwrap().geometry();
    (
        frame.x + f64::from(WINDOW.0) / 2.0,
        frame.y + f64::from(WINDOW.1) / 2.0,
    )
}

/// A press and release at `at`.
fn click(d: &mut Desktop, at: (f64, f64)) {
    d.state.pointer_moved_to(at.into(), 0, None);
    d.pump();
    d.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    d.pump();
    d.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    d.pump();
}

/// A desktop with `panel` mapped and its menu open with a popup grab, after a press on the panel.
fn menu_open(panel: &'static Panel) -> (Desktop, PanelClient) {
    let mut d = Desktop::new(1.0);
    let stream = if panel.window {
        UnixStream::connect(d.socket()).unwrap()
    } else {
        shell_process_connection(&d.state.display_handle, panel.roles).unwrap()
    };
    let client = PanelClient::spawn(stream, panel);
    if panel.window {
        wait_until(&mut d, "the window to map", |s| {
            s.windows.iter().any(|w| w.mapped)
        });
    } else {
        wait_until(&mut d, "the panel to map", |s| mapped(s, panel.namespace));
    }
    let at = panel.press.unwrap_or_else(|| window_centre(&d.state));
    click(&mut d, at);
    wait_until(&mut d, "the menu to open with a grab", |s| {
        client.menu_drawn() && s.popup_grab_root().is_some()
    });
    // the keyboard is with the menu
    let focus = keyboard_focus(&d.state).expect("the keyboard has a focus");
    assert!(
        d.state.popups.find_popup(&focus).is_some(),
        "the menu holds the keyboard"
    );
    (d, client)
}

/// What takes the keyboard from a menu.
#[derive(Clone, Copy, Debug)]
enum Taker {
    /// The launcher: an exclusive overlay surface of a shell process.
    Launcher,
    /// Control Center's card: an exclusive overlay surface of a shell process.
    ControlCenter,
    /// The dock's bar engaged by Focus Dock: an exclusive top layer surface at the bottom edge.
    FocusDock,
    /// The menu bar engaged by its shortcut: an exclusive top layer surface along the top edge.
    EngagedMenuBar,
    /// Another application's window, focused by the focus policy (as a click on it does).
    Window,
    LockScreen,
    Prompt,
}

/// `taker` takes the keyboard; the clients that make it are returned, to keep them connected.
fn take_keyboard(d: &mut Desktop, taker: Taker) -> Vec<common::Client> {
    let overlay = |d: &mut Desktop, stream: UnixStream, name: &'static str| {
        let client =
            common::Client::overlay(stream, name, (300, 200), KeyboardInteractivity::Exclusive);
        wait_until(d, &format!("the {name} to map"), |s| mapped(s, name));
        client
    };
    let bar = |d: &mut Desktop, roles: &[ShellRole], name: &'static str, anchor: Anchor| {
        let stream = shell_process_connection(&d.state.display_handle, roles).unwrap();
        let client = common::Client::layer(
            stream,
            Layer::Top,
            name,
            (600, 30),
            anchor,
            KeyboardInteractivity::Exclusive,
        );
        wait_until(d, &format!("the {name} to map"), |s| mapped(s, name));
        client
    };
    match taker {
        Taker::Launcher => {
            let stream =
                shell_process_connection(&d.state.display_handle, &[ShellRole::Session]).unwrap();
            vec![overlay(d, stream, "launcher-taker")]
        }
        Taker::FocusDock => vec![bar(
            d,
            &[ShellRole::Session, ShellRole::Dock],
            "focus-dock",
            Anchor::Bottom,
        )],
        Taker::EngagedMenuBar => vec![bar(
            d,
            &[ShellRole::Session, ShellRole::MenuBar],
            "engaged-menu-bar",
            Anchor::Top,
        )],
        Taker::Window => {
            let before = d.state.windows.len();
            let client = common::Client::toplevel(d.socket());
            wait_until(d, "the other window to map", |s| {
                s.windows.iter().filter(|w| w.mapped).count() > before
            });
            let window = d.state.windows.last().unwrap().id;
            d.state.command(Command::FocusWindow { window });
            vec![client]
        }
        Taker::ControlCenter => {
            let stream =
                shell_process_connection(&d.state.display_handle, &[ShellRole::Session]).unwrap();
            vec![overlay(d, stream, "control-center")]
        }
        Taker::Prompt => {
            let stream = common::prompt_agent(&d.state);
            vec![overlay(d, stream, "prompt")]
        }
        Taker::LockScreen => vec![d.lock()],
    }
}

/// Whether `taker` holds the keyboard.
fn holds_keyboard(s: &Cupertino, taker: Taker) -> bool {
    let Some(focus) = keyboard_focus(s) else {
        return false;
    };
    match taker {
        Taker::Launcher => namespace(s, &focus).as_deref() == Some("launcher-taker"),
        Taker::ControlCenter => namespace(s, &focus).as_deref() == Some("control-center"),
        Taker::FocusDock => namespace(s, &focus).as_deref() == Some("focus-dock"),
        Taker::EngagedMenuBar => namespace(s, &focus).as_deref() == Some("engaged-menu-bar"),
        Taker::Window => s.windows.last().is_some_and(|w| w.surface() == &focus),
        Taker::Prompt => cupertino_server::authorization::is_prompt(&focus),
        Taker::LockScreen => s.is_lock_surface(&focus),
    }
}

/// The menu of `panel` is open when `taker` takes the keyboard: the menu is dismissed, its grab is gone and the
/// keyboard is with `taker`.
fn dismissed_when_taken(panel: &'static Panel, taker: Taker) {
    let (mut d, menu) = menu_open(panel);
    let _taker = take_keyboard(&mut d, taker);
    wait_until(
        &mut d,
        &format!(
            "the {:?} menu to be dismissed by the {taker:?}",
            panel.namespace
        ),
        |_| menu.popup_done(),
    );
    assert!(d.state.popup_grab_root().is_none(), "the grab is gone");
    wait_until(
        &mut d,
        &format!("the {taker:?} to hold the keyboard"),
        |s| holds_keyboard(s, taker),
    );
}

#[test]
fn the_dock_menu_closes_when_the_launcher_takes_the_keyboard() {
    dismissed_when_taken(&DOCK, Taker::Launcher);
}

#[test]
fn the_dock_menu_closes_when_control_center_takes_the_keyboard() {
    dismissed_when_taken(&DOCK, Taker::ControlCenter);
}

#[test]
fn the_dock_menu_closes_when_the_session_locks() {
    dismissed_when_taken(&DOCK, Taker::LockScreen);
}

#[test]
fn the_dock_menu_closes_when_the_authentication_prompt_takes_the_keyboard() {
    dismissed_when_taken(&DOCK, Taker::Prompt);
}

#[test]
fn a_menu_bar_menu_closes_when_the_launcher_takes_the_keyboard() {
    dismissed_when_taken(&MENU_BAR, Taker::Launcher);
}

#[test]
fn a_menu_bar_menu_closes_when_control_center_takes_the_keyboard() {
    dismissed_when_taken(&MENU_BAR, Taker::ControlCenter);
}

#[test]
fn a_menu_bar_menu_closes_when_the_session_locks() {
    dismissed_when_taken(&MENU_BAR, Taker::LockScreen);
}

#[test]
fn a_menu_bar_menu_closes_when_the_authentication_prompt_takes_the_keyboard() {
    dismissed_when_taken(&MENU_BAR, Taker::Prompt);
}

#[test]
fn the_launchers_view_options_menu_closes_when_control_center_takes_the_keyboard() {
    dismissed_when_taken(&LAUNCHER, Taker::ControlCenter);
}

#[test]
fn the_launchers_view_options_menu_closes_when_the_session_locks() {
    dismissed_when_taken(&LAUNCHER, Taker::LockScreen);
}

#[test]
fn the_launchers_view_options_menu_closes_when_the_authentication_prompt_takes_the_keyboard() {
    dismissed_when_taken(&LAUNCHER, Taker::Prompt);
}

/// Focus given to the grab's own panel, or to the menu, keeps the menu open: only a surface outside its chain ends it.
#[test]
fn focus_on_the_menus_own_panel_or_the_menu_keeps_it_open() {
    let (mut d, _menu) = menu_open(&LAUNCHER);
    let panel = d
        .state
        .popup_grab_root()
        .cloned()
        .expect("the grab's panel");
    let menu = keyboard_focus(&d.state).expect("the menu");
    d.state.set_keyboard_focus(Some(panel.clone()));
    assert_eq!(d.state.popup_grab_root(), Some(&panel), "the grab goes on");
    d.state.set_keyboard_focus(Some(menu.clone()));
    assert_eq!(d.state.popup_grab_root(), Some(&panel), "the grab goes on");
    assert_eq!(
        keyboard_focus(&d.state),
        Some(menu),
        "the keys stay with the menu"
    );
}

#[test]
fn the_dock_menu_closes_when_the_menu_bar_is_engaged() {
    dismissed_when_taken(&DOCK, Taker::EngagedMenuBar);
}

#[test]
fn the_dock_menu_closes_when_a_window_takes_the_keyboard() {
    dismissed_when_taken(&DOCK, Taker::Window);
}

#[test]
fn a_menu_bar_menu_closes_when_focus_dock_takes_the_keyboard() {
    dismissed_when_taken(&MENU_BAR, Taker::FocusDock);
}

#[test]
fn a_menu_bar_menu_closes_when_a_window_takes_the_keyboard() {
    dismissed_when_taken(&MENU_BAR, Taker::Window);
}

#[test]
fn the_launchers_view_options_menu_closes_when_focus_dock_takes_the_keyboard() {
    dismissed_when_taken(&LAUNCHER, Taker::FocusDock);
}

#[test]
fn the_launchers_view_options_menu_closes_when_the_menu_bar_is_engaged() {
    dismissed_when_taken(&LAUNCHER, Taker::EngagedMenuBar);
}

#[test]
fn a_window_menu_closes_when_the_launcher_takes_the_keyboard() {
    dismissed_when_taken(&APP_WINDOW, Taker::Launcher);
}

#[test]
fn a_window_menu_closes_when_control_center_takes_the_keyboard() {
    dismissed_when_taken(&APP_WINDOW, Taker::ControlCenter);
}

#[test]
fn a_window_menu_closes_when_focus_dock_takes_the_keyboard() {
    dismissed_when_taken(&APP_WINDOW, Taker::FocusDock);
}

#[test]
fn a_window_menu_closes_when_the_menu_bar_is_engaged() {
    dismissed_when_taken(&APP_WINDOW, Taker::EngagedMenuBar);
}

#[test]
fn a_window_menu_closes_when_another_window_takes_the_keyboard() {
    dismissed_when_taken(&APP_WINDOW, Taker::Window);
}

#[test]
fn a_window_menu_closes_when_the_session_locks() {
    dismissed_when_taken(&APP_WINDOW, Taker::LockScreen);
}

#[test]
fn a_window_menu_closes_when_the_authentication_prompt_takes_the_keyboard() {
    dismissed_when_taken(&APP_WINDOW, Taker::Prompt);
}

/// While the launcher holds the keyboard, a window the focus policy picks does not get it (the exclusive layer keeps
/// it until it lets go), so the launcher's menu stays open with its keys.
#[test]
fn a_window_focused_under_the_launcher_leaves_its_menu_open() {
    let (mut d, menu) = menu_open(&LAUNCHER);
    let _window = take_keyboard(&mut d, Taker::Window);
    let focus = keyboard_focus(&d.state).expect("the menu");
    assert!(
        d.state.popups.find_popup(&focus).is_some(),
        "the menu keeps the keys"
    );
    assert!(d.state.popup_grab_root().is_some(), "the grab goes on");
    assert!(!menu.popup_done());
}

/// An on-demand layer surface takes the keyboard only by a press on it: while a menu is open that press is outside the
/// menu, so it closes the menu and is consumed; the next press gives the surface the keyboard.
#[test]
fn a_press_on_an_on_demand_layer_surface_closes_the_menu_then_the_next_takes_the_keyboard() {
    let (mut d, menu) = menu_open(&DOCK);
    let stream = shell_process_connection(&d.state.display_handle, &[ShellRole::Session]).unwrap();
    let _panel = common::Client::overlay(
        stream,
        "on-demand",
        (300, 200),
        KeyboardInteractivity::OnDemand,
    );
    wait_until(&mut d, "the on-demand surface to map", |s| {
        mapped(s, "on-demand")
    });
    assert!(
        d.state.popup_grab_root().is_some(),
        "mapping takes no keyboard"
    );
    click(&mut d, (150.0, 100.0));
    wait_until(&mut d, "the dock menu to be dismissed", |_| {
        menu.popup_done()
    });
    assert!(d.state.popup_grab_root().is_none());
    assert_ne!(
        keyboard_focus(&d.state)
            .and_then(|f| namespace(&d.state, &f))
            .as_deref(),
        Some("on-demand"),
        "the press that closed the menu was consumed"
    );
    click(&mut d, (150.0, 100.0));
    wait_until(&mut d, "the on-demand surface to take the keyboard", |s| {
        keyboard_focus(s).and_then(|f| namespace(s, &f)).as_deref() == Some("on-demand")
    });
}

/// The key window going (the keyboard goes to nothing) while the dock's menu is open ends the grab too: the keyboard
/// moves outside the menu's chain.
#[test]
fn the_key_window_closing_closes_the_dock_menu() {
    let mut d = Desktop::new(1.0);
    d.window();
    wait_until(&mut d, "the window to have the keyboard", |s| {
        keyboard_focus(s).is_some()
    });
    let stream = shell_process_connection(&d.state.display_handle, DOCK.roles).unwrap();
    let menu = PanelClient::spawn(stream, &DOCK);
    wait_until(&mut d, "the dock to map", |s| mapped(s, DOCK.namespace));
    click(&mut d, DOCK.press.unwrap());
    wait_until(&mut d, "the dock menu to open with a grab", |s| {
        menu.menu_drawn() && s.popup_grab_root().is_some()
    });
    d.clients.clear();
    wait_until(&mut d, "the window to go", |s| s.wm.windows().count() == 0);
    wait_until(&mut d, "the dock menu to be dismissed", |_| {
        menu.popup_done()
    });
    assert!(d.state.popup_grab_root().is_none());
}

/// A point of the output where `surface` takes the pointer, once it has a buffer there.
fn point_on(s: &Cupertino, surface: &ServerSurface) -> Option<(f64, f64)> {
    for y in (0..700).step_by(4) {
        for x in (0..1000).step_by(4) {
            let at = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            if s.surface_under(at.into())
                .is_some_and(|(under, _)| &under == surface)
            {
                return Some(at);
            }
        }
    }
    None
}

/// [`menu_open`], then a press on the menu opens a submenu beside it with a grab of its own, which takes the keyboard;
/// with the menu's and the submenu's surfaces.
fn submenu_open(panel: &'static Panel) -> (Desktop, PanelClient, ServerSurface, ServerSurface) {
    let (mut d, client) = menu_open(panel);
    let menu = keyboard_focus(&d.state).expect("the menu has the keyboard");
    wait_until(&mut d, "the menu's buffer", |s| {
        point_on(s, &menu).is_some()
    });
    let at = point_on(&d.state, &menu).unwrap();
    click(&mut d, at);
    wait_until(&mut d, "the submenu to open with a grab", |s| {
        client.submenu_drawn()
            && keyboard_focus(s).is_some_and(|f| f != menu && s.popups.find_popup(&f).is_some())
    });
    let submenu = keyboard_focus(&d.state).unwrap();
    (d, client, menu, submenu)
}

/// Wait until `client` has every event sent before, and the server handled its answers.
fn sync(d: &mut Desktop, client: &PanelClient) {
    let done = client.rounds.barrier();
    wait_until(d, "the client's roundtrips", move |_| done());
}

/// Focus given to the menu, to the panel or window that owns it, or to the submenu keeps both open: the submenu keeps
/// the keys, and neither is told it is done.
fn nested_focus_keeps_the_chain(panel: &'static Panel) {
    let (mut d, client, menu, submenu) = submenu_open(panel);
    let root = d
        .state
        .popup_grab_root()
        .cloned()
        .expect("the grab's owner");
    for target in [menu, root.clone(), submenu.clone()] {
        d.state.set_keyboard_focus(Some(target));
        assert_eq!(d.state.popup_grab_root(), Some(&root), "the grab goes on");
        assert_eq!(
            keyboard_focus(&d.state).as_ref(),
            Some(&submenu),
            "the keys stay with the submenu"
        );
    }
    sync(&mut d, &client);
    assert_eq!(client.done_counts(), (0, 0), "no menu was dismissed");
}

#[test]
fn focus_within_a_dock_menus_chain_keeps_its_submenu_open() {
    nested_focus_keeps_the_chain(&DOCK);
}

#[test]
fn focus_within_a_window_menus_chain_keeps_its_submenu_open() {
    nested_focus_keeps_the_chain(&APP_WINDOW);
}

/// A submenu opened by the pointer resting on its row, with a grab on the press that opened its menu, takes the
/// keyboard straight from the menu: the surface that opened the chain hears nothing of the keyboard in between (a
/// window that did would draw itself inactive for a frame and cross its colours back), and the submenu keeps the
/// keyboard grabbed.
#[test]
fn a_submenu_opened_by_a_rest_takes_the_keyboard_straight_from_its_menu() {
    let (mut d, client) = menu_open(&DOCK);
    let menu = keyboard_focus(&d.state).expect("the menu has the keyboard");
    client.rest_on_menu();
    wait_until(&mut d, "the submenu to open", |_| client.submenu_drawn());
    sync(&mut d, &client);
    let keys = client.keys();
    let opened = keys
        .iter()
        .position(|&key| key == ("menu", true))
        .expect("the menu got the keyboard");
    assert_eq!(
        keys[opened + 1..],
        [("menu", false), ("submenu", true)],
        "from the menu the keyboard goes to the submenu alone: {keys:?}"
    );
    let submenu = keyboard_focus(&d.state).expect("the keyboard has a focus");
    assert!(
        submenu != menu && d.state.popups.find_popup(&submenu).is_some(),
        "the submenu has the keyboard"
    );
    let keyboard = d.state.seat.get_keyboard().unwrap();
    assert!(keyboard.is_grabbed(), "the keyboard stays grabbed");
}

/// With a submenu open, `taker` takes the keyboard: the menu and the submenu are each dismissed exactly once, the grab
/// is gone and the keyboard is with `taker`; ending the grab again afterwards sends nothing more.
fn nested_dismissed_once_when_taken(panel: &'static Panel, taker: Taker) {
    let (mut d, client, _menu, _submenu) = submenu_open(panel);
    let _taker = take_keyboard(&mut d, taker);
    wait_until(&mut d, "the menu and the submenu to be dismissed", |_| {
        let (menu, submenu) = client.done_counts();
        menu > 0 && submenu > 0
    });
    assert!(d.state.popup_grab_root().is_none(), "the grab is gone");
    wait_until(
        &mut d,
        &format!("the {taker:?} to hold the keyboard"),
        |s| holds_keyboard(s, taker),
    );
    d.state.end_popup_grab(SERIAL_COUNTER.next_serial(), 0);
    sync(&mut d, &client);
    assert_eq!(client.done_counts(), (1, 1), "each menu is told once");
}

#[test]
fn a_dock_menu_and_its_submenu_close_once_when_the_launcher_takes_the_keyboard() {
    nested_dismissed_once_when_taken(&DOCK, Taker::Launcher);
}

#[test]
fn a_menu_bar_menu_and_its_submenu_close_once_when_the_authentication_prompt_takes_the_keyboard() {
    nested_dismissed_once_when_taken(&MENU_BAR, Taker::Prompt);
}

#[test]
fn the_launchers_menu_and_its_submenu_close_once_when_the_session_locks() {
    nested_dismissed_once_when_taken(&LAUNCHER, Taker::LockScreen);
}

#[test]
fn a_window_menu_and_its_submenu_close_once_when_another_window_takes_the_keyboard() {
    nested_dismissed_once_when_taken(&APP_WINDOW, Taker::Window);
}
