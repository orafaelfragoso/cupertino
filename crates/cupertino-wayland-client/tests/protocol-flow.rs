//! Runs the transport against a minimal in-process compositor that offers the core globals and, per test, some optional
//! ones, and checks what the compositor receives and what the client reports back.

use cupertino_geometry::{Point, Rect};
use cupertino_wayland_client::{
    Anchor, BufferFormat, Button, Capability, ClipboardOffer, Connection, ConstraintAdjustment,
    CursorShape, CursorTheme, Decorations, DragActions, DragEvent, DragIcon, DragOffer,
    DragSourceEvent, Error, Event, Extension, Gravity, HeldPress, KeyboardEvent, OutputGeometry,
    OutputTransform, PixelRect, PointerEvent, PopupPlacement, Preedit, Presented, PresentedKind,
    PressDevice, ReadId, ResizeEdge, Scale, ScrollSource, SurfaceId, SurroundingText,
    TextInputDone, TextInputEvent, TextInputState, Toplevel, ToplevelConfig, TouchEvent,
    WindowConfigure, WmCapabilities,
};
use std::collections::VecDeque;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_manager_v1::{
    self, WpFractionalScaleManagerV1,
};
use wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::presentation_time::server::wp_presentation::{self, WpPresentation};
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};
use wayland_protocols::wp::single_pixel_buffer::v1::server::wp_single_pixel_buffer_manager_v1::{
    self, WpSinglePixelBufferManagerV1,
};
use wayland_protocols::wp::text_input::zv3::server::zwp_text_input_manager_v3::{
    self, ZwpTextInputManagerV3,
};
use wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::{self, ZwpTextInputV3};
use wayland_protocols::wp::viewporter::server::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::server::wp_viewporter::{self, WpViewporter};
use wayland_protocols::xdg::activation::v1::server::xdg_activation_token_v1::{
    self, XdgActivationTokenV1,
};
use wayland_protocols::xdg::activation::v1::server::xdg_activation_v1::{self, XdgActivationV1};
use wayland_protocols::xdg::decoration::zv1::server::zxdg_decoration_manager_v1::{
    self, ZxdgDecorationManagerV1,
};
use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::{
    self, Mode, ZxdgToplevelDecorationV1,
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
use wayland_server::protocol::wl_data_device_manager::{self, DndAction, WlDataDeviceManager};
use wayland_server::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_server::protocol::wl_data_source::{self, WlDataSource};
use wayland_server::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_server::protocol::wl_output::{self, WlOutput};
use wayland_server::protocol::wl_pointer::{self, WlPointer};
use wayland_server::protocol::wl_region::{self, WlRegion};
use wayland_server::protocol::wl_seat::{self, WlSeat};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::protocol::wl_touch::WlTouch;
use wayland_server::{
    Client as ServerClient, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New,
    Resource,
};
use xkbcommon::xkb;

/// Evdev codes (`linux/input-event-codes.h`).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const KEY_A: u32 = 30;

/// The optional globals a test's compositor offers next to the core ones.
#[derive(Clone, Copy, Default)]
struct Globals {
    xdg_wm_base: bool,
    seat: bool,
    clipboard: bool,
    /// The mode the compositor answers a decoration request with.
    decoration: Option<Mode>,
    fractional_scale: bool,
    output_scale: Option<i32>,
    /// Never release a buffer.
    hold_buffers: bool,
    layer_shell: bool,
    activation: bool,
    /// `zxdg_output_manager_v1`, placing the output at (1920, 0) with a 1280 × 720 logical size.
    output_layout: bool,
    text_input: bool,
    /// Offer `wl_seat` version 9, whose wheels report half notches in 120ths and say the physical direction of each
    /// axis, instead of 7.
    high_resolution_wheel: bool,
    /// `wp_presentation` on `CLOCK_MONOTONIC`.
    presentation: bool,
    /// Keep each clipboard read's pipe open, writing nothing, until the test releases it.
    hold_reads: bool,
    /// Offer `wl_data_device_manager` version 2, from before drag actions, instead of 3.
    no_drag_actions: bool,
    /// `wp_single_pixel_buffer_manager_v1`, with `wp_viewporter` even without fractional scaling.
    single_pixel: bool,
}

impl Globals {
    fn core() -> Globals {
        Globals {
            xdg_wm_base: true,
            ..Globals::default()
        }
    }
}

/// What the test tells the compositor to do.
enum Control {
    Configure {
        width: i32,
        height: i32,
        states: Vec<xdg_toplevel::State>,
    },
    EnterOutput,
    /// Announce what the compositor supports, then configure the toplevel.
    WmCapabilities(Vec<xdg_toplevel::WmCapabilities>),
    PreferredBufferScale(i32),
    FractionalScale(u32),
    DismissPopup,
    Click {
        x: f64,
        y: f64,
    },
    /// The pointer leaves the toplevel.
    Leave,
    /// The left button, pressed by `Click`, goes up.
    ButtonUp,
    /// The pointer moves to `(x, y)` on the surface it is on.
    Motion {
        x: f64,
        y: f64,
    },
    /// One finger touches the toplevel and stays down; the compositor reports its serial as [`Seen::TouchDown`].
    TouchHold,
    /// The finger of `TouchHold` lifts.
    TouchUp,
    /// A second finger touches the toplevel beside the one of `TouchHold` and stays down.
    TouchSecondFinger,
    /// The pointer button `button` goes down or up on the surface the pointer is on.
    Button {
        button: u32,
        pressed: bool,
    },
    /// The seat loses its pointer and keeps its keyboard.
    DropPointer,
    /// Two fingers touch the toplevel and the first moves and lifts, each step a frame.
    Touch,
    /// The compositor takes the touch sequence over.
    CancelTouch,
    /// The seat loses its touch screen.
    DropTouch,
    TypeKey(u32),
    /// The key pressed by `TypeKey` goes up.
    ReleaseKey(u32),
    /// Key repeat at `rate` per second after `delay` ms.
    RepeatInfo {
        rate: i32,
        delay: i32,
    },
    Offer(Vec<&'static str>),
    /// Configure the layer surface this many times in a row, each a pixel wider.
    LayerConfigures(u32),
    /// Enter the toplevel, then scroll a wheel by `(dx, dy)` and `(0, dy)`, one notch each, within one pointer frame;
    /// then lift a finger in a frame of its own.
    Scroll {
        dx: f64,
        dy: f64,
    },
    /// Give the toplevel the text-input focus.
    TextInputEnter,
    TextInputLeave,
    /// A preedit the input method never finishes with a `done`.
    PreeditOnly(&'static str),
    /// Another client's drag enters the toplevel at (4, 5), offering `types` with `actions` allowed.
    DragEnter {
        types: Vec<&'static str>,
        actions: wl_data_device_manager::DndAction,
    },
    DragMotion,
    DragAction(wl_data_device_manager::DndAction),
    DragDrop,
    DragLeave,
    /// A drag enters the toplevel without an offer, as the client's own drag of no data does.
    DragEnterEmpty,
    /// To the client's own drag source: the target's type, a request for the data in `mime`, and the end.
    SourceTarget(Option<&'static str>),
    SourceSend(&'static str),
    SourceDropped,
    SourceFinished,
    SourceCancelled,
    /// Write `bytes` to every held clipboard read and close it.
    Release(&'static [u8]),
    /// From a thread of its own, write one byte every `every` to every held clipboard read, `count` times, then close
    /// it; `None` writes until the reader goes.
    Trickle {
        every: Duration,
        count: Option<usize>,
    },
    /// Write this many bytes to every held clipboard read from a thread of its own, keeping it open for two seconds
    /// after.
    Flood(usize),
    /// Answer every presentation feedback asked for so far: presented, or discarded.
    Presentation {
        discard: bool,
    },
    /// One input method batch, done with `serial` or else the number of commits so far.
    Compose {
        preedit: Option<(&'static str, i32, i32)>,
        commit: Option<&'static str>,
        delete: (u32, u32),
        serial: Option<u32>,
    },
}

/// A request on the text input.
#[derive(Debug, PartialEq)]
enum TextInputRequest {
    Enable,
    Disable,
    Surrounding(String, i32, i32),
    Cause(zwp_text_input_v3::ChangeCause),
    Content(
        zwp_text_input_v3::ContentHint,
        zwp_text_input_v3::ContentPurpose,
    ),
    CursorRect([i32; 4]),
    Commit,
}

/// What the compositor saw.
#[derive(Debug, PartialEq)]
enum Seen {
    Ack(u32),
    Commit {
        size: (i32, i32),
        scale: i32,
        format: wl_shm::Format,
    },
    Viewport(i32, i32),
    ViewportSource(f64, f64, f64, f64),
    /// A commit attached a one-pixel buffer of this colour (premultiplied, over the whole `u32` range), at this buffer
    /// scale; `buffer` is its protocol id.
    SolidCommit {
        color: [u32; 4],
        scale: i32,
        buffer: u32,
    },
    /// The one-pixel buffer of this colour was destroyed.
    SolidDestroyed([u32; 4]),
    DecorationMode(Mode),
    Positioner(PositionerState),
    Receive(String),
    LayerAck(u32),
    LayerPopup,
    LayerDestroyed,
    /// The rectangles of the input region set, `None` for the whole surface.
    InputRegion(Option<Vec<[i32; 4]>>),
    Reposition {
        token: u32,
        positioner: PositionerState,
    },
    /// An activation token committed for this application id.
    TokenCommit(Option<String>),
    Activate {
        token: String,
        surface: u32,
    },
    TextInput(TextInputRequest),
    WindowMenu {
        serial: u32,
        x: i32,
        y: i32,
    },
    Move(u32),
    Resize {
        serial: u32,
        edge: xdg_toplevel::ResizeEdge,
    },
    /// The serial of the touch down of `Control::TouchHold`.
    TouchDown(u32),
    /// `wl_data_device.start_drag` of a source offering `types` with `actions`.
    StartDrag {
        serial: u32,
        types: Vec<String>,
        actions: u32,
    },
    /// The serial of a drag's enter.
    DragEntered(u32),
    /// A drag motion was sent to the client.
    DragMoved,
    /// What the client wrote for a type its drag source was asked for.
    SourceData(Vec<u8>),
    /// `wl_data_offer` requests of the drag offer.
    Accept {
        serial: u32,
        mime: Option<String>,
    },
    DragActions {
        actions: u32,
        preferred: u32,
    },
    Finish,
    OfferDestroyed,
    /// The drag icon surface of the latest `start_drag` committed a buffer of `size` pixels at `scale`, the pointer at
    /// `hotspot` in its points (the negated `wl_surface.offset`, or the attach offset).
    IconShown {
        size: (i32, i32),
        scale: i32,
        hotspot: (i32, i32),
    },
    /// The drag icon's surface, and the buffer it showed, were destroyed.
    IconSurfaceDestroyed,
    IconBufferDestroyed,
    /// `wl_pointer.set_cursor`, with whether it named a surface.
    Cursor {
        serial: u32,
        hotspot: (i32, i32),
        surface: bool,
    },
}

#[derive(Default)]
struct SurfaceState {
    buffer: Option<WlBuffer>,
    scale: i32,
    frames: Vec<WlCallback>,
    /// The sum of `wl_surface.offset` and attach offsets.
    offset: (i32, i32),
}

type SurfaceData = Mutex<SurfaceState>;

struct BufferData {
    width: i32,
    height: i32,
    format: wl_shm::Format,
}

/// The colour of a one-pixel buffer.
struct SolidPixel([u32; 4]);

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct PositionerState {
    size: (i32, i32),
    anchor_rect: [i32; 4],
    offset: (i32, i32),
    reactive: bool,
    parent_size: Option<(i32, i32)>,
    parent_configure: Option<u32>,
}

struct Mock {
    globals: Globals,
    seen: Sender<Seen>,
    next_serial: u32,
    surfaces: Vec<WlSurface>,
    toplevel: Option<(
        WlSurface,
        XdgSurface,
        XdgToplevel,
        Option<ZxdgToplevelDecorationV1>,
    )>,
    configured: bool,
    popup: Option<(XdgSurface, XdgPopup, PositionerState, bool)>,
    output: Option<WlOutput>,
    fractional: Option<WpFractionalScaleV1>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    touch: Option<WlTouch>,
    data_device: Option<WlDataDevice>,
    shown: Option<WlBuffer>,
    frames: Vec<WlCallback>,
    /// The layer surface with the size it asked for, and whether it was configured.
    layer: Option<(WlSurface, ZwlrLayerSurfaceV1, (u32, u32), bool)>,
    /// The text input with the number of commits on it.
    text_input: Option<(ZwpTextInputV3, u32)>,
    /// Presentation feedback not answered yet.
    feedbacks: Vec<WpPresentationFeedback>,
    /// Clipboard reads held open.
    held: Vec<std::fs::File>,
    seat: Option<WlSeat>,
    drag_offer: Option<WlDataOffer>,
    drag_source: Option<WlDataSource>,
    /// The icon surface of the latest `start_drag`, and the buffer it showed.
    drag_icon: Option<WlSurface>,
    icon_buffer: Option<WlBuffer>,
}

impl Mock {
    fn serial(&mut self) -> u32 {
        self.next_serial += 1;
        self.next_serial
    }

    fn surface(&self) -> &WlSurface {
        &self.toplevel.as_ref().expect("a toplevel").0
    }
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
    WpFractionalScaleManagerV1,
    WpViewporter,
    WlDataDeviceManager,
    ZwlrLayerShellV1,
    XdgActivationV1,
    ZxdgOutputManagerV1,
    ZwpTextInputManagerV3,
    WpSinglePixelBufferManagerV1
);

/// Requests the compositor ignores.
macro_rules! ignore_requests {
    ($($interface:ty => $data:ty),*) => {$(
        impl Dispatch<$interface, $data> for Mock {
            fn request(_: &mut Self, _: &ServerClient, _: &$interface, _: <$interface as Resource>::Request, _: &$data, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }
    )*};
}

ignore_requests!(WlCallback => (), WlKeyboard => (), WlTouch => (), WlOutput => (), WpFractionalScaleV1 => (), ZxdgOutputV1 => (), WpPresentationFeedback => ());

impl Dispatch<WlBuffer, BufferData> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        buffer: &WlBuffer,
        request: wl_buffer::Request,
        _: &BufferData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, wl_buffer::Request::Destroy)
            && state.icon_buffer.as_ref() == Some(buffer)
        {
            state.icon_buffer = None;
            state.seen.send(Seen::IconBufferDestroyed).unwrap();
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

impl Dispatch<WlBuffer, SolidPixel> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlBuffer,
        request: wl_buffer::Request,
        pixel: &SolidPixel,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, wl_buffer::Request::Destroy) {
            state.seen.send(Seen::SolidDestroyed(pixel.0)).unwrap();
        }
    }
}

/// What a client's data source offers.
#[derive(Default)]
struct SourceState {
    types: Vec<String>,
    actions: u32,
}

impl Dispatch<WlDataSource, Mutex<SourceState>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &WlDataSource,
        request: wl_data_source::Request,
        data: &Mutex<SourceState>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut source = data.lock().unwrap();
        match request {
            wl_data_source::Request::Offer { mime_type } => source.types.push(mime_type),
            wl_data_source::Request::SetActions { dnd_actions } => {
                source.actions = u32::from(dnd_actions)
            }
            _ => {}
        }
    }
}

/// `CLOCK_MONOTONIC`
const MONOTONIC: u32 = 1;

impl GlobalDispatch<WpPresentation, ()> for Mock {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WpPresentation>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(r, ()).clock_id(MONOTONIC);
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
            state.feedbacks.push(init.init(callback, ()));
        }
    }
}

impl Dispatch<WlPointer, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlPointer,
        request: wl_pointer::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_pointer::Request::SetCursor {
            serial,
            surface,
            hotspot_x,
            hotspot_y,
        } = request
        {
            state
                .seen
                .send(Seen::Cursor {
                    serial,
                    hotspot: (hotspot_x, hotspot_y),
                    surface: surface.is_some(),
                })
                .unwrap();
        }
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
                state.surfaces.push(surface);
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
        let icon = state.drag_icon.as_ref() == Some(surface);
        match request {
            wl_surface::Request::Attach { buffer, x, y } => {
                s.buffer = buffer;
                s.offset = (s.offset.0 + x, s.offset.1 + y);
            }
            wl_surface::Request::Offset { x, y } => s.offset = (s.offset.0 + x, s.offset.1 + y),
            wl_surface::Request::Commit if icon => {
                if let Some(buffer) = s.buffer.take() {
                    let b = buffer.data::<BufferData>().unwrap();
                    state
                        .seen
                        .send(Seen::IconShown {
                            size: (b.width, b.height),
                            scale: s.scale.max(1),
                            hotspot: (-s.offset.0, -s.offset.1),
                        })
                        .unwrap();
                    state.icon_buffer = Some(buffer);
                }
            }
            wl_surface::Request::Destroy if icon => {
                state.drag_icon = None;
                state.seen.send(Seen::IconSurfaceDestroyed).unwrap();
            }
            wl_surface::Request::SetBufferScale { scale } => s.scale = scale,
            wl_surface::Request::Frame { callback } => s.frames.push(init.init(callback, ())),
            wl_surface::Request::SetInputRegion { region } => state
                .seen
                .send(Seen::InputRegion(region.map(|r| {
                    r.data::<Mutex<Vec<[i32; 4]>>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .clone()
                })))
                .unwrap(),
            wl_surface::Request::Commit => {
                let initial = s.buffer.is_none();
                if initial
                    && !state.configured
                    && state.toplevel.as_ref().is_some_and(|t| t.0 == *surface)
                {
                    state.configured = true;
                    let serial = state.serial();
                    let (_, xdg, toplevel, decoration) = state.toplevel.as_ref().unwrap();
                    if let (Some(d), Some(mode)) = (decoration, state.globals.decoration) {
                        d.configure(mode);
                    }
                    toplevel.configure(0, 0, Vec::new());
                    xdg.configure(serial);
                }
                if initial
                    && state
                        .layer
                        .as_ref()
                        .is_some_and(|l| l.0 == *surface && !l.3)
                {
                    let serial = state.serial();
                    let (_, layer, (width, height), configured) = state.layer.as_mut().unwrap();
                    *configured = true;
                    layer.configure(serial, *width, *height);
                }
                if initial && state.popup.as_ref().is_some_and(|p| !p.3) {
                    let serial = state.serial();
                    let (xdg, popup, p, mapped) = state.popup.as_mut().unwrap();
                    *mapped = true;
                    // below the anchor rectangle, at its left edge
                    popup.configure(
                        p.anchor_rect[0],
                        p.anchor_rect[1] + p.anchor_rect[3],
                        p.size.0,
                        p.size.1,
                    );
                    xdg.configure(serial);
                }
                if let Some(buffer) = s.buffer.take() {
                    let seen = match (buffer.data::<BufferData>(), buffer.data::<SolidPixel>()) {
                        (Some(b), _) => Seen::Commit {
                            size: (b.width, b.height),
                            scale: s.scale.max(1),
                            format: b.format,
                        },
                        (None, Some(pixel)) => Seen::SolidCommit {
                            color: pixel.0,
                            scale: s.scale.max(1),
                            buffer: buffer.id().protocol_id(),
                        },
                        (None, None) => unreachable!("a buffer the mock made"),
                    };
                    state.seen.send(seen).unwrap();
                    // like a compositor that keeps the current buffer until the next one replaces it
                    let previous = state.shown.replace(buffer);
                    if let Some(previous) = previous.filter(|_| !state.globals.hold_buffers) {
                        previous.release();
                    }
                    state.frames.append(&mut s.frames);
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
            id,
            width,
            height,
            format,
            ..
        } = request
        {
            let format = format.into_result().unwrap();
            init.init(
                id,
                BufferData {
                    width,
                    height,
                    format,
                },
            );
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
        match request {
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                init.init(id, surface);
            }
            xdg_wm_base::Request::CreatePositioner { id } => {
                init.init(id, Mutex::new(PositionerState::default()));
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgPositioner, Mutex<PositionerState>> for Mock {
    fn request(
        _: &mut Self,
        _: &ServerClient,
        _: &XdgPositioner,
        request: xdg_positioner::Request,
        data: &Mutex<PositionerState>,
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
            xdg_positioner::Request::SetOffset { x, y } => p.offset = (x, y),
            xdg_positioner::Request::SetReactive => p.reactive = true,
            xdg_positioner::Request::SetParentSize {
                parent_width,
                parent_height,
            } => p.parent_size = Some((parent_width, parent_height)),
            xdg_positioner::Request::SetParentConfigure { serial } => {
                p.parent_configure = Some(serial)
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
            xdg_surface::Request::GetToplevel { id } => {
                let toplevel = init.init(id, ());
                state.toplevel = Some((surface.clone(), xdg.clone(), toplevel, None));
            }
            xdg_surface::Request::GetPopup { id, positioner, .. } => {
                let p = *positioner
                    .data::<Mutex<PositionerState>>()
                    .unwrap()
                    .lock()
                    .unwrap();
                state.seen.send(Seen::Positioner(p)).unwrap();
                let popup = init.init(id, ());
                state.popup = Some((xdg.clone(), popup, p, false));
            }
            xdg_surface::Request::AckConfigure { serial } => {
                state.seen.send(Seen::Ack(serial)).unwrap()
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
        let seen = match request {
            xdg_toplevel::Request::ShowWindowMenu { serial, x, y, .. } => {
                Seen::WindowMenu { serial, x, y }
            }
            xdg_toplevel::Request::Move { serial, .. } => Seen::Move(serial),
            xdg_toplevel::Request::Resize {
                serial,
                edges: wayland_server::WEnum::Value(edge),
                ..
            } => Seen::Resize { serial, edge },
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}

impl Dispatch<XdgPopup, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &XdgPopup,
        request: xdg_popup::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let xdg_popup::Request::Reposition { positioner, token } = request {
            let p = *positioner
                .data::<Mutex<PositionerState>>()
                .unwrap()
                .lock()
                .unwrap();
            state
                .seen
                .send(Seen::Reposition {
                    token,
                    positioner: p,
                })
                .unwrap();
            let serial = state.serial();
            let (xdg, popup, placed, _) = state.popup.as_mut().unwrap();
            *placed = p;
            popup.repositioned(token);
            popup.configure(
                p.anchor_rect[0],
                p.anchor_rect[1] + p.anchor_rect[3],
                p.size.0,
                p.size.1,
            );
            xdg.configure(serial);
        }
    }
}

impl Dispatch<XdgActivationV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &XdgActivationV1,
        request: xdg_activation_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_activation_v1::Request::GetActivationToken { id } => {
                init.init(id, Mutex::new(None));
            }
            xdg_activation_v1::Request::Activate { token, surface } => {
                state
                    .seen
                    .send(Seen::Activate {
                        token,
                        surface: surface.id().protocol_id(),
                    })
                    .unwrap();
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgActivationTokenV1, Mutex<Option<String>>> for Mock {
    fn request(
        state: &mut Self,
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
                let app_id = app_id.lock().unwrap().clone();
                token.done(format!("token-{}", app_id.as_deref().unwrap_or("none")));
                state.seen.send(Seen::TokenCommit(app_id)).unwrap();
            }
            _ => {}
        }
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
            let xdg = init.init(id, ());
            xdg.logical_position(1920, 0);
            xdg.logical_size(1280, 720);
            output.done();
        }
    }
}

impl Dispatch<ZxdgDecorationManagerV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_decoration_manager_v1::Request::GetToplevelDecoration { id, .. } = request {
            let decoration = init.init(id, ());
            state.toplevel.as_mut().unwrap().3 = Some(decoration);
        }
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Request::SetMode {
            mode: wayland_server::WEnum::Value(mode),
        } = request
        {
            state.seen.send(Seen::DecorationMode(mode)).unwrap();
        }
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WpFractionalScaleManagerV1,
        request: wp_fractional_scale_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wp_fractional_scale_manager_v1::Request::GetFractionalScale { id, .. } = request {
            state.fractional = Some(init.init(id, ()));
        }
    }
}

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
        if let wp_viewporter::Request::GetViewport { id, .. } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<WpViewport, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WpViewport,
        request: wp_viewport::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_viewport::Request::SetDestination { width, height } => {
                state.seen.send(Seen::Viewport(width, height)).unwrap();
            }
            wp_viewport::Request::SetSource {
                x,
                y,
                width,
                height,
            } => {
                state
                    .seen
                    .send(Seen::ViewportSource(x, y, width, height))
                    .unwrap();
            }
            _ => {}
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
        output.geometry(
            0,
            0,
            344,
            194,
            wl_output::Subpixel::Unknown,
            "Example".into(),
            "Unknown".into(),
            wl_output::Transform::Normal,
        );
        output.mode(wl_output::Mode::Current, 1920, 1080, 60_000);
        output.scale(state.globals.output_scale.unwrap_or(1));
        output.name("TEST-1".into());
        output.done();
        state.output = Some(output);
    }
}

impl GlobalDispatch<WlSeat, ()> for Mock {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &ServerClient,
        r: New<WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(r, ());
        seat.capabilities(
            wl_seat::Capability::Pointer
                | wl_seat::Capability::Keyboard
                | wl_seat::Capability::Touch,
        );
        state.seat = Some(seat);
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
            wl_seat::Request::GetKeyboard { id } => {
                let keyboard = init.init(id, ());
                let (fd, size) = us_keymap();
                keyboard.keymap(wl_keyboard::KeymapFormat::XkbV1, fd.as_fd(), size);
                keyboard.repeat_info(0, 0);
                state.keyboard = Some(keyboard);
            }
            _ => {}
        }
    }
}

/// The US layout as the keymap file a compositor sends: its text and a NUL.
fn us_keymap() -> (std::fs::File, u32) {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_names(
        &context,
        "",
        "",
        "us",
        "",
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .expect("the us layout compiles");
    let mut text = keymap
        .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
        .into_bytes();
    text.push(0);
    let mut file = std::fs::File::from(
        rustix::fs::memfd_create("keymap", rustix::fs::MemfdFlags::CLOEXEC).unwrap(),
    );
    file.write_all(&text).unwrap();
    (file, text.len() as u32)
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
            wl_data_device_manager::Request::GetDataDevice { id, .. } => {
                state.data_device = Some(init.init(id, ()))
            }
            wl_data_device_manager::Request::CreateDataSource { id } => {
                init.init(id, Mutex::new(SourceState::default()));
            }
            _ => {}
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
            serial,
            icon,
            ..
        } = request
        {
            state.drag_icon = icon;
            let offered = source.data::<Mutex<SourceState>>().unwrap().lock().unwrap();
            state
                .seen
                .send(Seen::StartDrag {
                    serial,
                    types: offered.types.clone(),
                    actions: offered.actions,
                })
                .unwrap();
            drop(offered);
            state.drag_source = Some(source);
        }
    }
}

impl Dispatch<WlDataOffer, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &WlDataOffer,
        request: wl_data_offer::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            wl_data_offer::Request::Accept { serial, mime_type } => Seen::Accept {
                serial,
                mime: mime_type,
            },
            wl_data_offer::Request::SetActions {
                dnd_actions,
                preferred_action,
            } => Seen::DragActions {
                actions: u32::from(dnd_actions),
                preferred: u32::from(preferred_action),
            },
            wl_data_offer::Request::Finish => Seen::Finish,
            wl_data_offer::Request::Destroy => Seen::OfferDestroyed,
            wl_data_offer::Request::Receive { mime_type, fd } => {
                let mut file = std::fs::File::from(fd);
                if state.globals.hold_reads {
                    state.held.push(file);
                } else {
                    file.write_all(format!("as {mime_type}").as_bytes())
                        .unwrap();
                }
                Seen::Receive(mime_type)
            }
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_layer_shell_v1::Request::GetLayerSurface { id, surface, .. } = request {
            let layer = init.init(id, ());
            state.layer = Some((surface, layer, (0, 0), false));
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            zwlr_layer_surface_v1::Request::SetSize { width, height } => {
                if let Some(layer) = state.layer.as_mut() {
                    layer.2 = (width, height);
                }
                return;
            }
            zwlr_layer_surface_v1::Request::AckConfigure { serial } => Seen::LayerAck(serial),
            zwlr_layer_surface_v1::Request::GetPopup { .. } => Seen::LayerPopup,
            zwlr_layer_surface_v1::Request::Destroy => Seen::LayerDestroyed,
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}

impl Dispatch<ZwpTextInputManagerV3, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZwpTextInputManagerV3,
        request: zwp_text_input_manager_v3::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_text_input_manager_v3::Request::GetTextInput { id, .. } = request {
            state.text_input = Some((init.init(id, ()), 0));
        }
    }
}

impl Dispatch<ZwpTextInputV3, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &ServerClient,
        _: &ZwpTextInputV3,
        request: zwp_text_input_v3::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            zwp_text_input_v3::Request::Enable => TextInputRequest::Enable,
            zwp_text_input_v3::Request::Disable => TextInputRequest::Disable,
            zwp_text_input_v3::Request::SetSurroundingText {
                text,
                cursor,
                anchor,
            } => TextInputRequest::Surrounding(text, cursor, anchor),
            zwp_text_input_v3::Request::SetTextChangeCause {
                cause: wayland_server::WEnum::Value(cause),
            } => TextInputRequest::Cause(cause),
            zwp_text_input_v3::Request::SetContentType {
                hint: wayland_server::WEnum::Value(hint),
                purpose: wayland_server::WEnum::Value(purpose),
            } => TextInputRequest::Content(hint, purpose),
            zwp_text_input_v3::Request::SetCursorRectangle {
                x,
                y,
                width,
                height,
            } => TextInputRequest::CursorRect([x, y, width, height]),
            zwp_text_input_v3::Request::Commit => {
                if let Some((_, commits)) = state.text_input.as_mut() {
                    *commits += 1;
                }
                TextInputRequest::Commit
            }
            _ => return,
        };
        state.seen.send(Seen::TextInput(seen)).unwrap();
    }
}

/// Run a compositor with `globals` on its own thread for the client at the other end of `stream`.
fn spawn_compositor(stream: UnixStream, globals: Globals) -> (Sender<Control>, Receiver<Seen>) {
    let (control_tx, control_rx) = channel::<Control>();
    let (seen_tx, seen_rx) = channel();
    std::thread::spawn(move || {
        let mut display = Display::<Mock>::new().unwrap();
        let dh = display.handle();
        dh.create_global::<Mock, WlCompositor, _>(6, ());
        dh.create_global::<Mock, WlShm, _>(1, ());
        if globals.xdg_wm_base {
            dh.create_global::<Mock, XdgWmBase, _>(6, ());
        }
        if globals.seat {
            dh.create_global::<Mock, WlSeat, _>(
                if globals.high_resolution_wheel { 9 } else { 7 },
                (),
            );
        }
        if globals.clipboard {
            let version = if globals.no_drag_actions { 2 } else { 3 };
            dh.create_global::<Mock, WlDataDeviceManager, _>(version, ());
        }
        if globals.decoration.is_some() {
            dh.create_global::<Mock, ZxdgDecorationManagerV1, _>(1, ());
        }
        if globals.fractional_scale {
            dh.create_global::<Mock, WpFractionalScaleManagerV1, _>(1, ());
        }
        if globals.fractional_scale || globals.single_pixel {
            dh.create_global::<Mock, WpViewporter, _>(1, ());
        }
        if globals.single_pixel {
            dh.create_global::<Mock, WpSinglePixelBufferManagerV1, _>(1, ());
        }
        if globals.output_scale.is_some() {
            dh.create_global::<Mock, WlOutput, _>(4, ());
        }
        if globals.layer_shell {
            dh.create_global::<Mock, ZwlrLayerShellV1, _>(4, ());
        }
        if globals.activation {
            dh.create_global::<Mock, XdgActivationV1, _>(1, ());
        }
        if globals.output_layout {
            dh.create_global::<Mock, ZxdgOutputManagerV1, _>(3, ());
        }
        if globals.text_input {
            dh.create_global::<Mock, ZwpTextInputManagerV3, _>(1, ());
        }
        if globals.presentation {
            dh.create_global::<Mock, WpPresentation, _>(2, ());
        }
        let mut mock = Mock {
            globals,
            seen: seen_tx,
            next_serial: 100,
            surfaces: Vec::new(),
            toplevel: None,
            configured: false,
            popup: None,
            output: None,
            fractional: None,
            pointer: None,
            keyboard: None,
            touch: None,
            data_device: None,
            shown: None,
            frames: Vec::new(),
            layer: None,
            text_input: None,
            feedbacks: Vec::new(),
            held: Vec::new(),
            seat: None,
            drag_offer: None,
            drag_source: None,
            drag_icon: None,
            icon_buffer: None,
        };
        let mut handle = display.handle();
        handle.insert_client(stream, Arc::new(Data)).unwrap();
        loop {
            match control_rx.try_recv() {
                Ok(control) => apply(&mut mock, &handle, control),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
            mock.frames.drain(..).for_each(|cb| cb.done(0));
            if display.dispatch_clients(&mut mock).is_err() {
                return;
            }
            let _ = display.flush_clients();
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    (control_tx, seen_rx)
}

fn apply(mock: &mut Mock, handle: &DisplayHandle, control: Control) {
    match control {
        Control::Configure {
            width,
            height,
            states,
        } => {
            let serial = mock.serial();
            let (_, xdg, toplevel, _) = mock.toplevel.as_ref().unwrap();
            toplevel.configure(
                width,
                height,
                states
                    .iter()
                    .flat_map(|s| (*s as u32).to_ne_bytes())
                    .collect(),
            );
            xdg.configure(serial);
        }
        Control::EnterOutput => mock.surface().enter(mock.output.as_ref().unwrap()),
        Control::WmCapabilities(capabilities) => {
            let serial = mock.serial();
            let (_, xdg, toplevel, _) = mock.toplevel.as_ref().unwrap();
            toplevel.wm_capabilities(
                capabilities
                    .iter()
                    .flat_map(|c| (*c as u32).to_ne_bytes())
                    .collect(),
            );
            toplevel.configure(0, 0, Vec::new());
            xdg.configure(serial);
        }
        Control::PreferredBufferScale(factor) => mock.surface().preferred_buffer_scale(factor),
        Control::FractionalScale(value) => mock.fractional.as_ref().unwrap().preferred_scale(value),
        Control::DismissPopup => mock.popup.as_ref().unwrap().1.popup_done(),
        Control::Click { x, y } => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let pointer = mock.pointer.as_ref().unwrap();
            pointer.enter(serial, &surface, x, y);
            pointer.motion(4000, x, y);
            pointer.button(serial + 1, 4242, BTN_LEFT, wl_pointer::ButtonState::Pressed);
            pointer.frame();
        }
        Control::ButtonUp => {
            let serial = mock.serial();
            let pointer = mock.pointer.as_ref().unwrap();
            pointer.button(serial, 4300, BTN_LEFT, wl_pointer::ButtonState::Released);
            pointer.frame();
        }
        Control::Motion { x, y } => {
            let pointer = mock.pointer.as_ref().unwrap();
            pointer.motion(4100, x, y);
            pointer.frame();
        }
        Control::TouchHold => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let touch = mock.touch.as_ref().unwrap();
            touch.down(serial, 200, &surface, 0, 5.0, 6.0);
            touch.frame();
            mock.seen.send(Seen::TouchDown(serial)).unwrap();
        }
        Control::TouchUp => {
            let serial = mock.serial();
            let touch = mock.touch.as_ref().unwrap();
            touch.up(serial, 220, 0);
            touch.frame();
        }
        Control::TouchSecondFinger => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let touch = mock.touch.as_ref().unwrap();
            touch.down(serial, 210, &surface, 1, 9.0, 9.0);
            touch.frame();
        }
        Control::Button { button, pressed } => {
            // `Click` presses with the serial after the one it took: skip it, so this serial is a new one
            let serial = mock.serial() + 1;
            mock.next_serial = serial;
            let state = if pressed {
                wl_pointer::ButtonState::Pressed
            } else {
                wl_pointer::ButtonState::Released
            };
            let pointer = mock.pointer.as_ref().unwrap();
            pointer.button(serial, 4250, button, state);
            pointer.frame();
        }
        Control::Leave => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let pointer = mock.pointer.as_ref().unwrap();
            pointer.leave(serial, &surface);
            pointer.frame();
        }
        Control::Touch => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let touch = mock.touch.as_ref().unwrap();
            touch.down(serial, 100, &surface, 0, 5.0, 6.0);
            touch.frame();
            touch.down(serial + 1, 105, &surface, 1, 9.0, 9.0);
            touch.motion(110, 0, 7.5, 8.5);
            touch.frame();
            touch.up(serial + 2, 120, 0);
            touch.frame();
        }
        Control::CancelTouch => mock.touch.as_ref().unwrap().cancel(),
        Control::DropTouch => mock
            .seat
            .as_ref()
            .unwrap()
            .capabilities(wl_seat::Capability::Pointer | wl_seat::Capability::Keyboard),
        Control::DropPointer => mock
            .seat
            .as_ref()
            .unwrap()
            .capabilities(wl_seat::Capability::Keyboard),
        Control::TypeKey(code) => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let keyboard = mock.keyboard.as_ref().unwrap();
            keyboard.enter(serial, &surface, Vec::new());
            keyboard.key(serial + 1, 777, code, wl_keyboard::KeyState::Pressed);
        }
        Control::ReleaseKey(code) => {
            let serial = mock.serial();
            let keyboard = mock.keyboard.as_ref().unwrap();
            keyboard.key(serial, 778, code, wl_keyboard::KeyState::Released);
        }
        Control::RepeatInfo { rate, delay } => {
            mock.keyboard.as_ref().unwrap().repeat_info(rate, delay);
        }
        Control::Offer(types) => {
            let device = mock.data_device.as_ref().unwrap();
            let offer = device
                .client()
                .unwrap()
                .create_resource::<WlDataOffer, (), Mock>(handle, device.version(), ())
                .unwrap();
            device.data_offer(&offer);
            for t in types {
                offer.offer(t.into());
            }
            device.selection(Some(&offer));
        }
        Control::LayerConfigures(count) => {
            for i in 0..count {
                let serial = mock.serial();
                let (_, layer, (width, height), _) = mock.layer.as_ref().unwrap();
                layer.configure(serial, width + i + 1, *height);
            }
        }
        Control::Scroll { dx, dy } => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let pointer = mock.pointer.as_ref().unwrap();
            pointer.enter(serial, &surface, 1.0, 2.0);
            pointer.frame();
            pointer.axis_source(wl_pointer::AxisSource::Wheel);
            for (axis, value, notches) in [
                (wl_pointer::Axis::VerticalScroll, dy, -1),
                (wl_pointer::Axis::HorizontalScroll, dx, 1),
                (wl_pointer::Axis::VerticalScroll, dy, -1),
            ] {
                if pointer.version() >= 8 {
                    pointer.axis_value120(axis, notches * 60);
                } else {
                    pointer.axis_discrete(axis, notches);
                }
                if pointer.version() >= 9 && axis == wl_pointer::Axis::VerticalScroll {
                    pointer
                        .axis_relative_direction(axis, wl_pointer::AxisRelativeDirection::Inverted);
                }
                pointer.axis(900, axis, value);
            }
            pointer.frame();
            pointer.axis_source(wl_pointer::AxisSource::Finger);
            pointer.axis_stop(950, wl_pointer::Axis::VerticalScroll);
            pointer.frame();
        }
        Control::TextInputEnter => {
            let surface = mock.surface().clone();
            mock.text_input.as_ref().unwrap().0.enter(&surface);
        }
        Control::TextInputLeave => {
            let surface = mock.surface().clone();
            mock.text_input.as_ref().unwrap().0.leave(&surface);
        }
        Control::DragEnter { types, actions } => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let device = mock.data_device.as_ref().unwrap();
            let offer = device
                .client()
                .unwrap()
                .create_resource::<WlDataOffer, (), Mock>(handle, device.version(), ())
                .unwrap();
            device.data_offer(&offer);
            for t in types {
                offer.offer(t.into());
            }
            if offer.version() >= 3 {
                offer.source_actions(actions);
            }
            device.enter(serial, &surface, 4.0, 5.0, Some(&offer));
            mock.drag_offer = Some(offer);
            mock.seen.send(Seen::DragEntered(serial)).unwrap();
        }
        Control::DragMotion => {
            mock.data_device.as_ref().unwrap().motion(5000, 6.0, 7.5);
            mock.seen.send(Seen::DragMoved).unwrap();
        }
        Control::DragAction(action) => mock.drag_offer.as_ref().unwrap().action(action),
        Control::DragDrop => mock.data_device.as_ref().unwrap().drop(),
        Control::DragLeave => mock.data_device.as_ref().unwrap().leave(),
        Control::DragEnterEmpty => {
            let (serial, surface) = (mock.serial(), mock.surface().clone());
            let device = mock.data_device.as_ref().unwrap();
            device.enter(serial, &surface, 4.0, 5.0, None);
        }
        Control::SourceTarget(mime) => mock
            .drag_source
            .as_ref()
            .unwrap()
            .target(mime.map(String::from)),
        Control::SourceSend(mime) => {
            let (mut reader, writer) = std::io::pipe().unwrap();
            mock.drag_source
                .as_ref()
                .unwrap()
                .send(mime.into(), writer.as_fd());
            drop(writer);
            let seen = mock.seen.clone();
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut reader, &mut bytes).unwrap();
                let _ = seen.send(Seen::SourceData(bytes));
            });
        }
        Control::SourceDropped => mock.drag_source.as_ref().unwrap().dnd_drop_performed(),
        Control::SourceFinished => mock.drag_source.as_ref().unwrap().dnd_finished(),
        Control::SourceCancelled => mock.drag_source.as_ref().unwrap().cancelled(),
        Control::Release(bytes) => {
            for mut file in mock.held.drain(..) {
                let _ = file.write_all(bytes);
            }
        }
        Control::Trickle { every, count } => {
            for mut file in mock.held.drain(..) {
                std::thread::spawn(move || {
                    for _ in 0..count.unwrap_or(usize::MAX) {
                        std::thread::sleep(every);
                        if file.write_all(b"x").is_err() {
                            return;
                        }
                    }
                });
            }
        }
        Control::Flood(len) => {
            for mut file in mock.held.drain(..) {
                // the reader stops at its limit and closes the pipe, which ends the write
                std::thread::spawn(move || {
                    let _ = file.write_all(&vec![b'x'; len]);
                    std::thread::sleep(Duration::from_secs(2));
                });
            }
        }
        Control::Presentation { discard } => {
            for feedback in mock.feedbacks.drain(..) {
                if discard {
                    feedback.discarded();
                } else {
                    // 2^32 + 5 s and 250 ns, on a 60 Hz output's 2^32 + 7th retrace
                    feedback.presented(
                        1,
                        5,
                        250,
                        16_666_666,
                        1,
                        7,
                        wp_presentation_feedback::Kind::Vsync
                            | wp_presentation_feedback::Kind::HwClock,
                    );
                }
            }
        }
        Control::PreeditOnly(text) => {
            let input = &mock.text_input.as_ref().unwrap().0;
            input.preedit_string(Some(text.into()), 0, 0);
        }
        Control::Compose {
            preedit,
            commit,
            delete,
            serial,
        } => {
            let (input, commits) = mock.text_input.as_ref().unwrap();
            if let Some((text, begin, end)) = preedit {
                input.preedit_string(Some(text.into()), begin, end);
            }
            if let Some(text) = commit {
                input.commit_string(Some(text.into()));
            }
            input.delete_surrounding_text(delete.0, delete.1);
            input.done(serial.unwrap_or(*commits));
        }
    }
}

/// A connection to a fresh compositor with `globals`.
fn connect(globals: Globals) -> (Connection, Sender<Control>, Receiver<Seen>) {
    let (client, server) = UnixStream::pair().unwrap();
    let (control, seen) = spawn_compositor(server, globals);
    (Connection::from_stream(client).unwrap(), control, seen)
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

fn next_seen(seen: &Receiver<Seen>) -> Seen {
    seen.recv_timeout(Duration::from_secs(5))
        .expect("the compositor saw a request")
}

fn window(conn: &mut Connection, decorations: Decorations) -> Toplevel {
    conn.toplevel(&ToplevelConfig {
        app_id: "test".into(),
        title: "Test".into(),
        min_size: None,
        max_size: None,
        decorations,
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

fn present(conn: &mut Connection, surface: SurfaceId, preferred: (u32, u32)) {
    conn.lease(surface, preferred, BufferFormat::Xrgb8888)
        .unwrap()
        .present()
        .unwrap();
}

#[test]
fn the_core_globals_alone_map_a_window_the_client_decorates() {
    let (mut conn, _control, seen) = connect(Globals::core());
    assert_eq!(conn.capabilities().iter().count(), 0);
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Server);
    let configure = configured(&mut conn, &mut stash, w);
    assert_eq!(
        (configure.size, configure.decorations),
        (None, Decorations::Client)
    );
    present(&mut conn, w.surface(), (320, 240));
    assert_eq!(next_seen(&seen), Seen::Ack(configure.serial));
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (320, 240),
            scale: 1,
            format: wl_shm::Format::Xrgb8888
        }
    );
    // the same configure is acknowledged once
    wait_for(&mut conn, &mut stash, |e| {
        matches!(e, Event::Frame(_)).then_some(())
    });
    present(&mut conn, w.surface(), (320, 240));
    assert!(matches!(next_seen(&seen), Seen::Commit { .. }));
}

#[test]
fn a_compositor_without_xdg_wm_base_is_refused_by_name() {
    let (client, server) = UnixStream::pair().unwrap();
    let _compositor = spawn_compositor(server, Globals::default());
    assert!(matches!(
        Connection::from_stream(client),
        Err(Error::MissingRequiredProtocol {
            interface: "xdg_wm_base",
            ..
        })
    ));
}

#[test]
fn each_resize_draws_at_the_configured_size_and_acknowledges_its_serial() {
    let (mut conn, control, seen) = connect(Globals::core());
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    present(&mut conn, w.surface(), (320, 240));
    let mut last = match next_seen(&seen) {
        Seen::Ack(serial) => serial,
        other => panic!("{other:?}"),
    };
    next_seen(&seen);
    for (width, height, states) in [
        (3840, 2160, vec![xdg_toplevel::State::Maximized]),
        (1280, 800, vec![xdg_toplevel::State::Resizing]),
        (320, 240, vec![]),
    ] {
        control
            .send(Control::Configure {
                width,
                height,
                states,
            })
            .unwrap();
        let c = configured(&mut conn, &mut stash, w);
        assert_eq!(c.size, Some((width as u32, height as u32)));
        assert!(c.serial > last, "configure serials increase");
        last = c.serial;
        present(&mut conn, w.surface(), (100, 100));
        assert_eq!(next_seen(&seen), Seen::Ack(c.serial));
        assert_eq!(
            next_seen(&seen),
            Seen::Commit {
                size: (width, height),
                scale: 1,
                format: wl_shm::Format::Xrgb8888
            }
        );
    }
}

#[test]
fn a_configure_overtaken_before_the_draw_is_skipped_for_the_latest() {
    let (mut conn, control, seen) = connect(Globals::core());
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    present(&mut conn, w.surface(), (320, 240));
    next_seen(&seen);
    next_seen(&seen);
    for (width, height) in [(640, 480), (1024, 768)] {
        control
            .send(Control::Configure {
                width,
                height,
                states: vec![],
            })
            .unwrap();
    }
    let latest = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Configure { window, configure }
            if *window == w && configure.size == Some((1024, 768)) =>
        {
            Some(*configure)
        }
        _ => None,
    });
    let lease = conn
        .lease(w.surface(), (100, 100), BufferFormat::Xrgb8888)
        .unwrap();
    assert_eq!(lease.logical_size(), (1024, 768));
    lease.present().unwrap();
    assert_eq!(next_seen(&seen), Seen::Ack(latest.serial));
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (1024, 768),
            scale: 1,
            format: wl_shm::Format::Xrgb8888
        }
    );
}

#[test]
fn a_leased_buffer_names_what_it_missed_of_the_frames_shown_in_the_others() {
    let (mut conn, _control, _seen) = connect(Globals::core());
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let s = w.surface();
    let r = |x, y, width, height| PixelRect {
        x,
        y,
        width,
        height,
    };
    // lease, maybe write, present `damage` unless it is `None`, and say what the buffer missed
    let missed = |conn: &mut Connection, damage: Option<&[PixelRect]>, write: bool| {
        let mut lease = conn.lease(s, (64, 64), BufferFormat::Xrgb8888).unwrap();
        let missed = lease.missed().map(<[_]>::to_vec);
        if write {
            lease.pixels_mut()[0] = 1;
        }
        if let Some(damage) = damage {
            lease.present_damage(Some(damage)).unwrap();
        }
        // the compositor releases the buffer it showed before
        conn.roundtrip().unwrap();
        missed
    };
    let all = [r(0, 0, 64, 64)];
    assert_eq!(missed(&mut conn, Some(&all), false), None, "a new buffer");
    assert_eq!(
        missed(&mut conn, Some(&[r(60, 50, 10, 10)]), false),
        None,
        "a second new buffer while the first is shown"
    );
    assert_eq!(
        missed(&mut conn, Some(&[r(1, 2, 3, 4), r(5, 6, 7, 8)]), false),
        Some(vec![r(60, 50, 4, 10)]),
        "the first buffer missed the frame of the second, cut to the buffer"
    );
    assert_eq!(
        missed(&mut conn, None, true),
        Some(vec![r(1, 2, 3, 4), r(5, 6, 7, 8)])
    );
    let lease = conn.lease(s, (64, 64), BufferFormat::Xrgb8888).unwrap();
    assert_eq!(lease.missed(), None, "written but not presented");
    lease.present().unwrap();
    conn.roundtrip().unwrap();
    let lease = conn.lease(s, (64, 64), BufferFormat::Xrgb8888).unwrap();
    assert_eq!(lease.missed(), None, "a whole frame passed it by");
    lease.present_damage(Some(&[])).unwrap();
    conn.roundtrip().unwrap();
    assert_eq!(
        conn.lease(s, (64, 64), BufferFormat::Xrgb8888)
            .unwrap()
            .missed(),
        Some(&[][..]),
        "an empty frame passed it by"
    );
    let lease = conn.lease(s, (80, 64), BufferFormat::Xrgb8888).unwrap();
    assert_eq!(lease.missed(), None, "a buffer of the new size");
    lease.present_damage(Some(&[r(0, 0, 1, 1)])).unwrap();
    conn.roundtrip().unwrap();
    let lease = conn.lease(s, (64, 64), BufferFormat::Xrgb8888).unwrap();
    assert_eq!(
        lease.missed(),
        None,
        "back at the old size, after frames of another"
    );
}

#[test]
fn a_lease_waits_while_the_compositor_holds_three_buffers() {
    let (mut conn, _control, seen) = connect(Globals {
        hold_buffers: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    assert!(conn.buffer_free(w.surface()).unwrap());
    for _ in 0..3 {
        present(&mut conn, w.surface(), (64, 64));
    }
    assert!(!conn.buffer_free(w.surface()).unwrap());
    assert!(matches!(
        conn.lease(w.surface(), (64, 64), BufferFormat::Xrgb8888),
        Err(Error::NoFreeBuffer)
    ));
    let commits = std::iter::from_fn(|| seen.recv_timeout(Duration::from_millis(200)).ok())
        .filter(|s| matches!(s, Seen::Commit { .. }))
        .count();
    assert_eq!(commits, 3);
}

#[test]
fn the_scale_follows_the_output_then_the_preferred_buffer_scale() {
    let (mut conn, control, seen) = connect(Globals {
        output_scale: Some(2),
        ..Globals::core()
    });
    assert_eq!(conn.outputs()[0].name.as_deref(), Some("TEST-1"));
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::EnterOutput).unwrap();
    let scale = wait_for(&mut conn, &mut stash, |e| match e {
        Event::ScaleChanged { scale, .. } => Some(*scale),
        _ => None,
    });
    assert_eq!(scale, Scale::from_integer(2).unwrap());
    present(&mut conn, w.surface(), (320, 240));
    next_seen(&seen);
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (640, 480),
            scale: 2,
            format: wl_shm::Format::Xrgb8888
        }
    );
    control.send(Control::PreferredBufferScale(3)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(e, Event::ScaleChanged { .. }).then_some(())
    });
    present(&mut conn, w.surface(), (320, 240));
    assert!(matches!(
        next_seen(&seen),
        Seen::Commit {
            size: (960, 720),
            scale: 3,
            ..
        }
    ));
}

#[test]
fn a_zoomed_frame_shows_the_middle_of_its_buffer_and_the_next_unzoomed_one_all_of_it() {
    let (mut conn, _control, seen) = connect(Globals {
        fractional_scale: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let mut lease = conn
        .lease(w.surface(), (400, 200), BufferFormat::Xrgb8888)
        .unwrap();
    assert!(lease.set_zoom(1.25));
    lease.present().unwrap();
    assert!(matches!(next_seen(&seen), Seen::Ack(_)));
    assert_eq!(
        next_seen(&seen),
        Seen::ViewportSource(40.0, 20.0, 320.0, 160.0)
    );
    assert_eq!(next_seen(&seen), Seen::Viewport(400, 200));
    assert!(matches!(next_seen(&seen), Seen::Commit { .. }));
    present(&mut conn, w.surface(), (400, 200));
    assert_eq!(
        next_seen(&seen),
        Seen::ViewportSource(-1.0, -1.0, -1.0, -1.0)
    );
    assert_eq!(next_seen(&seen), Seen::Viewport(400, 200));
}

#[test]
fn a_fractional_scale_draws_more_pixels_into_the_same_logical_size() {
    let (mut conn, control, seen) = connect(Globals {
        fractional_scale: true,
        ..Globals::core()
    });
    assert!(conn.capabilities().contains(Capability::FractionalScale));
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::FractionalScale(180)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(e, Event::ScaleChanged { .. }).then_some(())
    });
    present(&mut conn, w.surface(), (320, 240));
    assert!(matches!(next_seen(&seen), Seen::Ack(_)));
    assert_eq!(next_seen(&seen), Seen::Viewport(320, 240));
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (480, 360),
            scale: 1,
            format: wl_shm::Format::Xrgb8888
        }
    );
}

#[test]
fn decorations_are_the_compositors_when_granted_and_the_clients_when_refused() {
    for (granted, expected) in [
        (Mode::ServerSide, Decorations::Server),
        (Mode::ClientSide, Decorations::Client),
    ] {
        let (mut conn, _control, seen) = connect(Globals {
            decoration: Some(granted),
            ..Globals::core()
        });
        let mut stash = Stash::new();
        let w = window(&mut conn, Decorations::Server);
        conn.flush().unwrap();
        assert_eq!(next_seen(&seen), Seen::DecorationMode(Mode::ServerSide));
        assert_eq!(configured(&mut conn, &mut stash, w).decorations, expected);
    }
}

#[test]
fn decorations_can_be_asked_again_and_only_a_compositor_with_the_protocol_draws_them() {
    let (mut conn, _control, seen) = connect(Globals {
        decoration: Some(Mode::ServerSide),
        ..Globals::core()
    });
    let w = window(&mut conn, Decorations::Server);
    conn.set_decorations(w, Decorations::Client).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_seen(&seen), Seen::DecorationMode(Mode::ServerSide));
    assert_eq!(next_seen(&seen), Seen::DecorationMode(Mode::ClientSide));

    let (mut conn, _control, _seen) = connect(Globals::core());
    let w = window(&mut conn, Decorations::Client);
    assert!(conn.set_decorations(w, Decorations::Client).is_ok());
    assert!(matches!(
        conn.set_decorations(w, Decorations::Server),
        Err(Error::Unsupported(Capability::ServerDecoration))
    ));
}

#[test]
fn the_window_menu_answers_the_latest_click_and_capabilities_arrive_with_the_configure() {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    assert_eq!(configured(&mut conn, &mut stash, w).capabilities, None);
    assert!(matches!(
        conn.show_window_menu(w, Point::new(1.0, 2.0)),
        Err(Error::PermissionDenied(_))
    ));
    control.send(Control::Click { x: 3.0, y: 4.0 }).unwrap();
    wait_for(&mut conn, &mut stash, |e| match e {
        Event::Pointer {
            event: PointerEvent::Button { .. },
            ..
        } => Some(()),
        _ => None,
    });
    let click_serial = conn.input_serial().unwrap();
    conn.show_window_menu(w, Point::new(10.6, 20.4)).unwrap();
    conn.flush().unwrap();
    let menu = loop {
        if let Seen::WindowMenu { serial, x, y } = next_seen(&seen) {
            break (serial, x, y);
        }
    };
    assert_eq!(menu, (click_serial, 11, 20));

    control
        .send(Control::WmCapabilities(vec![
            xdg_toplevel::WmCapabilities::WindowMenu,
            xdg_toplevel::WmCapabilities::Minimize,
        ]))
        .unwrap();
    assert_eq!(
        configured(&mut conn, &mut stash, w).capabilities,
        Some(WmCapabilities {
            window_menu: true,
            maximize: false,
            fullscreen: false,
            minimize: true,
        })
    );
}

/// The next move, resize or window menu the compositor saw, skipping other requests.
fn next_window_gesture(seen: &Receiver<Seen>) -> Seen {
    loop {
        let s = next_seen(seen);
        if matches!(
            s,
            Seen::Move(_) | Seen::Resize { .. } | Seen::WindowMenu { .. }
        ) {
            return s;
        }
    }
}

#[test]
fn a_move_resize_and_window_menu_answer_the_held_press_after_motion_and_keys() {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::Click { x: 3.0, y: 4.0 }).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Button { pressed: true, .. },
                ..
            }
        )
        .then_some(())
    });
    let press = conn.input_serial().unwrap();
    assert_eq!(
        conn.held_press(),
        Some(HeldPress {
            serial: press,
            device: PressDevice::Pointer,
        })
    );

    // the pointer moves past a drag threshold and a key goes down and up before the move starts
    control.send(Control::Motion { x: 9.0, y: 4.0 }).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Motion(_),
                ..
            }
        )
        .then_some(())
    });
    control.send(Control::TypeKey(KEY_A)).unwrap();
    control.send(Control::ReleaseKey(KEY_A)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Keyboard {
                event: KeyboardEvent::Key { pressed: false, .. },
                ..
            }
        )
        .then_some(())
    });
    assert_ne!(conn.input_serial(), Some(press));
    assert_eq!(conn.held_press().map(|p| p.serial), Some(press));

    conn.start_move(w).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_window_gesture(&seen), Seen::Move(press));
    conn.start_resize(w, ResizeEdge::BottomRight).unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_window_gesture(&seen),
        Seen::Resize {
            serial: press,
            edge: xdg_toplevel::ResizeEdge::BottomRight,
        }
    );
    conn.show_window_menu(w, Point::new(1.0, 2.0)).unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_window_gesture(&seen),
        Seen::WindowMenu {
            serial: press,
            x: 1,
            y: 2,
        }
    );
}

#[test]
fn with_no_press_held_a_move_resize_or_window_menu_is_refused_and_not_sent() {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let refused = |conn: &mut Connection, why: &str| {
        assert_eq!(conn.held_press(), None, "{why}");
        assert!(
            matches!(conn.start_move(w), Err(Error::PermissionDenied(_))),
            "{why}: move"
        );
        assert!(
            matches!(
                conn.start_resize(w, ResizeEdge::Left),
                Err(Error::PermissionDenied(_))
            ),
            "{why}: resize"
        );
        assert!(
            matches!(
                conn.show_window_menu(w, Point::new(1.0, 2.0)),
                Err(Error::PermissionDenied(_))
            ),
            "{why}: window menu"
        );
    };
    refused(&mut conn, "before any input");

    // a key gives the client a serial, but no press
    control.send(Control::TypeKey(KEY_A)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Keyboard {
                event: KeyboardEvent::Key { .. },
                ..
            }
        )
        .then_some(())
    });
    assert!(conn.input_serial().is_some());
    refused(&mut conn, "after a key");

    for (what, end) in [("released", Control::ButtonUp), ("left", Control::Leave)] {
        control.send(Control::Click { x: 3.0, y: 4.0 }).unwrap();
        wait_for(&mut conn, &mut stash, |e| {
            matches!(
                e,
                Event::Pointer {
                    event: PointerEvent::Button { pressed: true, .. },
                    ..
                }
            )
            .then_some(())
        });
        control.send(end).unwrap();
        conn.roundtrip().unwrap();
        stash.extend(conn.wait(Some(Duration::ZERO)).unwrap());
        refused(&mut conn, what);
    }

    // none of the refused requests reached the compositor: the first it sees answers a press held now
    control.send(Control::Click { x: 3.0, y: 4.0 }).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Button { pressed: true, .. },
                ..
            }
        )
        .then_some(())
    });
    let press = conn.input_serial().unwrap();
    conn.start_move(w).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_window_gesture(&seen), Seen::Move(press));
}

#[test]
fn a_move_answers_the_touch_down_held_and_not_after_the_finger_lifts() {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    for end in [Control::TouchUp, Control::CancelTouch] {
        control.send(Control::TouchHold).unwrap();
        let down = loop {
            if let Seen::TouchDown(serial) = next_seen(&seen) {
                break serial;
            }
        };
        wait_for(&mut conn, &mut stash, |e| {
            matches!(
                e,
                Event::Touch {
                    event: TouchEvent::Down { .. },
                    ..
                }
            )
            .then_some(())
        });
        control.send(Control::TypeKey(KEY_A)).unwrap();
        wait_for(&mut conn, &mut stash, |e| {
            matches!(
                e,
                Event::Keyboard {
                    event: KeyboardEvent::Key { .. },
                    ..
                }
            )
            .then_some(())
        });
        assert_ne!(conn.input_serial(), Some(down));
        assert_eq!(
            conn.held_press(),
            Some(HeldPress {
                serial: down,
                device: PressDevice::Touch,
            })
        );
        conn.start_move(w).unwrap();
        conn.flush().unwrap();
        assert_eq!(next_window_gesture(&seen), Seen::Move(down));

        control.send(end).unwrap();
        conn.roundtrip().unwrap();
        stash.extend(conn.wait(Some(Duration::ZERO)).unwrap());
        assert_eq!(conn.held_press(), None);
        assert!(matches!(
            conn.start_move(w),
            Err(Error::PermissionDenied(_))
        ));
    }
}

#[test]
fn the_held_press_is_the_first_button_or_finger_down_and_the_device_that_pressed_last() {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let pointer_button =
        |conn: &mut Connection, stash: &mut Stash, button: Button, pressed: bool| {
            wait_for(conn, stash, |e| {
                matches!(
                    e,
                    Event::Pointer {
                        event: PointerEvent::Button { button: b, pressed: p, .. },
                        ..
                    } if *b == button && *p == pressed
                )
                .then_some(())
            });
            conn.input_serial().unwrap()
        };
    let held = |serial, device| Some(HeldPress { serial, device });
    control.send(Control::Click { x: 3.0, y: 4.0 }).unwrap();
    let left = pointer_button(&mut conn, &mut stash, Button::Left, true);

    // a second button keeps the implicit grab of the first, which the compositor's grab serial names
    control
        .send(Control::Button {
            button: BTN_RIGHT,
            pressed: true,
        })
        .unwrap();
    let right = pointer_button(&mut conn, &mut stash, Button::Right, true);
    assert_ne!(right, left);
    assert_eq!(conn.held_press(), held(left, PressDevice::Pointer));

    // a finger going down while the button is held starts the touch grab, which is now the latest
    control.send(Control::TouchHold).unwrap();
    let down = loop {
        if let Seen::TouchDown(serial) = next_seen(&seen) {
            break serial;
        }
    };
    let touch = |conn: &mut Connection, stash: &mut Stash, up: bool| {
        wait_for(conn, stash, |e| match e {
            Event::Touch {
                event: TouchEvent::Up { .. },
                ..
            } => up.then_some(()),
            Event::Touch {
                event: TouchEvent::Down { .. },
                ..
            } => (!up).then_some(()),
            _ => None,
        })
    };
    touch(&mut conn, &mut stash, false);
    assert_eq!(conn.held_press(), held(down, PressDevice::Touch));

    // a second finger, and the first lifting while the second stays, keep the first finger's grab
    control.send(Control::TouchSecondFinger).unwrap();
    touch(&mut conn, &mut stash, false);
    assert_eq!(conn.held_press(), held(down, PressDevice::Touch));
    control.send(Control::TouchUp).unwrap();
    touch(&mut conn, &mut stash, true);
    assert_ne!(conn.input_serial(), Some(down));
    assert_eq!(conn.held_press(), held(down, PressDevice::Touch));
    conn.start_move(w).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_window_gesture(&seen), Seen::Move(down));

    // the second button going up leaves the first held; the touch screen going away leaves the pointer's press
    control
        .send(Control::Button {
            button: BTN_RIGHT,
            pressed: false,
        })
        .unwrap();
    pointer_button(&mut conn, &mut stash, Button::Right, false);
    assert_eq!(conn.held_press(), held(down, PressDevice::Touch));
    control.send(Control::DropTouch).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Touch {
                event: TouchEvent::Cancel,
                ..
            }
        )
        .then_some(())
    });
    assert_eq!(conn.held_press(), held(left, PressDevice::Pointer));
    conn.start_resize(w, ResizeEdge::Right).unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_window_gesture(&seen),
        Seen::Resize {
            serial: left,
            edge: xdg_toplevel::ResizeEdge::Right,
        }
    );

    control.send(Control::ButtonUp).unwrap();
    pointer_button(&mut conn, &mut stash, Button::Left, false);
    assert_eq!(conn.held_press(), None);
}

#[test]
fn a_popup_is_drawn_where_and_as_large_as_the_compositor_placed_it() {
    let (mut conn, control, seen) = connect(Globals::core());
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    present(&mut conn, w.surface(), (320, 240));
    next_seen(&seen);
    next_seen(&seen);
    let popup = conn
        .popup(
            w.surface(),
            &PopupPlacement {
                anchor_rect: Rect::new(10.0, 20.0, 30.5, 8.0),
                size: (120, 90),
                anchor: Anchor::BottomLeft,
                gravity: Gravity::BottomRight,
                constraint_adjustment: ConstraintAdjustment::FlipY,
                offset: (0, 0),
                reactive: false,
                parent_size: None,
                grab: false,
            },
        )
        .unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_seen(&seen),
        Seen::Positioner(PositionerState {
            size: (120, 90),
            anchor_rect: [10, 20, 31, 8],
            ..PositionerState::default()
        })
    );
    let placed = wait_for(&mut conn, &mut stash, |e| match e {
        Event::PopupConfigure {
            popup: p,
            configure,
        } if *p == popup => Some(*configure),
        _ => None,
    });
    assert_eq!(
        (placed.x, placed.y, placed.width, placed.height),
        (10, 28, 120, 90),
        "{placed:?}"
    );
    present(&mut conn, popup.surface(), (1, 1));
    assert_eq!(next_seen(&seen), Seen::Ack(placed.serial));
    assert!(matches!(
        next_seen(&seen),
        Seen::Commit {
            size: (120, 90),
            ..
        }
    ));
    control.send(Control::DismissPopup).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        (*e == Event::PopupDone(popup)).then_some(())
    });
    conn.destroy(w.surface()).unwrap();
    assert!(matches!(
        conn.scale(popup.surface()),
        Err(Error::UnknownSurface)
    ));
}

#[test]
fn pointer_and_key_input_arrive_typed_on_the_focused_window() {
    let (mut conn, control, _seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::Click { x: 12.5, y: 7.25 }).unwrap();
    let motion = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Pointer {
            time,
            event: PointerEvent::Motion(_),
            ..
        } => Some(*time),
        _ => None,
    });
    assert_eq!(motion, 4000);
    let button = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Pointer {
            surface,
            time,
            event:
                PointerEvent::Button {
                    button,
                    pressed,
                    position,
                },
        } if *surface == w.surface() => Some((*button, *pressed, position.x, position.y, *time)),
        _ => None,
    });
    assert_eq!(button, (Button::Left, true, 12.5, 7.25, 4242));
    control.send(Control::TypeKey(KEY_A)).unwrap();
    let key = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Keyboard {
            time,
            event:
                KeyboardEvent::Key {
                    code,
                    character,
                    repeat,
                    ..
                },
            ..
        } => Some((*code, *character, *repeat, *time)),
        _ => None,
    });
    assert_eq!(key, (KEY_A, Some('a'), false, 777));
    // an enter carries no time of its own: it takes the key's, advanced by the wait since
    control.send(Control::Scroll { dx: 0.0, dy: 1.0 }).unwrap();
    let enter = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Pointer {
            time,
            event: PointerEvent::Enter(_),
            ..
        } => Some(*time),
        _ => None,
    });
    assert!((777..1777).contains(&enter), "{enter}");
}

#[test]
fn a_key_released_while_the_client_was_busy_repeats_no_more() {
    let (mut conn, control, _seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control
        .send(Control::RepeatInfo {
            rate: 25,
            delay: 100,
        })
        .unwrap();
    control.send(Control::TypeKey(KEY_A)).unwrap();
    wait_for(&mut conn, &mut stash, |e| match e {
        Event::Keyboard {
            event: KeyboardEvent::Key { code: KEY_A, .. },
            ..
        } => Some(()),
        _ => None,
    });
    control.send(Control::ReleaseKey(KEY_A)).unwrap();
    // busy past the delay and several intervals, with the release unread in the socket
    std::thread::sleep(Duration::from_millis(500));
    let events = conn.wait(Some(Duration::ZERO)).unwrap();
    let keys: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Keyboard {
                event:
                    KeyboardEvent::Key {
                        pressed, repeat, ..
                    },
                ..
            } => Some((*pressed, *repeat)),
            _ => None,
        })
        .collect();
    assert_eq!(keys, [(false, false)]);
}

#[test]
fn an_offer_keeps_at_most_64_types_and_none_longer_than_255_bytes() {
    let (mut conn, control, _seen) = connect(Globals {
        seat: true,
        clipboard: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let long: &'static str = Box::leak("x".repeat(256).into_boxed_str());
    let mut types = vec![long];
    types.extend(
        (0..70).map(|i| -> &'static str { Box::leak(format!("type/{i}").into_boxed_str()) }),
    );
    control.send(Control::Offer(types)).unwrap();
    let offer = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Selection(Some(offer)) => Some(offer.clone()),
        _ => None,
    });
    let kept: Vec<String> = (0..64).map(|i| format!("type/{i}")).collect();
    assert_eq!(offer.mime_types(), kept.as_slice());
}

#[test]
fn the_selection_is_read_in_an_offered_type_only_and_a_replaced_offer_is_cancelled() {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        clipboard: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    control
        .send(Control::Offer(vec!["text/plain", "image/png"]))
        .unwrap();
    let offer = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Selection(Some(offer)) => Some(offer.clone()),
        _ => None,
    });
    assert_eq!(offer.mime_types(), ["text/plain", "image/png"]);
    assert_eq!(conn.receive(&offer, "image/png").unwrap(), b"as image/png");
    assert_eq!(next_seen(&seen), Seen::Receive("image/png".into()));
    assert!(matches!(
        conn.receive(&offer, "text/html"),
        Err(Error::UnsupportedFormat(mime)) if mime == "text/html"
    ));
    control.send(Control::Offer(vec!["text/plain"])).unwrap();
    wait_for(&mut conn, &mut stash, |e| match e {
        Event::Selection(Some(o)) if *o != offer => Some(()),
        _ => None,
    });
    assert!(matches!(
        conn.receive(&offer, "text/plain"),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        conn.set_selection(vec![("text/plain".into(), b"x".to_vec())]),
        Err(Error::PermissionDenied(_))
    ));
}

/// A compositor that holds clipboard reads, with a selection offered in plain text.
fn holding_clipboard() -> (
    Connection,
    Sender<Control>,
    Receiver<Seen>,
    Stash,
    ClipboardOffer,
) {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        clipboard: true,
        hold_reads: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    control.send(Control::Offer(vec!["text/plain"])).unwrap();
    let offer = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Selection(Some(offer)) => Some(offer.clone()),
        _ => None,
    });
    (conn, control, seen, stash, offer)
}

/// What `read` gives and how long it took, with each wait allowed three seconds: only the read's own pipe or deadline
/// ends one sooner. It gives up after 15 s, past every bound the tests assert, so their asserts decide.
fn received_in(conn: &mut Connection, read: ReadId) -> (Option<Vec<u8>>, Duration) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(15) {
        for event in conn.wait(Some(Duration::from_secs(3))).unwrap() {
            if let Event::Received { id, data } = event {
                if id == read {
                    return (data, started.elapsed());
                }
            }
        }
    }
    panic!("the read never ended");
}

#[test]
fn a_paste_is_read_while_the_loop_keeps_serving_input() {
    let (mut conn, control, seen, mut stash, offer) = holding_clipboard();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let read = conn.receive_async(&offer, "text/plain").unwrap();
    let read_started = Instant::now();
    assert_eq!(
        std::iter::repeat_with(|| next_seen(&seen)).find(|s| matches!(s, Seen::Receive(_))),
        Some(Seen::Receive("text/plain".into()))
    );
    // the source writes nothing yet, and a key still arrives
    control.send(Control::TypeKey(KEY_A)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Keyboard {
                event: KeyboardEvent::Key { .. },
                ..
            }
        )
        .then_some(())
    });
    assert!(!stash.iter().any(|e| matches!(e, Event::Received { .. })));
    control.send(Control::Release(b"pasted")).unwrap();
    let (data, _) = received_in(&mut conn, read);
    assert_eq!(data, Some(b"pasted".to_vec()));
    // before the read's own deadline, which would also end the wait: the pipe did
    assert!(
        read_started.elapsed() < Duration::from_secs(1),
        "the pipe did not end the wait: {:?}",
        read_started.elapsed()
    );
}

#[test]
fn a_paste_longer_than_the_limit_is_cut_and_one_that_never_ends_gives_up() {
    const LIMIT: usize = 16 << 20;
    let (mut conn, control, seen, _stash, offer) = holding_clipboard();
    let long = conn.receive_async(&offer, "text/plain").unwrap();
    assert!(std::iter::repeat_with(|| next_seen(&seen)).any(|s| matches!(s, Seen::Receive(_))));
    control.send(Control::Flood(LIMIT + 4096)).unwrap();
    // the source keeps its pipe open: the read ends at the limit, not at the source's close
    let (data, _) = received_in(&mut conn, long);
    assert_eq!(data.map(|d| d.len()), Some(LIMIT));
    let stuck = conn.receive_async(&offer, "text/plain").unwrap();
    let (data, took) = received_in(&mut conn, stuck);
    assert_eq!(data, None);
    // at its deadline, before the wait's own three seconds
    assert!(
        (Duration::from_millis(900)..Duration::from_millis(2800)).contains(&took),
        "a read gives up after a second, waking the wait: {took:?}"
    );
}

#[test]
fn a_paste_that_keeps_coming_is_read_past_a_second_and_an_endless_one_stops_at_ten() {
    let (mut conn, control, seen, _stash, offer) = holding_clipboard();
    let held = |conn: &mut Connection| {
        let read = conn.receive_async(&offer, "text/plain").unwrap();
        assert!(std::iter::repeat_with(|| next_seen(&seen)).any(|s| matches!(s, Seen::Receive(_))));
        read
    };
    // a byte every 300 ms for three seconds: never a second without one
    let steady = held(&mut conn);
    let every = Duration::from_millis(300);
    control
        .send(Control::Trickle {
            every,
            count: Some(10),
        })
        .unwrap();
    let (data, took) = received_in(&mut conn, steady);
    assert_eq!(data, Some(b"xxxxxxxxxx".to_vec()));
    assert!(
        took >= Duration::from_secs(2),
        "a read of three seconds ran out early: {took:?}"
    );
    let endless = held(&mut conn);
    control
        .send(Control::Trickle { every, count: None })
        .unwrap();
    let (data, took) = received_in(&mut conn, endless);
    assert_eq!(data, None);
    // at the ten-second cap, woken by it rather than by the wait's own three seconds
    assert!(
        (Duration::from_millis(9500)..Duration::from_secs(13)).contains(&took),
        "an endless read stops at its cap: {took:?}"
    );
}

#[test]
fn a_replaced_selection_cancels_its_reads_and_eight_run_at_once() {
    let (mut conn, control, _seen, _stash, offer) = holding_clipboard();
    let reads: Vec<_> = (0..8)
        .map(|_| conn.receive_async(&offer, "text/plain").unwrap())
        .collect();
    assert!(matches!(
        conn.receive_async(&offer, "text/plain"),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        conn.receive_async(&offer, "image/png"),
        Err(Error::PermissionDenied(_) | Error::UnsupportedFormat(_))
    ));
    control.send(Control::Offer(vec!["text/plain"])).unwrap();
    // the reads end in the same return of wait as the new selection, not later at their deadline
    let give_up = Instant::now() + Duration::from_secs(10);
    let (fresh, ended) = loop {
        assert!(Instant::now() < give_up, "the new selection never arrived");
        let events = conn.wait(Some(Duration::from_secs(3))).unwrap();
        let fresh = events.iter().find_map(|e| match e {
            Event::Selection(Some(o)) if *o != offer => Some(o.clone()),
            _ => None,
        });
        if let Some(fresh) = fresh {
            let ended: Vec<_> = events
                .into_iter()
                .filter_map(|e| match e {
                    Event::Received { id, data } => Some((id, data)),
                    _ => None,
                })
                .collect();
            break (fresh, ended);
        }
    };
    assert_eq!(
        ended,
        reads.into_iter().map(|id| (id, None)).collect::<Vec<_>>()
    );
    assert!(matches!(
        conn.receive_async(&offer, "text/plain"),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        conn.receive_async(&fresh, "image/png"),
        Err(Error::UnsupportedFormat(_))
    ));
}

/// A configured window on a compositor with a seat and the clipboard, and a drag of plain text or HTML, copy or move,
/// entered over it: the offer and the enter's serial.
fn dragged_over() -> (
    Connection,
    Sender<Control>,
    Receiver<Seen>,
    Stash,
    Toplevel,
    DragOffer,
) {
    dragged_over_with(Globals {
        seat: true,
        clipboard: true,
        ..Globals::core()
    })
}

/// [`dragged_over`] on a compositor with `globals`.
fn dragged_over_with(
    globals: Globals,
) -> (
    Connection,
    Sender<Control>,
    Receiver<Seen>,
    Stash,
    Toplevel,
    DragOffer,
) {
    let (mut conn, control, seen) = connect(globals);
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let offer = drag_enters(&mut conn, &control, &mut stash, w);
    (conn, control, seen, stash, w, offer)
}

/// A drag of plain text or HTML, copy or move, enters `w`: its offer.
fn drag_enters(
    conn: &mut Connection,
    control: &Sender<Control>,
    stash: &mut Stash,
    w: Toplevel,
) -> DragOffer {
    control
        .send(Control::DragEnter {
            types: vec!["text/plain", "text/html"],
            actions: DndAction::Copy | DndAction::Move,
        })
        .unwrap();
    wait_for(conn, stash, |e| match e {
        Event::Drag {
            surface,
            event: DragEvent::Enter { offer, point },
        } if *surface == w.surface() && *point == Point::new(4.0, 5.0) => Some(offer.clone()),
        _ => None,
    })
}

/// Accept `offer` in plain text for a copy, and have the compositor drop it.
fn accept_and_drop(
    conn: &mut Connection,
    control: &Sender<Control>,
    stash: &mut Stash,
    offer: &DragOffer,
) {
    conn.accept_drag(offer, Some("text/plain")).unwrap();
    conn.set_drag_actions(offer, DragActions::Copy, DragActions::Copy)
        .unwrap();
    conn.flush().unwrap();
    control.send(Control::DragDrop).unwrap();
    assert_eq!(next_drag(conn, stash), DragEvent::Drop);
}

fn next_drag(conn: &mut Connection, stash: &mut Stash) -> DragEvent {
    wait_for(conn, stash, |e| match e {
        Event::Drag { event, .. } => Some(event.clone()),
        _ => None,
    })
}

fn seen_matching(seen: &Receiver<Seen>, pick: impl Fn(&Seen) -> bool) -> Seen {
    std::iter::repeat_with(|| next_seen(seen))
        .find(|s| pick(s))
        .unwrap()
}

#[test]
fn a_drop_is_read_only_once_accepted_in_its_type_and_dropped_then_finished() {
    let (mut conn, control, seen, mut stash, _w, offer) = dragged_over();
    assert_eq!(offer.mime_types(), ["text/plain", "text/html"]);
    assert_eq!(
        offer.source_actions(),
        DragActions::Copy | DragActions::Move
    );
    control.send(Control::DragMotion).unwrap();
    assert_eq!(
        next_drag(&mut conn, &mut stash),
        DragEvent::Motion {
            point: Point::new(6.0, 7.5),
            time: 5000
        }
    );
    assert!(matches!(
        conn.accept_drag(&offer, Some("image/png")),
        Err(Error::UnsupportedFormat(_))
    ));
    conn.accept_drag(&offer, Some("text/plain")).unwrap();
    conn.set_drag_actions(&offer, DragActions::Copy, DragActions::Copy)
        .unwrap();
    conn.flush().unwrap();
    let Seen::DragEntered(enter_serial) =
        seen_matching(&seen, |s| matches!(s, Seen::DragEntered(_)))
    else {
        unreachable!()
    };
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::Accept { .. })),
        Seen::Accept {
            serial: enter_serial,
            mime: Some("text/plain".into())
        }
    );
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::DragActions { .. })),
        Seen::DragActions {
            actions: u32::from(DndAction::Copy),
            preferred: u32::from(DndAction::Copy),
        }
    );
    control.send(Control::DragAction(DndAction::Copy)).unwrap();
    assert_eq!(
        next_drag(&mut conn, &mut stash),
        DragEvent::Action(DragActions::Copy)
    );
    assert!(
        matches!(
            conn.receive_drop(&offer, "text/plain"),
            Err(Error::PermissionDenied(_))
        ),
        "not dropped yet"
    );
    control.send(Control::DragDrop).unwrap();
    assert_eq!(next_drag(&mut conn, &mut stash), DragEvent::Drop);
    assert!(
        matches!(
            conn.receive_drop(&offer, "text/html"),
            Err(Error::PermissionDenied(_))
        ),
        "not the accepted type"
    );
    let read = conn.receive_drop(&offer, "text/plain").unwrap();
    let data = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Received { id, data } if *id == read => Some(data.clone()),
        _ => None,
    });
    assert_eq!(data, Some(b"as text/plain".to_vec()));
    conn.finish_drag(&offer).unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::Finish)),
        Seen::Finish
    );
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::OfferDestroyed)),
        Seen::OfferDestroyed
    );
    assert!(matches!(
        conn.accept_drag(&offer, None),
        Err(Error::Cancelled)
    ));
}

#[test]
fn a_drag_that_leaves_lets_its_offer_go_and_finishes_only_after_a_drop() {
    let (mut conn, control, seen, mut stash, _w, offer) = dragged_over();
    assert!(matches!(conn.finish_drag(&offer), Err(Error::Protocol(_))));
    control.send(Control::DragLeave).unwrap();
    assert_eq!(next_drag(&mut conn, &mut stash), DragEvent::Leave);
    conn.flush().unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::OfferDestroyed)),
        Seen::OfferDestroyed
    );
    assert!(matches!(
        conn.accept_drag(&offer, Some("text/plain")),
        Err(Error::Cancelled)
    ));
}

#[test]
fn a_drag_that_enters_while_a_drop_is_read_leaves_that_read_running() {
    let (mut conn, control, seen, mut stash, w, first) = dragged_over_with(Globals {
        seat: true,
        clipboard: true,
        hold_reads: true,
        ..Globals::core()
    });
    accept_and_drop(&mut conn, &control, &mut stash, &first);
    let read = conn.receive_drop(&first, "text/plain").unwrap();
    conn.flush().unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::Receive(_)));
    let second = drag_enters(&mut conn, &control, &mut stash, w);
    assert_ne!(first, second);
    assert!(
        matches!(conn.finish_drag(&second), Err(Error::Protocol(_))),
        "the new drag has not dropped"
    );
    control.send(Control::Release(b"first")).unwrap();
    let data = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Received { id, data } if *id == read => Some(data.clone()),
        _ => None,
    });
    assert_eq!(data, Some(b"first".to_vec()));
    conn.finish_drag(&first).unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::Finish)),
        Seen::Finish
    );
    conn.accept_drag(&second, Some("text/html")).unwrap();
}

#[test]
fn a_refused_drop_lets_its_offer_go_unfinished_and_ends_its_read() {
    let (mut conn, control, seen, mut stash, _w, offer) = dragged_over_with(Globals {
        seat: true,
        clipboard: true,
        hold_reads: true,
        ..Globals::core()
    });
    assert!(
        matches!(conn.refuse_drop(&offer), Err(Error::Protocol(_))),
        "not dropped yet"
    );
    accept_and_drop(&mut conn, &control, &mut stash, &offer);
    let read = conn.receive_drop(&offer, "text/plain").unwrap();
    conn.refuse_drop(&offer).unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::Finish | Seen::OfferDestroyed)),
        Seen::OfferDestroyed,
        "destroyed without a finish"
    );
    let data = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Received { id, data } if *id == read => Some(data.clone()),
        _ => None,
    });
    assert_eq!(data, None);
    assert!(matches!(conn.finish_drag(&offer), Err(Error::Cancelled)));
}

#[test]
fn before_drag_actions_a_drag_copies_and_its_drop_is_destroyed_without_a_finish() {
    let (mut conn, control, seen, mut stash, _w, offer) = dragged_over_with(Globals {
        seat: true,
        clipboard: true,
        no_drag_actions: true,
        ..Globals::core()
    });
    assert_eq!(offer.source_actions(), DragActions::Copy);
    conn.accept_drag(&offer, Some("text/plain")).unwrap();
    conn.set_drag_actions(&offer, DragActions::Move, DragActions::Move)
        .unwrap();
    conn.flush().unwrap();
    assert!(matches!(
        seen_matching(&seen, |s| matches!(
            s,
            Seen::Accept { .. } | Seen::DragActions { .. }
        )),
        Seen::Accept { .. }
    ));
    control.send(Control::DragDrop).unwrap();
    assert_eq!(next_drag(&mut conn, &mut stash), DragEvent::Drop);
    let read = conn.receive_drop(&offer, "text/plain").unwrap();
    let data = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Received { id, data } if *id == read => Some(data.clone()),
        _ => None,
    });
    assert_eq!(data, Some(b"as text/plain".to_vec()));
    conn.finish_drag(&offer).unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(
            s,
            Seen::Finish | Seen::OfferDestroyed | Seen::DragActions { .. }
        )),
        Seen::OfferDestroyed,
        "no actions and no finish before version 3"
    );
}

#[test]
fn an_enter_without_an_offer_lets_the_drag_before_it_go() {
    let (mut conn, control, seen, mut stash, _w, offer) = dragged_over();
    control.send(Control::DragEnterEmpty).unwrap();
    // the enter is on the wire before the round trip's answer, so the round trip dispatches it
    conn.roundtrip().unwrap();
    stash.extend(conn.wait(Some(Duration::ZERO)).unwrap());
    conn.flush().unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::OfferDestroyed)),
        Seen::OfferDestroyed
    );
    assert!(matches!(
        conn.accept_drag(&offer, None),
        Err(Error::Cancelled)
    ));
    control.send(Control::DragMotion).unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::DragMoved));
    conn.roundtrip().unwrap();
    stash.extend(conn.wait(Some(Duration::ZERO)).unwrap());
    assert!(
        !stash.iter().any(|e| matches!(
            e,
            Event::Drag {
                event: DragEvent::Enter { .. } | DragEvent::Motion { .. },
                ..
            }
        )),
        "no drag to report: {stash:?}"
    );
}

#[test]
fn a_drag_needs_the_button_still_held_after_the_pointer_left_or_went_away() {
    for (what, gone) in [
        ("leave", Control::Leave),
        ("pointer gone", Control::DropPointer),
    ] {
        let (mut conn, control, _seen) = connect(Globals {
            seat: true,
            clipboard: true,
            ..Globals::core()
        });
        let mut stash = Stash::new();
        let w = window(&mut conn, Decorations::Client);
        configured(&mut conn, &mut stash, w);
        control.send(Control::Click { x: 3.0, y: 4.0 }).unwrap();
        wait_for(&mut conn, &mut stash, |e| {
            matches!(
                e,
                Event::Pointer {
                    event: PointerEvent::Button { pressed: true, .. },
                    ..
                }
            )
            .then_some(())
        });
        control.send(gone).unwrap();
        conn.roundtrip().unwrap();
        stash.extend(conn.wait(Some(Duration::ZERO)).unwrap());
        assert!(
            matches!(
                conn.start_drag(
                    w.surface(),
                    vec![("text/plain".into(), b"x".to_vec())],
                    DragActions::Copy
                ),
                Err(Error::PermissionDenied(_))
            ),
            "{what}: the press is gone with the pointer"
        );
    }
}

#[test]
fn a_window_destroyed_under_a_drag_lets_the_offer_go_and_hears_no_more() {
    let (mut conn, control, seen, mut stash, w, offer) = dragged_over();
    conn.destroy(w.surface()).unwrap();
    conn.flush().unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::OfferDestroyed)),
        Seen::OfferDestroyed
    );
    control.send(Control::DragMotion).unwrap();
    // the motion is on the wire before the round trip's answer, so the round trip dispatches it
    seen_matching(&seen, |s| matches!(s, Seen::DragMoved));
    conn.roundtrip().unwrap();
    stash.extend(conn.wait(Some(Duration::ZERO)).unwrap());
    assert!(!stash.iter().any(|e| matches!(e, Event::Drag { .. })));
    assert!(matches!(
        conn.accept_drag(&offer, None),
        Err(Error::Cancelled)
    ));
}

#[test]
fn a_drag_from_this_client_answers_the_button_and_serves_its_data() {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        clipboard: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let contents = || vec![("text/plain".to_string(), b"dragged".to_vec())];
    assert!(matches!(
        conn.start_drag(w.surface(), contents(), DragActions::Copy),
        Err(Error::PermissionDenied(_))
    ));
    control.send(Control::Click { x: 3.0, y: 4.0 }).unwrap();
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
    let button = conn.input_serial().unwrap();
    // a key while the button is held, as for a modifier choosing copy or move: the drag still answers the press
    control.send(Control::TypeKey(KEY_A)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Keyboard {
                event: KeyboardEvent::Key { .. },
                ..
            }
        )
        .then_some(())
    });
    assert_ne!(conn.input_serial(), Some(button));
    let source = conn
        .start_drag(
            w.surface(),
            contents(),
            DragActions::Copy | DragActions::Move,
        )
        .unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::StartDrag { .. })),
        Seen::StartDrag {
            serial: button,
            types: vec!["text/plain".into()],
            actions: u32::from(DndAction::Copy | DndAction::Move),
        }
    );
    let next = |conn: &mut Connection, stash: &mut Stash| {
        wait_for(conn, stash, |e| match e {
            Event::DragSource { source: s, event } if *s == source => Some(event.clone()),
            _ => None,
        })
    };
    control
        .send(Control::SourceTarget(Some("text/plain")))
        .unwrap();
    assert_eq!(
        next(&mut conn, &mut stash),
        DragSourceEvent::Target(Some("text/plain".into()))
    );
    control.send(Control::SourceSend("text/plain")).unwrap();
    conn.roundtrip().unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::SourceData(_))),
        Seen::SourceData(b"dragged".to_vec())
    );
    control.send(Control::SourceDropped).unwrap();
    assert_eq!(next(&mut conn, &mut stash), DragSourceEvent::Dropped);
    control.send(Control::SourceFinished).unwrap();
    assert_eq!(next(&mut conn, &mut stash), DragSourceEvent::Finished);

    let cancelled = conn
        .start_drag(w.surface(), contents(), DragActions::Copy)
        .unwrap();
    control.send(Control::ButtonUp).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Button { pressed: false, .. },
                ..
            }
        )
        .then_some(())
    });
    assert!(
        matches!(
            conn.start_drag(w.surface(), contents(), DragActions::Copy),
            Err(Error::PermissionDenied(_))
        ),
        "no button held"
    );
    assert_ne!(cancelled, source);
    seen_matching(&seen, |s| matches!(s, Seen::StartDrag { .. }));
    control.send(Control::SourceCancelled).unwrap();
    let event = wait_for(&mut conn, &mut stash, |e| match e {
        Event::DragSource { source: s, event } if *s == cancelled => Some(event.clone()),
        _ => None,
    });
    assert_eq!(event, DragSourceEvent::Cancelled);
}

#[test]
fn a_compositor_that_goes_away_ends_the_wait_with_disconnected() {
    let (mut conn, control, _seen) = connect(Globals::core());
    drop(control);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match conn.wait(Some(Duration::from_millis(50))) {
            Err(Error::Disconnected(_)) => break,
            Ok(_) if Instant::now() < deadline => {}
            other => panic!("{other:?}"),
        }
    }
}

/// Set in the copy of the test binary that `a_program_started_after_the_shell_connection_has_no_wayland_socket` starts.
const SHELL_PROCESS: &str = "CUPERTINO_TEST_SHELL_PROCESS";

/// A process that connected over the `WAYLAND_SOCKET` it was given (a shell process the server started) does not pass
/// that connection on: a program it starts afterwards has no such variable. The process is a copy of this test binary
/// started with the variable, as the server starts a shell process, so no test changes its own environment.
#[test]
fn a_program_started_after_the_shell_connection_has_no_wayland_socket() {
    const NAME: &str = "a_program_started_after_the_shell_connection_has_no_wayland_socket";
    if std::env::var_os(SHELL_PROCESS).is_some() {
        let _conn = Connection::connect().unwrap();
        let output = std::process::Command::new("sh")
            .args(["-c", "printf %s \"${WAYLAND_SOCKET-unset}\""])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "unset");
        return;
    }
    let (client, server) = UnixStream::pair().unwrap();
    let (_control, _seen) = spawn_compositor(server, Globals::core());
    let fd = client.as_raw_fd();
    let mut shell = std::process::Command::new(std::env::current_exe().unwrap());
    shell
        .args([NAME, "--exact", "--nocapture"])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(SHELL_PROCESS, "1")
        .env("WAYLAND_SOCKET", fd.to_string());
    // SAFETY: fcntl is async-signal-safe; the descriptor stays open in this process until the child has exited
    unsafe {
        shell.pre_exec(move || {
            rustix::io::fcntl_setfd(BorrowedFd::borrow_raw(fd), rustix::io::FdFlags::empty())
                .map_err(std::io::Error::from)
        });
    }
    let status = shell.status().unwrap();
    drop(client);
    assert!(status.success(), "the shell process failed: {status}");
}

/// An extension as a crate above the transport writes one, through the public API only: layer surfaces, a protocol the
/// transport does not know.
mod layer {
    use cupertino_wayland_client::{Connection, Error, ExternalRole, State, SurfaceId};
    use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle};
    use wayland_protocols::xdg::shell::client::xdg_popup::XdgPopup;
    use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
        Layer, ZwlrLayerShellV1,
    };
    use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
        self, ZwlrLayerSurfaceV1,
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Configured {
        pub surface: SurfaceId,
        pub serial: u32,
        pub size: (u32, u32),
    }

    /// User data of the extension's own type: `Dispatch` for the transport's `State` is only allowed with it.
    pub struct ShellData;

    pub struct SurfaceData(SurfaceId);

    pub struct Layers(ZwlrLayerShellV1);

    struct LayerRole(ZwlrLayerSurfaceV1);

    impl ExternalRole for LayerRole {
        fn ack_configure(&self, serial: u32) {
            self.0.ack_configure(serial);
        }

        fn destroy(&self) {
            self.0.destroy();
        }

        fn adopt_popup(&self, popup: &XdgPopup) -> Result<(), Error> {
            self.0.get_popup(popup);
            Ok(())
        }
    }

    impl Layers {
        pub fn bind(conn: &mut Connection) -> Layers {
            Layers(conn.bind_extension(1..=4, ShellData).unwrap())
        }

        pub fn surface(&self, conn: &mut Connection, size: (u32, u32)) -> SurfaceId {
            conn.external_surface(|wl, qh, surface| {
                let data = SurfaceData(surface);
                let layer = self
                    .0
                    .get_layer_surface(wl, None, Layer::Top, "test".into(), qh, data);
                layer.set_size(size.0, size.1);
                LayerRole(layer)
            })
        }
    }

    impl Dispatch<ZwlrLayerShellV1, ShellData> for State {
        fn event(
            _: &mut State,
            _: &ZwlrLayerShellV1,
            _: <ZwlrLayerShellV1 as wayland_client::Proxy>::Event,
            _: &ShellData,
            _: &WlConnection,
            _: &QueueHandle<State>,
        ) {
        }
    }

    impl Dispatch<ZwlrLayerSurfaceV1, SurfaceData> for State {
        fn event(
            state: &mut State,
            _: &ZwlrLayerSurfaceV1,
            event: zwlr_layer_surface_v1::Event,
            data: &SurfaceData,
            _: &WlConnection,
            _: &QueueHandle<State>,
        ) {
            if let zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } = event
            {
                state.configure_external(data.0, serial, (width, height));
                state.emit(Configured {
                    surface: data.0,
                    serial,
                    size: (width, height),
                });
            }
        }
    }
}

/// Wait until the extension emitted `count` configures, at most 5 s; them and how many waits that took.
fn layer_configures(conn: &mut Connection, count: usize) -> (Vec<layer::Configured>, u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut got, mut waits) = (Vec::new(), 0);
    while got.len() < count {
        assert!(
            Instant::now() < deadline,
            "{} of {count} configures",
            got.len()
        );
        let events = conn.wait(Some(Duration::from_secs(5))).unwrap();
        waits += 1;
        got.extend(events.into_iter().filter_map(|e| match e {
            Event::Extension(e) => e.downcast::<layer::Configured>().ok().map(|c| *c),
            _ => None,
        }));
    }
    (got, waits)
}

#[test]
fn an_extension_role_draws_through_the_lease_and_acknowledges_through_its_own_protocol() {
    let (mut conn, _control, seen) = connect(Globals {
        layer_shell: true,
        ..Globals::core()
    });
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, (200, 100));
    assert_eq!(
        conn.capabilities().extensions(),
        [Extension {
            interface: "zwlr_layer_shell_v1",
            version: 4
        }]
    );
    let (configures, _) = layer_configures(&mut conn, 1);
    assert_eq!(configures[0].size, (200, 100));
    {
        let lease = conn.lease(surface, (1, 1), BufferFormat::Argb8888).unwrap();
        assert_eq!(lease.logical_size(), (200, 100));
        lease.present().unwrap();
    }
    assert_eq!(next_seen(&seen), Seen::LayerAck(configures[0].serial));
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (200, 100),
            scale: 1,
            format: wl_shm::Format::Argb8888
        }
    );
    let placement = PopupPlacement {
        anchor_rect: Rect::new(10.0, 0.0, 20.0, 20.0),
        size: (80, 40),
        anchor: Anchor::BottomLeft,
        gravity: Gravity::BottomRight,
        constraint_adjustment: ConstraintAdjustment::empty(),
        offset: (0, 0),
        reactive: false,
        parent_size: None,
        grab: false,
    };
    let popup = conn.popup(surface, &placement).unwrap();
    conn.flush().unwrap();
    assert!(matches!(next_seen(&seen), Seen::Positioner { .. }));
    assert_eq!(next_seen(&seen), Seen::LayerPopup);
    let mut stash = Stash::new();
    let configure = wait_for(&mut conn, &mut stash, |e| match e {
        Event::PopupConfigure {
            popup: p,
            configure,
        } if *p == popup => Some(*configure),
        _ => None,
    });
    assert_eq!((configure.width, configure.height), (80, 40));
    conn.destroy(surface).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_seen(&seen), Seen::LayerDestroyed);
    assert!(matches!(
        conn.lease(popup.surface(), (1, 1), BufferFormat::Argb8888),
        Err(Error::UnknownSurface)
    ));
}

#[test]
fn a_zero_dimension_of_an_extension_configure_is_left_to_the_client() {
    let (mut conn, _control, _seen) = connect(Globals {
        layer_shell: true,
        ..Globals::core()
    });
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, (0, 30));
    layer_configures(&mut conn, 1);
    let lease = conn
        .lease(surface, (640, 1), BufferFormat::Argb8888)
        .unwrap();
    assert_eq!(lease.logical_size(), (640, 30));
}

/// A layer surface over the whole output that only takes input, as a panel's catcher of presses outside it is: one
/// transparent pixel stretched to the configured size, at buffer scale 1 whatever the surface's scale, its input region
/// left as set. The pixel is attached again while the colour stays, a new size (an output rotated or resized) only moves
/// the viewport's destination, and a buffer drawn afterwards is drawn whole.
#[test]
fn a_solid_colour_is_one_pixel_stretched_over_the_configured_size_at_any_scale() {
    const OUTPUT: (u32, u32) = (1000, 700);
    let (mut conn, control, seen) = connect(Globals {
        layer_shell: true,
        fractional_scale: true,
        single_pixel: true,
        ..Globals::core()
    });
    assert!(conn.capabilities().contains(Capability::SolidColor));
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, OUTPUT);
    let (configures, _) = layer_configures(&mut conn, 1);
    conn.set_input_region(surface, Some(&[Rect::new(0.0, 0.0, 1000.0, 700.0)]))
        .unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_seen(&seen),
        Seen::InputRegion(Some(vec![[0, 0, 1000, 700]]))
    );

    conn.present_solid(surface, (1, 1), [0.0; 4]).unwrap();
    assert_eq!(next_seen(&seen), Seen::LayerAck(configures[0].serial));
    assert_eq!(next_seen(&seen), Seen::Viewport(1000, 700));
    let Seen::SolidCommit {
        color,
        scale,
        buffer,
    } = next_seen(&seen)
    else {
        panic!("a one-pixel buffer is committed");
    };
    assert_eq!((color, scale), ([0; 4], 1));
    let mut stash = Stash::new();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(e, Event::Frame(f) if f.surface == surface).then_some(())
    });

    // a fractional scale changes nothing the pixel needs: the same one is attached at the same destination
    control.send(Control::FractionalScale(180)).unwrap();
    let scale = wait_for(&mut conn, &mut stash, |e| match e {
        Event::ScaleChanged { surface: s, scale } if *s == surface => Some(*scale),
        _ => None,
    });
    assert_eq!(scale.factor(), 1.5);
    conn.present_solid(surface, (1, 1), [0.0; 4]).unwrap();
    assert_eq!(next_seen(&seen), Seen::Viewport(1000, 700));
    assert_eq!(
        next_seen(&seen),
        Seen::SolidCommit {
            color: [0; 4],
            scale: 1,
            buffer
        }
    );

    // the output turns: the configure's new size is the new destination
    control.send(Control::LayerConfigures(1)).unwrap();
    let (configures, _) = layer_configures(&mut conn, 1);
    assert_eq!(configures[0].size, (1001, 700));
    conn.present_solid(surface, (1, 1), [0.0; 4]).unwrap();
    assert_eq!(next_seen(&seen), Seen::LayerAck(configures[0].serial));
    assert_eq!(next_seen(&seen), Seen::Viewport(1001, 700));
    assert!(matches!(next_seen(&seen), Seen::SolidCommit { buffer: b, .. } if b == buffer));

    // another colour is a new pixel, and the one it replaced goes once it is no longer attached
    conn.present_solid(surface, (1, 1), [0.0, 0.0, 0.0, 0.5]).unwrap();
    assert_eq!(next_seen(&seen), Seen::Viewport(1001, 700));
    let half = u32::MAX / 2 + 1;
    assert!(matches!(
        next_seen(&seen),
        Seen::SolidCommit { color: [0, 0, 0, a], scale: 1, buffer: b } if a == half && b != buffer
    ));
    assert_eq!(next_seen(&seen), Seen::SolidDestroyed([0; 4]));

    // drawn again, the buffer is drawn whole at the surface's scale, still stretched to the configured size
    let lease = conn.lease(surface, (1, 1), BufferFormat::Argb8888).unwrap();
    assert_eq!(lease.missed(), None);
    assert_eq!(lease.logical_size(), (1001, 700));
    lease.present().unwrap();
    assert_eq!(next_seen(&seen), Seen::Viewport(1001, 700));
    assert_eq!(
        next_seen(&seen),
        Seen::Commit {
            size: (1502, 1050),
            scale: 1,
            format: wl_shm::Format::Argb8888
        }
    );
}

/// Without fractional scaling the viewporter alone stretches the pixel; without the one-pixel buffers a solid colour
/// is refused, for the caller to draw it, and nothing is sent.
#[test]
fn a_solid_colour_needs_the_one_pixel_buffers_and_the_viewporter_only() {
    let (mut conn, _control, seen) = connect(Globals {
        layer_shell: true,
        single_pixel: true,
        ..Globals::core()
    });
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, (300, 200));
    let (configures, _) = layer_configures(&mut conn, 1);
    conn.present_solid(surface, (1, 1), [1.0, 0.0, 0.0, 1.0])
        .unwrap();
    assert_eq!(next_seen(&seen), Seen::LayerAck(configures[0].serial));
    assert_eq!(next_seen(&seen), Seen::Viewport(300, 200));
    assert!(matches!(
        next_seen(&seen),
        Seen::SolidCommit {
            color: [u32::MAX, 0, 0, u32::MAX],
            scale: 1,
            ..
        }
    ));

    let (mut conn, _control, seen) = connect(Globals {
        layer_shell: true,
        fractional_scale: true,
        ..Globals::core()
    });
    assert!(!conn.capabilities().contains(Capability::SolidColor));
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, (300, 200));
    layer_configures(&mut conn, 1);
    assert!(matches!(
        conn.present_solid(surface, (1, 1), [0.0; 4]),
        Err(Error::Unsupported(Capability::SolidColor))
    ));
    conn.flush().unwrap();
    assert!(seen.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn an_extension_the_compositor_does_not_offer_binds_nothing_and_is_not_listed() {
    let (mut conn, _control, _seen) = connect(Globals::core());
    let shell: Option<
        wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1,
    > = conn.bind_extension(1..=4, layer::ShellData);
    assert!(shell.is_none());
    assert_eq!(conn.capabilities().extensions(), []);
}

#[test]
fn a_burst_of_extension_events_ends_one_wait_not_one_per_event() {
    let (mut conn, control, _seen) = connect(Globals {
        layer_shell: true,
        ..Globals::core()
    });
    let layers = layer::Layers::bind(&mut conn);
    let surface = layers.surface(&mut conn, (200, 100));
    layer_configures(&mut conn, 1);
    control.send(Control::LayerConfigures(100)).unwrap();
    let (configures, waits) = layer_configures(&mut conn, 100);
    // the burst may straddle a socket read; it never takes a wait per event
    assert!(waits <= 3, "{waits} waits for 100 configures");
    assert_eq!(
        configures.iter().map(|c| c.size.0).collect::<Vec<_>>(),
        (201..=300).collect::<Vec<_>>()
    );
    let latest = configures.last().unwrap();
    assert_eq!(latest.surface, surface);
    let lease = conn.lease(surface, (1, 1), BufferFormat::Argb8888).unwrap();
    assert_eq!(
        (lease.serial(), lease.logical_size()),
        (latest.serial, (300, 100))
    );
}

#[test]
fn an_input_region_is_sent_rounded_outwards_and_saturated_and_none_restores_the_whole_surface() {
    let (mut conn, _control, seen) = connect(Globals::core());
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let rects = [
        Rect::new(0.5, 1.5, 10.0, 4.2),
        Rect::new(20.0, 0.0, 5.0, 5.0),
    ];
    conn.set_input_region(w.surface(), Some(&rects)).unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_seen(&seen),
        Seen::InputRegion(Some(vec![[0, 1, 11, 5], [20, 0, 5, 5]]))
    );
    conn.set_input_region(w.surface(), Some(&[Rect::new(-1e12, 0.0, 2e12, 1.0)]))
        .unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_seen(&seen),
        Seen::InputRegion(Some(vec![[i32::MIN, 0, i32::MAX, 1]]))
    );
    conn.set_input_region(w.surface(), None).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_seen(&seen), Seen::InputRegion(None));
}

#[test]
fn a_reactive_popup_answers_the_parents_pending_configure_and_a_reposition_reports_its_token() {
    let (mut conn, _control, seen) = connect(Globals::core());
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    // not presented: the window's configure is still pending
    let pending = configured(&mut conn, &mut stash, w).serial;
    let placement = |anchor_rect, size, reactive| PopupPlacement {
        anchor_rect,
        size,
        anchor: Anchor::BottomLeft,
        gravity: Gravity::BottomRight,
        constraint_adjustment: ConstraintAdjustment::FlipY,
        offset: (-3, 4),
        reactive,
        parent_size: reactive.then_some((640, 480)),
        grab: false,
    };
    let popup = conn
        .popup(
            w.surface(),
            &placement(Rect::new(10.0, 20.0, 30.0, 8.0), (120, 90), false),
        )
        .unwrap();
    conn.flush().unwrap();
    let created = std::iter::repeat_with(|| next_seen(&seen))
        .find_map(|s| match s {
            Seen::Positioner(p) => Some(p),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        created,
        PositionerState {
            size: (120, 90),
            anchor_rect: [10, 20, 30, 8],
            offset: (-3, 4),
            ..PositionerState::default()
        }
    );
    let popup_configure = |stash: &mut Stash, conn: &mut Connection| {
        wait_for(conn, stash, |e| match e {
            Event::PopupConfigure {
                popup: p,
                configure,
            } if *p == popup => Some((configure.x, configure.y, configure.width, configure.height)),
            _ => None,
        })
    };
    assert_eq!(popup_configure(&mut stash, &mut conn), (10, 28, 120, 90));
    present(&mut conn, popup.surface(), (1, 1));
    let token = conn
        .reposition_popup(
            popup,
            &placement(Rect::new(50.0, 60.0, 10.0, 10.0), (200, 100), true),
        )
        .unwrap();
    conn.flush().unwrap();
    let reposition = std::iter::repeat_with(|| next_seen(&seen))
        .find(|s| matches!(s, Seen::Reposition { .. }))
        .unwrap();
    assert_eq!(
        reposition,
        Seen::Reposition {
            token,
            positioner: PositionerState {
                size: (200, 100),
                anchor_rect: [50, 60, 10, 10],
                offset: (-3, 4),
                reactive: true,
                parent_size: Some((640, 480)),
                parent_configure: Some(pending),
            }
        }
    );
    let repositioned = wait_for(&mut conn, &mut stash, |e| match e {
        Event::PopupRepositioned { popup: p, token } if *p == popup => Some(*token),
        _ => None,
    });
    assert_eq!(repositioned, token);
    assert_eq!(popup_configure(&mut stash, &mut conn), (50, 70, 200, 100));
    present(&mut conn, w.surface(), (640, 480));
    let again = conn
        .reposition_popup(
            popup,
            &placement(Rect::new(50.0, 60.0, 10.0, 10.0), (200, 100), true),
        )
        .unwrap();
    conn.flush().unwrap();
    let acknowledged = std::iter::repeat_with(|| next_seen(&seen))
        .find_map(|s| match s {
            Seen::Reposition { token, positioner } => Some((token, positioner.parent_configure)),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        acknowledged,
        (again, None),
        "the window answered its configure"
    );
    assert_ne!(again, token);
}

#[test]
fn presented_frames_report_their_time_on_the_compositors_clock_and_discarded_ones_say_so() {
    let (mut conn, control, _seen) = connect(Globals {
        presentation: true,
        ..Globals::core()
    });
    assert!(conn.capabilities().contains(Capability::Presentation));
    assert_eq!(conn.presentation_clock(), Some(MONOTONIC));
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    present(&mut conn, w.surface(), (64, 48));
    conn.roundtrip().unwrap();
    control
        .send(Control::Presentation { discard: false })
        .unwrap();
    let presented = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Presented(p) => Some(*p),
        _ => None,
    });
    assert_eq!(
        presented,
        Presented {
            surface: w.surface(),
            time_ns: ((1 << 32) + 5) * 1_000_000_000 + 250,
            refresh_ns: 16_666_666,
            sequence: (1 << 32) + 7,
            kind: PresentedKind::Vsync | PresentedKind::HwClock,
        }
    );
    present(&mut conn, w.surface(), (64, 48));
    conn.roundtrip().unwrap();
    control
        .send(Control::Presentation { discard: true })
        .unwrap();
    let discarded = wait_for(&mut conn, &mut stash, |e| match e {
        Event::Discarded(surface) => Some(*surface),
        _ => None,
    });
    assert_eq!(discarded, w.surface());
}

/// An XCursor file holding one opaque image per `(nominal size, width, height, hotspot)`.
fn xcursor(images: &[(u32, u32, u32, (u32, u32))]) -> Vec<u8> {
    animated_xcursor(images, 0)
}

/// [`xcursor`] with each image shown for `delay` milliseconds.
fn animated_xcursor(images: &[(u32, u32, u32, (u32, u32))], delay: u32) -> Vec<u8> {
    const IMAGE: u32 = 0xfffd_0002;
    let words = |out: &mut Vec<u8>, ws: &[u32]| ws.iter().for_each(|w| out.extend(w.to_le_bytes()));
    let mut out = b"Xcur".to_vec();
    words(&mut out, &[16, 0x1_0000, images.len() as u32]);
    let mut position = 16 + 12 * images.len() as u32;
    for (nominal, width, height, _) in images {
        words(&mut out, &[IMAGE, *nominal, position]);
        position += 36 + width * height * 4;
    }
    for (nominal, width, height, (x, y)) in images {
        words(
            &mut out,
            &[36, IMAGE, *nominal, 1, *width, *height, *x, *y, delay],
        );
        words(&mut out, &vec![0xff00_0000; (width * height) as usize]);
    }
    out
}

#[test]
fn an_animated_theme_cursor_plays_its_images_until_the_pointer_leaves() {
    let themes = std::env::temp_dir().join(format!("cupertino-animated-{}", std::process::id()));
    let cursors = themes.join("test").join("cursors");
    std::fs::create_dir_all(&cursors).unwrap();
    let steps = [(24, 8, 8, (1, 1)), (24, 8, 8, (2, 2)), (24, 8, 8, (3, 3))];
    std::fs::write(cursors.join("wait"), animated_xcursor(&steps, 30)).unwrap();
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    conn.set_cursor_theme(CursorTheme {
        name: "test".into(),
        size: 24,
        search_path: vec![themes.clone()],
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::Click { x: 5.0, y: 5.0 }).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Enter(_),
                ..
            }
        )
        .then_some(())
    });
    conn.set_cursor(CursorShape::Wait).unwrap();
    let mut hotspots = Vec::new();
    let started = Instant::now();
    // a wait longer than the whole animation: only the cursor's own timer ends each one in time
    while hotspots.len() < 4 && started.elapsed() < Duration::from_secs(5) {
        stash.extend(conn.wait(Some(Duration::from_secs(2))).unwrap());
        hotspots.extend(seen.try_iter().filter_map(|s| match s {
            Seen::Cursor { hotspot, .. } => Some(hotspot),
            _ => None,
        }));
    }
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "the wait did not wake for the next image: {:?}",
        started.elapsed()
    );
    assert_eq!(
        hotspots,
        [(1, 1), (2, 2), (3, 3), (1, 1)],
        "each image in turn, then again"
    );
    control.send(Control::Leave).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Leave,
                ..
            }
        )
        .then_some(())
    });
    seen.try_iter().for_each(drop);
    let quiet = Instant::now() + Duration::from_millis(150);
    while Instant::now() < quiet {
        conn.wait(Some(Duration::from_millis(10))).unwrap();
    }
    assert!(
        !seen.try_iter().any(|s| matches!(s, Seen::Cursor { .. })),
        "the cursor animated after the pointer left"
    );
    std::fs::remove_dir_all(&themes).unwrap();
}

/// The ways a client's animated cursor leaves the screen besides the pointer leaving.
#[derive(Clone, Copy, Debug)]
enum Replaced {
    Hidden,
    StillShape,
    NewTheme,
    PointerGone,
}

#[test]
fn an_animated_theme_cursor_stops_when_hidden_replaced_or_its_pointer_goes() {
    let themes = std::env::temp_dir().join(format!("cupertino-stops-{}", std::process::id()));
    let cursors = themes.join("test").join("cursors");
    std::fs::create_dir_all(&cursors).unwrap();
    let steps = [(24, 8, 8, (1, 1)), (24, 8, 8, (2, 2)), (24, 8, 8, (3, 3))];
    std::fs::write(cursors.join("wait"), animated_xcursor(&steps, 20)).unwrap();
    std::fs::write(cursors.join("text"), xcursor(&[(24, 8, 8, (7, 7))])).unwrap();
    let theme = CursorTheme {
        name: "test".into(),
        size: 24,
        search_path: vec![themes.clone()],
    };
    let animated = |hotspot: (i32, i32)| [(1, 1), (2, 2), (3, 3)].contains(&hotspot);
    for case in [
        Replaced::Hidden,
        Replaced::StillShape,
        Replaced::NewTheme,
        Replaced::PointerGone,
    ] {
        let (mut conn, control, seen) = connect(Globals {
            seat: true,
            ..Globals::core()
        });
        conn.set_cursor_theme(theme.clone());
        let mut stash = Stash::new();
        let w = window(&mut conn, Decorations::Client);
        configured(&mut conn, &mut stash, w);
        control.send(Control::Click { x: 5.0, y: 5.0 }).unwrap();
        wait_for(&mut conn, &mut stash, |e| {
            matches!(
                e,
                Event::Pointer {
                    event: PointerEvent::Enter(_),
                    ..
                }
            )
            .then_some(())
        });
        conn.set_cursor(CursorShape::Wait).unwrap();
        let mut steps = 0;
        while steps < 2 {
            stash.extend(conn.wait(Some(Duration::from_millis(10))).unwrap());
            steps += seen
                .try_iter()
                .filter(|s| matches!(s, Seen::Cursor { hotspot, .. } if animated(*hotspot)))
                .count();
        }
        match case {
            Replaced::Hidden => conn.hide_cursor().unwrap(),
            Replaced::StillShape => conn.set_cursor(CursorShape::Text).unwrap(),
            Replaced::NewTheme => conn.set_cursor_theme(theme.clone()),
            Replaced::PointerGone => {
                control.send(Control::DropPointer).unwrap();
                conn.roundtrip().unwrap();
            }
        }
        conn.flush().unwrap();
        // what the stop itself sent arrives first; then nothing is drawn on the cursor's surface again
        std::thread::sleep(Duration::from_millis(30));
        seen.try_iter().for_each(drop);
        let quiet = Instant::now() + Duration::from_millis(150);
        while Instant::now() < quiet {
            conn.wait(Some(Duration::from_millis(10))).unwrap();
        }
        let late: Vec<_> = seen
            .try_iter()
            .filter(|s| matches!(s, Seen::Cursor { .. } | Seen::Commit { size: (8, 8), .. }))
            .collect();
        assert!(
            late.is_empty(),
            "{case:?}: the cursor was drawn again: {late:?}"
        );
    }
    std::fs::remove_dir_all(&themes).unwrap();
}

#[test]
fn without_cursor_shapes_the_cursor_is_drawn_from_the_theme_at_the_pointers_scale() {
    let themes = std::env::temp_dir().join(format!("cupertino-cursors-{}", std::process::id()));
    let cursors = |theme: &str| {
        let dir = themes.join(theme).join("cursors");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    };
    std::fs::write(
        cursors("test").join("text"),
        xcursor(&[(24, 24, 24, (4, 8)), (48, 48, 48, (8, 16))]),
    )
    .unwrap();
    std::fs::write(
        themes.join("test").join("index.theme"),
        "[Icon Theme]\nInherits=base\n",
    )
    .unwrap();
    std::fs::write(
        cursors("base").join("default"),
        xcursor(&[(24, 32, 32, (1, 2))]),
    )
    .unwrap();
    // base names no parent, so it inherits from default, which leads back to test
    std::fs::write(
        cursors("default").join("pointer"),
        xcursor(&[(24, 16, 16, (2, 3))]),
    )
    .unwrap();
    std::fs::write(
        themes.join("default").join("index.theme"),
        "[Icon Theme]\nInherits=test\n",
    )
    .unwrap();

    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    assert!(!conn.capabilities().contains(Capability::CursorShape));
    conn.set_cursor_theme(CursorTheme {
        name: "test".into(),
        size: 24,
        search_path: vec![themes.clone()],
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    control.send(Control::Click { x: 5.0, y: 5.0 }).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Enter(_),
                ..
            }
        )
        .then_some(())
    });
    let cursor = |conn: &mut Connection, shape| {
        conn.set_cursor(shape).unwrap();
        let commit = std::iter::repeat_with(|| next_seen(&seen))
            .find_map(|s| match s {
                Seen::Commit { size, scale, .. } => Some((size, scale)),
                _ => None,
            })
            .unwrap();
        match next_seen(&seen) {
            Seen::Cursor {
                hotspot,
                surface: true,
                ..
            } => (commit, hotspot),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(
        cursor(&mut conn, CursorShape::Text),
        (((24, 24), 1), (4, 8))
    );
    assert_eq!(
        cursor(&mut conn, CursorShape::Pointer),
        (((16, 16), 1), (2, 3))
    );
    // no theme of the cycle has zoom-in: the inherited default cursor stands in
    assert_eq!(
        cursor(&mut conn, CursorShape::ZoomIn),
        (((32, 32), 1), (1, 2))
    );
    control.send(Control::PreferredBufferScale(2)).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(e, Event::ScaleChanged { .. }).then_some(())
    });
    // the cursor showing is drawn again for the new scale without being set again
    let redrawn = std::iter::repeat_with(|| next_seen(&seen))
        .find_map(|s| match s {
            Seen::Commit { size, scale, .. } => Some((size, scale)),
            _ => None,
        })
        .unwrap();
    assert_eq!(redrawn, ((32, 32), 2));
    let shown = next_seen(&seen);
    assert!(
        matches!(
            shown,
            Seen::Cursor {
                hotspot: (0, 1),
                surface: true,
                ..
            }
        ),
        "the hotspot in points: {shown:?}"
    );
    assert_eq!(
        cursor(&mut conn, CursorShape::Text),
        (((48, 48), 2), (4, 8))
    );
    std::fs::remove_dir_all(&themes).unwrap();
}

#[test]
fn a_launch_token_arrives_for_its_request_and_needs_xdg_activation() {
    let (mut conn, _control, seen) = connect(Globals {
        activation: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let request = conn
        .request_activation_token(w.surface(), Some("org.example.Files"))
        .unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_seen(&seen),
        Seen::TokenCommit(Some("org.example.Files".into()))
    );
    let token = wait_for(&mut conn, &mut stash, |e| match e {
        Event::ActivationToken { request: r, token } if *r == request => Some(token.clone()),
        _ => None,
    });
    assert_eq!(token, "token-org.example.Files");

    let (mut conn, _control, _seen) = connect(Globals::core());
    let w = window(&mut conn, Decorations::Client);
    assert!(matches!(
        conn.request_activation_token(w.surface(), None),
        Err(Error::Unsupported(Capability::Activation))
    ));
}

#[test]
fn a_forwarded_activation_token_reaches_its_window_and_needs_xdg_activation() {
    use wayland_client::Proxy;

    let (mut conn, _control, seen) = connect(Globals {
        activation: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    let surface = conn.wl_surface(w.surface()).unwrap().id().protocol_id();
    conn.activate_with_token(w, "forwarded-launch-token")
        .unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_seen(&seen),
        Seen::Activate {
            token: "forwarded-launch-token".into(),
            surface
        }
    );

    let (mut conn, _control, _seen) = connect(Globals::core());
    let w = window(&mut conn, Decorations::Client);
    assert!(matches!(
        conn.activate_with_token(w, "forwarded-launch-token"),
        Err(Error::Unsupported(Capability::Activation))
    ));
}

#[test]
fn an_output_has_a_logical_geometry_only_with_xdg_output() {
    let geometry = |output_layout| {
        let (conn, _control, _seen) = connect(Globals {
            output_scale: Some(1),
            output_layout,
            ..Globals::core()
        });
        conn.outputs()[0].geometry
    };
    assert_eq!(
        geometry(true),
        Some(OutputGeometry {
            x: 1920,
            y: 0,
            width: 1280,
            height: 720
        })
    );
    assert_eq!(geometry(false), None);
}

#[test]
fn an_output_reports_its_identification_size_and_effective_scale() {
    let output = |output_layout| {
        let (conn, _control, _seen) = connect(Globals {
            output_scale: Some(1),
            output_layout,
            ..Globals::core()
        });
        conn.outputs()[0].clone()
    };
    let laid_out = output(true);
    assert_eq!(
        (
            laid_out.make.as_deref(),
            laid_out.model.as_deref(),
            laid_out.physical_size_mm,
            laid_out.transform,
        ),
        (
            Some("Example"),
            None,
            Some((344, 194)),
            OutputTransform::Normal
        )
    );
    assert_eq!(laid_out.effective_scale(), Some(1.5));
    assert_eq!(output(false).effective_scale(), None);
}

#[test]
fn the_axes_of_one_pointer_frame_arrive_as_one_scroll() {
    for (high_resolution_wheel, detents, inverted) in [
        (false, (120, -240), (false, false)),
        (true, (60, -120), (false, true)),
    ] {
        let (mut conn, control, _seen) = connect(Globals {
            seat: true,
            high_resolution_wheel,
            ..Globals::core()
        });
        let mut stash = Stash::new();
        let w = window(&mut conn, Decorations::Client);
        configured(&mut conn, &mut stash, w);
        control.send(Control::Scroll { dx: 4.0, dy: -2.5 }).unwrap();
        let mut scroll = || {
            wait_for(&mut conn, &mut stash, |e| match e {
                Event::Pointer {
                    time,
                    event:
                        PointerEvent::Scroll {
                            dx,
                            dy,
                            source,
                            value120,
                            stop,
                            inverted,
                            ..
                        },
                    ..
                } => Some((*time, *dx, *dy, *source, *value120, *stop, *inverted)),
                _ => None,
            })
        };
        assert_eq!(
            scroll(),
            (
                900,
                4.0,
                -5.0,
                Some(ScrollSource::Wheel),
                Some(detents),
                false,
                inverted
            )
        );
        assert_eq!(
            scroll(),
            (
                950,
                0.0,
                0.0,
                Some(ScrollSource::Finger),
                None,
                true,
                (false, false)
            )
        );
    }
}

fn text_input_window() -> (
    Connection,
    Sender<Control>,
    Receiver<Seen>,
    SurfaceId,
    Stash,
) {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        text_input: true,
        ..Globals::core()
    });
    assert!(conn.capabilities().contains(Capability::TextInput));
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    (conn, control, seen, w.surface(), stash)
}

fn text_input_event(conn: &mut Connection, stash: &mut Stash) -> TextInputEvent {
    wait_for(conn, stash, |e| match e {
        Event::TextInput { event, .. } => Some(event.clone()),
        _ => None,
    })
}

fn next_text_input(seen: &Receiver<Seen>) -> TextInputRequest {
    loop {
        if let Seen::TextInput(request) = next_seen(seen) {
            return request;
        }
    }
}

fn field(text: &str, cursor: usize) -> TextInputState {
    TextInputState {
        surrounding: SurroundingText::new(text, cursor, cursor),
        cursor_rect: Some(Rect::new(10.2, 20.0, 1.0, 16.5)),
        ..TextInputState::default()
    }
}

#[test]
fn the_focused_field_is_sent_on_text_input_enter_and_each_change_once() {
    use zwp_text_input_v3::{ChangeCause, ContentHint, ContentPurpose};
    use TextInputRequest::*;
    let (mut conn, control, seen, surface, mut stash) = text_input_window();
    conn.set_text_input(surface, Some(&field("hi", 2))).unwrap();
    control.send(Control::TextInputEnter).unwrap();
    assert_eq!(
        text_input_event(&mut conn, &mut stash),
        TextInputEvent::Enter
    );
    conn.flush().unwrap();
    let full = |text: &str, cursor| {
        vec![
            Surrounding(text.into(), cursor, cursor),
            Cause(ChangeCause::Other),
            Content(ContentHint::None, ContentPurpose::Normal),
            CursorRect([10, 20, 2, 17]),
            Commit,
        ]
    };
    let mut expected = vec![Enable];
    expected.extend(full("hi", 2));
    assert_eq!(
        (0..6).map(|_| next_text_input(&seen)).collect::<Vec<_>>(),
        expected
    );

    conn.set_text_input(surface, Some(&field("hi", 2))).unwrap();
    conn.set_text_input(surface, Some(&field("hi!", 3)))
        .unwrap();
    conn.flush().unwrap();
    assert_eq!(
        (0..5).map(|_| next_text_input(&seen)).collect::<Vec<_>>(),
        full("hi!", 3)
    );

    // withdrawing the surrounding text needs a fresh enable
    let unknown = TextInputState {
        surrounding: None,
        ..field("", 0)
    };
    conn.set_text_input(surface, Some(&unknown)).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_text_input(&seen), Enable);
    assert_eq!(next_text_input(&seen), Cause(ChangeCause::Other));

    conn.set_text_input(surface, None).unwrap();
    conn.flush().unwrap();
    while next_text_input(&seen) != Commit {}
    assert_eq!(
        (next_text_input(&seen), next_text_input(&seen)),
        (Disable, Commit)
    );
}

#[test]
fn a_reset_enables_afresh_once_and_then_the_state_is_kept_as_sent() {
    use zwp_text_input_v3::ChangeCause;
    use TextInputRequest::*;
    let (mut conn, control, seen, surface, mut stash) = text_input_window();
    conn.set_text_input(surface, Some(&field("ka", 2))).unwrap();
    control.send(Control::TextInputEnter).unwrap();
    text_input_event(&mut conn, &mut stash);
    conn.flush().unwrap();
    while next_text_input(&seen) != Commit {}

    // the field kept a composition as typed on a click: the input method starts afresh
    let reset = TextInputState {
        reset: true,
        ..field("ka", 2)
    };
    conn.set_text_input(surface, Some(&reset)).unwrap();
    conn.flush().unwrap();
    assert_eq!(next_text_input(&seen), Enable);
    assert_eq!(
        next_text_input(&seen),
        Surrounding("ka".into(), 2, 2),
        "the whole state follows the enable"
    );
    assert_eq!(next_text_input(&seen), Cause(ChangeCause::Other));
    while next_text_input(&seen) != Commit {}

    // the same state again sends nothing: the reset went once
    conn.set_text_input(surface, Some(&field("ka", 2))).unwrap();
    conn.set_text_input(surface, Some(&field("kai", 3)))
        .unwrap();
    conn.flush().unwrap();
    assert_eq!(
        next_text_input(&seen),
        Surrounding("kai".into(), 3, 3),
        "no second enable"
    );
}

#[test]
fn a_password_field_never_shows_its_text_to_the_input_method() {
    use zwp_text_input_v3::{ChangeCause, ContentHint, ContentPurpose};
    use TextInputRequest::*;
    let (mut conn, control, seen, surface, mut stash) = text_input_window();
    control.send(Control::TextInputEnter).unwrap();
    text_input_event(&mut conn, &mut stash);
    let secret = TextInputState {
        purpose: cupertino_wayland_client::ContentPurpose::Password,
        ..field("hunter2", 7)
    };
    conn.set_text_input(surface, Some(&secret)).unwrap();
    conn.flush().unwrap();
    assert_eq!(
        (0..5).map(|_| next_text_input(&seen)).collect::<Vec<_>>(),
        [
            Enable,
            Cause(ChangeCause::Other),
            Content(ContentHint::SensitiveData, ContentPurpose::Password),
            CursorRect([10, 20, 2, 17]),
            Commit
        ]
    );
}

#[test]
fn an_input_method_batch_arrives_at_done_and_is_current_only_for_the_latest_commit() {
    let (mut conn, control, seen, surface, mut stash) = text_input_window();
    conn.set_text_input(surface, Some(&field("ab", 2))).unwrap();
    control.send(Control::TextInputEnter).unwrap();
    text_input_event(&mut conn, &mut stash);
    conn.flush().unwrap();
    while next_text_input(&seen) != TextInputRequest::Commit {}
    control
        .send(Control::Compose {
            preedit: Some(("añ", 1, 3)),
            commit: Some("x"),
            delete: (1, 0),
            serial: None,
        })
        .unwrap();
    assert_eq!(
        text_input_event(&mut conn, &mut stash),
        TextInputEvent::Done(TextInputDone {
            preedit: Some(Preedit {
                text: "añ".into(),
                cursor: Some((1, 3)),
            }),
            commit: Some("x".into()),
            delete: (1, 0),
            current: true,
        })
    );
    control
        .send(Control::Compose {
            preedit: None,
            commit: Some("y"),
            delete: (0, 0),
            serial: Some(0),
        })
        .unwrap();
    assert_eq!(
        text_input_event(&mut conn, &mut stash),
        TextInputEvent::Done(TextInputDone {
            commit: Some("y".into()),
            ..TextInputDone::default()
        })
    );
    // a batch cut short by a focus change is dropped, not carried into the next one
    control.send(Control::PreeditOnly("lost")).unwrap();
    control.send(Control::TextInputLeave).unwrap();
    assert_eq!(
        text_input_event(&mut conn, &mut stash),
        TextInputEvent::Leave
    );
    control.send(Control::TextInputEnter).unwrap();
    text_input_event(&mut conn, &mut stash);
    conn.flush().unwrap();
    while next_text_input(&seen) != TextInputRequest::Commit {}
    control
        .send(Control::Compose {
            preedit: None,
            commit: Some("z"),
            delete: (0, 0),
            serial: None,
        })
        .unwrap();
    assert_eq!(
        text_input_event(&mut conn, &mut stash),
        TextInputEvent::Done(TextInputDone {
            commit: Some("z".into()),
            current: true,
            ..TextInputDone::default()
        })
    );
}

#[test]
fn a_text_field_needs_a_compositor_with_text_input() {
    let (mut conn, _control, _seen) = connect(Globals {
        seat: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    assert!(matches!(
        conn.set_text_input(w.surface(), Some(&field("a", 1))),
        Err(Error::Unsupported(Capability::TextInput))
    ));
    assert!(conn.set_text_input(w.surface(), None).is_ok());
}

#[test]
fn a_field_is_sent_again_after_each_enter_and_disabled_when_its_window_goes() {
    use TextInputRequest::*;
    let (mut conn, control, seen, surface, mut stash) = text_input_window();
    conn.set_text_input(surface, Some(&field("ab", 2))).unwrap();
    for _ in 0..2 {
        control.send(Control::TextInputEnter).unwrap();
        assert_eq!(
            text_input_event(&mut conn, &mut stash),
            TextInputEvent::Enter
        );
        conn.flush().unwrap();
        assert_eq!(next_text_input(&seen), Enable);
        while next_text_input(&seen) != Commit {}
        control.send(Control::TextInputLeave).unwrap();
        assert_eq!(
            text_input_event(&mut conn, &mut stash),
            TextInputEvent::Leave
        );
    }
    control.send(Control::TextInputEnter).unwrap();
    text_input_event(&mut conn, &mut stash);
    conn.flush().unwrap();
    while next_text_input(&seen) != Commit {}
    conn.destroy(surface).unwrap();
    conn.flush().unwrap();
    assert_eq!(
        (next_text_input(&seen), next_text_input(&seen)),
        (Disable, Commit)
    );
}

#[test]
fn touch_contacts_report_on_their_surface_until_up_and_a_cancel_or_a_lost_touch_screen_ends_them() {
    for end in [Control::CancelTouch, Control::DropTouch] {
        let (mut conn, control, _seen) = connect(Globals {
            seat: true,
            ..Globals::core()
        });
        let mut stash = Stash::new();
        let w = window(&mut conn, Decorations::Client);
        configured(&mut conn, &mut stash, w);
        control.send(Control::Touch).unwrap();
        let mut touches = Vec::new();
        while touches.len() < 4 {
            touches.push(wait_for(&mut conn, &mut stash, |e| match e {
                Event::Touch {
                    surface,
                    time,
                    event,
                } if *surface == w.surface() => Some((*time, *event)),
                _ => None,
            }));
        }
        let at = Point::new;
        assert_eq!(
            touches,
            vec![
                (
                    100,
                    TouchEvent::Down {
                        id: 0,
                        position: at(5.0, 6.0)
                    }
                ),
                (
                    105,
                    TouchEvent::Down {
                        id: 1,
                        position: at(9.0, 9.0)
                    }
                ),
                (
                    110,
                    TouchEvent::Motion {
                        id: 0,
                        position: at(7.5, 8.5)
                    }
                ),
                (120, TouchEvent::Up { id: 0 }),
            ]
        );
        control.send(end).unwrap();
        let cancel = wait_for(&mut conn, &mut stash, |e| match e {
            Event::Touch { surface, event, .. } => Some((*surface, *event)),
            _ => None,
        });
        assert_eq!(cancel, (w.surface(), TouchEvent::Cancel));
    }
}

/// A configured window on a compositor with a seat and the clipboard, with the left button pressed in it.
fn pressed_window() -> (Connection, Sender<Control>, Receiver<Seen>, Stash, Toplevel) {
    let (mut conn, control, seen) = connect(Globals {
        seat: true,
        clipboard: true,
        ..Globals::core()
    });
    let mut stash = Stash::new();
    let w = window(&mut conn, Decorations::Client);
    configured(&mut conn, &mut stash, w);
    press(&mut conn, &control, &mut stash);
    (conn, control, seen, stash, w)
}

fn press(conn: &mut Connection, control: &Sender<Control>, stash: &mut Stash) {
    control.send(Control::Click { x: 3.0, y: 4.0 }).unwrap();
    wait_for(conn, stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Button { pressed: true, .. },
                ..
            }
        )
        .then_some(())
    });
}

/// An icon of 8 × 6 pixels at scale 2 (4 × 3 points) with the pointer at (1.4, 2) points.
fn icon() -> DragIcon {
    DragIcon::new(8, 6, 2, vec![0x80; 8 * 6 * 4], Point::new(1.4, 2.0)).unwrap()
}

fn drag_contents() -> Vec<(String, Vec<u8>)> {
    vec![("text/uri-list".to_string(), b"file:///a\r\n".to_vec())]
}

fn source_event(
    conn: &mut Connection,
    stash: &mut Stash,
    source: cupertino_wayland_client::DragSource,
) -> DragSourceEvent {
    wait_for(conn, stash, |e| match e {
        Event::DragSource { source: s, event } if *s == source => Some(event.clone()),
        _ => None,
    })
}

#[test]
fn a_drag_icon_must_be_whole_points_of_the_pixels_it_names() {
    let pixels = |w: u32, h: u32| vec![0; (w * h * 4) as usize];
    let at = Point::new(0.0, 0.0);
    assert!(DragIcon::new(8, 6, 2, pixels(8, 6), at).is_some());
    assert!(DragIcon::new(0, 6, 1, Vec::new(), at).is_none(), "empty");
    assert!(
        DragIcon::new(9, 6, 2, pixels(9, 6), at).is_none(),
        "a half point wide"
    );
    assert!(
        DragIcon::new(8, 6, 0, pixels(8, 6), at).is_none(),
        "no scale"
    );
    assert!(
        DragIcon::new(8, 6, 1, pixels(8, 5), at).is_none(),
        "short pixels"
    );
    let side = DragIcon::MAX_EXTENT + 1;
    assert!(
        DragIcon::new(side, 1, 1, pixels(side, 1), at).is_none(),
        "too wide"
    );
    assert!(DragIcon::new(8, 6, 1, pixels(8, 6), Point::new(f64::NAN, 0.0)).is_none());
    assert_eq!(icon().hotspot(), (1, 2));
    assert!(
        !format!("{:?}", icon()).contains("128"),
        "Debug leaves the pixels out"
    );
}

/// The icon surface goes with `start_drag` and shows its buffer at its scale with the pointer at the hotspot; the drop
/// ends the pointer's drag, so the surface and buffer go then, before the target finishes.
#[test]
fn a_drag_icon_is_the_start_drag_surface_at_its_hotspot_until_the_drop() {
    let (mut conn, control, seen, mut stash, w) = pressed_window();
    let source = conn
        .start_drag_with_icon(
            w.surface(),
            drag_contents(),
            DragActions::Copy,
            Some(&icon()),
        )
        .unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::StartDrag { .. }));
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::IconShown { .. })),
        Seen::IconShown {
            size: (8, 6),
            scale: 2,
            hotspot: (1, 2),
        }
    );
    // the compositor takes the pointer for the drag
    control.send(Control::Leave).unwrap();
    control.send(Control::SourceDropped).unwrap();
    assert_eq!(
        source_event(&mut conn, &mut stash, source),
        DragSourceEvent::Dropped
    );
    conn.flush().unwrap();
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::IconSurfaceDestroyed)),
        Seen::IconSurfaceDestroyed
    );
    assert_eq!(
        seen_matching(&seen, |s| matches!(s, Seen::IconBufferDestroyed)),
        Seen::IconBufferDestroyed
    );
    control.send(Control::SourceFinished).unwrap();
    assert_eq!(
        source_event(&mut conn, &mut stash, source),
        DragSourceEvent::Finished
    );
}

/// A drag cancelled by the compositor (released over nothing that accepts it, or a target that refused it) frees its
/// icon, and the next press starts a drag as before.
#[test]
fn a_cancelled_drag_frees_its_icon_and_the_next_press_drags_again() {
    let (mut conn, control, seen, mut stash, w) = pressed_window();
    let source = conn
        .start_drag_with_icon(
            w.surface(),
            drag_contents(),
            DragActions::Move,
            Some(&icon()),
        )
        .unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::IconShown { .. }));
    control.send(Control::Leave).unwrap();
    control.send(Control::SourceCancelled).unwrap();
    assert_eq!(
        source_event(&mut conn, &mut stash, source),
        DragSourceEvent::Cancelled
    );
    conn.flush().unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::IconSurfaceDestroyed));
    seen_matching(&seen, |s| matches!(s, Seen::IconBufferDestroyed));

    press(&mut conn, &control, &mut stash);
    let again = conn
        .start_drag_with_icon(
            w.surface(),
            drag_contents(),
            DragActions::Move,
            Some(&icon()),
        )
        .unwrap();
    assert_ne!(again, source);
    seen_matching(&seen, |s| matches!(s, Seen::IconShown { .. }));
}

/// A compositor ignores a drag that reached it after the button went up: the client hears the release instead of a
/// leave, ends the drag as cancelled and frees its icon, and the pointer goes on as before.
#[test]
fn a_drag_the_compositor_ignored_ends_cancelled_at_the_release_and_frees_its_icon() {
    let (mut conn, control, seen, mut stash, w) = pressed_window();
    let source = conn
        .start_drag_with_icon(
            w.surface(),
            drag_contents(),
            DragActions::Copy,
            Some(&icon()),
        )
        .unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::IconShown { .. }));
    control.send(Control::ButtonUp).unwrap();
    wait_for(&mut conn, &mut stash, |e| {
        matches!(
            e,
            Event::Pointer {
                event: PointerEvent::Button { pressed: false, .. },
                ..
            }
        )
        .then_some(())
    });
    assert_eq!(
        source_event(&mut conn, &mut stash, source),
        DragSourceEvent::Cancelled
    );
    conn.flush().unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::IconSurfaceDestroyed));
    seen_matching(&seen, |s| matches!(s, Seen::IconBufferDestroyed));
    press(&mut conn, &control, &mut stash);
}

/// A drag taken by the compositor is not ended by a later release: only the compositor ends it.
#[test]
fn a_release_after_the_compositor_took_the_drag_leaves_it_running() {
    let (mut conn, control, seen, mut stash, w) = pressed_window();
    let source = conn
        .start_drag_with_icon(
            w.surface(),
            drag_contents(),
            DragActions::Copy,
            Some(&icon()),
        )
        .unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::IconShown { .. }));
    control.send(Control::Leave).unwrap();
    press(&mut conn, &control, &mut stash);
    control.send(Control::ButtonUp).unwrap();
    conn.roundtrip().unwrap();
    stash.extend(conn.wait(Some(Duration::ZERO)).unwrap());
    assert!(!stash
        .iter()
        .any(|e| matches!(e, Event::DragSource { source: s, .. } if *s == source)));
    control.send(Control::SourceCancelled).unwrap();
    assert_eq!(
        source_event(&mut conn, &mut stash, source),
        DragSourceEvent::Cancelled
    );
}

/// A drag started while another runs replaces it, and the old one's icon goes with it.
#[test]
fn a_drag_that_replaces_another_frees_the_old_icon() {
    let (mut conn, _control, seen, _stash, w) = pressed_window();
    conn.start_drag_with_icon(
        w.surface(),
        drag_contents(),
        DragActions::Copy,
        Some(&icon()),
    )
    .unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::IconShown { .. }));
    conn.start_drag(w.surface(), drag_contents(), DragActions::Copy)
        .unwrap();
    seen_matching(&seen, |s| matches!(s, Seen::IconSurfaceDestroyed));
    seen_matching(&seen, |s| matches!(s, Seen::IconBufferDestroyed));
}
