//! Runs the client against a minimal in-process server (shm, layer shell, popups, toplevels with
//! decorations, backdrops, seat, activation, outputs) and checks what the server receives and what the client reports back.

use cupertino_client::{
    ActiveApp, Anchor, Button, Client, ClientError, Connection, DisplayOptions, DockChange,
    DockWindow, Event, KeyboardInteractivity, Layer, LayerSurfaceConfig, LockState, Margin,
    MenuBarUpdate, OutputGeometry, PointerEvent, Scale, TitlebarStyle, ToplevelConfig,
    WindowCommand, WindowMenu, WindowState,
};
use cupertino_geometry::{Point, Rect};
use cupertino_protocols::server::app_effects::cupertino_app_effects_v1::{
    self, CupertinoAppEffectsV1,
};
use cupertino_protocols::server::app_effects::cupertino_appearance_v1::{
    self, CupertinoAppearanceV1, DisplayOption as AppearanceOption,
};
use cupertino_protocols::server::app_effects::cupertino_backdrop_v1::{self, CupertinoBackdropV1};
use cupertino_protocols::server::cupertino_dock_v1::{
    self, CupertinoDockV1, WindowState as DockWindowState,
};
use cupertino_protocols::server::cupertino_menu_bar_v1::{self, CupertinoMenuBarV1};
use cupertino_protocols::server::cupertino_session_v1::{self, CupertinoSessionV1, DisplayOption};
use cupertino_protocols::server::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_protocols::server::scroll::cupertino_pointer_scroll_v1::{
    self, CupertinoPointerScrollV1,
};
use cupertino_protocols::server::scroll::cupertino_scroll_v1::{self, CupertinoScrollV1};
use memmap2::Mmap;
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::{AsFd, IntoRawFd};
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
use wayland_protocols::xdg::decoration::zv1::server::zxdg_decoration_manager_v1::{
    self, ZxdgDecorationManagerV1,
};
use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::{
    self, ZxdgToplevelDecorationV1,
};
use wayland_protocols::xdg::shell::server::xdg_popup::{self, XdgPopup};
use wayland_protocols::xdg::shell::server::xdg_positioner::{self, XdgPositioner};
use wayland_protocols::xdg::shell::server::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::server::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_manager_v1::{
    self, ZxdgOutputManagerV1,
};
use wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_v1::ZxdgOutputV1;
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_buffer::{self, WlBuffer};
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_data_device::{self, WlDataDevice};
use wayland_server::protocol::wl_data_device_manager::{self, WlDataDeviceManager};
use wayland_server::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_server::protocol::wl_data_source::{self, WlDataSource};
use wayland_server::protocol::wl_output::{self, WlOutput};
use wayland_server::protocol::wl_pointer::{self, WlPointer};
use wayland_server::protocol::wl_region::{self, WlRegion};
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
    stride: i32,
    /// The protocol id of the committed `wl_buffer`.
    buffer: u32,
    buffer_scale: i32,
    first_pixel: [u8; 4],
    backdrops: Vec<BackdropState>,
    input_region: Option<Vec<[i32; 4]>>,
    /// The `damage_buffer` rectangles of the commit (x, y, width, height).
    damage: Vec<[i32; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct BackdropState {
    material: String,
    shape: [f64; 5],
    alpha: f64,
}

enum Control {
    /// Answer the frame callbacks of the committed surface.
    FrameDone,
    /// Enter the surface with the pointer, move, and press the left button.
    PointerPress,
    /// Ping the client through `xdg_wm_base`.
    Ping(u32),
    /// Dismiss the most recent popup.
    DismissPopup,
    /// Send the menu bar the engage shortcut, then a capture start in a group of its own.
    Engage,
    /// Tell the dock a window closed, a program is launching, the app-grid action fired and a switch opened and was cancelled.
    DockChange,
    /// Switch the session to the light appearance, lock it, and unlock it.
    SessionChange,
    /// Resize the toplevel to 640 x 480 and activate it.
    ResizeToplevel,
    /// Enter the toplevel with the pointer and scroll it twice down and once right.
    ScrollToplevel,
    /// Ask the toplevel to close.
    CloseToplevel,
    /// Another client's text becomes the selection: offer it to the client's data device.
    OfferSelection(String),
}

/// A request the server saw that the commit record does not cover.
#[derive(Debug, PartialEq)]
enum Seen {
    /// The client set the selection with this serial, offering these types.
    Selection {
        serial: u32,
        types: Vec<String>,
    },
    /// What the client wrote when the server asked its selection for the first type (a paste elsewhere).
    Served(String),
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
    /// The title bar style of a window, chosen with `set_titlebar_style`.
    TitlebarStyle(cupertino_app_effects_v1::TitlebarStyle),
    ShellBound,
    /// `xdg_toplevel.set_maximized` (true) or `unset_maximized`.
    Maximize(bool),
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
    AppId(String),
    Title(String),
    MinSize(i32, i32),
    DecorationMode(zxdg_toplevel_decoration_v1::Mode),
}

/// The toplevel of the test client.
struct ToplevelState {
    surface: WlSurface,
    xdg: XdgSurface,
    toplevel: XdgToplevel,
    /// The initial configure has been sent.
    configured: bool,
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
    damage: Vec<[i32; 4]>,
    scale: i32,
    region: Option<Arc<Mutex<Vec<[i32; 4]>>>>,
    frames: Vec<WlCallback>,
    backdrops: Vec<Arc<Mutex<BackdropState>>>,
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

struct BufferData {
    map: Arc<Mmap>,
    offset: i32,
    width: i32,
    height: i32,
    stride: i32,
}

struct Mock {
    committed: Sender<Committed>,
    seen: Sender<Seen>,
    report_shell_bind: bool,
    next_serial: u32,
    popups: Vec<XdgPopup>,
    wm_base: Option<XdgWmBase>,
    menu_bar: Option<CupertinoMenuBarV1>,
    dock: Option<CupertinoDockV1>,
    session: Option<CupertinoSessionV1>,
    appearance: Option<CupertinoAppearanceV1>,
    outputs: Vec<WlOutput>,
    pointer: Option<WlPointer>,
    scroll: Option<CupertinoPointerScrollV1>,
    surface: Option<WlSurface>,
    pending_frames: Vec<WlCallback>,
    shown: Option<WlBuffer>,
    idle: Option<ExtIdleNotificationV1>,
    toplevel: Option<ToplevelState>,
    data_device: Option<WlDataDevice>,
    /// The read end of a paste from the client's selection, and what arrived so far.
    pasting: Option<(std::io::PipeReader, Vec<u8>)>,
}

struct Data;
impl ClientData for Data {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

impl GlobalDispatch<WlCompositor, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WlCompositor>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
    }
}
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
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                let surface = init.init(id, SurfaceData::default());
                // the pointer targets the first surface, the layer surface
                state.surface.get_or_insert(surface);
            }
            wl_compositor::Request::CreateRegion { id } => {
                init.init(id, Arc::new(Mutex::new(Vec::<[i32; 4]>::new())));
            }
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlRegion, Arc<Mutex<Vec<[i32; 4]>>>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlRegion,
        request: wl_region::Request,
        rects: &Arc<Mutex<Vec<[i32; 4]>>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_region::Request::Add {
            x,
            y,
            width,
            height,
        } = request
        {
            rects.lock().unwrap().push([x, y, width, height]);
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
            wl_surface::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => s.damage.push([x, y, width, height]),
            wl_surface::Request::SetBufferScale { scale } => s.scale = scale,
            wl_surface::Request::SetInputRegion { region } => {
                s.region = region.map(|r| {
                    r.data::<Arc<Mutex<Vec<[i32; 4]>>>>()
                        .expect("region data")
                        .clone()
                });
            }
            wl_surface::Request::Frame { callback } => s.frames.push(init.init(callback, ())),
            wl_surface::Request::Commit => {
                if let Some(layer) = s.layer.clone().filter(|_| s.buffer.is_none()) {
                    // initial commit of a layer surface: place it on the output and size it
                    surface.enter(&state.outputs[0]);
                    layer.configure(1, 100, 50);
                }
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
                    let b = buffer.data::<BufferData>().expect("buffer data");
                    let start = b.offset as usize;
                    let first_pixel = b.map[start..start + 4].try_into().unwrap();
                    state
                        .committed
                        .send(Committed {
                            buffer_size: (b.width, b.height),
                            stride: b.stride,
                            buffer: buffer.id().protocol_id(),
                            buffer_scale: s.scale,
                            first_pixel,
                            backdrops: s
                                .backdrops
                                .iter()
                                .map(|b| b.lock().unwrap().clone())
                                .collect(),
                            input_region: s.region.as_ref().map(|r| r.lock().unwrap().clone()),
                            damage: std::mem::take(&mut s.damage),
                        })
                        .unwrap();
                    // like a compositor that keeps the current buffer until the next one replaces it
                    if let Some(previous) = state.shown.replace(buffer) {
                        previous.release();
                    }
                    state.pending_frames.append(&mut s.frames);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlCallback,
        _: wayland_server::protocol::wl_callback::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlShm, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WlShm>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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
        if let wl_shm::Request::CreatePool { id, fd, .. } = request {
            // SAFETY: the client sized the memfd before sending it and keeps it that size.
            let map = unsafe { Mmap::map(&std::fs::File::from(fd)) }.unwrap();
            init.init(id, Arc::new(map));
        }
    }
}
impl Dispatch<WlShmPool, Arc<Mmap>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlShmPool,
        request: wl_shm_pool::Request,
        map: &Arc<Mmap>,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm_pool::Request::CreateBuffer {
            id,
            offset,
            width,
            height,
            stride,
            format,
        } = request
        {
            assert_eq!(format.into_result().ok(), Some(wl_shm::Format::Argb8888));
            init.init(
                id,
                BufferData {
                    map: map.clone(),
                    offset,
                    width,
                    height,
                    stride,
                },
            );
        }
    }
}
impl Dispatch<WlBuffer, BufferData> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlBuffer,
        _: wl_buffer::Request,
        _: &BufferData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
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
impl Dispatch<WlOutput, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlOutput,
        _: wl_output::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<ZxdgOutputManagerV1, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<ZxdgOutputManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
    }
}
impl Dispatch<ZxdgOutputManagerV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &ZxdgOutputManagerV1,
        request: zxdg_output_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_output_manager_v1::Request::GetXdgOutput { id, output } = request {
            // the output sits right of a 1920 point wide neighbour
            let xdg = init.init(id, ());
            xdg.logical_position(1920, 0);
            xdg.logical_size(1280, 800);
            // version 3 ends the description with the output's own `done`
            if xdg.version() >= 3 {
                output.done();
            } else {
                xdg.done();
            }
        }
    }
}
impl Dispatch<ZxdgOutputV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &ZxdgOutputV1,
        _: wayland_protocols::xdg::xdg_output::zv1::server::zxdg_output_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
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
impl Dispatch<WlPointer, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlPointer,
        _: wl_pointer::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<CupertinoScrollV1, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<CupertinoScrollV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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
impl Dispatch<CupertinoPointerScrollV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &CupertinoPointerScrollV1,
        _: cupertino_pointer_scroll_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<ZwlrLayerShellV1, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<ZwlrLayerShellV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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
        state: &mut Self,
        _: &ServerClient,
        xdg: &XdgSurface,
        request: xdg_surface::Request,
        surface: &WlSurface,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_surface::Request::GetPopup { id, positioner, .. } => {
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
            xdg_surface::Request::GetToplevel { id } => {
                let toplevel = init.init(id, ());
                state.toplevel = Some(ToplevelState {
                    surface: surface.clone(),
                    xdg: xdg.clone(),
                    toplevel,
                    configured: false,
                });
            }
            _ => {}
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
        match request {
            xdg_toplevel::Request::SetAppId { app_id } => {
                state.seen.send(Seen::AppId(app_id)).unwrap()
            }
            xdg_toplevel::Request::SetTitle { title } => {
                state.seen.send(Seen::Title(title)).unwrap()
            }
            xdg_toplevel::Request::SetMinSize { width, height } => {
                state.seen.send(Seen::MinSize(width, height)).unwrap()
            }
            xdg_toplevel::Request::SetMaximized => state.seen.send(Seen::Maximize(true)).unwrap(),
            xdg_toplevel::Request::UnsetMaximized => {
                state.seen.send(Seen::Maximize(false)).unwrap()
            }
            _ => {}
        }
    }
}
impl GlobalDispatch<ZxdgDecorationManagerV1, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<ZxdgDecorationManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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

impl GlobalDispatch<CupertinoShellV1, ()> for Mock {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<CupertinoShellV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
        if state.report_shell_bind {
            state.seen.send(Seen::ShellBound).unwrap();
        }
    }
}
impl GlobalDispatch<CupertinoAppEffectsV1, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<CupertinoAppEffectsV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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
                init.init(id, state.clone());
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
            cupertino_app_effects_v1::Request::GetAppearance { id } => {
                let observer = init.init(id, ());
                // the session starts dark and with reduce motion
                observer.appearance(1);
                observer.display_options(AppearanceOption::ReduceMotion);
                state.appearance = Some(observer);
            }
            _ => {}
        }
    }
}
impl Dispatch<CupertinoAppearanceV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &CupertinoAppearanceV1,
        _: cupertino_appearance_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
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
                        surface_has_layer_role: surface
                            .data::<SurfaceData>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .layer
                            .is_some(),
                    })
                    .unwrap();
                // the first group follows the object's creation
                bar.active_app("org.example.editor".into(), "notes.txt".into());
                bar.menu_dbusmenu(":1.42".into(), "/MenuBar".into());
                bar.window_state(1, 2, 4);
                bar.done();
                state.menu_bar = Some(bar);
            }
            cupertino_shell_v1::Request::GetDock { id, surface, .. } => {
                let dock = init.init(id, ());
                state
                    .seen
                    .send(Seen::Dock {
                        surface_has_layer_role: surface
                            .data::<SurfaceData>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .layer
                            .is_some(),
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
impl Dispatch<CupertinoSessionV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &CupertinoSessionV1,
        _: cupertino_session_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
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
            _ => {}
        }
    }
}

impl GlobalDispatch<XdgActivationV1, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<XdgActivationV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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

impl GlobalDispatch<ExtSessionLockManagerV1, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<ExtSessionLockManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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

impl GlobalDispatch<ExtIdleNotifierV1, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<ExtIdleNotifierV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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
impl Dispatch<ExtIdleNotificationV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &ExtIdleNotificationV1,
        _: wayland_protocols::ext::idle_notify::v1::server::ext_idle_notification_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlDataDeviceManager, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WlDataDeviceManager>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
    }
}
impl Dispatch<WlDataDeviceManager, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                init.init(id, Mutex::new(Vec::new()));
            }
            wl_data_device_manager::Request::GetDataDevice { id, .. } => {
                state.data_device = Some(init.init(id, ()));
            }
            _ => {}
        }
    }
}
impl Dispatch<WlDataSource, Mutex<Vec<String>>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlDataSource,
        request: wl_data_source::Request,
        types: &Mutex<Vec<String>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_source::Request::Offer { mime_type } = request {
            types.lock().unwrap().push(mime_type);
        }
    }
}
impl Dispatch<WlDataDevice, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlDataDevice,
        request: wl_data_device::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_device::Request::SetSelection {
            source: Some(source),
            serial,
        } = request
        {
            let types = source
                .data::<Mutex<Vec<String>>>()
                .unwrap()
                .lock()
                .unwrap()
                .clone();
            // paste it at once, as another client would
            let (reader, writer) = std::io::pipe().unwrap();
            source.send(types[0].clone(), writer.as_fd());
            state.pasting = Some((reader, Vec::new()));
            state.seen.send(Seen::Selection { serial, types }).unwrap();
        }
    }
}
impl Dispatch<WlDataOffer, String> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlDataOffer,
        request: wl_data_offer::Request,
        text: &String,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_offer::Request::Receive { fd, .. } = request {
            std::fs::File::from(fd).write_all(text.as_bytes()).unwrap();
        }
    }
}

/// Run the server on a thread until the test drops the control channel.
fn spawn_server(stream: UnixStream) -> (Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
    spawn_server_reporting(stream, false)
}

/// [`spawn_server`]; with `report_shell_bind` the server tells [`Seen::ShellBound`] when a client binds the private shell
/// global.
fn spawn_server_reporting(
    stream: UnixStream,
    report_shell_bind: bool,
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
        dh.create_global::<Mock, ZwlrLayerShellV1, _>(4, ());
        dh.create_global::<Mock, CupertinoShellV1, _>(9, ());
        dh.create_global::<Mock, CupertinoAppEffectsV1, _>(1, ());
        dh.create_global::<Mock, XdgActivationV1, _>(1, ());
        dh.create_global::<Mock, ZxdgOutputManagerV1, _>(3, ());
        dh.create_global::<Mock, XdgWmBase, _>(6, ());
        dh.create_global::<Mock, ExtSessionLockManagerV1, _>(1, ());
        dh.create_global::<Mock, ExtIdleNotifierV1, _>(2, ());
        dh.create_global::<Mock, ZxdgDecorationManagerV1, _>(1, ());
        dh.create_global::<Mock, CupertinoScrollV1, _>(1, ());
        dh.create_global::<Mock, WlDataDeviceManager, _>(3, ());
        let mut mock = Mock {
            committed: committed_tx,
            seen: seen_tx,
            next_serial: 1,
            popups: Vec::new(),
            wm_base: None,
            menu_bar: None,
            dock: None,
            report_shell_bind,
            session: None,
            appearance: None,
            outputs: Vec::new(),
            pointer: None,
            scroll: None,
            surface: None,
            pending_frames: Vec::new(),
            shown: None,
            idle: None,
            toplevel: None,
            data_device: None,
            pasting: None,
        };
        let mut display_handle = display.handle();
        display_handle
            .insert_client(stream, Arc::new(Data))
            .unwrap();
        loop {
            match control_rx.try_recv() {
                Ok(Control::FrameDone) => mock.pending_frames.drain(..).for_each(|cb| cb.done(0)),
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
                    if let Some(session) = mock.session.as_ref() {
                        session.appearance(0);
                        session.display_options(
                            DisplayOption::ReduceTransparency | DisplayOption::IncreaseContrast,
                        );
                        session.locked();
                        session.unlocked();
                        session.lock_requested(1);
                    }
                    if let Some(observer) = mock.appearance.as_ref() {
                        observer.appearance(0);
                        observer.display_options(
                            AppearanceOption::ReduceTransparency
                                | AppearanceOption::IncreaseContrast,
                        );
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
                Ok(Control::OfferSelection(text)) => {
                    let device = mock.data_device.as_ref().unwrap();
                    let offer = device
                        .client()
                        .unwrap()
                        .create_resource::<WlDataOffer, String, Mock>(
                            &display_handle,
                            device.version(),
                            text,
                        )
                        .unwrap();
                    device.data_offer(&offer);
                    offer.offer("text/plain;charset=utf-8".into());
                    device.selection(Some(&offer));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
            display.dispatch_clients(&mut mock).unwrap();
            display.flush_clients().unwrap();
            if let Some((reader, bytes)) = mock.pasting.as_mut() {
                let mut fds = [PollFd::new(&*reader, PollFlags::IN)];
                if poll(&mut fds, Some(&Timespec::default())).unwrap() > 0 {
                    let mut chunk = [0u8; 256];
                    match reader.read(&mut chunk).unwrap() {
                        0 => {
                            let text = String::from_utf8(std::mem::take(bytes)).unwrap();
                            mock.seen.send(Seen::Served(text)).unwrap();
                            mock.pasting = None;
                        }
                        n => bytes.extend_from_slice(&chunk[..n]),
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    (control_tx, committed_rx, seen_rx)
}

/// Events the client reported but the test has not looked at yet.
type Stash = VecDeque<Event>;

/// Take events in order, waiting for more when the stash runs dry, until `pick` accepts one.
fn wait_for<T>(
    client: &mut Client,
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
        stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    }
    panic!("timed out waiting for an event");
}

#[test]
fn surface_lifecycle_against_a_protocol_server() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, _seen) = spawn_server(server_stream);
    let mut client =
        Client::from_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    let outputs = client.outputs();
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].geometry,
        Some(OutputGeometry {
            x: 1920,
            y: 0,
            width: 1280,
            height: 800
        })
    );
    let surface = client
        .layer_surface(&LayerSurfaceConfig {
            namespace: "test-shell".to_string(),
            output: Some(outputs[0].id),
            layer: Layer::Overlay,
            anchor: Anchor::Top | Anchor::Right | Anchor::Bottom,
            size: (100, 0),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::None,
        })
        .unwrap();

    // the server sized the surface 100x50; the output is at scale 2, so the first draw is 200x100
    let mut stash = Stash::new();
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Configured { size, .. } if *size == (100, 50)).then_some(())
    });
    assert_eq!(
        client.surface_outputs(surface).unwrap(),
        vec![outputs[0].id],
        "the server entered the surface into its output"
    );
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::ScaleChanged { scale, .. } if *scale == Scale::from_integer(2).unwrap())
            .then_some(())
    });
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Draw(_)).then_some(())
    });
    let backdrop = client.backdrop(surface).unwrap();
    backdrop.set_material("Menu");
    backdrop
        .set_shape(&Rect::new(4.0, 6.0, 40.0, 20.0), 12.0)
        .unwrap();
    backdrop.set_alpha(0.5).unwrap();
    client
        .set_input_region(surface, &[Rect::new(0.0, 0.0, 10.5, 10.0)])
        .unwrap();
    client
        .draw(surface, |pixmap, scale| {
            assert_eq!(
                (pixmap.width(), pixmap.height(), scale),
                (200, 100, Scale::from_integer(2).unwrap())
            );
            // premultiplied RGBA (10, 20, 30, 40)
            pixmap.pixels_mut()[0] =
                tiny_skia::PremultipliedColorU8::from_rgba(10, 20, 30, 40).unwrap();
        })
        .unwrap();

    // everything staged before the draw arrives in that commit, in wl_shm byte order (B, G, R, A)
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    let first = committed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        first,
        Committed {
            buffer_size: (200, 100),
            stride: 800,
            buffer: first.buffer,
            buffer_scale: 2,
            first_pixel: [30, 20, 10, 40],
            backdrops: vec![BackdropState {
                material: "Menu".to_string(),
                shape: [4.0, 6.0, 40.0, 20.0, 12.0],
                alpha: 0.5
            }],
            input_region: Some(vec![[0, 0, 11, 10]]),
            damage: vec![[0, 0, i32::MAX, i32::MAX]],
        }
    );

    // no new frame is offered before the server answered the frame callback
    client.request_redraw(surface).unwrap();
    let early = client.wait(Some(Duration::from_millis(100))).unwrap();
    assert!(
        !early.iter().any(|e| matches!(e, Event::Draw(_))),
        "{early:?}"
    );
    control.send(Control::FrameDone).unwrap();
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Draw(_)).then_some(())
    });

    // the second frame uses another buffer while the server holds the first
    client.draw(surface, |_, _| {}).unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    let second = committed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_ne!(second.buffer, first.buffer);
    assert_eq!(second.first_pixel, [0, 0, 0, 0]);

    // pointer input arrives in surface-local logical coordinates; a token follows the press serial
    control.send(Control::PointerPress).unwrap();
    let mut seen = Vec::new();
    wait_for(&mut client, &mut stash, |e| {
        if let Event::Pointer { event, .. } = e {
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
    let request = client.request_activation_token(surface).unwrap();
    let token = wait_for(&mut client, &mut stash, |e| match e {
        Event::ActivationToken { request: r, token } if *r == request => Some(token.clone()),
        _ => None,
    });
    assert_eq!(token, "token-for-serial-6");
}

/// Connect, create the layer surface, map it and give the client an input serial (6).
fn connected_layer_surface() -> (
    Client,
    Sender<Control>,
    Receiver<Committed>,
    Receiver<Seen>,
    cupertino_client::SurfaceId,
    Stash,
) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, seen) = spawn_server(server_stream);
    let mut client =
        Client::from_shell_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    let surface = client
        .layer_surface(&LayerSurfaceConfig {
            namespace: "test-shell".to_string(),
            output: Some(client.outputs()[0].id),
            layer: Layer::Overlay,
            anchor: Anchor::Top | Anchor::Right | Anchor::Bottom,
            size: (100, 0),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::None,
        })
        .unwrap();
    let mut stash = Stash::new();
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Draw(_)).then_some(())
    });
    control.send(Control::PointerPress).unwrap();
    wait_for(&mut client, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Button { .. },
                ..
            }
        )
        .then_some(())
    });
    (client, control, committed, seen, surface, stash)
}

#[test]
fn popups_are_placed_grabbed_resized_and_dismissed() {
    let (mut client, control, committed, seen, layer, mut stash) = connected_layer_surface();

    // the popup covers the rect rounded outwards, hangs off the layer surface and grabs with the press serial
    let popup = client
        .popup(layer, &Rect::new(10.4, 24.0, 153.2, 88.5))
        .unwrap();
    let placed = PositionerState {
        size: (154, 89),
        anchor_rect: [10, 24, 154, 89],
        anchor: Some(xdg_positioner::Anchor::TopLeft),
        gravity: Some(xdg_positioner::Gravity::BottomRight),
    };
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Popup {
            positioner: placed,
            layer_parent: true,
            grab_serial: 6
        }
    );
    let size = wait_for(&mut client, &mut stash, |e| match e {
        Event::Configured { surface, size } if *surface == popup => Some(*size),
        _ => None,
    });
    assert_eq!(size, (154, 89));
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Draw(s) if *s == popup).then_some(())
    });
    client
        .draw(popup, |pixmap, _| {
            assert_eq!((pixmap.width(), pixmap.height()), (308, 178))
        })
        .unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        committed
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .buffer_size,
        (308, 178)
    );

    // a submenu is a popup of the popup, placed relative to it
    let child = client
        .popup(popup, &Rect::new(150.0, 6.0, 80.0, 40.0))
        .unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
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
    assert!(matches!(
        client.destroy_surface(popup),
        Err(cupertino_client::ClientError::HasPopups)
    ));
    assert!(matches!(
        client.set_keyboard_interactivity(popup, KeyboardInteractivity::Exclusive),
        Err(cupertino_client::ClientError::NotLayerSurface)
    ));
    client.destroy_surface(child).unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::PopupDestroyed
    );

    // a repositioned popup reports its new size
    client
        .reposition_popup(popup, &Rect::new(10.0, 24.0, 200.0, 60.0))
        .unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert!(
        matches!(seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Reposition { token: 1, positioner } if positioner.size == (200, 60))
    );
    let size = wait_for(&mut client, &mut stash, |e| match e {
        Event::Configured { surface, size } if *surface == popup => Some(*size),
        _ => None,
    });
    assert_eq!(size, (200, 60));
    assert!(matches!(
        client.reposition_popup(layer, &Rect::new(0.0, 0.0, 10.0, 10.0)),
        Err(cupertino_client::ClientError::NotPopup)
    ));

    // the server dismisses the grab; the client answers pings
    control.send(Control::DismissPopup).unwrap();
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::PopupDone(s) if *s == popup).then_some(())
    });
    client.destroy_surface(popup).unwrap();
    control.send(Control::Ping(77)).unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(100))).unwrap());
    stash.extend(client.wait(Some(Duration::from_millis(100))).unwrap());
    let mut seen_after = Vec::new();
    while let Ok(s) = seen.recv_timeout(Duration::from_millis(200)) {
        seen_after.push(s);
    }
    assert_eq!(seen_after, vec![Seen::PopupDestroyed, Seen::Pong(77)]);
}

#[test]
fn menu_bar_groups_arrive_typed_and_requests_reach_the_server() {
    let (mut client, control, _committed, seen, layer, mut stash) = connected_layer_surface();
    let bar = client.menu_bar(layer, client.outputs()[0].id).unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::MenuBar {
            surface_has_layer_role: true
        }
    );

    let first = wait_for(&mut client, &mut stash, |e| match e {
        Event::MenuBar { surface, update } if *surface == layer => Some(update.clone()),
        _ => None,
    });
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
                can_zoom: true,
                workspace: 2,
                workspaces: 4
            }),
            engage: false,
            screen_capture: None,
            session_agent: None,
        }
    );
    control.send(Control::Engage).unwrap();
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::MenuBar { update, .. } if update.engage).then_some(())
    });
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::MenuBar { update, .. } if update.screen_capture == Some(true))
            .then_some(())
    });

    bar.set_menu_open(true);
    bar.window_command(WindowCommand::Minimize);
    bar.move_to_workspace(3);
    bar.set_menu_open(false);
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
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
fn dock_groups_arrive_typed_and_requests_reach_the_server() {
    let (mut client, control, _committed, seen, layer, mut stash) = connected_layer_surface();
    let dock = client.dock(layer, client.outputs()[0].id).unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Dock {
            surface_has_layer_role: true
        }
    );

    let dock_update = |e: &Event| match e {
        Event::Dock { surface, update } if *surface == layer => Some(update.clone()),
        _ => None,
    };
    let first = wait_for(&mut client, &mut stash, dock_update);
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
    let second = wait_for(&mut client, &mut stash, dock_update);
    assert_eq!(
        second,
        vec![
            DockChange::WindowClosed(2),
            DockChange::Launching("foot".into()),
            DockChange::ToggleAppGrid,
            DockChange::SwitcherChanged {
                output: client.outputs()[0].id,
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
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
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
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (_control, committed, seen) = spawn_server(server_stream);
    let mut client =
        Client::from_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    let mut stash = Stash::new();
    assert_eq!(client.lock_state(), None);
    assert!(matches!(
        client.unlock_session(),
        Err(ClientError::NotLocked)
    ));
    assert!(matches!(
        client.lock_surface(client.outputs()[0].id),
        Err(ClientError::NoSessionLock)
    ));

    client.lock_session().unwrap();
    assert!(matches!(
        client.lock_session(),
        Err(ClientError::SessionLockActive)
    ));
    let surface = client.lock_surface(client.outputs()[0].id).unwrap();
    assert!(matches!(
        client.set_keyboard_interactivity(surface, KeyboardInteractivity::Exclusive),
        Err(ClientError::NotLayerSurface)
    ));
    let size = wait_for(&mut client, &mut stash, |e| match e {
        Event::Configured { size, .. } => Some(*size),
        _ => None,
    });
    assert_eq!(size, (640, 480));
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Draw(_)).then_some(())
    });
    client.draw(surface, |_, _| {}).unwrap();
    // requests go out when the client waits
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        committed
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .buffer_size,
        (640, 480)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.lock_state() != Some(LockState::Locked) {
        assert!(Instant::now() < deadline, "the lock was never confirmed");
        stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    }

    client.unlock_session().unwrap();
    assert_eq!(client.lock_state(), None);
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
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (_control, _committed, seen) = spawn_server(server_stream);
    let mut client =
        Client::from_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    assert!(!client.take_idle());
    client.watch_idle(Duration::from_secs(600)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !client.take_idle() {
        assert!(Instant::now() < deadline, "the idle edge never arrived");
        client.wait(Some(Duration::from_millis(50))).unwrap();
    }
    assert_eq!(seen.try_recv().unwrap(), Seen::IdleWatch(600_000));
    assert!(!client.take_idle());
}

#[test]
fn the_session_state_at_connect_is_read_and_changes_arrive_as_events() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, _committed, _seen) = spawn_server(server_stream);
    let mut client =
        Client::from_shell_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    // no event for what connect already read
    assert!(client.is_dark());
    assert_eq!(
        client.display_options(),
        DisplayOptions {
            reduce_motion: true,
            ..DisplayOptions::default()
        }
    );
    assert!(!client.session_locked());
    let mut stash = Stash::new();
    control.send(Control::SessionChange).unwrap();
    let mut changes = Vec::new();
    wait_for(&mut client, &mut stash, |e| {
        match e {
            Event::Appearance { dark } => changes.push(format!("dark={dark}")),
            Event::DisplayOptions(o) => changes.push(format!(
                "motion={} transparency={} contrast={}",
                o.reduce_motion, o.reduce_transparency, o.increase_contrast
            )),
            Event::SessionLock { locked } => changes.push(format!("locked={locked}")),
            Event::LockRequested { explicit } => changes.push(format!("lock requested={explicit}")),
            _ => {}
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
    assert!(!client.is_dark());
    assert_eq!(
        client.display_options(),
        DisplayOptions {
            reduce_transparency: true,
            increase_contrast: true,
            ..DisplayOptions::default()
        }
    );
    assert!(!client.session_locked());
}

#[test]
fn the_public_connection_follows_the_appearance_but_not_the_lock_and_binds_no_private_object() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, _committed, seen) = spawn_server_reporting(server_stream, true);
    let mut client =
        Client::from_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    assert!(client.is_dark());
    assert!(client.display_options().reduce_motion);
    let mut stash = Stash::new();
    control.send(Control::SessionChange).unwrap();
    let mut changes = Vec::new();
    wait_for(&mut client, &mut stash, |e| {
        match e {
            Event::Appearance { dark } => changes.push(format!("dark={dark}")),
            Event::DisplayOptions(o) => changes.push(format!(
                "transparency={} contrast={}",
                o.reduce_transparency, o.increase_contrast
            )),
            _ => {}
        }
        (changes.len() == 2).then_some(())
    });
    assert_eq!(changes, ["dark=false", "transparency=true contrast=true"]);
    assert!(!client.session_locked());
    let output = client.outputs()[0].id;
    let surface = client
        .layer_surface(&LayerSurfaceConfig {
            namespace: "public".to_string(),
            output: Some(output),
            layer: Layer::Top,
            anchor: Anchor::Top,
            size: (100, 20),
            margin: Margin::default(),
            exclusive_zone: 0,
            keyboard: KeyboardInteractivity::None,
        })
        .unwrap();
    assert!(matches!(
        client.menu_bar(surface, output),
        Err(ClientError::MissingGlobal("cupertino_shell_v1"))
    ));
    assert!(matches!(
        client.dock(surface, output),
        Err(ClientError::MissingGlobal("cupertino_shell_v1"))
    ));
    assert!(
        seen.try_iter().all(|s| !matches!(s, Seen::ShellBound)),
        "the public connection bound cupertino_shell_v1"
    );
}

#[test]
fn a_unified_toplevel_asks_for_its_title_bar_style_and_moves_and_zooms_itself() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (_control, _committed, seen) = spawn_server(server_stream);
    let mut client =
        Client::from_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    let window = client
        .toplevel(&ToplevelConfig {
            app_id: "org.example.files".into(),
            title: "Home".into(),
            size: (920, 436),
            min_size: (445, 139),
            titlebar: TitlebarStyle::Unified,
        })
        .unwrap();
    let mut stash = Stash::new();
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Configured { surface, .. } if *surface == window).then_some(())
    });
    let requests: Vec<Seen> = (0..5)
        .map(|_| seen.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert_eq!(
        requests[4],
        Seen::TitlebarStyle(cupertino_app_effects_v1::TitlebarStyle::Unified)
    );

    // a window that has not been configured as maximized zooms; a move needs the press it answers
    client.toplevel_toggle_maximized(window).unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Maximize(true)
    );
    assert!(matches!(
        client.toplevel_move(window),
        Err(ClientError::NoInputSerial)
    ));
}

#[test]
fn toplevels_are_decorated_by_the_server_sized_scrolled_and_closed() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, seen) = spawn_server(server_stream);
    let mut client =
        Client::from_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    let window = client
        .toplevel(&ToplevelConfig {
            app_id: "org.example.files".into(),
            title: "Home".into(),
            size: (920, 436),
            min_size: (445, 139),
            titlebar: TitlebarStyle::Standard,
        })
        .unwrap();
    let mut stash = Stash::new();

    // the first configure suggests no size: the window takes the requested one, inactive
    let size = wait_for(&mut client, &mut stash, |e| match e {
        Event::Configured { surface, size } if *surface == window => Some(*size),
        _ => None,
    });
    assert_eq!(size, (920, 436));
    assert!(!client.toplevel_activated(window).unwrap());
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
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Draw(s) if *s == window).then_some(())
    });
    client.draw(window, |_, _| {}).unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    // the mock's output has scale 2
    assert_eq!(
        committed
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .buffer_size,
        (1840, 872)
    );
    client.set_title(window, "Documents").unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Title("Documents".into())
    );
    assert!(matches!(
        client.set_exclusive_zone(window, 0),
        Err(ClientError::NotLayerSurface)
    ));

    // a later configure resizes and activates
    control.send(Control::FrameDone).unwrap();
    control.send(Control::ResizeToplevel).unwrap();
    let size = wait_for(&mut client, &mut stash, |e| match e {
        Event::Configured { surface, size } if *surface == window => Some(*size),
        _ => None,
    });
    assert_eq!(size, (640, 480));
    assert!(client.toplevel_activated(window).unwrap());
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Draw(s) if *s == window).then_some(())
    });
    client.draw(window, |_, _| {}).unwrap();
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    assert_eq!(
        committed
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .buffer_size,
        (1280, 960)
    );

    // scrolling over it arrives as pointer events, one per axis event, with the count of its frame
    control.send(Control::ScrollToplevel).unwrap();
    let mut scrolls = Vec::new();
    while scrolls.len() < 3 {
        scrolls.push(wait_for(&mut client, &mut stash, |e| match e {
            Event::Pointer {
                surface,
                event: PointerEvent::Scroll { dx, dy, count, .. },
            } if *surface == window => Some((*dx, *dy, *count)),
            _ => None,
        }));
    }
    assert_eq!(scrolls, [(0.0, 15.0, 3), (0.0, 5.0, 0), (-2.0, 0.0, 7)]);

    // the close request is the application's to act on
    control.send(Control::CloseToplevel).unwrap();
    wait_for(&mut client, &mut stash, |e| {
        matches!(e, Event::Closed(s) if *s == window).then_some(())
    });
    client.destroy_surface(window).unwrap();
}

#[test]
fn a_paste_reads_another_clients_text_selection() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, _committed, _seen) = spawn_server(server_stream);
    let mut client =
        Client::from_connection(Connection::from_socket(client_stream).unwrap()).unwrap();
    assert_eq!(client.clipboard_text().unwrap(), None);
    control
        .send(Control::OfferSelection("from elsewhere".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        assert!(Instant::now() < deadline, "the selection never arrived");
        client.wait(Some(Duration::from_millis(20))).unwrap();
        if let Some(text) = client.clipboard_text().unwrap() {
            break text;
        }
    };
    assert_eq!(text, "from elsewhere");
}

#[test]
fn copied_text_is_offered_with_the_input_serial_and_served_to_a_paste() {
    let (mut client, _control, _committed, seen, _surface, _stash) = connected_layer_surface();
    client.set_clipboard_text("copied text".into()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while !events.iter().any(|e| matches!(e, Seen::Served(_))) {
        assert!(
            Instant::now() < deadline,
            "the paste was never served: {events:?}"
        );
        client.wait(Some(Duration::from_millis(20))).unwrap();
        events.extend(
            seen.try_iter()
                .filter(|e| matches!(e, Seen::Selection { .. } | Seen::Served(_))),
        );
    }
    assert_eq!(
        events,
        vec![
            Seen::Selection {
                serial: 6,
                types: vec![
                    "text/plain;charset=utf-8".into(),
                    "text/plain".into(),
                    "UTF8_STRING".into()
                ],
            },
            Seen::Served("copied text".into()),
        ]
    );
    assert_eq!(
        client.clipboard_text().unwrap().as_deref(),
        Some("copied text"),
        "a paste in the copying client reads its own text"
    );
}

#[test]
fn a_damaged_frame_keeps_the_last_one_repaints_its_rectangles_and_reports_only_them() {
    let (mut client, control, committed, _seen, surface, mut stash) = connected_layer_surface();
    while committed.try_recv().is_ok() {}
    let red = tiny_skia::Color::from_rgba8(255, 0, 0, 255);
    let blue = tiny_skia::Color::from_rgba8(0, 0, 255, 255);
    client.draw(surface, |pixmap, _| pixmap.fill(red)).unwrap();
    // the wait sends the commit
    stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
    let mut frames = vec![committed.recv_timeout(Duration::from_secs(5)).unwrap()];
    // a rectangle in points; the output is at scale 2
    let rect = Rect::new(10.0, 5.25, 4.0, 3.0);
    for damage in [vec![rect], vec![]] {
        client.request_redraw(surface).unwrap();
        control.send(Control::FrameDone).unwrap();
        wait_for(&mut client, &mut stash, |e| {
            matches!(e, Event::Draw(_)).then_some(())
        });
        client
            .draw_damage(surface, &damage, |pixmap, _| {
                let at = |x, y| pixmap.pixel(x, y).unwrap().demultiply();
                assert_eq!(
                    at(0, 0),
                    red.to_color_u8(),
                    "outside the damage: the last frame"
                );
                if !damage.is_empty() {
                    assert_eq!(at(20, 10).alpha(), 0, "inside: cleared");
                    assert_eq!(at(27, 16).alpha(), 0, "inside: cleared");
                    assert_eq!(at(28, 16), red.to_color_u8(), "right of it: kept");
                    let r = tiny_skia::Rect::from_xywh(20.0, 10.0, 8.0, 7.0).unwrap();
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color(blue);
                    pixmap.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
                }
            })
            .unwrap();
        stash.extend(client.wait(Some(Duration::from_millis(50))).unwrap());
        frames.push(committed.recv_timeout(Duration::from_secs(5)).unwrap());
    }
    let damage: Vec<_> = frames.iter().map(|c| c.damage.clone()).collect();
    assert_eq!(
        damage,
        vec![
            vec![[0, 0, i32::MAX, i32::MAX]],
            vec![[20, 10, 8, 7]],
            vec![]
        ],
        "a whole frame, then the rectangle rounded out to device pixels, then nothing"
    );
    // the second buffer was never written: it got the whole frame, red at its corner
    assert_eq!(frames[1].first_pixel, [0, 0, 255, 255]);
    assert_ne!(frames[0].buffer, frames[1].buffer, "the other buffer");
}

/// A shell process that connected over the `WAYLAND_SOCKET` the server gave it does not pass that connection on: a
/// program it starts afterwards has no such variable.
#[test]
fn a_program_started_after_the_shell_connection_has_no_wayland_socket() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (_control, _committed, _seen) = spawn_server(server_stream);
    std::env::set_var("WAYLAND_SOCKET", client_stream.into_raw_fd().to_string());
    let _client = Client::connect_shell().unwrap();
    let output = std::process::Command::new("sh")
        .args(["-c", "printf %s \"${WAYLAND_SOCKET-unset}\""])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "unset");
}
