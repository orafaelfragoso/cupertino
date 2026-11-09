//! Runs the shell roles on the transport against a minimal in-process server (shm, an output at scale 2, a seat with a
//! pointer, the layer shell, popups, activation, the session lock, idle notification and `cupertino_shell_v1`) and
//! checks what the server receives and what the client reports back.

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_geometry::{Point, Rect};
use cupertino_protocols::server::cupertino_dock_v1::{
    self, CupertinoDockV1, WindowState as DockWindowState,
};
use cupertino_protocols::server::cupertino_menu_bar_v1::{self, CupertinoMenuBarV1};
use cupertino_protocols::server::cupertino_session_v1::{CupertinoSessionV1, DisplayOption};
use cupertino_protocols::server::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_shell_client::{
    ActiveApp, Anchor, DockChange, DockWindow, EditAction, IdleNotifier, KeyboardInteractivity,
    Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin, MenuBarUpdate, SessionChange,
    SessionLockManager, SessionState, Shell, ShellEvent, WindowCommand, WindowMenu, WindowState,
};
use cupertino_wayland_client::{
    Anchor as PopupAnchor, BufferFormat, Button, Connection, ConstraintAdjustment, Event, Gravity,
    PointerEvent, Popup, PopupPlacement, SurfaceId,
};
use std::collections::VecDeque;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_protocols::ext::idle_notify::v1::server::ext_idle_notification_v1::ExtIdleNotificationV1;
use wayland_protocols::ext::idle_notify::v1::server::ext_idle_notifier_v1::{
    self, ExtIdleNotifierV1,
};
use wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::{
    self, ExtSessionLockManagerV1,
};
use wayland_protocols::ext::session_lock::v1::server::ext_session_lock_surface_v1::{
    self, ExtSessionLockSurfaceV1,
};
use wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::{
    self, ExtSessionLockV1,
};
use wayland_protocols::xdg::activation::v1::server::xdg_activation_token_v1::{
    self, XdgActivationTokenV1,
};
use wayland_protocols::xdg::activation::v1::server::xdg_activation_v1::{self, XdgActivationV1};
use wayland_protocols::xdg::shell::server::xdg_popup::{self, XdgPopup};
use wayland_protocols::xdg::shell::server::xdg_positioner::{self, XdgPositioner};
use wayland_protocols::xdg::shell::server::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};
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
    buffer_scale: i32,
}

enum Control {
    /// Enter the layer surface with the pointer, move, and press the left button.
    PointerPress,
    /// Ping the client through `xdg_wm_base`.
    Ping(u32),
    /// Dismiss the most recent popup.
    DismissPopup,
    /// Send the menu bar the engage shortcut, then a capture start in a group of its own.
    Engage,
    /// Tell the dock a window closed, a program is launching, the app-grid action fired and a switch opened and was cancelled.
    DockChange,
    /// Switch the session to the light appearance, lock it, unlock it, and ask for a lock.
    SessionChange,
}

/// A request the server saw that the commit record does not cover.
#[derive(Debug, PartialEq)]
enum Seen {
    /// A popup was created: the geometry of its positioner, and whether it hangs off the layer
    /// surface or off another popup.
    Popup {
        positioner: PositionerState,
        layer_parent: bool,
        grab_serial: u32,
    },
    Reposition {
        positioner: PositionerState,
        token: u32,
    },
    PopupDestroyed,
    Pong(u32),
    /// The menu-bar role was requested for the surface on the output.
    MenuBar {
        surface_has_layer_role: bool,
    },
    MenuOpen(bool),
    Command(cupertino_menu_bar_v1::Command),
    Workspace(u32),
    /// The dock role was requested for the surface on the output.
    Dock {
        surface_has_layer_role: bool,
    },
    Activate(String),
    Restore {
        id: u32,
        rect: [i32; 4],
    },
    MinimizeTarget([i32; 4]),
    SwitcherSelect(u32),
    SwitcherActivate(u32),
    SwitcherCancel,
    LockRequested,
    /// The client acknowledged the configure of a lock surface.
    LockSurfaceAcked(u32),
    LockSurfaceDestroyed,
    Unlocked,
    /// An idle notification with this timeout in milliseconds was created.
    IdleWatch(u32),
}

/// What a positioner was told; the popup is placed from it.
#[derive(Clone, Debug, Default, PartialEq)]
struct PositionerState {
    size: (i32, i32),
    anchor_rect: [i32; 4],
    anchor: Option<xdg_positioner::Anchor>,
    gravity: Option<xdg_positioner::Gravity>,
}

#[derive(Default)]
struct SurfaceState {
    buffer: Option<WlBuffer>,
    scale: i32,
    layer: Option<ZwlrLayerSurfaceV1>,
    popup: Option<PopupState>,
}

struct PopupState {
    xdg: XdgSurface,
    popup: XdgPopup,
    positioner: PositionerState,
    /// The popup has been configured once.
    mapped: bool,
    /// Set by `get_popup` on the layer surface.
    layer_parent: bool,
}

type SurfaceData = Arc<Mutex<SurfaceState>>;

struct Mock {
    committed: Sender<Committed>,
    seen: Sender<Seen>,
    next_serial: u32,
    popups: Vec<XdgPopup>,
    wm_base: Option<XdgWmBase>,
    menu_bar: Option<CupertinoMenuBarV1>,
    dock: Option<CupertinoDockV1>,
    session: Option<CupertinoSessionV1>,
    outputs: Vec<WlOutput>,
    pointer: Option<WlPointer>,
    surface: Option<WlSurface>,
    shown: Option<WlBuffer>,
    idle: Option<ExtIdleNotificationV1>,
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
    ZwlrLayerShellV1,
    CupertinoShellV1,
    XdgActivationV1,
    ExtSessionLockManagerV1,
    ExtIdleNotifierV1
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
    CupertinoSessionV1: (),
    ExtIdleNotificationV1: ()
);

impl Dispatch<WlCompositor, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_compositor::Request::CreateSurface { id } = request {
            let surface = init.init(id, SurfaceData::default());
            // the pointer targets the first surface, the layer surface
            state.surface.get_or_insert(surface);
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
            wl_surface::Request::SetBufferScale { scale } => s.scale = scale,
            wl_surface::Request::Frame { callback } => {
                init.init(callback, ());
            }
            wl_surface::Request::Commit => {
                let unbuffered = s.buffer.is_none();
                if let Some(layer) = s.layer.clone().filter(|_| unbuffered) {
                    // initial commit of a layer surface: place it on the output and size it
                    surface.enter(&state.outputs[0]);
                    layer.configure(1, 100, 50);
                }
                if let Some(popup) = s.popup.as_mut().filter(|p| !p.mapped && unbuffered) {
                    popup.mapped = true;
                    surface.enter(&state.outputs[0]);
                    let [x, y, ..] = popup.positioner.anchor_rect;
                    state.next_serial += 1;
                    popup
                        .popup
                        .configure(x, y, popup.positioner.size.0, popup.positioner.size.1);
                    popup.xdg.configure(state.next_serial);
                }
                if let Some(buffer) = s.buffer.take() {
                    state
                        .committed
                        .send(Committed {
                            buffer_size: *buffer.data::<(i32, i32)>().expect("buffer data"),
                            buffer_scale: s.scale,
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

impl Dispatch<ZwlrLayerShellV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_layer_shell_v1::Request::GetLayerSurface {
            id,
            surface,
            output,
            namespace,
            layer,
        } = request
        {
            assert_eq!(
                (
                    namespace.as_str(),
                    layer.into_result().ok(),
                    output.is_some()
                ),
                (
                    "test-shell",
                    Some(zwlr_layer_shell_v1::Layer::Overlay),
                    true
                )
            );
            let layer_surface = init.init(id, surface.clone());
            surface.data::<SurfaceData>().unwrap().lock().unwrap().layer = Some(layer_surface);
        }
    }
}
impl Dispatch<ZwlrLayerSurfaceV1, WlSurface> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        _: &WlSurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_layer_surface_v1::Request::GetPopup { popup } => {
                let surface = popup.data::<WlSurface>().expect("popup data");
                surface
                    .data::<SurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .popup
                    .as_mut()
                    .expect("popup state")
                    .layer_parent = true;
            }
            zwlr_layer_surface_v1::Request::SetSize { width, height } => {
                assert_eq!((width, height), (100, 0))
            }
            zwlr_layer_surface_v1::Request::SetExclusiveZone { zone } => assert_eq!(zone, -1),
            zwlr_layer_surface_v1::Request::AckConfigure { serial } => assert_eq!(serial, 1),
            _ => {}
        }
    }
}

impl GlobalDispatch<XdgWmBase, ()> for Mock {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<XdgWmBase>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        state.wm_base = Some(init.init(r, ()));
    }
}
impl Dispatch<XdgWmBase, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &XdgWmBase,
        request: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_wm_base::Request::CreatePositioner { id } => {
                init.init(id, Arc::new(Mutex::new(PositionerState::default())));
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                init.init(id, surface);
            }
            xdg_wm_base::Request::Pong { serial } => state.seen.send(Seen::Pong(serial)).unwrap(),
            _ => {}
        }
    }
}
impl Dispatch<XdgPositioner, Arc<Mutex<PositionerState>>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &XdgPositioner,
        request: xdg_positioner::Request,
        data: &Arc<Mutex<PositionerState>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut p = data.lock().unwrap();
        match request {
            xdg_positioner::Request::SetSize { width, height } => p.size = (width, height),
            xdg_positioner::Request::SetAnchorRect {
                x,
                y,
                width,
                height,
            } => p.anchor_rect = [x, y, width, height],
            xdg_positioner::Request::SetAnchor { anchor } => p.anchor = anchor.into_result().ok(),
            xdg_positioner::Request::SetGravity { gravity } => {
                p.gravity = gravity.into_result().ok()
            }
            _ => {}
        }
    }
}
impl Dispatch<XdgSurface, WlSurface> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        xdg: &XdgSurface,
        request: xdg_surface::Request,
        surface: &WlSurface,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_surface::Request::GetPopup { id, positioner, .. } = request {
            let popup = init.init(id, surface.clone());
            let positioner = positioner
                .data::<Arc<Mutex<PositionerState>>>()
                .expect("positioner data")
                .lock()
                .unwrap()
                .clone();
            surface.data::<SurfaceData>().unwrap().lock().unwrap().popup = Some(PopupState {
                xdg: xdg.clone(),
                popup,
                positioner,
                mapped: false,
                layer_parent: false,
            });
        }
    }
}
impl Dispatch<XdgPopup, WlSurface> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        popup: &XdgPopup,
        request: xdg_popup::Request,
        surface: &WlSurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let data = surface.data::<SurfaceData>().unwrap();
        match request {
            xdg_popup::Request::Grab { serial, .. } => {
                // the layer parent is set by a request that follows get_popup; report both at the grab
                let s = data.lock().unwrap();
                let p = s.popup.as_ref().expect("popup state");
                state
                    .seen
                    .send(Seen::Popup {
                        positioner: p.positioner.clone(),
                        layer_parent: p.layer_parent,
                        grab_serial: serial,
                    })
                    .unwrap();
                state.popups.push(popup.clone());
            }
            xdg_popup::Request::Reposition { positioner, token } => {
                let positioner = positioner
                    .data::<Arc<Mutex<PositionerState>>>()
                    .expect("positioner data")
                    .lock()
                    .unwrap()
                    .clone();
                let mut s = data.lock().unwrap();
                let p = s.popup.as_mut().expect("popup state");
                p.positioner = positioner.clone();
                state
                    .seen
                    .send(Seen::Reposition {
                        positioner: positioner.clone(),
                        token,
                    })
                    .unwrap();
                state.next_serial += 1;
                p.popup.repositioned(token);
                let [x, y, ..] = positioner.anchor_rect;
                p.popup
                    .configure(x, y, positioner.size.0, positioner.size.1);
                p.xdg.configure(state.next_serial);
            }
            xdg_popup::Request::Destroy => {
                state.popups.retain(|p| p != popup);
                state.seen.send(Seen::PopupDestroyed).unwrap();
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgActivationV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &XdgActivationV1,
        request: xdg_activation_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_activation_v1::Request::GetActivationToken { id } = request {
            init.init(id, Arc::new(Mutex::new(None::<u32>)));
        }
    }
}
impl Dispatch<XdgActivationTokenV1, Arc<Mutex<Option<u32>>>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        token: &XdgActivationTokenV1,
        request: xdg_activation_token_v1::Request,
        serial: &Arc<Mutex<Option<u32>>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_activation_token_v1::Request::SetSerial { serial: s, .. } => {
                *serial.lock().unwrap() = Some(s)
            }
            xdg_activation_token_v1::Request::Commit => token.done(format!(
                "token-for-serial-{}",
                serial.lock().unwrap().expect("serial set")
            )),
            _ => {}
        }
    }
}

fn has_layer_role(surface: &WlSurface) -> bool {
    surface
        .data::<SurfaceData>()
        .unwrap()
        .lock()
        .unwrap()
        .layer
        .is_some()
}

impl Dispatch<CupertinoShellV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &CupertinoShellV1,
        request: cupertino_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_shell_v1::Request::GetMenuBar { id, surface, .. } => {
                let bar = init.init(id, ());
                state
                    .seen
                    .send(Seen::MenuBar {
                        surface_has_layer_role: has_layer_role(&surface),
                    })
                    .unwrap();
                // the first group follows the object's creation
                bar.active_app("org.example.editor".into(), "notes.txt".into());
                bar.menu_dbusmenu(":1.42".into(), "/MenuBar".into());
                bar.window_state(1, 2, 4);
                if bar.version() >= 15 {
                    bar.window_controls(cupertino_menu_bar_v1::WindowControl::Zoom);
                }
                bar.done();
                if bar.version() >= 14 {
                    bar.backdrop_luminance(620);
                    bar.done();
                }
                if bar.version() >= 21 {
                    bar.backdrop_spread(
                        [0_u16, 300, 45]
                            .iter()
                            .flat_map(|b| b.to_le_bytes())
                            .collect(),
                    );
                    bar.done();
                }
                state.menu_bar = Some(bar);
            }
            cupertino_shell_v1::Request::GetDock { id, surface, .. } => {
                let dock = init.init(id, ());
                state
                    .seen
                    .send(Seen::Dock {
                        surface_has_layer_role: has_layer_role(&surface),
                    })
                    .unwrap();
                // the first group lists every window
                dock.window(
                    1,
                    "org.example.editor".into(),
                    "notes.txt".into(),
                    DockWindowState::Active,
                );
                dock.window(
                    2,
                    "org.example.mail".into(),
                    "Inbox".into(),
                    DockWindowState::Minimized,
                );
                dock.done();
                state.dock = Some(dock);
            }
            cupertino_shell_v1::Request::GetSession { id } => {
                let session = init.init(id, ());
                // the session starts dark, with reduce motion, and unlocked
                session.appearance(1);
                session.display_options(DisplayOption::ReduceMotion);
                session.unlocked();
                state.session = Some(session);
            }
            _ => {}
        }
    }
}
impl Dispatch<CupertinoMenuBarV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &CupertinoMenuBarV1,
        request: cupertino_menu_bar_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            cupertino_menu_bar_v1::Request::SetMenuOpen { open } => Seen::MenuOpen(open != 0),
            cupertino_menu_bar_v1::Request::WindowCommand { command } => {
                Seen::Command(command.into_result().unwrap())
            }
            cupertino_menu_bar_v1::Request::MoveToWorkspace { workspace } => {
                Seen::Workspace(workspace)
            }
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}
impl Dispatch<CupertinoDockV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &CupertinoDockV1,
        request: cupertino_dock_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            cupertino_dock_v1::Request::ActivateApp { app_id } => Seen::Activate(app_id),
            cupertino_dock_v1::Request::RestoreWindow {
                id,
                x,
                y,
                width,
                height,
            } => Seen::Restore {
                id,
                rect: [x, y, width, height],
            },
            cupertino_dock_v1::Request::SetMinimizeTarget {
                x,
                y,
                width,
                height,
            } => Seen::MinimizeTarget([x, y, width, height]),
            cupertino_dock_v1::Request::SwitcherSelect { index } => Seen::SwitcherSelect(index),
            cupertino_dock_v1::Request::SwitcherActivate { index } => Seen::SwitcherActivate(index),
            cupertino_dock_v1::Request::SwitcherCancel => Seen::SwitcherCancel,
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ExtSessionLockManagerV1,
        request: ext_session_lock_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_session_lock_manager_v1::Request::Lock { id } = request {
            init.init(id, ());
            state.seen.send(Seen::LockRequested).unwrap();
        }
    }
}
impl Dispatch<ExtSessionLockV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        lock: &ExtSessionLockV1,
        request: ext_session_lock_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_session_lock_v1::Request::GetLockSurface {
                id,
                surface,
                output,
            } => {
                assert_eq!(output, state.outputs[0]);
                let lock_surface = init.init(id, surface);
                lock_surface.configure(7, 640, 480);
                lock.locked();
            }
            ext_session_lock_v1::Request::UnlockAndDestroy => {
                state.seen.send(Seen::Unlocked).unwrap()
            }
            _ => {}
        }
    }
}
impl Dispatch<ExtSessionLockSurfaceV1, WlSurface> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ExtSessionLockSurfaceV1,
        request: ext_session_lock_surface_v1::Request,
        _: &WlSurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_session_lock_surface_v1::Request::AckConfigure { serial } => {
                state.seen.send(Seen::LockSurfaceAcked(serial)).unwrap()
            }
            ext_session_lock_surface_v1::Request::Destroy => {
                state.seen.send(Seen::LockSurfaceDestroyed).unwrap()
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ExtIdleNotifierV1,
        request: ext_idle_notifier_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_idle_notifier_v1::Request::GetIdleNotification { id, timeout, .. } = request {
            let notification = init.init(id, ());
            notification.idled();
            state.idle = Some(notification);
            state.seen.send(Seen::IdleWatch(timeout)).unwrap();
        }
    }
}

/// Run the server on a thread until the test drops the control channel; `layer_shell` is whether it offers
/// `zwlr_layer_shell_v1`, `shell_version` the version of `cupertino_shell_v1` it offers.
fn spawn_server(
    stream: UnixStream,
    layer_shell: bool,
    shell_version: u32,
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
        if layer_shell {
            dh.create_global::<Mock, ZwlrLayerShellV1, _>(4, ());
        }
        dh.create_global::<Mock, CupertinoShellV1, _>(shell_version, ());
        dh.create_global::<Mock, XdgActivationV1, _>(1, ());
        dh.create_global::<Mock, XdgWmBase, _>(6, ());
        dh.create_global::<Mock, ExtSessionLockManagerV1, _>(1, ());
        dh.create_global::<Mock, ExtIdleNotifierV1, _>(2, ());
        let mut mock = Mock {
            committed: committed_tx,
            seen: seen_tx,
            next_serial: 1,
            popups: Vec::new(),
            wm_base: None,
            menu_bar: None,
            dock: None,
            session: None,
            outputs: Vec::new(),
            pointer: None,
            surface: None,
            shown: None,
            idle: None,
        };
        display
            .handle()
            .insert_client(stream, Arc::new(Data))
            .unwrap();
        loop {
            match control_rx.try_recv() {
                Ok(Control::PointerPress) => {
                    let (pointer, surface) = (
                        mock.pointer.as_ref().unwrap(),
                        mock.surface.as_ref().unwrap(),
                    );
                    pointer.enter(5, surface, 1.5, 2.5);
                    pointer.motion(0, 10.25, 20.75);
                    pointer.button(6, 0, 0x110, wl_pointer::ButtonState::Pressed);
                }
                Ok(Control::Ping(serial)) => mock.wm_base.as_ref().unwrap().ping(serial),
                Ok(Control::DismissPopup) => mock.popups.last().unwrap().popup_done(),
                Ok(Control::Engage) => {
                    let bar = mock.menu_bar.as_ref().unwrap();
                    bar.engage();
                    bar.done();
                    bar.screen_capture(1);
                    bar.done();
                }
                Ok(Control::DockChange) => {
                    let dock = mock.dock.as_ref().unwrap();
                    dock.window_closed(2);
                    dock.launching("foot".into());
                    dock.toggle_app_grid();
                    dock.switcher_output(&mock.outputs[0]);
                    dock.switcher_entry("org.example.mail".into());
                    dock.switcher_entry("org.example.editor".into());
                    dock.switcher_changed(1);
                    dock.switcher_closed(0);
                    dock.attention("org.example.mail".into());
                    dock.done();
                }
                Ok(Control::SessionChange) => {
                    let session = mock.session.as_ref().unwrap();
                    session.appearance(0);
                    session.display_options(
                        DisplayOption::ReduceTransparency | DisplayOption::IncreaseContrast,
                    );
                    session.locked();
                    session.unlocked();
                    session.lock_requested(1);
                }
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

fn connect(
    layer_shell: bool,
) -> (
    Connection,
    Sender<Control>,
    Receiver<Committed>,
    Receiver<Seen>,
) {
    connect_at(layer_shell, 9)
}

/// [`connect`] to a server that offers `cupertino_shell_v1` at `shell_version`.
fn connect_at(
    layer_shell: bool,
    shell_version: u32,
) -> (
    Connection,
    Sender<Control>,
    Receiver<Committed>,
    Receiver<Seen>,
) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, seen) = spawn_server(server_stream, layer_shell, shell_version);
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

/// Send the queued requests with a wait, keeping its events.
fn flush(conn: &mut Connection, stash: &mut Stash) {
    stash.extend(conn.wait(Some(Duration::from_millis(50))).unwrap());
}

fn shell_event(event: &Event) -> Option<&ShellEvent> {
    match event {
        Event::Extension(e) => e.downcast_ref::<ShellEvent>(),
        _ => None,
    }
}

/// Present a frame of `surface` at its configured size.
fn present(conn: &mut Connection, surface: SurfaceId) {
    conn.lease(surface, (0, 0), BufferFormat::Argb8888)
        .unwrap()
        .present()
        .unwrap();
}

/// A layer surface on the first output, as a shell process makes its panel.
fn layer_surface(conn: &mut Connection) -> LayerSurface {
    let output = conn.outputs()[0].id;
    LayerShell::bind(conn)
        .unwrap()
        .layer_surface(
            conn,
            &LayerSurfaceConfig {
                namespace: "test-shell".to_string(),
                output: Some(output),
                layer: Layer::Overlay,
                anchor: Anchor::Top | Anchor::Right | Anchor::Bottom,
                size: (100, 0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: KeyboardInteractivity::None,
            },
        )
        .unwrap()
}

/// Wait for the configure of `surface`; the size as sent.
fn configured(conn: &mut Connection, stash: &mut Stash, surface: SurfaceId) -> (u32, u32) {
    wait_for(conn, stash, |e| match shell_event(e) {
        Some(ShellEvent::Configure { surface: s, size }) if *s == surface => Some(*size),
        _ => None,
    })
}

/// Connect, create the layer surface, map it and give the client an input serial (6).
fn connected_layer_surface() -> (
    Connection,
    Sender<Control>,
    Receiver<Committed>,
    Receiver<Seen>,
    SurfaceId,
    Stash,
) {
    connected_layer_surface_at(9)
}

/// [`connected_layer_surface`] on a server that offers `cupertino_shell_v1` at `shell_version`.
fn connected_layer_surface_at(
    shell_version: u32,
) -> (
    Connection,
    Sender<Control>,
    Receiver<Committed>,
    Receiver<Seen>,
    SurfaceId,
    Stash,
) {
    let (mut conn, control, committed, seen) = connect_at(true, shell_version);
    let layer = layer_surface(&mut conn).surface();
    let mut stash = Stash::new();
    configured(&mut conn, &mut stash, layer);
    present(&mut conn, layer);
    control.send(Control::PointerPress).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Button { .. },
                ..
            }
        )
        .then_some(())
    });
    (conn, control, committed, seen, layer, stash)
}

fn placement(rect: Rect, size: (u32, u32)) -> PopupPlacement {
    PopupPlacement {
        anchor_rect: rect,
        size,
        anchor: PopupAnchor::TopLeft,
        gravity: Gravity::BottomRight,
        constraint_adjustment: ConstraintAdjustment::empty(),
        offset: (0, 0),
        reactive: false,
        parent_size: None,
        grab: true,
    }
}

fn popup_configured(conn: &mut Connection, stash: &mut Stash, popup: Popup) -> (u32, u32) {
    wait_for(conn, stash, |e| match e {
        Event::PopupConfigure {
            popup: p,
            configure,
        } if *p == popup => Some((configure.width, configure.height)),
        _ => None,
    })
}

#[test]
fn surface_lifecycle_against_a_protocol_server() {
    let (mut conn, control, committed, _seen) = connect(true);
    let output = conn.outputs()[0].id;
    let layer = layer_surface(&mut conn);
    let surface = layer.surface();

    // the server sized the surface 100x50 on its output; the output is at scale 2, so the first frame is 200x100
    let mut stash = Stash::new();
    let size = configured(&mut conn, &mut stash, surface);
    assert_eq!(layer.configured(size).unwrap(), (100, 50));
    assert_eq!(
        conn.surface_outputs(surface).unwrap(),
        vec![output],
        "the server entered the surface into its output"
    );
    present(&mut conn, surface);
    flush(&mut conn, &mut stash);
    assert_eq!(
        committed.recv_timeout(Duration::from_secs(5)).unwrap(),
        Committed {
            buffer_size: (200, 100),
            buffer_scale: 2,
        }
    );

    // pointer input arrives in surface-local logical coordinates; a token follows the press serial
    control.send(Control::PointerPress).unwrap();
    let mut seen = Vec::new();
    wait_for(&mut conn, &mut stash, |e| {
        if let Event::Pointer {
            surface: s, event, ..
        } = e
        {
            assert_eq!(*s, surface);
            seen.push(*event);
        }
        (seen.len() == 3).then_some(())
    });
    assert_eq!(
        seen,
        vec![
            PointerEvent::Enter(Point::new(1.5, 2.5)),
            PointerEvent::Motion(Point::new(10.25, 20.75)),
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position: Point::new(10.25, 20.75)
            },
        ]
    );
    let request = conn.request_activation_token(surface, None).unwrap();
    let token = wait_for(&mut conn, &mut stash, |e| match e {
        Event::ActivationToken { request: r, token } if *r == request => Some(token.clone()),
        _ => None,
    });
    assert_eq!(token, "token-for-serial-6");
}

#[test]
fn popups_are_placed_grabbed_resized_and_dismissed() {
    let (mut conn, control, committed, seen, layer, mut stash) = connected_layer_surface();
    while committed.try_recv().is_ok() {}

    // the popup covers the rect rounded outwards, hangs off the layer surface and grabs with the press serial
    let popup = conn
        .popup(
            layer,
            &placement(Rect::new(10.4, 24.0, 153.2, 88.5), (154, 89)),
        )
        .unwrap();
    let placed = PositionerState {
        size: (154, 89),
        anchor_rect: [10, 24, 154, 89],
        anchor: Some(xdg_positioner::Anchor::TopLeft),
        gravity: Some(xdg_positioner::Gravity::BottomRight),
    };
    flush(&mut conn, &mut stash);
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Popup {
            positioner: placed,
            layer_parent: true,
            grab_serial: 6
        }
    );
    assert_eq!(popup_configured(&mut conn, &mut stash, popup), (154, 89));
    present(&mut conn, popup.surface());
    flush(&mut conn, &mut stash);
    assert_eq!(
        committed
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .buffer_size,
        (308, 178)
    );

    // a submenu is a popup of the popup, placed relative to it
    let child = conn
        .popup(
            popup.surface(),
            &placement(Rect::new(150.0, 6.0, 80.0, 40.0), (80, 40)),
        )
        .unwrap();
    flush(&mut conn, &mut stash);
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Popup {
            positioner: PositionerState {
                size: (80, 40),
                anchor_rect: [150, 6, 80, 40],
                anchor: Some(xdg_positioner::Anchor::TopLeft),
                gravity: Some(xdg_positioner::Gravity::BottomRight)
            },
            layer_parent: false,
            grab_serial: 6,
        }
    );
    conn.destroy(child.surface()).unwrap();
    flush(&mut conn, &mut stash);
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::PopupDestroyed
    );

    // a repositioned popup reports its new size
    conn.reposition_popup(
        popup,
        &placement(Rect::new(10.0, 24.0, 200.0, 60.0), (200, 60)),
    )
    .unwrap();
    flush(&mut conn, &mut stash);
    assert!(
        matches!(seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Reposition { token: 1, positioner } if positioner.size == (200, 60))
    );
    assert_eq!(popup_configured(&mut conn, &mut stash, popup), (200, 60));

    // the server dismisses the grab; the client answers pings
    control.send(Control::DismissPopup).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        (*e == Event::PopupDone(popup)).then_some(())
    });
    conn.destroy(popup.surface()).unwrap();
    control.send(Control::Ping(77)).unwrap();
    flush(&mut conn, &mut stash);
    flush(&mut conn, &mut stash);
    let mut seen_after = Vec::new();
    while let Ok(s) = seen.recv_timeout(Duration::from_millis(200)) {
        seen_after.push(s);
    }
    assert_eq!(seen_after, vec![Seen::PopupDestroyed, Seen::Pong(77)]);
}

#[test]
fn menu_bar_groups_arrive_typed_and_requests_reach_the_server() {
    let (mut conn, control, _committed, seen, layer, mut stash) = connected_layer_surface();
    let shell = Shell::bind(&mut conn).unwrap();
    let bar = shell.menu_bar(&conn, layer, conn.outputs()[0].id).unwrap();
    flush(&mut conn, &mut stash);
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::MenuBar {
            surface_has_layer_role: true
        }
    );

    let menu_bar_update = |e: &Event| match shell_event(e) {
        Some(ShellEvent::MenuBar { surface, update }) if *surface == layer => {
            Some((**update).clone())
        }
        _ => None,
    };
    let first = wait_for(&mut conn, &mut stash, menu_bar_update);
    assert_eq!(
        first,
        MenuBarUpdate {
            active_app: Some(ActiveApp {
                app_id: "org.example.editor".into(),
                title: "notes.txt".into()
            }),
            menu: Some(WindowMenu::DbusMenu {
                service: ":1.42".into(),
                path: "/MenuBar".into()
            }),
            window_state: Some(WindowState {
                can_minimize: true,
                can_zoom: true,
                full_screen: false,
                workspace: 2,
                workspaces: 4
            }),
            engage: false,
            engage_help: false,
            screen_capture: None,
            session_agent: None,
            backdrop_lightness: None,
            backdrop_spread: None,
            windows: None,
            shortcuts: None,
        }
    );
    control.send(Control::Engage).unwrap();
    let engaged = wait_for(&mut conn, &mut stash, menu_bar_update);
    assert!(engaged.engage);
    let capture = wait_for(&mut conn, &mut stash, menu_bar_update);
    assert_eq!(capture.screen_capture, Some(true));

    bar.set_menu_open(true);
    bar.window_command(WindowCommand::Minimize);
    // a version 9 server has none of these requests: they are not sent
    assert!(!bar.can_edit());
    assert!(!bar.can_force_quit_app());
    bar.window_command(WindowCommand::TileLeft);
    bar.activate_window(7);
    bar.edit(EditAction::Copy);
    bar.force_quit_app("org.example.app");
    bar.move_to_workspace(3);
    bar.set_menu_open(false);
    flush(&mut conn, &mut stash);
    let sent: Vec<Seen> = (0..4)
        .map(|_| seen.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert_eq!(
        sent,
        vec![
            Seen::MenuOpen(true),
            Seen::Command(cupertino_menu_bar_v1::Command::Minimize),
            Seen::Workspace(3),
            Seen::MenuOpen(false)
        ]
    );
}

#[test]
fn a_version_14_server_tells_the_bar_the_backdrop_lightness() {
    let (mut conn, _control, _committed, _seen, layer, mut stash) = connected_layer_surface_at(14);
    let shell = Shell::bind(&mut conn).unwrap();
    let _bar = shell.menu_bar(&conn, layer, conn.outputs()[0].id).unwrap();
    flush(&mut conn, &mut stash);
    let lightness = wait_for(&mut conn, &mut stash, |e| match shell_event(e) {
        Some(ShellEvent::MenuBar { surface, update }) if *surface == layer => {
            update.backdrop_lightness
        }
        _ => None,
    });
    assert_eq!(lightness, 620);
}

#[test]
fn a_version_21_server_tells_the_bar_the_backdrop_spread_band_by_band() {
    let (mut conn, _control, _committed, _seen, layer, mut stash) = connected_layer_surface_at(21);
    let shell = Shell::bind(&mut conn).unwrap();
    let _bar = shell.menu_bar(&conn, layer, conn.outputs()[0].id).unwrap();
    flush(&mut conn, &mut stash);
    let spread = wait_for(&mut conn, &mut stash, |e| match shell_event(e) {
        Some(ShellEvent::MenuBar { surface, update }) if *surface == layer => {
            update.backdrop_spread.clone()
        }
        _ => None,
    });
    assert_eq!(spread, [0, 300, 45]);
}

#[test]
fn a_version_15_server_tells_the_bar_the_active_windows_enabled_controls() {
    let (mut conn, _control, _committed, _seen, layer, mut stash) = connected_layer_surface_at(15);
    let shell = Shell::bind(&mut conn).unwrap();
    let _bar = shell.menu_bar(&conn, layer, conn.outputs()[0].id).unwrap();
    flush(&mut conn, &mut stash);
    let state = wait_for(&mut conn, &mut stash, |e| match shell_event(e) {
        Some(ShellEvent::MenuBar { surface, update }) if *surface == layer => update.window_state,
        _ => None,
    });
    assert_eq!(
        (state.can_minimize, state.can_zoom),
        (false, true),
        "the server enabled zoom alone"
    );
}

#[test]
fn dock_groups_arrive_typed_and_requests_reach_the_server() {
    let (mut conn, control, _committed, seen, layer, mut stash) = connected_layer_surface();
    let shell = Shell::bind(&mut conn).unwrap();
    let output = conn.outputs()[0].id;
    let dock = shell.dock(&conn, layer, output).unwrap();
    flush(&mut conn, &mut stash);
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Dock {
            surface_has_layer_role: true
        }
    );

    let dock_update = |e: &Event| match shell_event(e) {
        Some(ShellEvent::Dock { surface, update }) if *surface == layer => Some(update.clone()),
        _ => None,
    };
    let first = wait_for(&mut conn, &mut stash, dock_update);
    assert_eq!(
        first,
        vec![
            DockChange::Window(DockWindow {
                id: 1,
                app_id: "org.example.editor".into(),
                title: "notes.txt".into(),
                minimized: false,
                active: true
            }),
            DockChange::Window(DockWindow {
                id: 2,
                app_id: "org.example.mail".into(),
                title: "Inbox".into(),
                minimized: true,
                active: false
            }),
        ]
    );
    control.send(Control::DockChange).unwrap();
    let second = wait_for(&mut conn, &mut stash, dock_update);
    assert_eq!(
        second,
        vec![
            DockChange::WindowClosed(2),
            DockChange::Launching("foot".into()),
            DockChange::ToggleAppGrid,
            DockChange::SwitcherChanged {
                output,
                apps: vec!["org.example.mail".into(), "org.example.editor".into()],
                selected: 1
            },
            DockChange::SwitcherClosed { committed: false },
            DockChange::Attention("org.example.mail".into()),
        ]
    );

    dock.activate_app("org.example.editor");
    dock.restore_window(2, &Rect::new(10.4, 20.0, 64.0, 64.5))
        .unwrap();
    dock.set_minimize_target(&Rect::new(100.0, 8.0, 64.0, 64.0))
        .unwrap();
    assert!(dock
        .set_minimize_target(&Rect::new(f64::NAN, 0.0, 1.0, 1.0))
        .is_err());
    dock.switcher_select(1).unwrap();
    dock.switcher_activate(0).unwrap();
    dock.switcher_cancel();
    flush(&mut conn, &mut stash);
    let sent: Vec<Seen> = (0..6)
        .map(|_| seen.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert_eq!(
        sent,
        vec![
            Seen::Activate("org.example.editor".into()),
            Seen::Restore {
                id: 2,
                rect: [10, 20, 65, 65]
            },
            Seen::MinimizeTarget([100, 8, 64, 64]),
            Seen::SwitcherSelect(1),
            Seen::SwitcherActivate(0),
            Seen::SwitcherCancel,
        ]
    );
}

#[test]
fn a_session_lock_creates_lock_surfaces_and_unlocks_after_they_are_gone() {
    let (mut conn, _control, committed, seen) = connect(true);
    let mut stash = Stash::new();
    let mut manager = SessionLockManager::bind(&mut conn).unwrap();
    let lock = manager.lock(&conn);
    let output = conn.outputs()[0].id;
    let surface = lock.lock_surface(&mut conn, output).unwrap();
    assert_eq!(configured(&mut conn, &mut stash, surface), (640, 480));
    present(&mut conn, surface);
    // requests go out when the client waits
    flush(&mut conn, &mut stash);
    assert_eq!(
        committed
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .buffer_size,
        (640, 480)
    );
    let generation = lock.generation();
    wait_for(&mut conn, &mut stash, |e| {
        (shell_event(e)
            == Some(&ShellEvent::Lock {
                generation,
                locked: true,
            }))
        .then_some(())
    });

    conn.destroy(surface).unwrap();
    lock.unlock();
    conn.roundtrip().unwrap();
    let seen: Vec<Seen> = seen.try_iter().collect();
    assert_eq!(
        seen,
        [
            Seen::LockRequested,
            Seen::LockSurfaceAcked(7),
            Seen::LockSurfaceDestroyed,
            Seen::Unlocked
        ]
    );
}

#[test]
fn an_idle_watch_reports_the_idle_edge_once() {
    let (mut conn, _control, _committed, seen) = connect(true);
    let mut stash = Stash::new();
    let notifier = IdleNotifier::bind(&mut conn).unwrap();
    let seat = conn.seat().unwrap();
    let _watch = notifier.watch(&conn, &seat, 600_000);
    wait_for(&mut conn, &mut stash, |e| {
        (shell_event(e) == Some(&ShellEvent::Idle(true))).then_some(())
    });
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::IdleWatch(600_000)
    );
    flush(&mut conn, &mut stash);
    assert!(
        !stash
            .iter()
            .any(|e| matches!(shell_event(e), Some(ShellEvent::Idle(_)))),
        "{stash:?}"
    );
}

#[test]
fn the_session_state_at_connect_is_read_and_changes_arrive_as_events() {
    let (mut conn, control, _committed, _seen) = connect(true);
    let shell = Shell::bind(&mut conn).unwrap();
    let (_session, mut state, rest) = shell.observe(&mut conn).unwrap();
    // no event for what connect already read
    assert_eq!(
        state,
        SessionState {
            dark: true,
            display: DisplayOptions {
                reduce_motion: true,
                ..DisplayOptions::default()
            },
            locked: false,
        }
    );
    let mut stash: Stash = rest.into();
    control.send(Control::SessionChange).unwrap();
    let mut changes = Vec::new();
    wait_for(&mut conn, &mut stash, |e| {
        if let Event::Extension(event) = e {
            match state.apply(event) {
                Some(SessionChange::Appearance { dark }) => changes.push(format!("dark={dark}")),
                Some(SessionChange::DisplayOptions(o)) => changes.push(format!(
                    "motion={} transparency={} contrast={}",
                    o.reduce_motion, o.reduce_transparency, o.increase_contrast
                )),
                Some(SessionChange::Lock { locked }) => changes.push(format!("locked={locked}")),
                None => {
                    if let Some(ShellEvent::LockRequested { explicit }) = shell_event(e) {
                        changes.push(format!("lock requested={explicit}"))
                    }
                }
            }
        }
        (changes.len() == 5).then_some(())
    });
    assert_eq!(
        changes,
        [
            "dark=false",
            "motion=false transparency=true contrast=true",
            "locked=true",
            "locked=false",
            "lock requested=true"
        ]
    );
    assert_eq!(
        state,
        SessionState {
            dark: false,
            display: DisplayOptions {
                reduce_transparency: true,
                increase_contrast: true,
                ..DisplayOptions::default()
            },
            locked: false,
        }
    );
}

#[test]
fn observe_reads_the_session_state_before_it_returns() {
    let (mut conn, _control, _committed, _seen) = connect(true);
    let shell = Shell::bind(&mut conn).unwrap();
    let (_session, state, rest) = shell.observe(&mut conn).unwrap();
    assert_eq!(
        state,
        SessionState {
            dark: true,
            display: DisplayOptions {
                reduce_motion: true,
                ..DisplayOptions::default()
            },
            locked: false,
        }
    );
    assert!(!rest.iter().any(|e| matches!(e, Event::Extension(_))));
}

/// A shell process (login, dock, menu bar) needs the layer shell and fails at start, naming the global, when the
/// server does not offer it: the layer shell binds nothing there, while the shell global does.
#[test]
fn a_shell_process_does_not_start_without_the_layer_shell() {
    let (mut conn, _control, _committed, _seen) = connect(false);
    assert!(LayerShell::bind(&mut conn).is_none());
    assert!(Shell::bind(&mut conn).is_some());
    let (mut conn, _control, _committed, _seen) = connect(true);
    assert!(LayerShell::bind(&mut conn).is_some());
}
