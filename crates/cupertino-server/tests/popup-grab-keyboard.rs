//! A menu never stays open without its keys: when another surface takes the keyboard, the popup grab of a shell panel's
//! menu ends and the menu gets `popup_done`. Each menu belongs to a layer surface placed as the shell process places its
//! panel (the dock's bar at the bottom edge, the menu bar along the top, the launcher's panel on the overlay layer, which
//! holds the keyboard), opened with a grab after a press on the panel; the keyboard is then taken by the launcher or
//! Control Center (exclusive overlay surfaces of shell processes), the lock screen or the authentication prompt. The
//! tests need an offscreen EGL device.

mod common;

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
use smithay::backend::input::ButtonState;
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// The size of a menu.
const MENU: (i32, i32) = (160, 120);

/// A shell panel with a menu: where its layer surface lies, the roles of its process and where a press opens the
/// menu, in output coordinates.
struct Panel {
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
    press: (f64, f64),
}

/// The dock's bar: on the top layer at the bottom edge of the 1000 x 700 output, centred, without the keyboard.
const DOCK: Panel = Panel {
    namespace: "dock",
    roles: &[ShellRole::Session, ShellRole::Dock],
    layer: Layer::Top,
    anchor: Anchor::Bottom,
    size: (600, 80),
    keyboard: KeyboardInteractivity::None,
    opener: (300, 0),
    upward: true,
    press: (500.0, 660.0),
};

/// The menu bar: on the top layer along the top edge, without the keyboard.
const MENU_BAR: Panel = Panel {
    namespace: "menu-bar",
    roles: &[ShellRole::Session, ShellRole::MenuBar],
    layer: Layer::Top,
    anchor: Anchor::Top.union(Anchor::Left).union(Anchor::Right),
    size: (0, 24),
    keyboard: KeyboardInteractivity::None,
    opener: (40, 24),
    upward: false,
    press: (40.0, 12.0),
};

/// The launcher's panel, whose View Options button opens a menu: on the overlay layer at the top, centred, holding the
/// keyboard.
const LAUNCHER: Panel = Panel {
    namespace: "launcher",
    roles: &[ShellRole::Session],
    layer: Layer::Overlay,
    anchor: Anchor::Top,
    size: (600, 400),
    keyboard: KeyboardInteractivity::Exclusive,
    opener: (560, 40),
    upward: false,
    press: (760.0, 20.0),
};

/// What a panel's client saw.
#[derive(Default)]
struct Seen {
    menu_drawn: bool,
    popup_done: bool,
}

struct State {
    seen: Arc<Mutex<Seen>>,
    shm: wl_shm::WlShm,
    press: Option<u32>,
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

/// The menu: it draws itself at its first configure.
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
            if !state.seen.lock().unwrap().menu_drawn {
                draw(state, surface, MENU, qh);
                state.seen.lock().unwrap().menu_drawn = true;
            }
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for State {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            state.seen.lock().unwrap().popup_done = true;
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
        }
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
            state.press.get_or_insert(serial);
        }
    }
}

/// A panel's client on a thread of its own: it maps the panel, and the first press on it opens the menu with a popup
/// grab on that press. It disconnects when dropped.
struct PanelClient {
    seen: Arc<Mutex<Seen>>,
    stop: Arc<AtomicBool>,
}

impl PanelClient {
    fn spawn(stream: UnixStream, panel: &'static Panel) -> PanelClient {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
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
            surface.commit();
            let mut state = State {
                seen: seen2,
                shm,
                press: None,
                buffers: Vec::new(),
            };
            let mut menu = None;
            while !stop2.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut state).is_err() {
                    return;
                }
                if let (Some(serial), None) = (state.press, &menu) {
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
                    let popup = xdg.get_popup(None, &positioner, &qh, ());
                    layer.get_popup(&popup);
                    popup.grab(&seat, serial);
                    menu_surface.commit();
                    menu = Some((menu_surface, xdg, popup));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        PanelClient { seen, stop }
    }

    fn menu_drawn(&self) -> bool {
        self.seen.lock().unwrap().menu_drawn
    }

    fn popup_done(&self) -> bool {
        self.seen.lock().unwrap().popup_done
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

/// A desktop with `panel` mapped and its menu open with a popup grab, after a press on the panel.
fn menu_open(panel: &'static Panel) -> (Desktop, PanelClient) {
    let mut d = Desktop::new(1.0);
    let stream = shell_process_connection(&d.state.display_handle, panel.roles).unwrap();
    let client = PanelClient::spawn(stream, panel);
    wait_until(&mut d, "the panel to map", |s| mapped(s, panel.namespace));
    d.state.pointer_moved_to(panel.press.into(), 0, None);
    d.pump();
    d.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    d.pump();
    d.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
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
    Launcher,
    ControlCenter,
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
    match taker {
        Taker::Launcher => {
            let stream =
                shell_process_connection(&d.state.display_handle, &[ShellRole::Session]).unwrap();
            vec![overlay(d, stream, "launcher-taker")]
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
            "the {} menu to be dismissed by the {taker:?}",
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
