//! `cupertino_shell_v1.get_window_manager` end to end: the compositor runs in-process on an
//! offscreen EGL device, a window manager client and application clients connect through its
//! socket. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_protocols::client::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_protocols::client::cupertino_window_manager_v1::{
    self, CupertinoWindowManagerV1, Placement, WindowState,
};
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::events::BTN_LEFT;
use cupertino_server::input::{translate_action, Action};
use cupertino_server::window_manager::Look;
use cupertino_server::Cupertino;
use cupertino_wm::{Rect, WindowId};
use smithay::backend::input::ButtonState;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Logical, Point};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
/// A strip thumbnail rectangle, global logical points.
const THUMB: (i32, i32, i32, i32) = (20, 100, 150, 100);

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
        common::each_connection_a_process(&mut state);
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

    /// A connection of a shell process the server supervises: the only kind that may be a window manager.
    fn shell_stream(&mut self) -> UnixStream {
        cupertino_server::shell_connection::shell_process_connection(
            &self.state.display_handle,
            &[ShellRole::WindowManager],
        )
        .unwrap()
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Move the model clock forward and settle the animations.
    fn advance(&mut self, dt: f64) {
        let time = self.state.now() + dt;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        for _ in 0..3 {
            self.step();
        }
    }

    /// Two application windows of different applications, opening animations finished: (older, newer).
    /// Both open centred, so the older one is the larger and the newer one leaves its edges uncovered.
    fn two_windows(&mut self) -> (WindowId, WindowId, AppClient, AppClient) {
        let a = AppClient::spawn(self.socket_path(), "org.example.editor", (400, 300));
        self.wait_until("the first window to map", |s| s.wm.windows().count() == 1);
        let b = AppClient::spawn(self.socket_path(), "org.example.viewer", (300, 200));
        self.wait_until("the second window to map", |s| s.wm.windows().count() == 2);
        self.advance(5.0);
        let mut ids: Vec<WindowId> = self.state.wm.windows().map(|w| w.id).collect();
        ids.sort();
        (ids[0], ids[1], a, b)
    }

    /// How `id` is drawn now.
    fn look(&self, id: WindowId) -> Look {
        self.state.window_managers.look(id, self.state.now())
    }

    fn window_at(&self, p: (f64, f64)) -> Option<WindowId> {
        self.state
            .window_surface_under(Point::<f64, Logical>::from(p))
            .map(|(id, _, _)| id)
    }

    fn click(&mut self, p: (f64, f64)) {
        self.state.pointer_moved_to(Point::from(p), 0, None);
        self.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
        self.state
            .pointer_button(BTN_LEFT, ButtonState::Released, 0);
        self.step();
    }
}

// ---- test clients --------------------------------------------------------------------------

/// What the window manager object sent, in order.
#[derive(Clone, Debug, PartialEq)]
enum WmEvent {
    UsableArea(i32, i32, i32, i32),
    Window {
        id: u32,
        app_id: String,
        frame: (i32, i32, i32, i32),
        active: bool,
    },
    Closed(u32),
    ToggleStage,
    StripPressed(u32),
    Done,
}

#[derive(Default)]
struct Status {
    events: Vec<WmEvent>,
    error: Option<(String, u32)>,
}

enum Cmd {
    Place(u32, Placement),
    Commit,
    Activate(u32),
    /// Close the connection, as a dying process does.
    Disconnect,
}

struct WmState {
    status: Arc<Mutex<Status>>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WmState {
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
impl Dispatch<wl_output::WlOutput, ()> for WmState {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<CupertinoShellV1, ()> for WmState {
    fn event(
        _: &mut Self,
        _: &CupertinoShellV1,
        _: cupertino_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoWindowManagerV1, ()> for WmState {
    fn event(
        state: &mut Self,
        _: &CupertinoWindowManagerV1,
        event: cupertino_window_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use cupertino_window_manager_v1::Event as E;
        let event = match event {
            E::UsableArea {
                x,
                y,
                width,
                height,
            } => WmEvent::UsableArea(x, y, width, height),
            E::Window {
                id,
                app_id,
                x,
                y,
                width,
                height,
                state,
            } => {
                let active = matches!(state, WEnum::Value(s) if s.contains(WindowState::Active));
                WmEvent::Window {
                    id,
                    app_id,
                    frame: (x, y, width, height),
                    active,
                }
            }
            E::WindowClosed { id } => WmEvent::Closed(id),
            E::ToggleStage => WmEvent::ToggleStage,
            E::StripPressed { id } => WmEvent::StripPressed(id),
            E::Done => WmEvent::Done,
            other => panic!("unexpected event {other:?}"),
        };
        state.status.lock().unwrap().events.push(event);
    }
}

/// A window manager client on the first output, on its own thread.
struct WmClient {
    commands: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
}

impl WmClient {
    fn spawn(stream: UnixStream) -> WmClient {
        let (commands, inbox): (Sender<Cmd>, Receiver<Cmd>) = channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let st = status.clone();
        // detached: it ends on Disconnect or when the compositor drops its socket
        std::thread::spawn(move || {
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<WmState>(&conn).unwrap();
            let qh = queue.handle();
            let shell: CupertinoShellV1 = globals.bind(&qh, 1..=1, ()).unwrap();
            let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
            let wm = shell.get_window_manager(&output, &qh, ());
            let mut state = WmState { status: st.clone() };
            loop {
                while let Ok(cmd) = inbox.try_recv() {
                    match cmd {
                        Cmd::Place(id, placement) => {
                            wm.set_placement(id, placement, THUMB.0, THUMB.1, THUMB.2, THUMB.3)
                        }
                        Cmd::Commit => wm.commit(),
                        Cmd::Activate(id) => wm.activate_window(id),
                        Cmd::Disconnect => return,
                    }
                }
                if queue.roundtrip(&mut state).is_err() {
                    let error = conn
                        .protocol_error()
                        .expect("the roundtrip failed with a protocol error");
                    st.lock().unwrap().error = Some((error.object_interface, error.code));
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        WmClient { commands, status }
    }

    fn send(&self, cmd: Cmd) {
        self.commands.send(cmd).unwrap();
    }

    fn events(&self) -> Vec<WmEvent> {
        self.status.lock().unwrap().events.clone()
    }

    fn error(&self) -> Option<(String, u32)> {
        self.status.lock().unwrap().error.clone()
    }
}

struct AppState {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    size: (i32, i32),
    attached: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for AppState {
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
        impl Dispatch<$t, ()> for AppState {
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
    xdg_toplevel::XdgToplevel
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for AppState {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for AppState {
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
                let (w, h) = state.size;
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

/// An application with one toplevel of `size`; its thread lives until the compositor drops the socket.
struct AppClient;

impl AppClient {
    fn spawn(socket: PathBuf, app_id: &'static str, size: (i32, i32)) -> AppClient {
        std::thread::spawn(move || {
            let conn = Connection::from_socket(UnixStream::connect(&socket).unwrap()).unwrap();
            let (globals, mut queue) = registry_queue_init::<AppState>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            surface.commit();
            let mut state = AppState {
                shm,
                surface,
                size,
                attached: false,
            };
            while queue.blocking_dispatch(&mut state).is_ok() {}
        });
        AppClient
    }
}

fn whole(r: Rect) -> (i32, i32, i32, i32) {
    (
        r.x.round() as i32,
        r.y.round() as i32,
        r.w.round() as i32,
        r.h.round() as i32,
    )
}

const THUMB_RECT: Rect = Rect {
    x: THUMB.0 as f64,
    y: THUMB.1 as f64,
    w: THUMB.2 as f64,
    h: THUMB.3 as f64,
};

/// A point of window `a` that window `b`, above it, does not cover.
fn uncovered_point(h: &Harness, a: WindowId, b: WindowId) -> (f64, f64) {
    let (fa, fb) = (
        h.state.wm.window(a).unwrap().geometry(),
        h.state.wm.window(b).unwrap().geometry(),
    );
    // past the 3 pt resize band, which the server keeps for client-decorated windows too
    let p = (fa.x + 8.0, fa.y + 8.0);
    assert!(
        !fb.contains(cupertino_wm::Point::new(p.0, p.1)),
        "the smaller window leaves the top-left corner of {fa:?} uncovered: {fb:?}"
    );
    p
}

// ---- tests ---------------------------------------------------------------------------------

/// An application on the public socket asking to be a window manager is refused.
#[test]
fn a_shell_process_without_the_window_manager_role_cannot_be_a_window_manager() {
    let Some(mut h) = Harness::new() else { return };
    let intruder = WmClient::spawn(
        cupertino_server::shell_connection::shell_process_connection(
            &h.state.display_handle,
            &[ShellRole::Session, ShellRole::Dock],
        )
        .unwrap(),
    );
    h.wait_until("the permission error", |_| intruder.error().is_some());
    // error 2 of cupertino_shell_v1.error is `permission`
    assert_eq!(
        intruder.error(),
        Some(("cupertino_shell_v1".to_string(), 2))
    );
}

#[test]
fn the_first_group_has_the_usable_area_and_the_arranged_windows_and_an_output_takes_one_manager() {
    let Some(mut h) = Harness::new() else { return };
    let (a, b, _ca, _cb) = h.two_windows();
    let wm = WmClient::spawn(h.shell_stream());
    h.wait_until("the first group", |_| wm.events().contains(&WmEvent::Done));
    let out = h.state.outputs[0].id;
    let frame = |id: WindowId| whole(h.state.wm.window(id).unwrap().geometry());
    let (x, y, w, hh) = whole(h.state.usable_area(out));
    assert_eq!(
        wm.events(),
        vec![
            WmEvent::UsableArea(x, y, w, hh),
            WmEvent::Window {
                id: a.0 as u32,
                app_id: "org.example.editor".into(),
                frame: frame(a),
                active: false
            },
            WmEvent::Window {
                id: b.0 as u32,
                app_id: "org.example.viewer".into(),
                frame: frame(b),
                active: true
            },
            WmEvent::Done,
        ]
    );
    let second = WmClient::spawn(h.shell_stream());
    h.wait_until("the role error", |_| second.error().is_some());
    // error 0 of cupertino_shell_v1.error is `role`
    assert_eq!(
        second.error(),
        Some((CupertinoShellV1::interface().name.to_string(), 0))
    );
    assert_eq!(wm.error(), None);
}

#[test]
fn an_application_id_longer_than_the_limit_is_cut_and_the_manager_stays_connected() {
    let Some(mut h) = Harness::new() else { return };
    // 4080 bytes overflow the wire with the window's other arguments; a two-byte character spans byte 1024
    let long: &'static str =
        Box::leak(format!("{}é{}", "a".repeat(1023), "b".repeat(4080 - 1025)).into_boxed_str());
    let _app = AppClient::spawn(h.socket_path(), long, (400, 300));
    h.wait_until("the window to map", |s| s.wm.windows().count() == 1);
    h.advance(5.0);
    let wm = WmClient::spawn(h.shell_stream());
    h.wait_until("the first group", |_| wm.events().contains(&WmEvent::Done));
    assert!(
        wm.events().iter().any(|e| matches!(
            e,
            WmEvent::Window { app_id, .. } if *app_id == "a".repeat(1023)
        )),
        "{:?}",
        wm.events()
    );
    assert_eq!(wm.error(), None, "the manager stays connected");
}

#[test]
fn a_strip_placement_animates_into_its_rectangle_takes_no_input_and_reports_presses() {
    let Some(mut h) = Harness::new() else { return };
    let (a, b, _ca, _cb) = h.two_windows();
    let wm = WmClient::spawn(h.shell_stream());
    h.wait_until("the first group", |_| wm.events().contains(&WmEvent::Done));
    let corner = uncovered_point(&h, a, b);
    assert_eq!(
        h.window_at(corner),
        Some(a),
        "before the placement the window takes the pointer"
    );

    wm.send(Cmd::Place(a.0 as u32, Placement::Strip));
    wm.send(Cmd::Commit);
    h.wait_until("the placement to apply", |s| {
        s.window_managers.passes_input(a)
    });
    let Look::Placed {
        rect: start,
        strip: true,
        ..
    } = h.look(a)
    else {
        panic!("not placed in the strip: {:?}", h.look(a))
    };
    assert!(
        start.w > THUMB_RECT.w,
        "the swap starts where the window was drawn: {start:?}"
    );
    h.advance(0.1);
    let Look::Placed { rect: mid, .. } = h.look(a) else {
        panic!("still placed")
    };
    assert!(
        mid.w < start.w && mid.w > THUMB_RECT.w,
        "mid-swap the window shrinks: {mid:?}"
    );
    h.advance(5.0);
    let Look::Placed {
        rect: end,
        strip: true,
        rounding,
        ..
    } = h.look(a)
    else {
        panic!("still in the strip")
    };
    assert!(
        whole(end) == THUMB,
        "the swap ends on the committed rectangle: {end:?}"
    );
    assert_eq!(
        h.state.window_managers.corner_radius(12.0, rounding),
        8.0,
        "the thumbnail ends with the strip's corner radius"
    );
    assert_eq!(
        h.window_at(corner),
        None,
        "the window's own frame no longer takes the pointer"
    );
    assert_eq!(h.look(b), Look::Natural);

    h.click((THUMB_RECT.cx(), THUMB_RECT.cy()));
    h.wait_until("the press event", |_| {
        wm.events().contains(&WmEvent::StripPressed(a.0 as u32))
    });
    assert_eq!(
        h.state.wm.key_window(),
        Some(b),
        "a press on a thumbnail does not focus by itself"
    );
    wm.send(Cmd::Activate(a.0 as u32));
    wm.send(Cmd::Place(a.0 as u32, Placement::Stage));
    wm.send(Cmd::Commit);
    h.wait_until("the activation", |s| s.wm.key_window() == Some(a));
    h.wait_until("window a back on the stage", |s| {
        !s.window_managers.passes_input(a)
    });
    h.advance(5.0);
    assert_eq!(
        h.look(a),
        Look::Natural,
        "back at its own frame once the swap ended"
    );
    let active = wm.events().into_iter().rev().find_map(|e| match e {
        WmEvent::Window { id, active, .. } if id == a.0 as u32 => Some(active),
        _ => None,
    });
    assert_eq!(
        active,
        Some(true),
        "the window manager is told the window it activated is active"
    );
}

/// The role is checked on every request, not only at `get_window_manager`: once the process the connection was made
/// for exits, a request on the window manager it already has is refused with the permission error and not carried out.
#[test]
fn a_window_manager_whose_process_exited_is_refused_its_next_request() {
    let Some(mut h) = Harness::new() else { return };
    let (a, b, _ca, _cb) = h.two_windows();
    let (stream, holder) = common::held_shell_connection(&h.state, &[ShellRole::WindowManager]);
    let wm = WmClient::spawn(stream);
    h.wait_until("the first group", |_| wm.events().contains(&WmEvent::Done));
    wm.send(Cmd::Activate(a.0 as u32));
    h.wait_until("the activation", |s| s.wm.key_window() == Some(a));

    holder.end();
    wm.send(Cmd::Activate(b.0 as u32));
    h.wait_until("the permission error", |_| wm.error().is_some());
    // error 2 of cupertino_shell_v1.error is `permission`
    assert_eq!(
        wm.error(),
        Some(("cupertino_window_manager_v1".to_string(), 2))
    );
    assert_eq!(h.state.wm.key_window(), Some(a));
}

#[test]
fn a_dead_window_manager_puts_every_window_back_at_once() {
    let Some(mut h) = Harness::new() else { return };
    let (a, b, _ca, _cb) = h.two_windows();
    let wm = WmClient::spawn(h.shell_stream());
    h.wait_until("the first group", |_| wm.events().contains(&WmEvent::Done));
    let corner = uncovered_point(&h, a, b);
    wm.send(Cmd::Place(a.0 as u32, Placement::Strip));
    wm.send(Cmd::Place(b.0 as u32, Placement::Hidden));
    wm.send(Cmd::Commit);
    h.wait_until("the placements to apply", |s| {
        s.window_managers.passes_input(a) && s.window_managers.passes_input(b)
    });
    assert!(
        matches!(h.look(b), Look::Placed { alpha, .. } if alpha > 0.0),
        "a hidden window fades out first: {:?}",
        h.look(b)
    );
    h.advance(5.0);
    assert_eq!(h.look(b), Look::Hidden);

    wm.send(Cmd::Disconnect);
    h.wait_until("the windows to be released", |s| {
        !s.window_managers.passes_input(a) && !s.window_managers.passes_input(b)
    });
    // no animation is left to wait for
    assert_eq!((h.look(a), h.look(b)), (Look::Natural, Look::Natural));
    assert_eq!(h.window_at(corner), Some(a));
}

#[test]
fn the_stage_action_reaches_the_window_manager_and_unknown_or_closed_windows_are_ignored() {
    let Some(mut h) = Harness::new() else { return };
    let wm = WmClient::spawn(h.shell_stream());
    h.wait_until("the first group", |_| wm.events().contains(&WmEvent::Done));
    let ctx = h.state.action_context();
    h.state
        .execute_plan(translate_action(Action::ToggleStage, &ctx));
    h.wait_until("the toggle", |_| {
        wm.events()
            .ends_with(&[WmEvent::ToggleStage, WmEvent::Done])
    });
    // an id that is not an arranged window: the server ignores it at commit
    wm.send(Cmd::Place(4242, Placement::Strip));
    wm.send(Cmd::Commit);
    wm.send(Cmd::Activate(4242));
    for _ in 0..20 {
        h.step();
    }
    assert_eq!(wm.error(), None);
}

#[test]
fn a_window_whose_frame_changes_during_the_swap_back_heads_for_the_new_frame() {
    let Some(mut h) = Harness::new() else { return };
    let (a, _b, _ca, _cb) = h.two_windows();
    let wm = WmClient::spawn(h.shell_stream());
    h.wait_until("the first group", |_| wm.events().contains(&WmEvent::Done));
    wm.send(Cmd::Place(a.0 as u32, Placement::Strip));
    wm.send(Cmd::Commit);
    h.wait_until("the placement to apply", |s| {
        s.window_managers.passes_input(a)
    });
    h.advance(5.0);
    wm.send(Cmd::Place(a.0 as u32, Placement::Stage));
    wm.send(Cmd::Commit);
    h.wait_until("window a back on the stage", |s| {
        !s.window_managers.passes_input(a)
    });
    h.advance(0.1);
    // drag the window by its title bar 150 pt right and 80 pt down (the model moves its frame)
    let old = h.state.wm.window(a).unwrap().geometry();
    let grab = cupertino_wm::Point::new(old.cx(), old.y + 10.0);
    h.state.command(cupertino_wm::Command::BeginMove {
        window: a,
        pointer: grab,
    });
    h.state.command(cupertino_wm::Command::PointerMotion {
        pointer: cupertino_wm::Point::new(grab.x + 150.0, grab.y + 80.0),
        modifiers: Default::default(),
    });
    h.state.command(cupertino_wm::Command::PointerRelease);
    let moved = h.state.wm.window(a).unwrap().geometry();
    assert!(
        moved.x > old.x + 100.0 && moved.y > old.y + 50.0,
        "the drag moved the frame: {old:?} to {moved:?}"
    );
    // frame by frame, as the server ticks, to the end of the swap: no jump between two frames,
    // also not when the window turns natural
    let rect_of = |look: Look| match look {
        Look::Placed { rect, .. } => rect,
        Look::Natural => moved,
        other => panic!("not drawn: {other:?}"),
    };
    let mut last = rect_of(h.look(a));
    for step in 0..300 {
        h.advance(0.01);
        let now = rect_of(h.look(a));
        assert!(
            (now.x - last.x).abs() < 40.0 && (now.y - last.y).abs() < 40.0,
            "no jump at step {step}: {last:?} then {now:?}"
        );
        if step == 14 {
            assert!(
                now.x > old.x && now.y > old.y,
                "0.25 s into the swap it heads for the new frame {moved:?}: {now:?}"
            );
        }
        if h.look(a) == Look::Natural {
            break;
        }
        last = now;
    }
    assert_eq!(h.look(a), Look::Natural);
    assert_eq!(h.state.wm.window(a).unwrap().geometry(), moved);
}
