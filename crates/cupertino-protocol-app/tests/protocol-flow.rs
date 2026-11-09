//! Runs the public protocols on the transport against a minimal in-process server (shm, an output at scale 2, a seat
//! with a pointer, server-decorated windows, the app effects and scroll globals) and checks what the server receives
//! and what the client reports back.

use cupertino_app_protocols::server::app_effects::cupertino_app_effects_v1::{
    self, CupertinoAppEffectsV1,
};
use cupertino_app_protocols::server::app_effects::cupertino_appearance_v1::{
    CupertinoAppearanceV1, DisplayOption,
};
use cupertino_app_protocols::server::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1,
};
use cupertino_app_protocols::server::scroll::cupertino_pointer_scroll_v1::CupertinoPointerScrollV1;
use cupertino_app_protocols::server::scroll::cupertino_scroll_v1::{self, CupertinoScrollV1};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_geometry::Rect;
use cupertino_protocol_app::{
    AppEffects, AppEvent, BackdropId, BackdropTone, GlassStyle, ScrollCounts, WindowControls,
};
use cupertino_wayland_client::{
    BufferFormat, Connection, Decorations, Error, Event, PointerEvent, SurfaceId, Toplevel,
    ToplevelConfig, WindowConfigure,
};
use std::collections::VecDeque;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_protocols::xdg::decoration::zv1::server::zxdg_decoration_manager_v1::{
    self, ZxdgDecorationManagerV1,
};
use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::{
    self, ZxdgToplevelDecorationV1,
};
use wayland_protocols::xdg::shell::server::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::server::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_pointer::{self, WlPointer};
use wayland_server::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{
    Client as ServerClient, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New,
    Resource,
};

/// What the server saw in one surface commit that carried a buffer.
#[derive(Debug, PartialEq)]
struct Committed {
    buffer_size: (i32, i32),
    backdrops: Vec<BackdropState>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct BackdropState {
    material: String,
    shape: [f64; 5],
    alpha: f64,
    /// Style, container and spacing of `set_glass`.
    glass: Option<(u32, u32, f64)>,
}

enum Control {
    /// Switch the session to the light appearance with reduce transparency and increase contrast.
    AppearanceChange,
    /// Lock the session, telling an observer of version 5 or later.
    Lock,
    /// Resize the toplevel to 640 x 480 and activate it.
    ResizeToplevel,
    /// Enter the toplevel with the pointer and scroll it twice down and once right.
    ScrollToplevel,
    /// Ask the toplevel to close.
    CloseToplevel,
}

/// A request the server saw that the commit record does not cover.
#[derive(Debug, PartialEq)]
enum Seen {
    /// The title bar style of a window, chosen with `set_titlebar_style`.
    TitlebarStyle(cupertino_app_effects_v1::TitlebarStyle),
    /// The controls a window disabled with `set_disabled_controls`.
    DisabledControls(cupertino_app_effects_v1::WindowControl),
    /// `xdg_toplevel.set_maximized` (true) or `unset_maximized`.
    Maximize(bool),
    AppId(String),
    Title(String),
    MinSize(i32, i32),
    DecorationMode(zxdg_toplevel_decoration_v1::Mode),
}

#[derive(Default)]
struct SurfaceState {
    buffer: Option<WlBuffer>,
    backdrops: Vec<Arc<Mutex<BackdropState>>>,
}

type SurfaceData = Arc<Mutex<SurfaceState>>;

/// The window of the test client.
struct ToplevelState {
    surface: WlSurface,
    xdg: XdgSurface,
    toplevel: XdgToplevel,
    /// The initial configure has been sent.
    configured: bool,
}

struct Mock {
    committed: Sender<Committed>,
    seen: Sender<Seen>,
    next_serial: u32,
    appearance: Option<CupertinoAppearanceV1>,
    outputs: Vec<WlOutput>,
    pointer: Option<WlPointer>,
    scroll: Option<CupertinoPointerScrollV1>,
    shown: Option<WlBuffer>,
    toplevel: Option<ToplevelState>,
}

struct Data;
impl ClientData for Data {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

/// A global whose bind only creates the resource.
macro_rules! plain_global {
    ($($interface:ty),*) => {$(
        impl GlobalDispatch<$interface, ()> for Mock {
            fn bind(_: &mut Self, _: &DisplayHandle, _: &ServerClient, r: New<$interface>, _: &(), init: &mut DataInit<'_, Self>) {
                init.init(r, ());
            }
        }
    )*};
}
plain_global!(
    WlCompositor,
    WlShm,
    XdgWmBase,
    ZxdgDecorationManagerV1,
    CupertinoAppEffectsV1,
    CupertinoScrollV1
);

/// Objects whose requests the server ignores.
macro_rules! ignore_requests {
    ($($interface:ty: $data:ty),*) => {$(
        impl Dispatch<$interface, $data> for Mock {
            fn request(_: &mut Self, _: &ServerClient, _: &$interface, _: <$interface as Resource>::Request, _: &$data, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }
    )*};
}
ignore_requests!(
    WlCallback: (),
    WlBuffer: (i32, i32),
    WlOutput: (),
    WlPointer: (),
    CupertinoAppearanceV1: (),
    CupertinoPointerScrollV1: ()
);

impl Dispatch<WlCompositor, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_compositor::Request::CreateSurface { id } = request {
            init.init(id, SurfaceData::default());
        }
    }
}

impl Dispatch<WlSurface, SurfaceData> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        surface: &WlSurface,
        request: wl_surface::Request,
        data: &SurfaceData,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let mut s = data.lock().unwrap();
        match request {
            wl_surface::Request::Attach { buffer, .. } => s.buffer = buffer,
            wl_surface::Request::Frame { callback } => {
                init.init(callback, ());
            }
            wl_surface::Request::Commit => {
                let unbuffered = s.buffer.is_none();
                if let Some(t) = state
                    .toplevel
                    .as_mut()
                    .filter(|t| !t.configured && unbuffered && t.surface == *surface)
                {
                    // initial commit of a toplevel: leave the size to the client
                    t.configured = true;
                    surface.enter(&state.outputs[0]);
                    state.next_serial += 1;
                    t.toplevel.configure(0, 0, Vec::new());
                    t.xdg.configure(state.next_serial);
                }
                if let Some(buffer) = s.buffer.take() {
                    state
                        .committed
                        .send(Committed {
                            buffer_size: *buffer.data::<(i32, i32)>().expect("buffer data"),
                            backdrops: s
                                .backdrops
                                .iter()
                                .map(|b| b.lock().unwrap().clone())
                                .collect(),
                        })
                        .unwrap();
                    // like a compositor that keeps the current buffer until the next one replaces it
                    if let Some(previous) = state.shown.replace(buffer) {
                        previous.release();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlShm, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
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
impl Dispatch<WlShmPool, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlShmPool,
        request: wl_shm_pool::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm_pool::Request::CreateBuffer {
            id, width, height, ..
        } = request
        {
            init.init(id, (width, height));
        }
    }
}

impl GlobalDispatch<WlOutput, ()> for Mock {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WlOutput>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let output = init.init(r, ());
        output.scale(2);
        output.done();
        state.outputs.push(output);
    }
}

impl GlobalDispatch<WlSeat, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ()).capabilities(Capability::Pointer);
    }
}
impl Dispatch<WlSeat, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlSeat,
        request: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_seat::Request::GetPointer { id } = request {
            state.pointer = Some(init.init(id, ()));
        }
    }
}

impl Dispatch<CupertinoScrollV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &CupertinoScrollV1,
        request: cupertino_scroll_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let cupertino_scroll_v1::Request::GetScroll { id, .. } = request {
            state.scroll = Some(init.init(id, ()));
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &XdgWmBase,
        request: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_wm_base::Request::GetXdgSurface { id, surface } = request {
            init.init(id, surface);
        }
    }
}
impl Dispatch<XdgSurface, WlSurface> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        xdg: &XdgSurface,
        request: xdg_surface::Request,
        surface: &WlSurface,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_surface::Request::GetToplevel { id } = request {
            let toplevel = init.init(id, ());
            state.toplevel = Some(ToplevelState {
                surface: surface.clone(),
                xdg: xdg.clone(),
                toplevel,
                configured: false,
            });
        }
    }
}
impl Dispatch<XdgToplevel, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &XdgToplevel,
        request: xdg_toplevel::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            xdg_toplevel::Request::SetAppId { app_id } => Seen::AppId(app_id),
            xdg_toplevel::Request::SetTitle { title } => Seen::Title(title),
            xdg_toplevel::Request::SetMinSize { width, height } => Seen::MinSize(width, height),
            xdg_toplevel::Request::SetMaximized => Seen::Maximize(true),
            xdg_toplevel::Request::UnsetMaximized => Seen::Maximize(false),
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}
impl Dispatch<ZxdgDecorationManagerV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_decoration_manager_v1::Request::GetToplevelDecoration { id, .. } = request {
            init.init(id, ());
        }
    }
}
impl Dispatch<ZxdgToplevelDecorationV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        decoration: &ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Request::SetMode { mode } = request {
            let mode = mode.into_result().expect("a known decoration mode");
            state.seen.send(Seen::DecorationMode(mode)).unwrap();
            decoration.configure(mode);
        }
    }
}

impl Dispatch<CupertinoAppEffectsV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &CupertinoAppEffectsV1,
        request: cupertino_app_effects_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_app_effects_v1::Request::GetBackdrop { id, surface } => {
                let state = Arc::new(Mutex::new(BackdropState::default()));
                let backdrop = init.init(id, state.clone());
                // the face behind every backdrop is light, from version 7, which tells it
                if backdrop.version() >= 7 {
                    backdrop.tone(cupertino_backdrop_v1::Tone::Light);
                }
                surface
                    .data::<SurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .backdrops
                    .push(state);
            }
            cupertino_app_effects_v1::Request::SetTitlebarStyle { style, .. } => state
                .seen
                .send(Seen::TitlebarStyle(style.into_result().unwrap()))
                .unwrap(),
            cupertino_app_effects_v1::Request::SetDisabledControls { disabled, .. } => state
                .seen
                .send(Seen::DisabledControls(disabled.into_result().unwrap()))
                .unwrap(),
            cupertino_app_effects_v1::Request::GetAppearance { id } => {
                let observer = init.init(id, ());
                // the session starts dark and with reduce motion
                observer.appearance(1);
                observer.display_options(DisplayOption::ReduceMotion);
                if observer.version() >= 5 {
                    observer.session_lock(0);
                }
                state.appearance = Some(observer);
            }
            _ => {}
        }
    }
}
impl Dispatch<CupertinoBackdropV1, Arc<Mutex<BackdropState>>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &CupertinoBackdropV1,
        request: cupertino_backdrop_v1::Request,
        data: &Arc<Mutex<BackdropState>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut b = data.lock().unwrap();
        match request {
            cupertino_backdrop_v1::Request::SetMaterial { name } => b.material = name,
            cupertino_backdrop_v1::Request::SetShape {
                x,
                y,
                width,
                height,
                radius,
            } => {
                b.shape = [
                    f64::from(x),
                    f64::from(y),
                    f64::from(width),
                    f64::from(height),
                    radius,
                ]
            }
            cupertino_backdrop_v1::Request::SetAlpha { alpha } => b.alpha = alpha,
            cupertino_backdrop_v1::Request::SetGlass {
                style,
                container,
                spacing,
            } => b.glass = Some((u32::from(style), container, spacing)),
            _ => {}
        }
    }
}

/// Run the server, with the app effects global at `effects_version`, on a thread until the test drops the control
/// channel.
fn spawn_server(
    stream: UnixStream,
    effects_version: u32,
) -> (Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
    let (control_tx, control_rx) = channel::<Control>();
    let (committed_tx, committed_rx) = channel();
    let (seen_tx, seen_rx) = channel();
    std::thread::spawn(move || {
        let mut display = Display::<Mock>::new().unwrap();
        let dh = display.handle();
        dh.create_global::<Mock, WlCompositor, _>(6, ());
        dh.create_global::<Mock, WlShm, _>(1, ());
        dh.create_global::<Mock, WlOutput, _>(4, ());
        dh.create_global::<Mock, WlSeat, _>(7, ());
        dh.create_global::<Mock, XdgWmBase, _>(6, ());
        dh.create_global::<Mock, ZxdgDecorationManagerV1, _>(1, ());
        dh.create_global::<Mock, CupertinoAppEffectsV1, _>(effects_version, ());
        dh.create_global::<Mock, CupertinoScrollV1, _>(1, ());
        let mut mock = Mock {
            committed: committed_tx,
            seen: seen_tx,
            next_serial: 1,
            appearance: None,
            outputs: Vec::new(),
            pointer: None,
            scroll: None,
            shown: None,
            toplevel: None,
        };
        display
            .handle()
            .insert_client(stream, Arc::new(Data))
            .unwrap();
        loop {
            match control_rx.try_recv() {
                Ok(Control::AppearanceChange) => {
                    let observer = mock.appearance.as_ref().unwrap();
                    observer.appearance(0);
                    // as the server does, the scroll bar bit only to an observer that knows it
                    let mut options =
                        DisplayOption::ReduceTransparency | DisplayOption::IncreaseContrast;
                    if observer.version() >= 8 {
                        options |= DisplayOption::AlwaysShowScrollBars;
                    }
                    if observer.version() >= 9 {
                        options |= DisplayOption::SteadyCaret;
                    }
                    observer.display_options(options);
                }
                Ok(Control::Lock) => {
                    let observer = mock.appearance.as_ref().unwrap();
                    if observer.version() >= 5 {
                        observer.session_lock(1);
                    }
                }
                Ok(Control::ResizeToplevel) => {
                    let t = mock.toplevel.as_ref().unwrap();
                    mock.next_serial += 1;
                    let activated = (xdg_toplevel::State::Activated as u32)
                        .to_ne_bytes()
                        .to_vec();
                    t.toplevel.configure(640, 480, activated);
                    t.xdg.configure(mock.next_serial);
                }
                Ok(Control::ScrollToplevel) => {
                    let (pointer, t) = (
                        mock.pointer.as_ref().unwrap(),
                        mock.toplevel.as_ref().unwrap(),
                    );
                    pointer.enter(20, &t.surface, 30.0, 40.0);
                    // the count of a frame goes after its axis event and before the frame
                    let scroll = mock.scroll.as_ref().unwrap();
                    pointer.axis(0, wl_pointer::Axis::VerticalScroll, 15.0);
                    scroll.scroll_count(wl_pointer::Axis::VerticalScroll, 3);
                    pointer.frame();
                    pointer.axis(0, wl_pointer::Axis::VerticalScroll, 5.0);
                    pointer.frame();
                    pointer.axis(0, wl_pointer::Axis::HorizontalScroll, -2.0);
                    scroll.scroll_count(wl_pointer::Axis::HorizontalScroll, 7);
                    pointer.frame();
                }
                Ok(Control::CloseToplevel) => mock.toplevel.as_ref().unwrap().toplevel.close(),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
            display.dispatch_clients(&mut mock).unwrap();
            display.flush_clients().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    (control_tx, committed_rx, seen_rx)
}

fn connect() -> (
    Connection,
    Sender<Control>,
    Receiver<Committed>,
    Receiver<Seen>,
) {
    connect_at(1)
}

/// [`connect`] to a server that offers `cupertino_app_effects_v1` at `effects_version`.
fn connect_at(
    effects_version: u32,
) -> (
    Connection,
    Sender<Control>,
    Receiver<Committed>,
    Receiver<Seen>,
) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, seen) = spawn_server(server_stream, effects_version);
    (
        Connection::from_stream(client_stream).unwrap(),
        control,
        committed,
        seen,
    )
}

/// Events the client reported but the test has not looked at yet.
type Stash = VecDeque<Event>;

/// Take events in order, waiting for more when the stash runs dry, until `pick` accepts one.
fn wait_for<T>(
    conn: &mut Connection,
    stash: &mut Stash,
    mut pick: impl FnMut(&Event) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Some(event) = stash.pop_front() {
            if let Some(found) = pick(&event) {
                return found;
            }
        }
        stash.extend(conn.wait(Some(Duration::from_millis(50))).unwrap());
    }
    panic!("timed out waiting for an event");
}

fn app_event(event: &Event) -> Option<AppEvent> {
    match event {
        Event::Extension(e) => e.downcast_ref::<AppEvent>().cloned(),
        _ => None,
    }
}

/// A server-decorated window, as the applications open theirs.
fn window(conn: &mut Connection) -> Toplevel {
    conn.toplevel(&ToplevelConfig {
        app_id: "org.example.files".into(),
        title: "Home".into(),
        min_size: Some((445, 139)),
        max_size: None,
        decorations: Decorations::Server,
        foreign_parent: None,
    })
    .unwrap()
}

fn configured(conn: &mut Connection, stash: &mut Stash, window: Toplevel) -> WindowConfigure {
    wait_for(conn, stash, |e| match e {
        Event::Configure {
            window: w,
            configure,
        } if *w == window => Some(*configure),
        _ => None,
    })
}

/// Present a frame of `surface`, sending it with a wait.
fn present(conn: &mut Connection, stash: &mut Stash, surface: SurfaceId, preferred: (u32, u32)) {
    conn.lease(surface, preferred, BufferFormat::Argb8888)
        .unwrap()
        .present()
        .unwrap();
    stash.extend(conn.wait(Some(Duration::from_millis(50))).unwrap());
}

/// From version 8 each display options is followed by whether scroll bars stay shown, and from version 9 by whether
/// text carets keep from blinking; an older observer is never sent the bits and the views keep their own ways.
#[test]
fn the_appearance_and_display_options_arrive_at_once_and_on_every_change() {
    for version in [7, 8, 9] {
        let (mut conn, control, _committed, _seen) = connect_at(version);
        let effects = AppEffects::bind(&mut conn).unwrap();
        let _appearance = effects.appearance(&conn);
        conn.roundtrip().unwrap();
        let mut stash = Stash::new();
        let mut changes = Vec::new();
        let told = version >= 8;
        let steady_told = version >= 9;
        let (first, all) = match version {
            9 => (4, 8),
            8 => (3, 6),
            _ => (2, 4),
        };
        wait_for(&mut conn, &mut stash, |e| {
            // the lock state is another test's
            changes.extend(app_event(e).filter(|e| !matches!(e, AppEvent::SessionLock { .. })));
            (changes.len() == first).then_some(())
        });
        control.send(Control::AppearanceChange).unwrap();
        wait_for(&mut conn, &mut stash, |e| {
            // the lock state is another test's
            changes.extend(app_event(e).filter(|e| !matches!(e, AppEvent::SessionLock { .. })));
            (changes.len() == all).then_some(())
        });
        let mut want = vec![
            AppEvent::Appearance { dark: true },
            AppEvent::DisplayOptions(DisplayOptions {
                reduce_motion: true,
                ..DisplayOptions::default()
            }),
            AppEvent::ScrollBars { always: false },
            AppEvent::SteadyCaret { on: false },
            AppEvent::Appearance { dark: false },
            AppEvent::DisplayOptions(DisplayOptions {
                reduce_transparency: true,
                increase_contrast: true,
                ..DisplayOptions::default()
            }),
            AppEvent::ScrollBars { always: true },
            AppEvent::SteadyCaret { on: true },
        ];
        if !told {
            want.retain(|e| !matches!(e, AppEvent::ScrollBars { .. }));
        }
        if !steady_told {
            want.retain(|e| !matches!(e, AppEvent::SteadyCaret { .. }));
        }
        assert_eq!(changes, want, "version {version}");
    }
}

#[test]
fn a_version_5_observer_is_told_the_lock_state_after_the_appearance_and_on_every_change() {
    for (version, told) in [(4, false), (5, true)] {
        let (mut conn, control, _committed, _seen) = connect_at(version);
        let effects = AppEffects::bind(&mut conn).unwrap();
        assert_eq!(effects.tells_session_lock(), told);
        let _appearance = effects.appearance(&conn);
        conn.roundtrip().unwrap();
        control.send(Control::Lock).unwrap();
        control.send(Control::AppearanceChange).unwrap();
        let mut stash = Stash::new();
        let mut changes = Vec::new();
        wait_for(&mut conn, &mut stash, |e| {
            changes.extend(app_event(e));
            matches!(changes.last(), Some(AppEvent::DisplayOptions(_)) if changes.len() > 2)
                .then_some(())
        });
        let locks: Vec<_> = changes
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, AppEvent::SessionLock { .. }))
            .collect();
        if told {
            assert_eq!(
                locks,
                // after the appearance and the display options (no scroll bars below version 8)
                [
                    (2, &AppEvent::SessionLock { locked: false }),
                    (3, &AppEvent::SessionLock { locked: true })
                ]
            );
        } else {
            assert_eq!(locks, []);
        }
    }
}

#[test]
fn a_backdrop_arrives_with_the_next_commit_of_its_surface() {
    let (mut conn, _control, committed, _seen) = connect();
    let effects = AppEffects::bind(&mut conn).unwrap();
    let window = window(&mut conn);
    let mut stash = Stash::new();
    configured(&mut conn, &mut stash, window);
    let backdrop = effects.backdrop(&conn, window.surface()).unwrap();
    backdrop.set_material("Menu");
    backdrop
        .set_shape(&Rect::new(4.0, 6.0, 40.0, 20.0), 12.0)
        .unwrap();
    backdrop.set_alpha(0.5).unwrap();
    present(&mut conn, &mut stash, window.surface(), (920, 436));
    assert_eq!(
        committed.recv_timeout(Duration::from_secs(5)).unwrap(),
        Committed {
            buffer_size: (1840, 872),
            backdrops: vec![BackdropState {
                material: "Menu".to_string(),
                shape: [4.0, 6.0, 40.0, 20.0, 12.0],
                alpha: 0.5,
                glass: None,
            }],
        }
    );
}

#[test]
fn glass_backdrops_name_their_container_and_fall_back_to_a_material_on_an_older_server() {
    for (version, expect) in [
        (6, (String::new(), Some((1, 3, 8.0)))),
        (5, ("Menu".to_string(), None)),
    ] {
        let (mut conn, _control, committed, _seen) = connect_at(version);
        let effects = AppEffects::bind(&mut conn).unwrap();
        let window = window(&mut conn);
        let mut stash = Stash::new();
        configured(&mut conn, &mut stash, window);
        let backdrop = effects.backdrop(&conn, window.surface()).unwrap();
        backdrop.set_glass(GlassStyle::Clear, 3, 8.0).unwrap();
        assert!(matches!(
            backdrop.set_glass(GlassStyle::Regular, 3, -1.0),
            Err(cupertino_protocol_app::Error::InvalidGlass)
        ));
        backdrop
            .set_shape(&Rect::new(4.0, 6.0, 40.0, 20.0), 10.0)
            .unwrap();
        present(&mut conn, &mut stash, window.surface(), (920, 436));
        assert_eq!(
            committed.recv_timeout(Duration::from_secs(5)).unwrap(),
            Committed {
                buffer_size: (1840, 872),
                backdrops: vec![BackdropState {
                    material: expect.0,
                    shape: [4.0, 6.0, 40.0, 20.0, 10.0],
                    alpha: 0.0,
                    glass: expect.1,
                }],
            },
            "server version {version}"
        );
    }
}

#[test]
fn a_version_7_server_tells_each_backdrop_its_tone_and_an_older_one_tells_none() {
    for version in [7, 6] {
        let (mut conn, _control, committed, _seen) = connect_at(version);
        let effects = AppEffects::bind(&mut conn).unwrap();
        let window = window(&mut conn);
        let mut stash = Stash::new();
        configured(&mut conn, &mut stash, window);
        let first = effects.backdrop(&conn, window.surface()).unwrap();
        let second = effects.backdrop(&conn, window.surface()).unwrap();
        assert_ne!(first.id(), second.id());
        assert_eq!(first.tells_tone(), version >= 7);
        for b in [&first, &second] {
            b.set_glass(GlassStyle::Regular, 0, 0.0).unwrap();
            b.set_shape(&Rect::new(4.0, 6.0, 40.0, 20.0), 10.0).unwrap();
        }
        present(&mut conn, &mut stash, window.surface(), (920, 436));
        committed.recv_timeout(Duration::from_secs(5)).unwrap();
        // a few more round trips, so that an event the server sent would have arrived
        for _ in 0..3 {
            stash.extend(conn.wait(Some(Duration::from_millis(30))).unwrap());
        }
        let tones: Vec<(BackdropId, BackdropTone)> = stash
            .iter()
            .filter_map(app_event)
            .filter_map(|e| match e {
                AppEvent::BackdropTone { backdrop, tone } => Some((backdrop, tone)),
                _ => None,
            })
            .collect();
        if version >= 7 {
            assert_eq!(
                tones,
                vec![
                    (first.id(), BackdropTone::Light),
                    (second.id(), BackdropTone::Light)
                ]
            );
        } else {
            assert!(tones.is_empty(), "{tones:?}");
        }
    }
}

#[test]
fn a_unified_toplevel_asks_for_its_title_bar_style_and_moves_and_zooms_itself() {
    let (mut conn, _control, _committed, seen) = connect();
    let effects = AppEffects::bind(&mut conn).unwrap();
    let window = window(&mut conn);
    effects.unify_titlebar(&conn, window.surface()).unwrap();
    let mut stash = Stash::new();
    configured(&mut conn, &mut stash, window);
    let requests: Vec<Seen> = (0..5)
        .map(|_| seen.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert_eq!(
        requests[4],
        Seen::TitlebarStyle(cupertino_app_effects_v1::TitlebarStyle::Unified)
    );

    // the window zooms itself; a move needs the press it answers
    conn.set_maximized(window, true).unwrap();
    stash.extend(conn.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Maximize(true)
    );
    assert!(matches!(
        conn.start_move(window),
        Err(Error::PermissionDenied(_))
    ));
}

#[test]
fn disabled_controls_reach_a_version_4_server_and_are_not_sent_below() {
    for (version, sent) in [(4, true), (3, false)] {
        let (mut conn, _control, _committed, seen) = connect_at(version);
        let effects = AppEffects::bind(&mut conn).unwrap();
        assert_eq!(effects.can_set_disabled_controls(), sent);
        let window = window(&mut conn);
        effects
            .set_disabled_controls(
                &conn,
                window.surface(),
                WindowControls {
                    minimize: true,
                    zoom: false,
                },
            )
            .unwrap();
        // a request after it, so the server's order shows whether it went out
        effects.unify_titlebar(&conn, window.surface()).unwrap();
        conn.roundtrip().unwrap();
        let unified = Seen::TitlebarStyle(cupertino_app_effects_v1::TitlebarStyle::Unified);
        let mut requests = Vec::new();
        while requests.last() != Some(&unified) {
            requests.push(seen.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        assert_eq!(
            requests.contains(&Seen::DisabledControls(
                cupertino_app_effects_v1::WindowControl::Minimize
            )),
            sent,
            "version {version}: {requests:?}"
        );
    }
}

#[test]
fn toplevels_are_decorated_by_the_server_sized_scrolled_and_closed() {
    let (mut conn, control, committed, seen) = connect();
    let scroll_counts = ScrollCounts::bind(&mut conn).unwrap();
    let window = window(&mut conn);
    let mut stash = Stash::new();

    // the first configure suggests no size: the window takes the requested one, inactive
    let configure = configured(&mut conn, &mut stash, window);
    assert_eq!(configure.size, None);
    assert_eq!(configure.decorations, Decorations::Server);
    assert!(!configure.states.activated);
    let requests: Vec<Seen> = (0..4)
        .map(|_| seen.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert_eq!(
        requests,
        [
            Seen::AppId("org.example.files".into()),
            Seen::Title("Home".into()),
            Seen::MinSize(445, 139),
            Seen::DecorationMode(zxdg_toplevel_decoration_v1::Mode::ServerSide)
        ]
    );
    present(&mut conn, &mut stash, window.surface(), (920, 436));
    // the mock's output has scale 2
    assert_eq!(
        committed
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .buffer_size,
        (1840, 872)
    );
    conn.set_title(window, "Documents").unwrap();
    stash.extend(conn.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Title("Documents".into())
    );

    // a later configure resizes and activates
    control.send(Control::ResizeToplevel).unwrap();
    let configure = configured(&mut conn, &mut stash, window);
    assert_eq!(configure.size, Some((640, 480)));
    assert!(configure.states.activated);
    present(&mut conn, &mut stash, window.surface(), (920, 436));
    assert_eq!(
        committed
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .buffer_size,
        (1280, 960)
    );

    // scrolling over it arrives as one scroll per pointer frame, each after the count of its frame
    let _pointer_scroll = scroll_counts.watch(&conn, &conn.pointer().unwrap());
    conn.roundtrip().unwrap();
    control.send(Control::ScrollToplevel).unwrap();
    let mut scrolls = Vec::new();
    wait_for(&mut conn, &mut stash, |e| {
        match e {
            Event::Pointer {
                surface,
                event: PointerEvent::Scroll { dx, dy, .. },
                ..
            } if *surface == window.surface() => scrolls.push(format!("scroll {dx} {dy}")),
            e => {
                if let Some(AppEvent::ScrollCount { vertical, count }) = app_event(e) {
                    scrolls.push(format!("count vertical={vertical} {count}"))
                }
            }
        }
        (scrolls.len() == 5).then_some(())
    });
    assert_eq!(
        scrolls,
        [
            "count vertical=true 3",
            "scroll 0 15",
            "scroll 0 5",
            "count vertical=false 7",
            "scroll -2 0"
        ]
    );

    // the close request is the application's to act on
    control.send(Control::CloseToplevel).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        (*e == Event::Close(window)).then_some(())
    });
    conn.destroy(window.surface()).unwrap();
}

/// From version 11 the backdrops of a subsurface draw, above the rest of its window; an older binding says they don't,
/// so a toolkit keeps its bar's glass on the window's own surface there.
#[test]
fn subsurface_backdrops_draw_from_version_11() {
    for (version, draws) in [(9, false), (10, false), (11, true)] {
        let (mut conn, _control, _committed, _seen) = connect_at(version);
        let effects = AppEffects::bind(&mut conn).unwrap();
        assert_eq!(
            effects.draws_subsurface_backdrops(),
            draws,
            "version {version}"
        );
    }
}
