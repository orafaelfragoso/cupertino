//! Runs the presenter and the text clipboard on the transport against a minimal in-process server (shm, an output at
//! scale 2, a seat with a pointer, one window, the clipboard) and checks what the server receives.

use cupertino_app_protocols::server::app_effects::cupertino_app_effects_v1::{
    self, CupertinoAppEffectsV1,
};
use cupertino_app_protocols::server::app_effects::cupertino_app_menu_v1::{
    self, CupertinoAppMenuV1,
};
use cupertino_app_protocols::server::app_effects::cupertino_appearance_v1::{
    CupertinoAppearanceV1, DisplayOption,
};
use cupertino_app_protocols::server::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1,
};
use cupertino_geometry::Rect;
use cupertino_ui_wayland::clipboard::Clipboard;
use cupertino_ui_wayland::{rgba_to_argb8888, Ready, ShmPresenter};
use cupertino_wayland_client::{
    Connection, Decorations, Event, PointerEvent, Scale, Toplevel, ToplevelConfig,
};
use memmap2::Mmap;
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_protocols::wp::presentation_time::server::wp_presentation::{self, WpPresentation};
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::{
    Kind, WpPresentationFeedback,
};
use wayland_protocols::wp::single_pixel_buffer::v1::server::wp_single_pixel_buffer_manager_v1::{
    self, WpSinglePixelBufferManagerV1,
};
use wayland_protocols::wp::viewporter::server::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::server::wp_viewporter::{self, WpViewporter};
use wayland_protocols::xdg::activation::v1::server::xdg_activation_token_v1::{
    self, XdgActivationTokenV1,
};
use wayland_protocols::xdg::activation::v1::server::xdg_activation_v1::{self, XdgActivationV1};
use wayland_protocols::xdg::shell::server::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel;
use wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_callback::WlCallback;
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_data_device::{self, WlDataDevice};
use wayland_server::protocol::wl_data_device_manager::{self, WlDataDeviceManager};
use wayland_server::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_server::protocol::wl_data_source::{self, WlDataSource};
use wayland_server::protocol::wl_keyboard::WlKeyboard;
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_pointer::{self, WlPointer};
use wayland_server::protocol::wl_region::{self, WlRegion};
use wayland_server::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::protocol::wl_touch::WlTouch;
use wayland_server::{
    Client as ServerClient, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New,
    Resource,
};

/// The size the window asks for; the server leaves the size to it.
const SIZE: (u32, u32) = (100, 50);

/// What the server saw in one surface commit that carried a buffer.
#[derive(Debug, PartialEq)]
struct Committed {
    buffer_size: (i32, i32),
    stride: i32,
    /// The protocol id of the committed `wl_buffer`.
    buffer: u32,
    buffer_scale: i32,
    first_pixel: [u8; 4],
    pixels: Pixels,
    /// The `damage_buffer` rectangles of the commit (x, y, width, height).
    damage: Vec<[i32; 4]>,
    /// The input region set since the last commit, when one was: its rectangles, or `None` for the whole surface.
    input_region: Option<Option<Vec<[i32; 4]>>>,
}

/// The bytes of a committed buffer.
#[derive(Clone, PartialEq)]
struct Pixels(Vec<u8>);

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({} bytes)", self.0.len())
    }
}

enum Control {
    /// Answer the frame callbacks of the committed surface.
    FrameDone,
    /// Enter the surface with the pointer, move, and press the left button.
    PointerPress,
    /// A finger touches the surface and lifts.
    Tap,
    /// Another client's text becomes the selection: offer it to the client's data device.
    OfferSelection(String),
    /// Keep every buffer the next commits replace.
    HoldBuffers,
    /// Release the held buffers and release replaced ones again.
    ReleaseBuffers,
    /// Tell the oldest presentation feedback its frame turned visible this long ago, on `CLOCK_MONOTONIC`.
    Shown(Duration),
    /// Tell the oldest presentation feedback its frame was discarded.
    Discard,
    /// Name the bus connection that reads the window's menus.
    MenuReader(String),
    /// Ask the window to close.
    Close,
    /// End the client's drag without a drop, as a release over nothing that accepts it does.
    CancelDrag,
    /// Another client's drag of this URI list, copy or move, enters the window at (3, 4) and moves to (8, 9).
    DragIn(String),
    /// Then it drops there.
    DropIn,
    /// The keyboard enters the window's surface.
    KeyboardEnter,
    /// The keyboard leaves it.
    KeyboardLeave,
    /// Configure the window again, still leaving the size to the client and with no states.
    Reconfigure,
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
    /// The window announced its menus on this bus connection, the menu bar at this path.
    MenuAddress {
        bus_name: String,
        menubar: String,
    },
    MenuWithdrawn,
    /// `wl_data_device.start_drag` with the press's serial, the source's types, and whether it named an icon surface.
    DragStarted {
        serial: u32,
        types: Vec<String>,
        icon: bool,
    },
    /// The drag's icon surface committed a buffer of `size` pixels at `scale` with the pointer at `hotspot` points (the
    /// negated `wl_surface.offset`); its first pixel's bytes in memory.
    IconCommitted {
        size: (i32, i32),
        scale: i32,
        hotspot: (i32, i32),
        first_pixel: [u8; 4],
    },
    /// The client made a backdrop under its window.
    BackdropMade,
    /// A backdrop of the window took this shape (x, y, width, height).
    BackdropShape([i32; 4]),
    BackdropDestroyed,
    /// A drag over the window was accepted in this type (`None`: refused).
    DropAccepted(Option<String>),
    /// It was given these actions and this preferred one.
    DropActions {
        actions: u32,
        preferred: u32,
    },
    /// Its drop was finished.
    DropFinished,
    /// Its offer was destroyed.
    DropDestroyed,
    /// A commit attached a one-pixel buffer of `color` (premultiplied, over the whole `u32` range).
    SolidCommitted {
        color: [u32; 4],
        /// The viewport's destination then, in points.
        destination: Option<(i32, i32)>,
        buffer_scale: i32,
        damage: Vec<[i32; 4]>,
        /// The input region set since the last commit, when one was.
        input_region: Option<Option<Vec<[i32; 4]>>>,
    },
}

#[derive(Default)]
struct SurfaceState {
    buffer: Option<WlBuffer>,
    damage: Vec<[i32; 4]>,
    scale: i32,
    frames: Vec<WlCallback>,
    input_region: Option<Option<Vec<[i32; 4]>>>,
    offset: (i32, i32),
    /// The destination of the surface's viewport.
    destination: Option<(i32, i32)>,
}

type SurfaceData = Arc<Mutex<SurfaceState>>;

/// The colour of a one-pixel buffer.
struct SolidPixel([u32; 4]);

struct BufferData {
    map: Arc<Mmap>,
    offset: i32,
    width: i32,
    height: i32,
    stride: i32,
}

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
    outputs: Vec<WlOutput>,
    pointer: Option<WlPointer>,
    touch: Option<WlTouch>,
    keyboard: Option<WlKeyboard>,
    pending_frames: Vec<WlCallback>,
    shown: Option<WlBuffer>,
    /// Replaced buffers kept unreleased, while holding.
    held: Option<Vec<WlBuffer>>,
    toplevel: Option<ToplevelState>,
    data_device: Option<WlDataDevice>,
    /// The read end of a paste from the client's selection, and what arrived so far.
    pasting: Option<(std::io::PipeReader, Vec<u8>)>,
    /// Presentation feedback not yet answered, oldest first.
    feedbacks: VecDeque<WpPresentationFeedback>,
    menu: Option<CupertinoAppMenuV1>,
    /// The source and icon surface of the client's drag.
    drag: Option<(WlDataSource, Option<WlSurface>)>,
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
    WlDataDeviceManager,
    CupertinoAppEffectsV1,
    WpViewporter,
    WpSinglePixelBufferManagerV1
);

impl Dispatch<WpViewporter, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WpViewporter,
        request: wp_viewporter::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wp_viewporter::Request::GetViewport { id, surface } = request {
            init.init(id, surface.data::<SurfaceData>().unwrap().clone());
        }
    }
}

impl Dispatch<WpViewport, SurfaceData> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WpViewport,
        request: wp_viewport::Request,
        data: &SurfaceData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wp_viewport::Request::SetDestination { width, height } = request {
            data.lock().unwrap().destination = (width > 0).then_some((width, height));
        }
    }
}

impl Dispatch<WpSinglePixelBufferManagerV1, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WpSinglePixelBufferManagerV1,
        request: wp_single_pixel_buffer_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wp_single_pixel_buffer_manager_v1::Request::CreateU32RgbaBuffer { id, r, g, b, a } =
            request
        {
            init.init(id, SolidPixel([r, g, b, a]));
        }
    }
}

/// The session is locked: an observer that can tell (version 5) is told so at once. The menu bar shows a window's
/// menus.
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
            cupertino_app_effects_v1::Request::GetAppearance { id } => {
                let observer = init.init(id, ());
                observer.appearance(0);
                observer.display_options(DisplayOption::empty());
                if observer.version() >= 5 {
                    observer.session_lock(1);
                }
            }
            cupertino_app_effects_v1::Request::GetMenu { id, .. } => {
                let menu = init.init(id, ());
                menu.menu_bar(1);
                state.menu = Some(menu);
            }
            cupertino_app_effects_v1::Request::GetBackdrop { id, .. } => {
                init.init(id, ());
                state.seen.send(Seen::BackdropMade).unwrap();
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoAppMenuV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &CupertinoAppMenuV1,
        request: cupertino_app_menu_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            cupertino_app_menu_v1::Request::SetAddress {
                bus_name,
                menubar_object_path,
                ..
            } => Seen::MenuAddress {
                bus_name,
                menubar: menubar_object_path,
            },
            cupertino_app_menu_v1::Request::Destroy => Seen::MenuWithdrawn,
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}

impl Dispatch<CupertinoBackdropV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &CupertinoBackdropV1,
        request: cupertino_backdrop_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            cupertino_backdrop_v1::Request::SetShape {
                x,
                y,
                width,
                height,
                ..
            } => Seen::BackdropShape([x, y, width, height]),
            cupertino_backdrop_v1::Request::Destroy => Seen::BackdropDestroyed,
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}

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
    WlBuffer: BufferData,
    WlBuffer: SolidPixel,
    WlOutput: (),
    WlPointer: (),
    WlTouch: (),
    WlKeyboard: (),
    XdgToplevel: (),
    CupertinoAppearanceV1: ()
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
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                init.init(id, SurfaceData::default());
            }
            wl_compositor::Request::CreateRegion { id } => {
                init.init(id, Mutex::new(Vec::new()));
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegion, Mutex<Vec<[i32; 4]>>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlRegion,
        request: wl_region::Request,
        rects: &Mutex<Vec<[i32; 4]>>,
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
        let icon = state
            .drag
            .as_ref()
            .is_some_and(|(_, icon)| icon.as_ref() == Some(surface));
        match request {
            wl_surface::Request::Offset { x, y } => s.offset = (s.offset.0 + x, s.offset.1 + y),
            wl_surface::Request::Commit if icon => {
                if let Some(buffer) = s.buffer.take() {
                    let b = buffer.data::<BufferData>().expect("buffer data");
                    let start = b.offset as usize;
                    state
                        .seen
                        .send(Seen::IconCommitted {
                            size: (b.width, b.height),
                            scale: s.scale,
                            hotspot: (-s.offset.0, -s.offset.1),
                            first_pixel: b.map[start..start + 4].try_into().unwrap(),
                        })
                        .unwrap();
                }
            }
            wl_surface::Request::Attach { buffer, .. } => s.buffer = buffer,
            wl_surface::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => s.damage.push([x, y, width, height]),
            wl_surface::Request::SetBufferScale { scale } => s.scale = scale,
            wl_surface::Request::Frame { callback } => s.frames.push(init.init(callback, ())),
            wl_surface::Request::SetInputRegion { region } => {
                s.input_region = Some(region.map(|r| {
                    r.data::<Mutex<Vec<[i32; 4]>>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .clone()
                }));
            }
            wl_surface::Request::Commit => {
                let unbuffered = s.buffer.is_none();
                if let Some(t) = state
                    .toplevel
                    .as_mut()
                    .filter(|t| !t.configured && unbuffered && t.surface == *surface)
                {
                    // initial commit of the window: place it on the output and leave the size to the client
                    t.configured = true;
                    surface.enter(&state.outputs[0]);
                    state.next_serial += 1;
                    t.toplevel.configure(0, 0, Vec::new());
                    t.xdg.configure(state.next_serial);
                }
                if let Some(pixel) = s.buffer.as_ref().and_then(|b| b.data::<SolidPixel>()) {
                    state
                        .seen
                        .send(Seen::SolidCommitted {
                            color: pixel.0,
                            destination: s.destination,
                            buffer_scale: s.scale,
                            damage: std::mem::take(&mut s.damage),
                            input_region: s.input_region.take(),
                        })
                        .unwrap();
                    let buffer = s.buffer.take().expect("checked above");
                    if let Some(previous) = state.shown.replace(buffer) {
                        previous.release();
                    }
                    state.pending_frames.append(&mut s.frames);
                } else if let Some(buffer) = s.buffer.take() {
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
                            pixels: Pixels(
                                b.map[start..start + (b.stride * b.height) as usize].to_vec(),
                            ),
                            damage: std::mem::take(&mut s.damage),
                            input_region: s.input_region.take(),
                        })
                        .unwrap();
                    // like a compositor that keeps the current buffer until the next one replaces it
                    if let Some(previous) = state.shown.replace(buffer) {
                        match &mut state.held {
                            Some(held) => held.push(previous),
                            None => previous.release(),
                        }
                    }
                    state.pending_frames.append(&mut s.frames);
                }
            }
            _ => {}
        }
    }
}

impl GlobalDispatch<WpPresentation, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WpPresentation>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ())
            .clock_id(rustix::time::ClockId::Monotonic as u32);
    }
}

impl Dispatch<WpPresentation, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WpPresentation,
        request: wp_presentation::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wp_presentation::Request::Feedback { callback, .. } = request {
            state.feedbacks.push_back(init.init(callback, ()));
        }
    }
}

impl Dispatch<WpPresentationFeedback, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WpPresentationFeedback,
        _: <WpPresentationFeedback as Resource>::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
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
        init.init(r, ())
            .capabilities(Capability::Pointer | Capability::Touch | Capability::Keyboard);
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
        match request {
            wl_seat::Request::GetPointer { id } => state.pointer = Some(init.init(id, ())),
            wl_seat::Request::GetTouch { id } => state.touch = Some(init.init(id, ())),
            wl_seat::Request::GetKeyboard { id } => state.keyboard = Some(init.init(id, ())),
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
            init.init(id, Mutex::new(None::<String>));
        }
    }
}
/// A token names the application it was asked for: `token-for-<app_id>`, or `token-for-nothing`.
impl Dispatch<XdgActivationTokenV1, Mutex<Option<String>>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        token: &XdgActivationTokenV1,
        request: xdg_activation_token_v1::Request,
        app_id: &Mutex<Option<String>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_activation_token_v1::Request::SetAppId { app_id: id } => {
                *app_id.lock().unwrap() = Some(id)
            }
            xdg_activation_token_v1::Request::Commit => {
                let named = app_id.lock().unwrap().clone();
                token.done(format!(
                    "token-for-{}",
                    named.as_deref().unwrap_or("nothing")
                ))
            }
            _ => {}
        }
    }
}
impl GlobalDispatch<XdgWmBase, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<XdgWmBase>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ());
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
        if let wl_data_device::Request::StartDrag {
            source: Some(source),
            icon,
            serial,
            ..
        } = request
        {
            let types = source
                .data::<Mutex<Vec<String>>>()
                .unwrap()
                .lock()
                .unwrap()
                .clone();
            state
                .seen
                .send(Seen::DragStarted {
                    serial,
                    types,
                    icon: icon.is_some(),
                })
                .unwrap();
            // the drag takes the pointer, as a compositor's does
            if let (Some(pointer), Some(t)) = (&state.pointer, &state.toplevel) {
                pointer.leave(state.next_serial + 100, &t.surface);
            }
            state.drag = Some((source, icon));
            return;
        }
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
        state: &mut Self,
        _: &ServerClient,
        _: &WlDataOffer,
        request: wl_data_offer::Request,
        text: &String,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_offer::Request::Receive { fd, .. } => {
                std::fs::File::from(fd).write_all(text.as_bytes()).unwrap()
            }
            wl_data_offer::Request::Accept { mime_type, .. } => {
                let _ = state.seen.send(Seen::DropAccepted(mime_type));
            }
            wl_data_offer::Request::SetActions {
                dnd_actions,
                preferred_action,
            } => {
                let _ = state.seen.send(Seen::DropActions {
                    actions: u32::from(dnd_actions),
                    preferred: u32::from(preferred_action),
                });
            }
            wl_data_offer::Request::Finish => {
                let _ = state.seen.send(Seen::DropFinished);
            }
            wl_data_offer::Request::Destroy => {
                let _ = state.seen.send(Seen::DropDestroyed);
            }
            _ => {}
        }
    }
}

/// Run the server on a thread until the test drops the control channel.
fn spawn_server(stream: UnixStream) -> (Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
    spawn_server_with(stream, None)
}

/// [`spawn_server`], offering `cupertino_app_effects_v1` at `effects` when it is some.
fn spawn_server_with(
    stream: UnixStream,
    effects: Option<u32>,
) -> (Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
    spawn_server_offering(stream, effects, false)
}

/// [`spawn_server_with`], offering `wp_viewporter` and `wp_single_pixel_buffer_manager_v1` when `solid`.
fn spawn_server_offering(
    stream: UnixStream,
    effects: Option<u32>,
    solid: bool,
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
        dh.create_global::<Mock, WlDataDeviceManager, _>(3, ());
        dh.create_global::<Mock, WpPresentation, _>(1, ());
        dh.create_global::<Mock, XdgActivationV1, _>(1, ());
        if let Some(version) = effects {
            dh.create_global::<Mock, CupertinoAppEffectsV1, _>(version, ());
        }
        if solid {
            dh.create_global::<Mock, WpViewporter, _>(1, ());
            dh.create_global::<Mock, WpSinglePixelBufferManagerV1, _>(1, ());
        }
        let mut mock = Mock {
            committed: committed_tx,
            seen: seen_tx,
            next_serial: 1,
            outputs: Vec::new(),
            pointer: None,
            touch: None,
            keyboard: None,
            pending_frames: Vec::new(),
            shown: None,
            held: None,
            toplevel: None,
            data_device: None,
            pasting: None,
            feedbacks: VecDeque::new(),
            menu: None,
            drag: None,
        };
        let mut display_handle = display.handle();
        display_handle
            .insert_client(stream, Arc::new(Data))
            .unwrap();
        loop {
            match control_rx.try_recv() {
                Ok(Control::FrameDone) => mock.pending_frames.drain(..).for_each(|cb| cb.done(0)),
                Ok(Control::HoldBuffers) => mock.held = Some(Vec::new()),
                Ok(Control::ReleaseBuffers) => mock
                    .held
                    .take()
                    .into_iter()
                    .flatten()
                    .for_each(|b| b.release()),
                Ok(Control::Shown(ago)) => {
                    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
                    let at = Duration::new(now.tv_sec as u64, now.tv_nsec as u32) - ago;
                    let seconds = at.as_secs();
                    mock.feedbacks.pop_front().unwrap().presented(
                        (seconds >> 32) as u32,
                        seconds as u32,
                        at.subsec_nanos(),
                        16_666_666,
                        0,
                        0,
                        Kind::Vsync,
                    );
                }
                Ok(Control::Discard) => mock.feedbacks.pop_front().unwrap().discarded(),
                Ok(Control::MenuReader(name)) => mock.menu.as_ref().unwrap().menu_reader(name),
                Ok(Control::Close) => mock.toplevel.as_ref().unwrap().toplevel.close(),
                Ok(Control::CancelDrag) => mock.drag.as_ref().unwrap().0.cancelled(),
                Ok(Control::DragIn(list)) => {
                    let device = mock.data_device.as_ref().unwrap();
                    let offer = device
                        .client()
                        .unwrap()
                        .create_resource::<WlDataOffer, String, Mock>(
                            &display_handle,
                            device.version(),
                            list,
                        )
                        .unwrap();
                    device.data_offer(&offer);
                    offer.offer("text/uri-list".into());
                    offer.source_actions(
                        wl_data_device_manager::DndAction::Copy
                            | wl_data_device_manager::DndAction::Move,
                    );
                    let surface = mock.toplevel.as_ref().unwrap().surface.clone();
                    device.enter(41, &surface, 3.0, 4.0, Some(&offer));
                    device.motion(50, 8.0, 9.0);
                    offer.action(wl_data_device_manager::DndAction::Move);
                }
                Ok(Control::DropIn) => mock.data_device.as_ref().unwrap().drop(),
                Ok(Control::KeyboardEnter) => {
                    let t = mock.toplevel.as_ref().unwrap();
                    mock.next_serial += 1;
                    let keyboard = mock.keyboard.as_ref().unwrap();
                    keyboard.enter(mock.next_serial, &t.surface, Vec::new());
                }
                Ok(Control::KeyboardLeave) => {
                    let t = mock.toplevel.as_ref().unwrap();
                    mock.next_serial += 1;
                    let keyboard = mock.keyboard.as_ref().unwrap();
                    keyboard.leave(mock.next_serial, &t.surface);
                }
                Ok(Control::Reconfigure) => {
                    let t = mock.toplevel.as_ref().unwrap();
                    mock.next_serial += 1;
                    t.toplevel.configure(0, 0, Vec::new());
                    t.xdg.configure(mock.next_serial);
                }
                Ok(Control::PointerPress) => {
                    let (pointer, t) = (
                        mock.pointer.as_ref().unwrap(),
                        mock.toplevel.as_ref().unwrap(),
                    );
                    pointer.enter(5, &t.surface, 1.5, 2.5);
                    pointer.motion(0, 10.25, 20.75);
                    pointer.button(6, 0, 0x110, wl_pointer::ButtonState::Pressed);
                }
                Ok(Control::Tap) => {
                    let (touch, t) = (
                        mock.touch.as_ref().unwrap(),
                        mock.toplevel.as_ref().unwrap(),
                    );
                    touch.down(7, 30, &t.surface, 3, 4.5, 5.5);
                    touch.frame();
                    touch.up(8, 40, 3);
                    touch.frame();
                }
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

/// What the client loop reports: a transport event, or what the presenter asks for after the events of a wait.
#[derive(Debug)]
enum Report {
    Event(Event),
    Ready(Ready),
}

/// The application side: one window drawn by a presenter, and the clipboard.
struct App {
    conn: Connection,
    window: Toplevel,
    presenter: ShmPresenter,
    clipboard: Clipboard,
    /// Reports the test has not looked at yet.
    stash: VecDeque<Report>,
}

impl App {
    /// Connect and create the window.
    fn connect(stream: UnixStream) -> App {
        let mut conn = Connection::from_stream(stream).unwrap();
        let window = conn
            .toplevel(&ToplevelConfig {
                app_id: "org.example.Test".into(),
                title: "Test".into(),
                min_size: None,
                max_size: None,
                decorations: Decorations::Client,
                foreign_parent: None,
            })
            .unwrap();
        App {
            presenter: ShmPresenter::new(window.surface()),
            conn,
            window,
            clipboard: Clipboard::default(),
            stash: VecDeque::new(),
        }
    }

    /// One wait of the transport: its events, with the presenter and the clipboard told, then the presenter's polls.
    fn wait(&mut self, timeout: Duration) -> Vec<Report> {
        let mut reports = Vec::new();
        for event in self.conn.wait(Some(timeout)).unwrap() {
            match &event {
                Event::Configure { window, configure } if *window == self.window => {
                    self.presenter.configured(configure.size.unwrap_or(SIZE))
                }
                Event::Frame(callback) if callback.surface == self.window.surface() => {
                    self.presenter.frame_done()
                }
                Event::Selection(offer) => self.clipboard.selection_changed(offer.clone()),
                _ => {}
            }
            reports.push(Report::Event(event));
        }
        loop {
            match self.presenter.poll(&self.conn).unwrap() {
                Ready::Idle => break,
                ready => reports.push(Report::Ready(ready)),
            }
        }
        reports
    }

    /// Send what was drawn: the wait flushes the requests.
    fn flush(&mut self) {
        let reports = self.wait(Duration::from_millis(50));
        self.stash.extend(reports);
    }

    /// Take reports in order, waiting for more when the stash runs dry, until `pick` accepts one.
    fn wait_for<T>(&mut self, mut pick: impl FnMut(&Report) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            while let Some(report) = self.stash.pop_front() {
                if let Some(found) = pick(&report) {
                    return found;
                }
            }
            let reports = self.wait(Duration::from_millis(50));
            self.stash.extend(reports);
        }
        panic!("timed out waiting for a report");
    }

    fn wait_for_draw(&mut self) {
        self.wait_for(|r| matches!(r, Report::Ready(Ready::Draw)).then_some(()));
    }
}

/// Connect, map the window once and give the client an input serial (6).
fn connected_window() -> (App, Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, seen) = spawn_server(server_stream);
    let mut app = App::connect(client_stream);
    app.wait_for_draw();
    control.send(Control::PointerPress).unwrap();
    app.wait_for(|r| {
        matches!(
            r,
            Report::Event(Event::Pointer {
                event: PointerEvent::Button { .. },
                ..
            })
        )
        .then_some(())
    });
    (app, control, committed, seen)
}

#[test]
fn a_frame_is_drawn_at_the_output_scale_in_wl_shm_byte_order_and_the_next_waits_for_the_frame_callback(
) {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, committed, _seen) = spawn_server(server_stream);
    let mut app = App::connect(client_stream);

    // the window asked for 100x50; the output is at scale 2, so the first draw is 200x100
    app.wait_for(|r| {
        matches!(r, Report::Ready(Ready::ScaleChanged(scale)) if *scale == Scale::from_integer(2).unwrap())
            .then_some(())
    });
    app.wait_for_draw();
    app.presenter
        .draw(&mut app.conn, None, |pixmap, scale| {
            assert_eq!(
                (pixmap.width(), pixmap.height(), scale),
                (200, 100, Scale::from_integer(2).unwrap())
            );
            // premultiplied RGBA (10, 20, 30, 40)
            pixmap.pixels_mut()[0] =
                tiny_skia::PremultipliedColorU8::from_rgba(10, 20, 30, 40).unwrap();
        })
        .unwrap();

    // the frame arrives whole, in wl_shm byte order (B, G, R, A)
    app.flush();
    let first = committed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        first,
        Committed {
            buffer_size: (200, 100),
            stride: 800,
            buffer: first.buffer,
            buffer_scale: 2,
            first_pixel: [30, 20, 10, 40],
            pixels: first.pixels.clone(),
            damage: vec![[0, 0, i32::MAX, i32::MAX]],
            input_region: None,
        }
    );

    // no new frame is offered before the server answered the frame callback
    app.presenter.request_redraw();
    let early = app.wait(Duration::from_millis(100));
    assert!(
        !early
            .iter()
            .any(|r| matches!(r, Report::Ready(Ready::Draw))),
        "{early:?}"
    );
    control.send(Control::FrameDone).unwrap();
    app.wait_for_draw();

    // the second frame uses another buffer while the server holds the first
    app.presenter.draw(&mut app.conn, None, |_, _| {}).unwrap();
    app.flush();
    let second = committed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_ne!(second.buffer, first.buffer);
    assert_eq!(second.first_pixel, [0, 0, 0, 0]);
}

/// A presenter shows one colour as one pixel the compositor stretches over the window where it offers that, the frame
/// callback pacing it like a frame, and the next frame is drawn whole; where it does not, as a frame filled with the
/// colour, which the next frame keeps outside its damage.
#[test]
fn a_solid_colour_is_one_stretched_pixel_or_where_the_compositor_cannot_a_filled_frame() {
    for offered in [true, false] {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (control, committed, seen) = spawn_server_offering(server_stream, None, offered);
        let mut app = App::connect(client_stream);
        app.wait_for_draw();
        // straight half-transparent red
        app.presenter
            .draw_solid(&mut app.conn, [1.0, 0.0, 0.0, 0.5])
            .unwrap();
        app.flush();
        if offered {
            let half = u32::MAX / 2 + 1;
            assert_eq!(
                seen.recv_timeout(Duration::from_secs(5)).unwrap(),
                Seen::SolidCommitted {
                    color: [half, 0, 0, half],
                    destination: Some((100, 50)),
                    buffer_scale: 1,
                    damage: vec![[0, 0, 1, 1]],
                    input_region: None,
                }
            );
        } else {
            let filled = committed.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(filled.buffer_size, (200, 100));
            // premultiplied, in wl_shm byte order (B, G, R, A)
            assert_eq!(filled.first_pixel, [0, 0, 128, 128]);
        }
        let early = app.wait(Duration::from_millis(100));
        assert!(
            !early
                .iter()
                .any(|r| matches!(r, Report::Ready(Ready::Draw))),
            "the frame callback paces it: {early:?}"
        );
        control.send(Control::FrameDone).unwrap();
        app.presenter.request_redraw();
        app.wait_for_draw();
        app.presenter
            .draw(
                &mut app.conn,
                Some(&[Rect::new(0.0, 0.0, 1.0, 1.0)]),
                |_, _| {},
            )
            .unwrap();
        app.flush();
        let next = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        let damage = if offered {
            [0, 0, i32::MAX, i32::MAX]
        } else {
            [0, 0, 2, 2]
        };
        assert_eq!(next.damage, vec![damage], "offered {offered}");
    }
}

#[test]
fn a_paste_reads_another_clients_text_selection() {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (control, _committed, _seen) = spawn_server(server_stream);
    let mut app = App::connect(client_stream);
    assert_eq!(app.clipboard.text(&mut app.conn).unwrap(), None);
    control
        .send(Control::OfferSelection("from elsewhere".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        assert!(Instant::now() < deadline, "the selection never arrived");
        app.wait(Duration::from_millis(20));
        if let Some(text) = app.clipboard.text(&mut app.conn).unwrap() {
            break text;
        }
    };
    assert_eq!(text, "from elsewhere");
}

#[test]
fn copied_text_is_offered_with_the_input_serial_and_served_to_a_paste() {
    let (mut app, _control, _committed, seen) = connected_window();
    app.clipboard
        .set_text(&mut app.conn, "copied text".into())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while !events.iter().any(|e| matches!(e, Seen::Served(_))) {
        assert!(
            Instant::now() < deadline,
            "the paste was never served: {events:?}"
        );
        app.wait(Duration::from_millis(20));
        events.extend(seen.try_iter());
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
        app.clipboard.text(&mut app.conn).unwrap().as_deref(),
        Some("copied text"),
        "a paste in the copying client reads its own text"
    );
}

#[test]
fn a_damaged_frame_keeps_the_last_one_repaints_its_rectangles_and_reports_only_them() {
    let (mut app, control, committed, _seen) = connected_window();
    while committed.try_recv().is_ok() {}
    let red = tiny_skia::Color::from_rgba8(255, 0, 0, 255);
    let blue = tiny_skia::Color::from_rgba8(0, 0, 255, 255);
    app.presenter
        .draw(&mut app.conn, None, |pixmap, _| pixmap.fill(red))
        .unwrap();
    app.flush();
    let mut frames = vec![committed.recv_timeout(Duration::from_secs(5)).unwrap()];
    // a rectangle in points; the output is at scale 2
    let rect = Rect::new(10.0, 5.25, 4.0, 3.0);
    for damage in [vec![rect], vec![]] {
        app.presenter.request_redraw();
        control.send(Control::FrameDone).unwrap();
        app.wait_for_draw();
        app.presenter
            .draw(&mut app.conn, Some(&damage), |pixmap, _| {
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
        app.flush();
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

/// A configure can change what a frame shows without changing its size (activation, maximized), so the frame after one
/// is whole even when the caller's damage is empty or partial.
#[test]
fn a_configure_makes_the_next_frame_whole_whatever_its_damage() {
    let (mut app, control, committed, _seen) = connected_window();
    while committed.try_recv().is_ok() {}
    let red = tiny_skia::Color::from_rgba8(255, 0, 0, 255);
    app.presenter
        .draw(&mut app.conn, None, |pixmap, _| pixmap.fill(red))
        .unwrap();
    app.flush();
    committed.recv_timeout(Duration::from_secs(5)).unwrap();
    for damage in [vec![], vec![Rect::new(10.0, 5.25, 4.0, 3.0)]] {
        let size = app.presenter.size().unwrap();
        app.presenter.configured(size);
        control.send(Control::FrameDone).unwrap();
        app.wait_for_draw();
        let mut cleared = None;
        app.presenter
            .draw_with(&mut app.conn, Some(&damage), |pixmap, _, rects| {
                cleared = Some(rects.map(<[_]>::len));
                assert_eq!(
                    pixmap.pixel(0, 0).unwrap().alpha(),
                    0,
                    "the frame is cleared"
                );
            })
            .unwrap();
        assert_eq!(
            cleared,
            Some(None),
            "a whole frame after a configure: {damage:?}"
        );
        app.flush();
        let frame = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(frame.damage, vec![[0, 0, i32::MAX, i32::MAX]]);
    }
    // the configure is answered once: the next damaged frame is damaged again
    app.presenter.request_redraw();
    control.send(Control::FrameDone).unwrap();
    app.wait_for_draw();
    let mut cleared = None;
    app.presenter
        .draw_with(&mut app.conn, Some(&[]), |_, _, rects| {
            cleared = Some(rects.map(<[_]>::len));
        })
        .unwrap();
    assert_eq!(cleared, Some(Some(0)));
}

/// The server holds the buffer on screen, so frames take turns between two buffers. A damaged frame converts only its
/// rectangles and what its buffer missed of the frame before, and every committed buffer equals the whole frame.
#[test]
fn buffers_taking_turns_hold_every_damaged_frame_exactly() {
    let (mut app, control, committed, _seen) = connected_window();
    while committed.try_recv().is_ok() {}
    let shade = |i: u8| tiny_skia::Color::from_rgba8(i.wrapping_mul(40), 200 - i, i, 255);
    let r = Rect::new;
    // `None` draws a whole frame
    let frames = [
        None,
        Some(vec![r(1.0, 1.0, 4.0, 3.0)]),
        Some(vec![r(50.0, 20.0, 30.0, 10.0), r(0.0, 0.0, 2.0, 2.0)]),
        Some(vec![]),
        Some(vec![r(90.0, 40.0, 20.0, 20.0)]),
        None,
        Some(vec![r(10.5, 7.25, 3.0, 12.0)]),
        Some(vec![r(1.0, 1.0, 4.0, 3.0)]),
        Some(vec![r(30.0, 60.0, 8.0, 8.0)]),
    ];
    let mut shown: Vec<Committed> = Vec::new();
    for (i, damage) in frames.iter().enumerate() {
        if i > 0 {
            app.presenter.request_redraw();
            control.send(Control::FrameDone).unwrap();
            app.wait_for_draw();
        }
        let mut frame = Vec::new();
        let paint = |pixmap: &mut tiny_skia::Pixmap, scale: Scale| {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(shade(i as u8));
            let to_device =
                tiny_skia::Transform::from_scale(scale.factor() as f32, scale.factor() as f32);
            match damage {
                None => pixmap.fill(shade(i as u8)),
                Some(rects) => {
                    for d in rects {
                        let d = tiny_skia::Rect::from_xywh(
                            d.x() as f32,
                            d.y() as f32,
                            d.width() as f32,
                            d.height() as f32,
                        )
                        .unwrap();
                        pixmap.fill_rect(d, &paint, to_device, None);
                    }
                }
            }
            frame = pixmap.data().to_vec();
        };
        app.presenter
            .draw(&mut app.conn, damage.as_deref(), paint)
            .unwrap();
        app.flush();
        let c = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut whole = vec![0; frame.len()];
        rgba_to_argb8888(&frame, &mut whole);
        assert!(
            c.pixels.0 == whole,
            "frame {i}: the buffer differs from the frame"
        );
        if let Some(last) = shown.last() {
            assert_ne!(c.buffer, last.buffer, "frame {i}: the buffer on screen");
        }
        shown.push(c);
    }
    assert_eq!(
        shown
            .iter()
            .map(|c| c.buffer)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2,
        "two buffers take turns"
    );
}

mod window_system {
    use super::*;
    use cupertino_geometry::{Point, Size};
    use cupertino_modifiers::Profile;
    use cupertino_ui_platform::{
        ClipboardKind, Color, DecorationMode, MaterialRegion, MenuDescription, MouseButton,
        PlatformEvent, PointerEvent, PresentOutcome, TouchEvent, WindowEvent, WindowId,
        WindowRequest, WindowSpec, WindowSystem,
    };
    use cupertino_ui_render::{CpuFrame, PixelFormat, PixelRect, RenderFrameId};
    use cupertino_ui_wayland::WaylandWindowSystem;
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedValue, Value};

    struct Kit {
        ws: WaylandWindowSystem,
        window: WindowId,
        stash: VecDeque<PlatformEvent>,
    }

    impl Kit {
        fn wait_for<T>(&mut self, mut pick: impl FnMut(&PlatformEvent) -> Option<T>) -> T {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                while let Some(event) = self.stash.pop_front() {
                    if let Some(found) = pick(&event) {
                        return found;
                    }
                }
                let mut events = Vec::new();
                self.ws
                    .dispatch(Some(Duration::from_millis(50)), &mut events)
                    .unwrap();
                self.stash.extend(events);
            }
            panic!("timed out waiting for an event");
        }

        fn window_event<T>(&mut self, mut pick: impl FnMut(&WindowEvent) -> Option<T>) -> T {
            let window = self.window;
            self.wait_for(|e| match e {
                PlatformEvent::Window { window: w, event } if *w == window => pick(event),
                _ => None,
            })
        }

        /// Dispatch once without waiting, so the requests go out and what arrived is read.
        fn pump(&mut self) {
            let mut events = Vec::new();
            self.ws
                .dispatch(Some(Duration::from_millis(20)), &mut events)
                .unwrap();
            self.stash.extend(events);
        }

        /// Whether the stashed events hold a frame for the window; empties the stash.
        fn took_frame(&mut self) -> bool {
            std::mem::take(&mut self.stash).iter().any(|e| {
                matches!(
                    e,
                    PlatformEvent::Window {
                        event: WindowEvent::Frame(_),
                        ..
                    }
                )
            })
        }
    }

    /// A window mapped at the output's scale 2, so its frames are 200 × 100.
    fn mapped() -> (Kit, Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
        let (mut kit, control, committed, seen) = created();
        let configure = kit.window_event(|e| match e {
            WindowEvent::Configure(c) => Some(*c),
            _ => None,
        });
        assert_eq!(
            (configure.size, configure.server_decorations),
            (None, false)
        );
        assert_eq!(
            kit.window_event(|e| match e {
                WindowEvent::Scale(s) => Some(*s),
                _ => None,
            }),
            2.0
        );
        (kit, control, committed, seen)
    }

    /// A window created but not yet configured.
    fn created() -> (Kit, Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
        created_with(None)
    }

    /// [`created`] on a server that offers the app effects at `effects`.
    fn created_with(
        effects: Option<u32>,
    ) -> (Kit, Sender<Control>, Receiver<Committed>, Receiver<Seen>) {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (control, committed, seen) = spawn_server_with(server_stream, effects);
        let mut ws = WaylandWindowSystem::new(
            Connection::from_stream(client_stream).unwrap(),
            Profile::cupertino(),
        );
        let window = ws
            .create_window(&WindowSpec {
                title: "Test".into(),
                app_id: "org.example.Test".into(),
                size: Size::new(100.0, 50.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Client,
                foreign_parent: None,
                input_region: None,
            })
            .unwrap();
        let kit = Kit {
            ws,
            window,
            stash: VecDeque::new(),
        };
        (kit, control, committed, seen)
    }

    fn frame(id: u64, first: [u8; 4], damage: &[PixelRect]) -> CpuFrame {
        frame_sized(id, (200, 100), first, damage)
    }

    fn frame_sized(
        id: u64,
        (width, height): (u32, u32),
        first: [u8; 4],
        damage: &[PixelRect],
    ) -> CpuFrame {
        let mut pixels = vec![0; (width * height * 4) as usize];
        pixels[..4].copy_from_slice(&first);
        CpuFrame {
            id: RenderFrameId(id),
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels,
            damage: damage.to_vec(),
        }
    }

    fn rect(x: u32, y: u32, width: u32, height: u32) -> PixelRect {
        PixelRect {
            x,
            y,
            width,
            height,
        }
    }

    fn notes_menus(disabled: &[&str]) -> WindowRequest {
        use cupertino_menu_model::{ActionName, Entry, Item, Menu, MenuBar, Scope};
        let item = |label: &str| {
            let action = ActionName::new(Scope::Win, &label.to_lowercase()).unwrap();
            Entry::Item(Item::new(label, action, None))
        };
        WindowRequest::Menus(Some(MenuDescription {
            bar: MenuBar {
                app: Menu::new("Test", Vec::new()),
                menus: vec![Menu::new("File", vec![vec![item("Save"), item("Zoom")]])],
            },
            disabled: disabled
                .iter()
                .map(|d| ActionName::new(Scope::Win, d).unwrap())
                .collect(),
        }))
    }

    fn seen_menu(seen: &Receiver<Seen>) -> Seen {
        loop {
            if let s @ (Seen::MenuAddress { .. } | Seen::MenuWithdrawn) =
                seen.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                return s;
            }
        }
    }

    /// The window's actions and whether each is enabled, as the menu bar reads them once the compositor has named it.
    fn described(
        kit: &mut Kit,
        reader: &zbus::blocking::Connection,
        bus_name: &str,
    ) -> Vec<(String, bool)> {
        type Described = HashMap<String, (bool, zbus::zvariant::Signature, Vec<OwnedValue>)>;
        let actions = zbus::blocking::Proxy::new(
            reader,
            bus_name.to_owned(),
            "/org/example/Test/window/1",
            "org.gtk.Actions",
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let all: Described = loop {
            kit.pump();
            match actions.call("DescribeAll", &()) {
                Ok(all) => break all,
                Err(e) => assert!(Instant::now() < deadline, "the reader was never named: {e}"),
            }
        };
        let mut all: Vec<(String, bool)> = all.into_iter().map(|(k, (e, _, _))| (k, e)).collect();
        all.sort();
        all
    }

    #[test]
    fn a_window_publishes_its_menus_once_configured_and_a_chosen_item_comes_back_as_its_action() {
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            eprintln!("skipping: dbus-daemon is not available");
            return;
        };
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address);
        let (mut kit, control, _committed, seen) = created_with(Some(3));
        let w = kit.window;
        kit.ws.request(w, notes_menus(&["save"])).unwrap();
        kit.window_event(|e| matches!(e, WindowEvent::Configure(_)).then_some(()));
        kit.pump();
        let Seen::MenuAddress { bus_name, menubar } = seen_menu(&seen) else {
            panic!("the menus were withdrawn before they were announced");
        };
        assert_eq!(menubar, "/org/example/Test/window/1/menus/menubar");

        let reader = bus.connect();
        control
            .send(Control::MenuReader(
                reader.unique_name().unwrap().to_string(),
            ))
            .unwrap();
        assert_eq!(
            described(&mut kit, &reader, &bus_name),
            [("save".to_string(), false), ("zoom".to_string(), true)]
        );

        let platform_data: HashMap<&str, Value<'_>> = HashMap::new();
        zbus::blocking::Proxy::new(
            &reader,
            bus_name.clone(),
            "/org/example/Test/window/1",
            "org.gtk.Actions",
        )
        .unwrap()
        .call::<_, _, ()>(
            "Activate",
            &("zoom", Vec::<Value<'_>>::new(), platform_data),
        )
        .unwrap();
        let chosen = kit.window_event(|e| match e {
            WindowEvent::MenuActivated(action) => Some(action.full()),
            _ => None,
        });
        assert_eq!(chosen, "win.zoom");

        kit.ws.request(w, notes_menus(&["zoom"])).unwrap();
        assert_eq!(
            described(&mut kit, &reader, &bus_name),
            [("save".to_string(), true), ("zoom".to_string(), false)]
        );

        kit.ws.request(w, WindowRequest::Menus(None)).unwrap();
        kit.pump();
        assert_eq!(seen_menu(&seen), Seen::MenuWithdrawn);
    }

    /// The next `n` backdrop requests the server saw, once the client has sent what it queued.
    fn backdrop_requests(kit: &mut Kit, seen: &Receiver<Seen>, n: usize) -> Vec<Seen> {
        kit.pump();
        let mut out = Vec::new();
        while out.len() < n {
            let s = seen.recv_timeout(Duration::from_secs(5)).unwrap();
            if matches!(
                s,
                Seen::BackdropMade | Seen::BackdropShape(_) | Seen::BackdropDestroyed
            ) {
                out.push(s);
            }
        }
        out
    }

    /// Each region under a window is a backdrop of `cupertino_app_effects_v1`: a shorter list keeps the first ones and
    /// destroys the rest, and a list past the compositor's limits for one surface (64) or one client (128) is refused
    /// before a backdrop is asked for, since the compositor would end the connection, and takes the window's away.
    #[test]
    fn a_window_s_materials_reuse_their_backdrops_and_a_list_past_the_compositor_s_limits_is_refused(
    ) {
        let (mut kit, _control, _committed, seen) = created_with(Some(5));
        let w = kit.window;
        let regions = |n: usize| {
            (0..n)
                .map(|i| MaterialRegion {
                    material: "Menu".into(),
                    rect: Rect::new(0.0, i as f64, 10.0, 10.0),
                    radius: 4.0,
                    alpha: 1.0,
                    glass: None,
                    raised: false,
                })
                .collect::<Vec<_>>()
        };
        let made = |n: usize| -> Vec<Seen> {
            (0..n)
                .flat_map(|i| {
                    [
                        Seen::BackdropMade,
                        Seen::BackdropShape([0, i as i32, 10, 10]),
                    ]
                })
                .collect()
        };

        kit.ws
            .request(w, WindowRequest::Materials(regions(2)))
            .unwrap();
        assert_eq!(backdrop_requests(&mut kit, &seen, 4), made(2));
        let mut moved = regions(1);
        moved[0].rect = Rect::new(0.0, 5.0, 10.0, 10.0);
        kit.ws.request(w, WindowRequest::Materials(moved)).unwrap();
        assert_eq!(
            backdrop_requests(&mut kit, &seen, 2),
            [Seen::BackdropDestroyed, Seen::BackdropShape([0, 5, 10, 10])],
            "the first backdrop takes the region and the second goes"
        );

        assert!(kit
            .ws
            .request(w, WindowRequest::Materials(regions(65)))
            .is_err());
        // what follows the refusal, up to a list that fits: the window's backdrop went and no other was made
        kit.ws
            .request(w, WindowRequest::Materials(regions(1)))
            .unwrap();
        let mut expected = vec![Seen::BackdropDestroyed];
        expected.extend(made(1));
        assert_eq!(backdrop_requests(&mut kit, &seen, 3), expected);

        // over the client's windows: 1 + 64 + 63 is all it may hold
        let spec = WindowSpec {
            title: "Other".into(),
            app_id: "org.example.Test".into(),
            size: Size::new(100.0, 50.0),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::Client,
            foreign_parent: None,
            input_region: None,
        };
        let second = kit.ws.create_window(&spec).unwrap();
        let third = kit.ws.create_window(&spec).unwrap();
        kit.ws
            .request(second, WindowRequest::Materials(regions(64)))
            .unwrap();
        assert_eq!(backdrop_requests(&mut kit, &seen, 128), made(64));
        assert!(kit
            .ws
            .request(third, WindowRequest::Materials(regions(64)))
            .is_err());
        kit.ws
            .request(third, WindowRequest::Materials(regions(63)))
            .unwrap();
        assert_eq!(
            backdrop_requests(&mut kit, &seen, 126),
            made(63),
            "the refused list made no backdrop"
        );
        assert!(
            kit.ws
                .request(w, WindowRequest::Materials(regions(2)))
                .is_err(),
            "one more than the client may hold"
        );
        // closing a window gives its backdrops back
        kit.ws.destroy(second);
        kit.ws
            .request(w, WindowRequest::Materials(regions(2)))
            .unwrap();
        let after = backdrop_requests(&mut kit, &seen, 1 + 64 + 4);
        assert_eq!(
            after
                .iter()
                .filter(|s| **s == Seen::BackdropDestroyed)
                .count(),
            65
        );
        assert_eq!(after[65..], made(2)[..]);
    }

    /// A compositor without app effects, or with them below version 5, can't tell the session is locked: the first
    /// dispatch says it isn't, once, so an adapter that starts muted speaks. From version 5 the compositor's own state
    /// comes instead (the mock's session is locked).
    #[test]
    fn a_compositor_that_cannot_tell_the_lock_state_is_reported_unlocked_once() {
        for (effects, told) in [(None, false), (Some(4), false), (Some(5), true)] {
            let (mut kit, _control, _committed, _seen) = created_with(effects);
            let locks = |kit: &mut Kit| {
                kit.pump();
                std::mem::take(&mut kit.stash)
                    .into_iter()
                    .filter(|e| matches!(e, PlatformEvent::SessionLock(_)))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                locks(&mut kit),
                [PlatformEvent::SessionLock(told)],
                "app effects {effects:?}"
            );
            assert_eq!(locks(&mut kit), [], "app effects {effects:?}");
        }
    }

    #[test]
    fn frames_are_paced_by_the_compositor_and_damage_a_frame_could_not_show_is_carried() {
        let (mut kit, control, committed, _seen) = mapped();
        let w = kit.window;

        // nothing paces a window that never presented: the frame is due at once, timed on the system's monotonic
        // clock, which the runtime reads too
        kit.ws.request_frame(w);
        let at = kit.window_event(|e| match e {
            WindowEvent::Frame(at) => Some(*at),
            _ => None,
        });
        let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        let now = now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64;
        assert!(
            at.0 <= now && now - at.0 < 5_000_000_000,
            "frame at {at:?}, monotonic now {now}"
        );
        let first = kit
            .ws
            .present(w, &frame(1, [10, 20, 30, 40], &[]), None)
            .unwrap();
        assert_eq!(first, PresentOutcome::Presented);
        kit.pump();
        let shown = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            (shown.buffer_size, shown.first_pixel, shown.damage),
            (
                (200, 100),
                [30, 20, 10, 40],
                vec![[0, 0, i32::MAX, i32::MAX]]
            )
        );

        // the next frame waits for the frame callback
        kit.ws.request_frame(w);
        kit.pump();
        assert!(!kit.stash.iter().any(|e| matches!(
            e,
            PlatformEvent::Window {
                event: WindowEvent::Frame(_),
                ..
            }
        )));
        control.send(Control::FrameDone).unwrap();
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));

        // a second buffer while the server holds the first; showing it releases the first
        kit.ws
            .present(w, &frame(2, [1, 1, 1, 1], &[rect(0, 0, 1, 1)]), None)
            .unwrap();
        kit.pump();
        committed.recv_timeout(Duration::from_secs(5)).unwrap();
        kit.pump();

        // a frame callback nobody asked for wakes nothing
        control.send(Control::FrameDone).unwrap();
        kit.pump();
        kit.pump();
        assert!(!kit.stash.iter().any(|e| matches!(
            e,
            PlatformEvent::Window {
                event: WindowEvent::Frame(_),
                ..
            }
        )));

        // a frame drawn at a stale size is not shown, and its damage is carried
        let stale = frame_sized(3, (100, 50), [0; 4], &[rect(40, 30, 5, 5)]);
        assert_eq!(
            kit.ws.present(w, &stale, None).unwrap(),
            PresentOutcome::NotReady
        );
        assert!(committed.recv_timeout(Duration::from_millis(100)).is_err());
        // the first buffer again: the compositor hears of the carried damage and this frame's; the buffer also gets
        // what it missed (frame 2's pixel), which the compositor already showed
        // the first buffer again: this frame's damage, the carried one, and what it missed (frame 2's pixel)
        kit.ws
            .present(w, &frame(4, [1, 1, 1, 1], &[rect(10, 10, 2, 2)]), None)
            .unwrap();
        kit.pump();
        let reused = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(reused.buffer, shown.buffer);
        assert_eq!(reused.damage, vec![[40, 30, 5, 5], [10, 10, 2, 2]]);
        assert_eq!(reused.first_pixel, [1, 1, 1, 1]);
    }

    #[test]
    fn a_frame_presented_before_the_first_configure_is_due_again_with_the_configure() {
        let (mut kit, _control, committed, _seen) = created();
        let w = kit.window;
        assert_eq!(
            kit.ws.present(w, &frame(1, [0; 4], &[]), None).unwrap(),
            PresentOutcome::NotReady
        );
        kit.ws.request_frame(w);
        let configured_first = kit.window_event(|e| match e {
            WindowEvent::Configure(_) => Some(true),
            WindowEvent::Frame(_) => Some(false),
            _ => None,
        });
        assert!(configured_first);
        let (mut framed, mut scaled) = (false, false);
        while !(framed && scaled) {
            let is_frame = kit.window_event(|e| match e {
                WindowEvent::Frame(_) => Some(true),
                WindowEvent::Scale(_) => Some(false),
                _ => None,
            });
            framed |= is_frame;
            scaled |= !is_frame;
        }
        assert_eq!(
            kit.ws.present(w, &frame(2, [0; 4], &[]), None).unwrap(),
            PresentOutcome::Presented
        );
        kit.pump();
        committed.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn a_frame_with_no_free_buffer_is_due_again_once_the_compositor_releases_one() {
        let (mut kit, control, committed, _seen) = mapped();
        let w = kit.window;
        control.send(Control::HoldBuffers).unwrap();
        let mut presented = 0;
        while kit
            .ws
            .present(w, &frame(presented, [0; 4], &[]), None)
            .unwrap()
            == PresentOutcome::Presented
        {
            presented += 1;
            assert!(presented < 8, "the transport never ran out of buffers");
        }
        // the frame callbacks are done, so only the held buffers keep the next frame back
        kit.pump();
        for _ in 0..presented {
            committed.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        control.send(Control::FrameDone).unwrap();
        kit.pump();
        kit.pump();
        assert_eq!(
            kit.ws.present(w, &frame(8, [0; 4], &[]), None).unwrap(),
            PresentOutcome::NotReady
        );
        kit.stash.clear();

        kit.ws.request_frame(w);
        kit.pump();
        kit.pump();
        assert!(!kit.took_frame());

        control.send(Control::ReleaseBuffers).unwrap();
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
        assert_eq!(
            kit.ws.present(w, &frame(9, [0; 4], &[]), None).unwrap(),
            PresentOutcome::Presented
        );
    }

    #[test]
    fn each_presented_frame_is_reported_shown_at_the_compositors_time_or_discarded() {
        let (mut kit, control, committed, _seen) = mapped();
        let w = kit.window;
        assert!(kit.ws.capabilities().presentation_time);
        kit.ws.request_frame(w);
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
        kit.ws.present(w, &frame(1, [0; 4], &[]), None).unwrap();
        kit.pump();
        committed.recv_timeout(Duration::from_secs(5)).unwrap();
        kit.ws.request_frame(w);
        control.send(Control::FrameDone).unwrap();
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
        kit.ws.present(w, &frame(2, [0; 4], &[]), None).unwrap();
        kit.pump();
        committed.recv_timeout(Duration::from_secs(5)).unwrap();

        control.send(Control::Discard).unwrap();
        control
            .send(Control::Shown(Duration::from_millis(4)))
            .unwrap();
        let discarded = kit.window_event(|e| match e {
            WindowEvent::Discarded { frame } => Some(*frame),
            _ => None,
        });
        let (shown, at) = kit.window_event(|e| match e {
            WindowEvent::Presented { frame, time } => Some((*frame, *time)),
            _ => None,
        });
        assert_eq!((discarded, shown), (RenderFrameId(1), RenderFrameId(2)));

        // the compositor's time is on the window system's clock: 4 ms before a frame callback that comes after it
        kit.ws.request_frame(w);
        control.send(Control::FrameDone).unwrap();
        let later = kit.window_event(|e| match e {
            WindowEvent::Frame(t) => Some(*t),
            _ => None,
        });
        let ago = Duration::from_nanos(later.0 - at.0);
        assert!(
            ago >= Duration::from_millis(4) && ago < Duration::from_secs(1),
            "{ago:?}"
        );
    }

    #[test]
    fn a_click_arrives_in_logical_points_with_its_serial_and_a_time_on_the_systems_clock() {
        let (mut kit, control, _committed, _seen) = mapped();
        control.send(Control::PointerPress).unwrap();
        let (button, time) = kit.window_event(|e| match e {
            WindowEvent::Pointer(p @ PointerEvent::Button { .. }, time) => Some((*p, *time)),
            _ => None,
        });
        assert_eq!(
            button,
            PointerEvent::Button {
                position: Point::new(10.25, 20.75),
                button: MouseButton::Left,
                pressed: true,
                serial: 6,
                mods: cupertino_ui_platform::Mods::NONE,
            }
        );
        assert!(time.0 > 0);
    }

    #[test]
    fn a_tap_arrives_as_a_touch_down_and_up_in_logical_points() {
        let (mut kit, control, _committed, _seen) = mapped();
        control.send(Control::Tap).unwrap();
        let down = kit.window_event(|e| match e {
            WindowEvent::Touch(t, _) => Some(*t),
            _ => None,
        });
        let up = kit.window_event(|e| match e {
            WindowEvent::Touch(t, _) => Some(*t),
            _ => None,
        });
        assert_eq!(
            (down, up),
            (
                TouchEvent::Down {
                    id: 3,
                    position: Point::new(4.5, 5.5)
                },
                TouchEvent::Up { id: 3 }
            )
        );
    }

    #[test]
    fn the_clipboard_reports_its_offer_reads_it_and_copies_with_the_latest_serial() {
        let (mut kit, control, _committed, seen) = mapped();
        control
            .send(Control::OfferSelection("hello".into()))
            .unwrap();
        let mimes = kit.wait_for(|e| match e {
            PlatformEvent::ClipboardOffered { kind, mimes } => Some((*kind, mimes.clone())),
            _ => None,
        });
        assert_eq!(
            mimes,
            (
                ClipboardKind::Clipboard,
                Some(vec!["text/plain;charset=utf-8".into()])
            )
        );
        let token = kit
            .ws
            .read_clipboard(ClipboardKind::Clipboard, "text/plain;charset=utf-8");
        let data = kit.wait_for(|e| match e {
            PlatformEvent::Clipboard { token: t, data } if *t == token => Some(data.clone()),
            _ => None,
        });
        assert_eq!(data.map(|d| d.bytes), Some(b"hello".to_vec()));
        // the primary selection is not offered here, whatever the clipboard holds
        let primary = kit
            .ws
            .read_clipboard(ClipboardKind::Primary, "text/plain;charset=utf-8");
        let nothing = kit.wait_for(|e| match e {
            PlatformEvent::Clipboard { token: t, data } if *t == primary => Some(data.clone()),
            _ => None,
        });
        assert_eq!(nothing, None);

        control.send(Control::PointerPress).unwrap();
        kit.window_event(|e| {
            matches!(e, WindowEvent::Pointer(PointerEvent::Button { .. }, _)).then_some(())
        });
        kit.ws.set_clipboard(
            ClipboardKind::Clipboard,
            cupertino_ui_platform::ClipboardData {
                mime: "text/plain".into(),
                bytes: b"copied".to_vec(),
            },
        );
        kit.pump();
        assert_eq!(
            seen.recv_timeout(Duration::from_secs(5)).unwrap(),
            Seen::Selection {
                serial: 6,
                types: vec!["text/plain".into()]
            }
        );
    }

    /// The drag goes out from the press with its data's types and its image as the icon surface, in `wl_shm`'s byte
    /// order at its scale with the pointer at the hotspot; the compositor's cancel ends it.
    #[test]
    fn a_drag_goes_out_with_its_image_as_the_icon_and_ends_when_the_compositor_cancels_it() {
        use cupertino_ui_platform::{
            ClipboardData, DragImage, DragProgress, DragRequest, DropAction,
        };
        let (mut kit, control, _committed, seen) = mapped();
        assert!(kit.ws.capabilities().drag_and_drop);
        let request = || {
            let mut pixels = vec![0; 4 * 2 * 4];
            pixels[..4].copy_from_slice(&[10, 20, 30, 255]);
            DragRequest {
                data: vec![ClipboardData {
                    mime: "text/uri-list".into(),
                    bytes: b"file:///a\r\n".to_vec(),
                }],
                actions: [DropAction::Copy, DropAction::Move].into(),
                image: Some(DragImage {
                    width: 4,
                    height: 2,
                    scale: 2,
                    pixels,
                    hotspot: Point::new(1.0, 1.0),
                }),
            }
        };
        assert!(
            kit.ws.start_drag(kit.window, request()).is_err(),
            "no button held"
        );
        control.send(Control::PointerPress).unwrap();
        kit.window_event(|e| {
            matches!(e, WindowEvent::Pointer(PointerEvent::Button { .. }, _)).then_some(())
        });
        let token = kit.ws.start_drag(kit.window, request()).unwrap();
        kit.pump();
        assert_eq!(
            seen.recv_timeout(Duration::from_secs(5)).unwrap(),
            Seen::DragStarted {
                serial: 6,
                types: vec!["text/uri-list".into()],
                icon: true,
            }
        );
        assert_eq!(
            seen.recv_timeout(Duration::from_secs(5)).unwrap(),
            Seen::IconCommitted {
                size: (4, 2),
                scale: 2,
                hotspot: (1, 1),
                first_pixel: [30, 20, 10, 255],
            }
        );
        // the window loses the pointer for the drag
        kit.window_event(|e| {
            matches!(e, WindowEvent::Pointer(PointerEvent::Leave, _)).then_some(())
        });
        control.send(Control::CancelDrag).unwrap();
        let progress = kit.wait_for(|e| match e {
            PlatformEvent::Drag { token: t, progress } if *t == token => Some(progress.clone()),
            _ => None,
        });
        assert_eq!(progress, DragProgress::Cancelled);
    }

    /// Another client's drag over the window arrives as the platform's drop events: an enter with its types and the
    /// source's actions, a motion, the compositor's choice; the answer accepts the type with the actions; the drop is
    /// read through the clipboard's read event and finished with the action taken.
    #[test]
    fn a_drag_from_another_client_is_answered_read_and_finished() {
        use cupertino_ui_platform::{ClipboardData, DropAction, DropAnswer, DropEvent, Mods};
        let (mut kit, control, _committed, seen) = mapped();
        control
            .send(Control::DragIn("file:///tmp/a\r\n".into()))
            .unwrap();
        let (token, mimes, actions, at) = kit.window_event(|e| match e {
            WindowEvent::Drop(DropEvent::Enter {
                token,
                mimes,
                actions,
                position,
                mods,
            }) => {
                assert_eq!(*mods, Mods::NONE);
                Some((*token, mimes.clone(), actions.clone(), *position))
            }
            _ => None,
        });
        assert_eq!(mimes, ["text/uri-list"]);
        assert_eq!(actions, [DropAction::Copy, DropAction::Move].into());
        assert_eq!(at, Point::new(3.0, 4.0));
        assert_eq!(
            kit.window_event(|e| match e {
                WindowEvent::Drop(DropEvent::Motion { position, .. }) => Some(*position),
                _ => None,
            }),
            Point::new(8.0, 9.0)
        );
        assert_eq!(
            kit.window_event(|e| match e {
                WindowEvent::Drop(DropEvent::Action(a)) => Some(*a),
                _ => None,
            }),
            Some(DropAction::Move)
        );
        kit.ws
            .answer_drop(
                token,
                &DropAnswer {
                    mime: Some("text/uri-list".into()),
                    actions: [DropAction::Copy, DropAction::Move].into(),
                    preferred: Some(DropAction::Move),
                },
            )
            .unwrap();
        let next = |seen: &Receiver<Seen>| seen.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            next(&seen),
            Seen::DropAccepted(Some("text/uri-list".into()))
        );
        assert_eq!(
            next(&seen),
            Seen::DropActions {
                actions: 3,
                preferred: 2
            }
        );
        assert!(
            kit.ws.read_drop(token, "text/uri-list").is_err(),
            "not dropped yet"
        );
        control.send(Control::DropIn).unwrap();
        kit.window_event(|e| matches!(e, WindowEvent::Drop(DropEvent::Drop)).then_some(()));
        let read = kit.ws.read_drop(token, "text/uri-list").unwrap();
        let data = kit.wait_for(|e| match e {
            PlatformEvent::Clipboard { token, data } if *token == read => Some(data.clone()),
            _ => None,
        });
        assert_eq!(
            data,
            Some(ClipboardData {
                mime: "text/uri-list".into(),
                bytes: b"file:///tmp/a\r\n".to_vec(),
            })
        );
        kit.ws.finish_drop(token, Some(DropAction::Move));
        assert_eq!(
            next(&seen),
            Seen::DropActions {
                actions: 2,
                preferred: 2
            },
            "the action taken, before the finish"
        );
        assert_eq!(next(&seen), Seen::DropFinished);
    }

    /// A drop the compositor chose to move, which the window copied, is given the copy before it finishes, so the
    /// source does not remove its data; one the window refuses is let go unfinished.
    #[test]
    fn a_drop_taken_with_another_action_than_the_one_shown_tells_the_source_first() {
        use cupertino_ui_platform::{DropAction, DropAnswer, DropEvent};
        let (mut kit, control, _committed, seen) = mapped();
        let next = |seen: &Receiver<Seen>| seen.recv_timeout(Duration::from_secs(5)).unwrap();
        let dropped = |kit: &mut Kit| {
            control
                .send(Control::DragIn("file:///tmp/a\r\n".into()))
                .unwrap();
            let token = kit.window_event(|e| match e {
                WindowEvent::Drop(DropEvent::Enter { token, .. }) => Some(*token),
                _ => None,
            });
            kit.window_event(|e| {
                matches!(
                    e,
                    WindowEvent::Drop(DropEvent::Action(Some(DropAction::Move)))
                )
                .then_some(())
            });
            let answer = DropAnswer {
                mime: Some("text/uri-list".into()),
                actions: [DropAction::Copy, DropAction::Move].into(),
                preferred: Some(DropAction::Move),
            };
            kit.ws.answer_drop(token, &answer).unwrap();
            assert!(matches!(next(&seen), Seen::DropAccepted(Some(_))));
            assert!(matches!(next(&seen), Seen::DropActions { .. }));
            control.send(Control::DropIn).unwrap();
            kit.window_event(|e| matches!(e, WindowEvent::Drop(DropEvent::Drop)).then_some(()));
            token
        };
        let token = dropped(&mut kit);
        kit.ws.finish_drop(token, Some(DropAction::Copy));
        assert_eq!(
            next(&seen),
            Seen::DropActions {
                actions: 1,
                preferred: 1
            },
            "a copy, before the finish"
        );
        assert_eq!(next(&seen), Seen::DropFinished);
        assert_eq!(next(&seen), Seen::DropDestroyed);

        let token = dropped(&mut kit);
        kit.ws.finish_drop(token, None);
        assert_eq!(next(&seen), Seen::DropDestroyed, "refused, not finished");
    }

    /// An image wider than the transport shows (as a large preview at a high scale is) leaves the drag without an icon
    /// surface; the drag itself still goes out.
    #[test]
    fn a_drag_whose_image_is_too_large_goes_out_without_an_icon() {
        use cupertino_ui_platform::{ClipboardData, DragImage, DragRequest, DropAction};
        let (mut kit, control, _committed, seen) = mapped();
        control.send(Control::PointerPress).unwrap();
        kit.window_event(|e| {
            matches!(e, WindowEvent::Pointer(PointerEvent::Button { .. }, _)).then_some(())
        });
        let (width, height) = (1026, 2);
        let request = DragRequest {
            data: vec![ClipboardData {
                mime: "text/uri-list".into(),
                bytes: b"file:///a\r\n".to_vec(),
            }],
            actions: [DropAction::Copy].into(),
            image: Some(DragImage {
                width,
                height,
                scale: 2,
                pixels: vec![0; (width * height * 4) as usize],
                hotspot: Point::new(1.0, 1.0),
            }),
        };
        kit.ws.start_drag(kit.window, request).unwrap();
        kit.pump();
        assert!(matches!(
            seen.recv_timeout(Duration::from_secs(5)).unwrap(),
            Seen::DragStarted { icon: false, .. }
        ));
    }

    #[test]
    fn a_waker_from_another_thread_ends_a_dispatch_with_wake() {
        let (mut kit, _control, _committed, _seen) = mapped();
        let waker = kit.ws.waker();
        std::thread::spawn(move || waker.wake());
        kit.wait_for(|e| matches!(e, PlatformEvent::Wake).then_some(()));
    }

    /// A role the test gives windows through the transport's external surfaces, the way a shell gives layer surfaces:
    /// an xdg toplevel of its own `xdg_wm_base`, whose configure leaves the size to the client.
    mod role {
        use cupertino_ui_platform::WindowSpec;
        use cupertino_ui_wayland::{RoleEvent, SurfaceRoles};
        use cupertino_wayland_client::{
            Connection, Error, ExtensionEvent, ExternalRole, State, SurfaceId,
        };
        use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle};
        use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
        use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
        use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

        pub const APP_ID: &str = "org.example.Card";
        /// The size the role resolves the client's extents to.
        pub const SIZE: (u32, u32) = (120, 40);

        pub struct Base;
        pub struct OfSurface(SurfaceId);

        struct Role(XdgSurface, XdgToplevel);

        impl ExternalRole for Role {
            fn ack_configure(&self, serial: u32) {
                self.0.ack_configure(serial);
            }

            fn destroy(&self) {
                self.1.destroy();
                self.0.destroy();
            }
        }

        #[derive(Default)]
        pub struct Roles {
            base: Option<XdgWmBase>,
        }

        impl SurfaceRoles for Roles {
            fn create(
                &mut self,
                conn: &mut Connection,
                spec: &WindowSpec,
            ) -> Option<Result<SurfaceId, Error>> {
                if spec.app_id != APP_ID {
                    return None;
                }
                if self.base.is_none() {
                    self.base = conn.bind_extension(1..=6, Base);
                }
                let Some(base) = self.base.clone() else {
                    return Some(Err(Error::Protocol("no xdg_wm_base")));
                };
                Some(Ok(conn.external_surface(|wl, qh, surface| {
                    let xdg = base.get_xdg_surface(wl, qh, OfSurface(surface));
                    let toplevel = xdg.get_toplevel(qh, OfSurface(surface));
                    Role(xdg, toplevel)
                })))
            }

            fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
                event.downcast_ref::<RoleEvent>().copied()
            }
        }

        impl Dispatch<XdgWmBase, Base> for State {
            fn event(
                _: &mut State,
                base: &XdgWmBase,
                event: xdg_wm_base::Event,
                _: &Base,
                _: &WlConnection,
                _: &QueueHandle<State>,
            ) {
                if let xdg_wm_base::Event::Ping { serial } = event {
                    base.pong(serial);
                }
            }
        }

        impl Dispatch<XdgSurface, OfSurface> for State {
            fn event(
                state: &mut State,
                _: &XdgSurface,
                event: xdg_surface::Event,
                data: &OfSurface,
                _: &WlConnection,
                _: &QueueHandle<State>,
            ) {
                if let xdg_surface::Event::Configure { serial } = event {
                    state.configure_external(data.0, serial, (0, 0));
                    state.emit(RoleEvent::Configure {
                        surface: data.0,
                        size: SIZE,
                    });
                }
            }
        }

        impl Dispatch<XdgToplevel, OfSurface> for State {
            fn event(
                state: &mut State,
                _: &XdgToplevel,
                event: xdg_toplevel::Event,
                data: &OfSurface,
                _: &WlConnection,
                _: &QueueHandle<State>,
            ) {
                if let xdg_toplevel::Event::Close = event {
                    state.emit(RoleEvent::Closed(data.0));
                }
            }
        }
    }

    #[test]
    fn a_window_given_a_role_by_the_application_draws_at_the_roles_size_and_closes_with_it() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (control, committed, _seen) = spawn_server(server_stream);
        let ws = WaylandWindowSystem::new(
            Connection::from_stream(client_stream).unwrap(),
            Profile::cupertino(),
        )
        .with_roles(role::Roles::default());
        let mut kit = Kit {
            ws,
            window: WindowId(0),
            stash: VecDeque::new(),
        };
        let w = kit
            .ws
            .create_window(&WindowSpec {
                title: "Card".into(),
                app_id: role::APP_ID.into(),
                size: Size::new(10.0, 10.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            })
            .unwrap();
        kit.window = w;

        let configure = kit.window_event(|e| match e {
            WindowEvent::Configure(c) => Some(*c),
            _ => None,
        });
        assert_eq!(configure.size, Some(Size::new(120.0, 40.0)));
        assert_eq!(
            kit.ws
                .request(w, cupertino_ui_platform::WindowRequest::Minimize),
            Err(cupertino_ui_platform::PlatformError::Unsupported)
        );
        kit.window_event(|e| matches!(e, WindowEvent::Scale(_)).then_some(()));

        kit.ws.request_frame(w);
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
        let outcome = kit
            .ws
            .present(w, &frame_sized(1, (240, 80), [10, 20, 30, 40], &[]), None)
            .unwrap();
        assert_eq!(outcome, PresentOutcome::Presented);
        kit.pump();
        let shown = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(shown.buffer_size, (240, 80));

        control.send(Control::Close).unwrap();
        kit.window_event(|e| matches!(e, WindowEvent::CloseRequested).then_some(()));
    }

    #[test]
    fn a_window_given_a_role_is_activated_by_its_configures_while_it_has_the_keyboard() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (control, _committed, _seen) = spawn_server(server_stream);
        let ws = WaylandWindowSystem::new(
            Connection::from_stream(client_stream).unwrap(),
            Profile::cupertino(),
        )
        .with_roles(role::Roles::default());
        let mut kit = Kit {
            ws,
            window: WindowId(0),
            stash: VecDeque::new(),
        };
        kit.window = kit
            .ws
            .create_window(&WindowSpec {
                title: "Search".into(),
                app_id: role::APP_ID.into(),
                size: Size::new(10.0, 10.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            })
            .unwrap();
        let configured = |kit: &mut Kit| {
            control.send(Control::Reconfigure).unwrap();
            kit.window_event(|e| match e {
                WindowEvent::Configure(c) => Some(c.states.activated),
                _ => None,
            })
        };
        let first = kit.window_event(|e| match e {
            WindowEvent::Configure(c) => Some(c.states.activated),
            _ => None,
        });
        assert!(!first, "no keyboard yet");

        control.send(Control::KeyboardEnter).unwrap();
        assert!(kit.window_event(|e| match e {
            WindowEvent::Focus(focused) => Some(*focused),
            _ => None,
        }));
        assert!(configured(&mut kit), "keys still reach the window");

        control.send(Control::KeyboardLeave).unwrap();
        assert!(!kit.window_event(|e| match e {
            WindowEvent::Focus(focused) => Some(*focused),
            _ => None,
        }));
        assert!(!configured(&mut kit));
    }

    #[test]
    fn a_windows_input_region_reaches_its_surface_with_the_next_present() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (control, committed, _seen) = spawn_server(server_stream);
        let ws = WaylandWindowSystem::new(
            Connection::from_stream(client_stream).unwrap(),
            Profile::cupertino(),
        )
        .with_roles(role::Roles::default());
        let mut kit = Kit {
            ws,
            window: WindowId(0),
            stash: VecDeque::new(),
        };
        let w = kit
            .ws
            .create_window(&WindowSpec {
                title: "Dock".into(),
                app_id: role::APP_ID.into(),
                size: Size::new(10.0, 10.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: Some(vec![Rect::new(10.0, 20.0, 100.0, 20.5)]),
            })
            .unwrap();
        kit.window = w;
        kit.window_event(|e| matches!(e, WindowEvent::Configure(_)).then_some(()));
        let present = |kit: &mut Kit, id: u64| {
            kit.ws.request_frame(w);
            kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
            let outcome = kit
                .ws
                .present(w, &frame_sized(id, (240, 80), [0; 4], &[]), None)
                .unwrap();
            assert_eq!(outcome, PresentOutcome::Presented);
            kit.pump();
            control.send(Control::FrameDone).unwrap();
            committed
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .input_region
        };

        assert_eq!(present(&mut kit, 1), Some(Some(vec![[10, 20, 100, 21]])));
        assert_eq!(
            present(&mut kit, 2),
            None,
            "an unchanged region is not sent again"
        );
        kit.ws
            .request(w, WindowRequest::SetInputRegion(Some(Vec::new())))
            .unwrap();
        assert_eq!(present(&mut kit, 3), Some(Some(Vec::new())));
        kit.ws
            .request(w, WindowRequest::SetInputRegion(None))
            .unwrap();
        assert_eq!(present(&mut kit, 4), Some(None));
    }

    /// A window of one colour over the output (a catcher of presses around a panel) is one transparent pixel the
    /// compositor stretches to the configured size, at buffer scale 1 on an output at 2, its input region going with
    /// the commit as a frame's does; the frame presented after it is a whole buffer at the window's scale, which the
    /// viewport the colour needed now stretches to the same size.
    #[test]
    fn a_solid_window_is_one_pixel_stretched_over_its_configured_size_and_keeps_its_input_region() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (control, committed, seen) = spawn_server_offering(server_stream, None, true);
        let ws = WaylandWindowSystem::new(
            Connection::from_stream(client_stream).unwrap(),
            Profile::cupertino(),
        )
        .with_roles(role::Roles::default());
        let mut kit = Kit {
            ws,
            window: WindowId(0),
            stash: VecDeque::new(),
        };
        let w = kit
            .ws
            .create_window(&WindowSpec {
                title: "Close the panel".into(),
                app_id: role::APP_ID.into(),
                size: Size::new(1.0, 1.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: Some(vec![Rect::new(0.0, 0.0, 120.0, 40.0)]),
            })
            .unwrap();
        kit.window = w;
        let configure = kit.window_event(|e| match e {
            WindowEvent::Configure(c) => Some(*c),
            _ => None,
        });
        assert_eq!(configure.size, Some(Size::new(120.0, 40.0)));
        let clear = Color::new(0.0, 0.0, 0.0, 0.0);
        let solid = |kit: &mut Kit, color: Color| {
            kit.ws.request_frame(w);
            kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
            let outcome = kit.ws.present_solid(w, color, Some(configure.id)).unwrap();
            assert_eq!(outcome, PresentOutcome::Presented);
            kit.pump();
            control.send(Control::FrameDone).unwrap();
            loop {
                if let s @ Seen::SolidCommitted { .. } =
                    seen.recv_timeout(Duration::from_secs(5)).unwrap()
                {
                    return s;
                }
            }
        };

        assert_eq!(
            solid(&mut kit, clear),
            Seen::SolidCommitted {
                color: [0; 4],
                destination: Some((120, 40)),
                buffer_scale: 1,
                damage: vec![[0, 0, 1, 1]],
                input_region: Some(Some(vec![[0, 0, 120, 40]])),
            }
        );
        let Seen::SolidCommitted { input_region, .. } = solid(&mut kit, clear) else {
            unreachable!()
        };
        assert_eq!(input_region, None, "an unchanged region is not sent again");
        kit.ws
            .request(w, WindowRequest::SetInputRegion(None))
            .unwrap();
        let Seen::SolidCommitted {
            input_region,
            destination,
            ..
        } = solid(&mut kit, clear)
        else {
            unreachable!()
        };
        assert_eq!(
            (input_region, destination),
            (Some(None), Some((120, 40))),
            "the whole surface takes input again"
        );

        kit.ws.request_frame(w);
        kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
        let outcome = kit
            .ws
            .present(w, &frame_sized(1, (240, 80), [1, 2, 3, 4], &[]), None)
            .unwrap();
        assert_eq!(outcome, PresentOutcome::Presented);
        kit.pump();
        let shown = committed.recv_timeout(Duration::from_secs(5)).unwrap();
        // the output's scale 2, the viewport stretching the buffer to the same 120 x 40 points
        assert_eq!(
            (shown.buffer_size, shown.buffer_scale, shown.damage),
            ((240, 80), 1, vec![[0, 0, i32::MAX, i32::MAX]]),
            "drawn whole after the colour"
        );
    }

    #[test]
    fn a_solid_window_is_refused_where_the_compositor_has_no_one_pixel_buffers() {
        let (mut kit, _control, _committed, _seen) = mapped();
        let w = kit.window;
        assert_eq!(
            kit.ws
                .present_solid(w, Color::new(0.0, 0.0, 0.0, 0.0), None),
            Err(cupertino_ui_platform::PlatformError::Unsupported)
        );
    }

    #[test]
    fn a_window_without_an_input_region_never_sets_one() {
        let (mut kit, control, committed, _seen) = mapped();
        let w = kit.window;
        for id in 1..=2 {
            kit.ws.request_frame(w);
            kit.window_event(|e| matches!(e, WindowEvent::Frame(_)).then_some(()));
            kit.ws.present(w, &frame(id, [0; 4], &[]), None).unwrap();
            kit.pump();
            control.send(Control::FrameDone).unwrap();
            assert_eq!(
                committed
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .input_region,
                None
            );
        }
    }

    #[test]
    fn requests_on_an_unknown_window_or_the_wrong_kind_are_refused() {
        let (mut kit, _control, _committed, _seen) = mapped();
        let gone = WindowId(99);
        assert_eq!(
            kit.ws
                .request(gone, cupertino_ui_platform::WindowRequest::Minimize),
            Err(cupertino_ui_platform::PlatformError::UnknownWindow(gone))
        );
        let w = kit.window;
        assert_eq!(
            kit.ws.request(
                w,
                cupertino_ui_platform::WindowRequest::Reposition(
                    cupertino_ui_platform::PopupSpec {
                        anchor_rect: Rect::new(0.0, 0.0, 1.0, 1.0),
                        anchor: Default::default(),
                        gravity: Default::default(),
                        offset: Point::ZERO,
                        size: Size::new(10.0, 10.0),
                        constraint: Default::default(),
                        reactive: false,
                        grab: false,
                    }
                )
            ),
            Err(cupertino_ui_platform::PlatformError::Unsupported)
        );
        kit.ws.destroy(w);
        assert_eq!(
            kit.ws.present(w, &frame(1, [0; 4], &[]), None),
            Err(cupertino_ui_platform::PlatformError::UnknownWindow(w))
        );
    }

    #[test]
    fn an_activation_token_names_the_application_and_a_gone_window_is_answered_with_none() {
        let (mut kit, _control, _committed, _seen) = mapped();
        let asked = kit
            .ws
            .request_activation_token(kit.window, Some("org.example.Viewer"));
        let token = kit.wait_for(|e| match e {
            PlatformEvent::ActivationToken { request, token } if *request == asked => {
                Some(token.clone())
            }
            _ => None,
        });
        assert_eq!(token.as_deref(), Some("token-for-org.example.Viewer"));

        let gone = WindowId(kit.window.0 + 100);
        let asked = kit.ws.request_activation_token(gone, None);
        let token = kit.wait_for(|e| match e {
            PlatformEvent::ActivationToken { request, token } if *request == asked => {
                Some(token.clone())
            }
            _ => None,
        });
        assert_eq!(token, None);
    }
}
